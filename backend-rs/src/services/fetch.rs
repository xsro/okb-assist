//! 大文件传输兜底（上传 / 下载解析结果包）
//!
//! **为什么单独一套客户端**：控制面（提交任务、轮询状态）用的 `reqwest::Client` 带
//! `.timeout(120s)`，而这个超时是**整个请求**（含响应体读完）的时限。在 OrangePi/树莓派 +
//! 家庭宽带的场景下，几十 MB 的解析结果 ZIP 经常读不完就被 reqwest 掐断，日志表现为：
//!
//! ```text
//! 读取 ZIP 响应体失败: request or response body error: operation timed out
//! 分片解析失败: 分片 1/3 获取结果失败: ...
//! ```
//!
//! 本模块提供三样兜底：
//!
//! 1. [`stream_client`]：**不设整体超时**的客户端（只有连接超时，读多久由下面控制）；
//! 2. [`download_to_file`]：**流式落盘**（不再 `resp.bytes()` 把整包读进内存，省内存也省一次拷贝）
//!    + `Range` **断点续传**（重试只补剩下的字节）+ 失败**重试**（指数退避）
//!    + **停滞检测**（`stall_timeout_secs` 秒内没读到任何字节才算失败，而不是一刀切 120 秒）；
//! 3. 下载进度日志（每 5 秒一次），慢链路上一眼能看出是「在动」还是「卡住」。

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use tokio::io::AsyncWriteExt;
use tracing::{debug, info, warn};

/// 下载策略（来自 `config.json` 的 `mineru[].download_timeout` / `download_retries`）
#[derive(Debug, Clone, Copy)]
pub struct DownloadPolicy {
    /// 单次尝试内「多少秒没有任何字节进账」就算失败（0 = 不限）
    pub stall_timeout_secs: u64,
    /// 最多尝试次数（含首次），至少 1
    pub attempts: u32,
    /// 重试基础退避秒数，按 2^n 递增，上限 30 秒
    pub backoff_secs: u64,
}

impl Default for DownloadPolicy {
    fn default() -> Self {
        Self {
            stall_timeout_secs: 120,
            attempts: 3,
            backoff_secs: 3,
        }
    }
}

impl DownloadPolicy {
    /// 由配置构造：`retries` = **失败重试次数**（0 = 只试一次）
    pub fn from_config(stall_timeout_secs: u64, retries: u64) -> Self {
        Self {
            stall_timeout_secs,
            attempts: (retries.min(10) as u32) + 1,
            backoff_secs: 3,
        }
    }
}

/// **不带整体超时**的客户端：大文件上传 / 解析结果下载都用它。
///
/// 注意：不要再给它加 `.timeout()`，那正是 ZIP 下载被掐断的原因。
pub fn stream_client() -> reqwest::Client {
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(20))
        .tcp_keepalive(Duration::from_secs(30))
        .pool_idle_timeout(Duration::from_secs(30))
        .build()
        .unwrap_or_else(|_| reqwest::Client::new())
}

/// 人类可读的字节数
pub fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{} B", bytes)
    } else {
        format!("{:.1} {}", value, UNITS[unit])
    }
}

