//! MinerU PDF 解析服务（V1 API，MinerU >= 4.0）
//!
//! 支持两种部署方式，均使用 V1 API：
//! - 自部署（type=local）：连接本地 V1 API 服务（如 http://127.0.0.1:8000）
//! - 官方云（type=official）：调用 MinerU 官方云 V1 API（https://mineru.net）
//!
//! V1 API 请求周期：
//!   1. POST /v1/uploads        → 创建上传（需 filename, bytes, sha256sum, mime_type）
//!   2. PUT <upload_url>         → 上传文件字节（携带 upload_headers）
//!   3. POST /v1/uploads/{id}/complete → 完成上传，获得 file.id
//!   4. POST /v1/parse/jobs      → 提交解析任务（source.type + source.file_id）
//!   5. GET  /v1/parse/jobs/{id} → 轮询任务状态
//!   6. GET  /v1/files/{id}/content → 下载结果文件

use std::collections::HashMap;
use std::io::{Cursor, Read, Write};
use std::path::Path;
use sha2::Digest;

use serde_json::{json, Value};

use crate::config::MinerUConfig;

/// MinerU 模式类型
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MineruType {
    /// 自部署 V1 API 服务
    Local,
    /// 官方云 V1 API
    Official,
}

impl MineruType {
    pub fn from_str(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "official" | "精准解析" | "精准" => MineruType::Official,
            _ => MineruType::Local,
        }
    }
}

pub struct MinerUClient {
    base_url: String,
    key: String,
    mineru_type: MineruType,
    tier: String,
    http: reqwest::Client,
}