/// 流式下载 `url` 到 `dest`（失败重试 + 断点续传 + 停滞检测）。
///
/// - 中途数据先写 `dest.part`，**成功后才 rename 成 `dest`**，所以不存在「半个文件冒充完整包」；
/// - `headers` 用于需要鉴权的下载（如 V1 的 `/v1/files/{id}/content`），跨域重定向时
///   reqwest 会自动去掉 `Authorization`，不会把令牌送给 CDN；
/// - 返回落盘后的文件字节数。
pub async fn download_to_file(
    client: &reqwest::Client,
    url: &str,
    dest: &Path,
    headers: &[(&str, String)],
    policy: DownloadPolicy,
    label: &str,
) -> anyhow::Result<u64> {
    if let Some(parent) = dest.parent() {
        tokio::fs::create_dir_all(parent).await.ok();
    }
    let part = PathBuf::from(format!("{}.part", dest.display()));
    let attempts = policy.attempts.max(1);
    let mut last_err = String::from("未知错误");

    for attempt in 1..=attempts {
        // 上一轮留下的半成品就是续传起点
        let have = tokio::fs::metadata(&part)
            .await
            .map(|m| m.len())
            .unwrap_or(0);
        let resume_from = if attempt > 1 { have } else { 0 };
        if resume_from == 0 && have > 0 {
            let _ = tokio::fs::remove_file(&part).await;
        }

        if attempt > 1 {
            warn!(
                "{} 第 {}/{} 次尝试{}",
                label,
                attempt,
                attempts,
                if resume_from > 0 {
                    format!("（从 {} 续传）", human_size(resume_from))
                } else {
                    String::new()
                }
            );
        }

        match fetch_once(client, url, &part, headers, resume_from, policy, label).await {
            Ok(total) => {
                // 校验后再落定：避免把截断的包当成完整结果
                if let Err(e) = tokio::fs::rename(&part, dest).await {
                    let _ = tokio::fs::remove_file(&part).await;
                    return Err(anyhow::anyhow!("保存下载结果失败: {}", e));
                }
                return Ok(total);
            }
            Err(e) => {
                last_err = e.to_string();
                warn!("{} 第 {}/{} 次下载失败: {}", label, attempt, attempts, last_err);
                if attempt < attempts {
                    let delay = (policy.backoff_secs * (1u64 << (attempt - 1).min(4))).min(30);
                    if delay > 0 {
                        tokio::time::sleep(Duration::from_secs(delay)).await;
                    }
                }
            }
        }
    }

    let _ = tokio::fs::remove_file(&part).await;
    anyhow::bail!(
        "{} 下载失败（已尝试 {} 次）: {}",
        label,
        attempts,
        last_err
    )
}

/// 单次下载尝试，返回累计写入的字节数
async fn fetch_once(
    client: &reqwest::Client,
    url: &str,
    part: &Path,
    headers: &[(&str, String)],
    resume_from: u64,
    policy: DownloadPolicy,
    label: &str,
) -> anyhow::Result<u64> {
    let mut req = client.get(url);
    for (key, value) in headers {
        req = req.header(*key, value.as_str());
    }
    if resume_from > 0 {
        req = req.header(reqwest::header::RANGE, format!("bytes={}-", resume_from));
    }

    let stall = if policy.stall_timeout_secs > 0 {
        Some(Duration::from_secs(policy.stall_timeout_secs))
    } else {
        None
    };

    // 等响应头也受「停滞超时」约束：否则服务端接了连接却不回包时，无整体超时的客户端会一直挂着
    let mut resp = match stall {
        Some(limit) => match tokio::time::timeout(limit, req.send()).await {
            Ok(r) => r?,
            Err(_) => anyhow::bail!(
                "等待响应头超过 {} 秒（服务端无响应）",
                limit.as_secs()
            ),
        },
        None => req.send().await?,
    };
    let status = resp.status();

    if status.as_u16() == 416 {
        // 续传点超出文件长度（服务端文件变了）：丢掉半成品，下次从头下
        let _ = tokio::fs::remove_file(part).await;
        anyhow::bail!("服务器拒绝续传（HTTP 416），已丢弃半成品");
    }
    if !status.is_success() {
        anyhow::bail!("HTTP {}", status);
    }

    // 服务器不理 Range（返回 200）时只能从头写
    let mut written = if status.as_u16() == 206 { resume_from } else { 0 };
    let expected = resp.content_length().map(|len| len + written);

    let mut file = if written > 0 {
        tokio::fs::OpenOptions::new().append(true).open(part).await?
    } else {
        tokio::fs::File::create(part).await?
    };

    let started = Instant::now();
    let mut last_report = Instant::now();

    loop {
        let next = match stall {
            Some(limit) => match tokio::time::timeout(limit, resp.chunk()).await {
                Ok(r) => r,
                Err(_) => {
                    let _ = file.flush().await;
                    anyhow::bail!(
                        "读取响应体停滞超过 {} 秒（已下载 {}）",
                        limit.as_secs(),
                        human_size(written)
                    );
                }
            },
            None => resp.chunk().await,
        };

        match next {
            Ok(Some(chunk)) => {
                file.write_all(&chunk).await?;
                written += chunk.len() as u64;
                if last_report.elapsed() >= Duration::from_secs(5) {
                    last_report = Instant::now();
                    match expected {
                        Some(total) if total > 0 => info!(
                            "{} 下载中: {} / {} ({}%)",
                            label,
                            human_size(written),
                            human_size(total),
                            (written.min(total) as f64 / total as f64 * 100.0) as u32
                        ),
                        _ => info!("{} 下载中: {}", label, human_size(written)),
                    }
                }
            }
            Ok(None) => break,
            Err(e) => {
                let _ = file.flush().await;
                anyhow::bail!("读取响应体失败: {}", e);
            }
        }
    }

    file.flush().await?;
    drop(file);

    if let Some(total) = expected {
        if written < total {
            anyhow::bail!(
                "响应体不完整：收到 {} / {}",
                human_size(written),
                human_size(total)
            );
        }
    }

    debug!(
        "{} 下载完成: {}，用时 {:.1}s",
        label,
        human_size(written),
        started.elapsed().as_secs_f64()
    );
    Ok(written)
}

#[cfg(test)]
mod tests {
    //! 只在 `cargo test` 里跑：用一个极简的 TCP HTTP 服务验证「中断 → 重试 → 断点续传」链路，
    //! 不引入任何测试依赖（不引 axum/wiremock 之类）。

    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    /// 起一个只服务一个文件的 HTTP 服务：支持 `Range`；
    /// `truncate_first = true` 时第一次请求只发一半就断开（模拟下载被打断）。
    async fn spawn_server(body: Vec<u8>, truncate_first: bool) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let mut first = true;
            loop {
                let Ok((mut socket, _)) = listener.accept().await else {
                    break;
                };
                let body = body.clone();
                let truncate = truncate_first && first;
                first = false;
                tokio::spawn(async move {
                    let mut buf = vec![0u8; 4096];
                    let n = socket.read(&mut buf).await.unwrap_or(0);
                    let req = String::from_utf8_lossy(&buf[..n]).to_string();
                    let start = req
                        .lines()
                        .find_map(|line| {
                            let lower = line.to_ascii_lowercase();
                            lower
                                .strip_prefix("range: bytes=")
                                .map(|v| v.trim().trim_end_matches('-').to_string())
                        })
                        .and_then(|v| v.parse::<usize>().ok())
                        .unwrap_or(0)
                        .min(body.len());
                    let slice = &body[start..];
                    let head = if start > 0 {
                        format!(
                            "HTTP/1.1 206 Partial Content\r\nContent-Range: bytes {}-{}/{}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                            start,
                            body.len().saturating_sub(1),
                            body.len(),
                            slice.len()
                        )
                    } else {
                        format!(
                            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                            slice.len()
                        )
                    };
                    let _ = socket.write_all(head.as_bytes()).await;
                    let split = if truncate { slice.len() / 2 } else { slice.len() };
                    let _ = socket.write_all(&slice[..split]).await;
                    let _ = socket.flush().await;
                    if truncate {
                        tokio::time::sleep(Duration::from_millis(50)).await;
                    }
                    let _ = socket.shutdown().await;
                });
            }
        });
        format!("http://{}/file", addr)
    }

    #[tokio::test]
    async fn download_retries_and_resumes() {
        let body: Vec<u8> = (0..300_000u32).map(|i| (i % 251) as u8).collect();
        let url = spawn_server(body.clone(), true).await;
        let dir = std::env::temp_dir().join(format!("okb-fetch-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let dest = dir.join("result.zip");

        let policy = DownloadPolicy {
            stall_timeout_secs: 5,
            attempts: 3,
            backoff_secs: 0,
        };
        let size = download_to_file(&stream_client(), &url, &dest, &[], policy, "测试包")
            .await
            .expect("重试后应该下完整");
        assert_eq!(size as usize, body.len());
        assert_eq!(std::fs::read(&dest).unwrap(), body);
        // 落定后不应残留 .part
        assert!(!Path::new(&format!("{}.part", dest.display())).exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