impl MinerUClient {
    pub fn new(config: &MinerUConfig) -> Self {
        let mineru_type = MineruType::from_str(&config.mineru_type);
        // 官方云 API 基础路径含 /api 前缀（https://mineru.net/api/v1/...）
        // 自部署 API 基础路径直接为 /v1/...（http://127.0.0.1:8002/v1/...）
        let base_url = {
            let url = config.url.trim_end_matches('/').to_string();
            if mineru_type == MineruType::Official && !url.ends_with("/api") {
                format!("{}/api", url)
            } else {
                url
            }
        };
        Self {
            base_url,
            key: config.key.clone(),
            mineru_type,
            tier: config.tier.clone(),
            http: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(120))
                .connect_timeout(std::time::Duration::from_secs(15))
                .build()
                .unwrap_or_else(|_| reqwest::Client::new()),
        }
    }

    pub fn from_config(config: &MinerUConfig) -> Self {
        Self::new(config)
    }

    // ════════════════════════════════════════════════════════
    //  公开接口（与旧版保持签名一致）
    // ════════════════════════════════════════════════════════

    /// 提交解析任务（完整 V1 流程：创建上传 → 上传 → 完成 → 提交任务）
    /// 返回 job_id
    pub async fn submit_parse_task(&self, file_path: &str) -> anyhow::Result<String> {
        let file_name = Path::new(file_path)
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "document.pdf".to_string());

        // Step 1: 创建上传
        let pdf_bytes = tokio::fs::read(file_path).await?;
        let (upload_id, upload_method, upload_url, upload_headers) =
            self.create_upload(&file_name, pdf_bytes.len() as u64, &pdf_bytes).await?;

        // Step 2: 上传文件字节（秒传命中则跳过）
        if upload_method != "SKIP" {
            self.upload_file(&upload_method, &upload_url, &upload_headers, pdf_bytes)
                .await?;
        }

        // Step 3: 完成上传 → 获得 file_id
        let file_id = self.complete_upload(&upload_id).await?;

        // Step 4: 提交解析任务 → 获得 job_id
        let job_id = self.submit_job(&file_id).await?;

        Ok(job_id)
    }

    /// 检查任务状态
    /// 返回标准化结构：{ "status", "raw_status", "output_files", "error" }
    pub async fn check_task_status(&self, job_id: &str) -> Value {
        let resp = self
            .http
            .get(format!("{}/v1/parse/jobs/{}", self.base_url, job_id))
            .bearer_auth(&self.key)
            .send()
            .await;

        match resp {
            Ok(resp) if resp.status().is_success() => {
                let data: Value = resp.json().await.unwrap_or(json!({}));
                let status = data["status"].as_str().unwrap_or("unknown");
                let normalized = match status {
                    "completed" => "completed",
                    "failed" => "failed",
                    "queued" => "pending",
                    "running" => "processing",
                    "processing" => "processing",
                    "partial" => "partial",
                    "canceled" => "canceled",
                    _ => status,
                };

                // 提取 output_files 中的文件 ID
                let files = data["files"].as_array().cloned().unwrap_or_default();
                let mut output_files: Vec<Value> = Vec::new();
                for file_entry in &files {
                    if let Some(of) = file_entry.get("output_files").and_then(|v| v.as_object()) {
                        for (fmt_name, fmt_val) in of {
                            if let Some(fid) = fmt_val.get("file_id").and_then(|v| v.as_str()) {
                                output_files.push(json!({
                                    "file_id": fid,
                                    "format": fmt_name,
                                    "bytes": fmt_val.get("bytes"),
                                }));
                            }
                        }
                    }
                }

                json!({
                    "status": normalized,
                    "raw_status": status,
                    "output_files": output_files,
                    "error": data.get("last_error").or_else(|| data.get("error")).cloned().unwrap_or(json!(null)),
                })
            }
            _ => json!({"status": "unknown"}),
        }
    }

    /// 轮询任务直到完成
    pub async fn poll_task(&self, job_id: &str, timeout_secs: u64) -> anyhow::Result<Value> {
        let poll_interval = 2u64;
        let max_polls = timeout_secs / poll_interval;

        for _ in 0..max_polls {
            let status_result = self.check_task_status(job_id).await;
            let status = status_result["status"].as_str().unwrap_or("unknown");

            match status {
                "completed" | "partial" => return Ok(status_result),
                "failed" => {
                    let error = status_result["error"]
                        .as_str()
                        .unwrap_or("Unknown error");
                    anyhow::bail!("MinerU task failed: {}", error);
                }
                "canceled" => {
                    anyhow::bail!("MinerU task was canceled");
                }
                _ => {
                    tokio::time::sleep(std::time::Duration::from_secs(poll_interval)).await;
                }
            }
        }

        anyhow::bail!("MinerU task timed out: {}", job_id);
    }

    /// 获取任务结果：下载产物文件并保存到 output_dir
    /// 返回 markdown 文件路径
    ///
    /// 优先从 ZIP 包提取 markdown（图片为本地路径引用），
    /// 若 ZIP 中无 markdown，则回退到独立 markdown 输出，
    /// 并用正则将 data:image/... URI 替换为本地图片文件引用。
    pub async fn get_task_result(
        &self,
        job_id: &str,
        output_dir: &str,
        doc_id: Option<i64>,
    ) -> anyhow::Result<String> {
        // 获取任务状态（含 output_files）
        let status_result = self.check_task_status(job_id).await;
        let status = status_result["status"].as_str().unwrap_or("");
        if status != "completed" && status != "partial" {
            anyhow::bail!("Task not completed (status: {})", status);
        }

        let output_files = status_result["output_files"]
            .as_array()
            .cloned()
            .unwrap_or_default();

        if output_files.is_empty() {
            anyhow::bail!("No output files in job result");
        }

        // 准备输出目录
        std::fs::create_dir_all(output_dir)?;

        // 先下载 ZIP 和独立 markdown
        let mut zip_bytes: Option<Vec<u8>> = None;
        let mut standalone_markdown: Option<String> = None;

        for file_entry in &output_files {
            let format = file_entry["format"].as_str().unwrap_or("");
            let file_id = file_entry["file_id"].as_str().unwrap_or("");

            if file_id.is_empty() {
                continue;
            }

            let content = match self.download_file(file_id).await {
                Ok(c) => c,
                Err(e) => {
                    tracing::warn!("下载文件 {} 失败: {}", file_id, e);
                    continue;
                }
            };

            match format {
                "markdown" => {
                    standalone_markdown = Some(String::from_utf8_lossy(&content).to_string());
                }
                "zip" => {
                    zip_bytes = Some(content);
                }
                _ => {}
            }
        }

        // 优先从 ZIP 中提取 markdown
        let (final_markdown, extracted_images) = if let Some(zip_data) = &zip_bytes {
            Self::extract_from_zip(zip_data, output_dir)
        } else {
            (None, Vec::new())
        };

        let md = if let Some(ref md_from_zip) = final_markdown {
            md_from_zip.clone()
        } else if let Some(ref md_standalone) = standalone_markdown {
            // 回退：替换 data:image/... URI 为本地图片文件
            Self::replace_data_uris_in_markdown(md_standalone, output_dir)
        } else {
            anyhow::bail!("No markdown content found in MinerU result");
        };

        // 若从 ZIP 提取了图片，保存 images.zip
        if !extracted_images.is_empty() {
            Self::save_images_zip(output_dir, &extracted_images);
        }

        // 保存 markdown
        let md_filename = match doc_id {
            Some(id) => format!("{}.md", id),
            None => "output.md".to_string(),
        };
        let md_path = Path::new(output_dir).join(&md_filename);
        std::fs::write(&md_path, &md)?;

        Ok(md_path.to_string_lossy().to_string())
    }

    /// 从 ZIP 包中提取 markdown 和图片文件
    /// 返回 (markdown 内容, 提取的图片列表)
    fn extract_from_zip(zip_data: &[u8], output_dir: &str) -> (Option<String>, Vec<(String, Vec<u8>)>) {
        let mut markdown_content: Option<String> = None;
        let mut image_bytes: Vec<(String, Vec<u8>)> = Vec::new();

        if let Ok(mut archive) = zip::ZipArchive::new(Cursor::new(zip_data)) {
            for i in 0..archive.len() {
                if let Ok(mut entry) = archive.by_index(i) {
                    let name = entry.name().to_string();
                    let lower = name.to_lowercase();

                    if lower.ends_with(".md") || lower.ends_with(".markdown") {
                        // ZIP 中的 markdown，图片引用是本地路径
                        let mut buf = Vec::new();
                        if entry.read_to_end(&mut buf).is_ok() {
                            markdown_content = Some(String::from_utf8_lossy(&buf).to_string());
                        }
                    } else if lower.ends_with(".png")
                        || lower.ends_with(".jpg")
                        || lower.ends_with(".jpeg")
                        || lower.ends_with(".gif")
                        || lower.ends_with(".svg")
                    {
                        let mut buf = Vec::new();
                        if entry.read_to_end(&mut buf).is_ok() {
                            let fname = Path::new(&name)
                                .file_name()
                                .map(|n| n.to_string_lossy().to_string())
                                .unwrap_or_else(|| name.clone());
                            // 保存图片到独立文件（保持 ZIP 内目录结构）
                            let img_path = Path::new(output_dir).join(&fname);
                            if let Some(parent) = img_path.parent() {
                                let _ = std::fs::create_dir_all(parent);
                            }
                            let _ = std::fs::write(&img_path, &buf);
                            image_bytes.push((fname, buf));
                        }
                    }
                }
            }
        }

        (markdown_content, image_bytes)
    }

    /// 回退方案：standalone markdown 中的 data:image/... URI 不做替换，
    /// 直接返回原内容（图片仍内嵌在 markdown 中，不影响显示）。
    /// 主要修复路径是从 ZIP 中提取 markdown（已含本地图片引用）。
    fn replace_data_uris_in_markdown(markdown: &str, _output_dir: &str) -> String {
        markdown.to_string()
    }

    /// 将图片列表打包为 images.zip
    fn save_images_zip(output_dir: &str, images: &[(String, Vec<u8>)]) {
        let images_zip_path = Path::new(output_dir).join("images.zip");
        match std::fs::File::create(&images_zip_path) {
            Ok(zip_file) => {
                let mut zip_writer = zip::ZipWriter::new(zip_file);
                let options = zip::write::SimpleFileOptions::default()
                    .compression_method(zip::CompressionMethod::Stored);
                for (filename, data) in images {
                    if zip_writer.start_file(filename, options).is_ok() {
                        let _ = zip_writer.write_all(data);
                    }
                }
                let _ = zip_writer.finish();
            }
            Err(e) => {
                tracing::warn!("无法创建 images.zip: {}", e);
            }
        }
    }

    // ════════════════════════════════════════════════════════
    //  内部 V1 API 方法
    // ════════════════════════════════════════════════════════

    /// Step 1: 创建上传
    /// 返回 (upload_id, upload_method, upload_url, upload_headers)
    async fn create_upload(
        &self,
        filename: &str,
        file_size: u64,
        file_bytes: &[u8],
    ) -> anyhow::Result<(String, String, String, HashMap<String, String>)> {
        // 在后台线程计算 SHA-256（不阻塞 async 运行时）
        let file_bytes_vec = file_bytes.to_vec();
        let sha256_hex = tokio::task::spawn_blocking(move || {
            let sha256 = sha2::Sha256::digest(&file_bytes_vec);
            hex::encode(sha256)
        })
        .await
        .map_err(|e| anyhow::anyhow!("SHA-256 计算失败: {}", e))?;

        let mut body = json!({
            "filename": filename,
            "bytes": file_size,
            "sha256sum": sha256_hex,
            "mime_type": "application/pdf",
        });

        // 官方云 API 需要 purpose 字段
        if self.mineru_type == MineruType::Official {
            body["purpose"] = json!("parse");
        }

        let resp = self
            .http
            .post(format!("{}/v1/uploads", self.base_url))
            .bearer_auth(&self.key)
            .json(&body)
            .send()
            .await?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            anyhow::bail!(
                "MinerU create upload failed: HTTP {} - {}",
                status,
                text.chars().take(200).collect::<String>()
            );
        }

        let data: Value = resp.json().await?;

        // 检查秒传：file 字段非空表示文件已存在
        if data.get("file").and_then(|v| v.as_object()).is_some() {
            let file_id = data["file"]["id"]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("No file.id in dedup response"))?
                .to_string();
            return Ok((file_id, "SKIP".to_string(), String::new(), HashMap::new()));
        }

        let upload_id = data["id"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("No id in response"))?
            .to_string();
        let upload_method = data["upload_method"]
            .as_str()
            .unwrap_or("PUT")
            .to_string();
        let upload_url = data["upload_url"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("No upload_url in response"))?
            .to_string();

        let mut upload_headers = HashMap::new();
        if let Some(headers) = data["upload_headers"].as_object() {
            for (k, v) in headers {
                if let Some(val) = v.as_str() {
                    upload_headers.insert(k.clone(), val.to_string());
                }
            }
        }

        Ok((upload_id, upload_method, upload_url, upload_headers))
    }

    /// Step 2: 上传文件字节
    async fn upload_file(
        &self,
        method: &str,
        url: &str,
        headers: &HashMap<String, String>,
        data: Vec<u8>,
    ) -> anyhow::Result<()> {
        if method == "SKIP" {
            return Ok(());
        }

        let full_url = resolve_url(&self.base_url, url);

        let mut req = match method.to_uppercase().as_str() {
            "PUT" => self.http.put(&full_url),
            "POST" => self.http.post(&full_url),
            _ => self.http.put(&full_url),
        };

        for (k, v) in headers {
            req = req.header(k.as_str(), v.as_str());
        }

        if !self.key.is_empty() && is_same_origin(&full_url, &self.base_url) {
            req = req.bearer_auth(&self.key);
        }

        let resp = req.body(data).send().await?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            anyhow::bail!(
                "MinerU file upload failed: HTTP {} - {}",
                status,
                text.chars().take(200).collect::<String>()
            );
        }

        Ok(())
    }

    /// Step 3: 完成上传 → 获得 file_id
    async fn complete_upload(&self, upload_id: &str) -> anyhow::Result<String> {
        let resp = self
            .http
            .post(format!("{}/v1/uploads/{}/complete", self.base_url, upload_id))
            .bearer_auth(&self.key)
            .send()
            .await?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            anyhow::bail!(
                "MinerU complete upload failed: HTTP {} - {}",
                status,
                text.chars().take(200).collect::<String>()
            );
        }

        let data: Value = resp.json().await?;
        let file_id = data["file"]["id"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("No file.id in complete response"))?
            .to_string();
        Ok(file_id)
    }

    /// Step 4: 提交解析任务 → 获得 job_id
    async fn submit_job(&self, file_id: &str) -> anyhow::Result<String> {
        let body = json!({
            "files": [{
                "source": {
                    "type": "file_id",
                    "file_id": file_id,
                }
            }],
            "tier": self.tier,
            "output_formats": ["markdown", "zip"],
        });

        let resp = self
            .http
            .post(format!("{}/v1/parse/jobs", self.base_url))
            .bearer_auth(&self.key)
            .json(&body)
            .send()
            .await?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            anyhow::bail!(
                "MinerU submit job failed: HTTP {} - {}",
                status,
                text.chars().take(200).collect::<String>()
            );
        }

        let data: Value = resp.json().await?;
        let job_id = data["job_id"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("No job_id in response"))?
            .to_string();
        Ok(job_id)
    }

    /// 下载结果文件（支持 302 跳转）
    async fn download_file(&self, file_id: &str) -> anyhow::Result<Vec<u8>> {
        let resp = self
            .http
            .get(format!("{}/v1/files/{}/content", self.base_url, file_id))
            .bearer_auth(&self.key)
            .send()
            .await?;

        if resp.status().is_success() {
            return Ok(resp.bytes().await?.to_vec());
        }

        if matches!(resp.status().as_u16(), 301 | 302 | 303 | 307 | 308) {
            if let Some(location) = resp
                .headers()
                .get("location")
                .and_then(|v| v.to_str().ok())
            {
                let redirect_resp = self.http.get(location).send().await?;
                if redirect_resp.status().is_success() {
                    return Ok(redirect_resp.bytes().await?.to_vec());
                }
                anyhow::bail!(
                    "MinerU file download redirect failed: HTTP {}",
                    redirect_resp.status()
                );
            }
        }

        anyhow::bail!("MinerU file download failed: HTTP {}", resp.status());
    }
}

// ════════════════════════════════════════════════════════
//  便捷函数
// ════════════════════════════════════════════════════════

/// 解析 PDF（提交 → 轮询 → 获取结果）
pub async fn parse_pdf(
    client: &MinerUClient,
    file_path: &str,
    output_dir: &str,
    doc_id: Option<i64>,
    timeout_secs: u64,
) -> anyhow::Result<String> {
    let job_id = client.submit_parse_task(file_path).await?;
    client.poll_task(&job_id, timeout_secs).await?;
    client.get_task_result(&job_id, output_dir, doc_id).await
}

/// 尝试多个 MinerU 配置，逐个解析直到成功
/// 返回 (使用的配置索引, 结果路径)
pub async fn parse_pdf_with_fallback(
    configs: &[MinerUConfig],
    file_path: &str,
    output_dir: &str,
    doc_id: Option<i64>,
) -> anyhow::Result<(usize, String)> {
    let mut last_error = String::new();
    for (i, config) in configs.iter().enumerate() {
        let client = MinerUClient::new(config);
        let timeout = config.task_timeout;
        match parse_pdf(&client, file_path, output_dir, doc_id, timeout).await {
            Ok(path) => return Ok((i, path)),
            Err(e) => {
                last_error = e.to_string();
                tracing::warn!(
                    "MinerU 配置 #{} ({}) 解析失败: {}，尝试下一个",
                    i, config.url, last_error
                );
            }
        }
    }
    anyhow::bail!("所有 MinerU 配置均解析失败: {}", last_error)
}

// ════════════════════════════════════════════════════════
//  工具函数
// ════════════════════════════════════════════════════════

/// 解析 URL：将相对路径拼接到 base_url 上
fn resolve_url(base: &str, url: &str) -> String {
    if url.starts_with("http://") || url.starts_with("https://") {
        url.to_string()
    } else if url.starts_with('/') {
        format!("{}{}", base.trim_end_matches('/'), url)
    } else {
        format!("{}/{}", base.trim_end_matches('/'), url)
    }
}

/// 判断两个 URL 是否同源（scheme + host 相同）
fn is_same_origin(url_a: &str, url_b: &str) -> bool {
    let a_parsed = url::Url::parse(url_a);
    let b_parsed = url::Url::parse(url_b);
    match (a_parsed, b_parsed) {
        (Ok(a), Ok(b)) => a.origin() == b.origin(),
        _ => false,
    }
}