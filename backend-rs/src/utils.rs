//! 工具函数。

use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

/// 计算文件 SHA256 哈希（可能阻塞，请在 spawn_blocking 中调用）
pub fn calculate_file_hash(file_path: &str) -> std::io::Result<String> {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    let file = std::fs::File::open(file_path)?;
    let mut reader = std::io::BufReader::with_capacity(8192, file);
    std::io::copy(&mut reader, &mut hasher)?;
    Ok(hex::encode(hasher.finalize()))
}

/// 异步计算文件 SHA256 哈希（在后台线程执行，不阻塞 async 运行时）
pub async fn calculate_file_hash_async(file_path: String) -> std::io::Result<String> {
    tokio::task::spawn_blocking(move || calculate_file_hash(&file_path)).await
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?
}

/// 获取 uploads 文件夹绝对路径
pub fn get_uploads_folder() -> String {
    let settings = crate::settings::get_settings();
    let folder = settings.uploads_folder();
    absolute_path(&folder)
}

/// 将相对路径转换为绝对路径（相对于当前工作目录）
pub fn absolute_path(path: &str) -> String {
    let p = Path::new(path);
    if p.is_absolute() {
        p.to_string_lossy().to_string()
    } else {
        let cwd = std::env::current_dir().unwrap_or_default();
        cwd.join(p).to_string_lossy().to_string()
    }
}

/// 将绝对路径转换为相对于当前工作目录的路径
pub fn relative_path(absolute_path: &str) -> String {
    let cwd = std::env::current_dir().unwrap_or_default();
    let path = Path::new(absolute_path);
    path.strip_prefix(&cwd)
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| absolute_path.to_string())
}

/// 确保目录存在
pub fn ensure_dir(path: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(path)
}

/// 确保文件所在目录存在
pub fn ensure_parent_dir(path: &Path) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    Ok(())
}

/// 检查路径是否存在
pub fn path_exists(path: &str) -> bool {
    Path::new(path).exists()
}

/// 拼接路径
pub fn join_path(base: &str, name: &str) -> PathBuf {
    Path::new(base).join(name)
}

/// 计算字节数组的 SHA256 哈希（十六进制）
pub fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex::encode(hasher.finalize())
}

/// 异步计算字节数组的 SHA256 哈希（在后台线程执行，不阻塞 async 运行时）
pub async fn sha256_hex_async(bytes: Vec<u8>) -> String {
    tokio::task::spawn_blocking(move || sha256_hex(&bytes)).await
        .unwrap_or_else(|_| String::new())
}

/// 返回当前 UTC 时间 ISO8601 字符串（秒级精度，带 Z 后缀）
pub fn now_iso() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

/// 返回当前 UTC 时间的 SQLite DateTime 字符串（微秒级，形如 `2026-01-01 12:00:00.000000`）
pub fn now_datetime() -> String {
    chrono::Utc::now().format("%Y-%m-%d %H:%M:%S%.6f").to_string()
}

/// 解析 Token 配置：若 value 以 `!` 开头，则执行后续 shell 命令取其输出作为 token。
/// 结果通过 `OnceLock` 缓存，仅首次执行命令，后续直接返回缓存值。
///
/// 特殊处理：若命令为 `cat <path>`，直接读取文件内容，避免在 Windows 上依赖 cat 命令。
pub fn resolve_token(value: &str) -> String {
    if !value.starts_with('!') {
        return value.to_string();
    }

    let cmd_str = &value[1..];
    static CACHE: OnceLock<Mutex<Vec<(String, String)>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(Vec::new()));

    // 检查缓存
    {
        let cache = cache.lock().unwrap();
        if let Some(entry) = cache.iter().find(|(k, _)| k == value) {
            return entry.1.clone();
        }
    }

    // 特殊处理：!cat <path> — 直接读取文件，避免在 Windows 上依赖 cat 命令
    let trimmed = cmd_str.trim();
    if let Some(path_str) = trimmed.strip_prefix("cat ").or_else(|| trimmed.strip_prefix("cat\t")) {
        // 去除可能的引号
        let path_str = path_str.trim().trim_matches('"').trim_matches('\'');
        match std::fs::read_to_string(path_str) {
            Ok(content) => {
                let resolved = content.trim().to_string();
                if !resolved.is_empty() {
                    tracing::info!("token 通过直接读取文件解析: {} -> {} 字节", path_str, resolved.len());
                    let mut cache = cache.lock().unwrap();
                    cache.push((value.to_string(), resolved.clone()));
                    if cache.len() > 32 {
                        cache.remove(0);
                    }
                }
                return resolved;
            }
            Err(e) => {
                tracing::warn!("读取 token 文件失败: {}: {}", path_str, e);
                return value.to_string();
            }
        }
    }

    // 执行命令
    let output = if cfg!(target_os = "windows") {
        std::process::Command::new("cmd")
            .args(["/C", cmd_str])
            .output()
    } else {
        std::process::Command::new("sh")
            .args(["-c", cmd_str])
            .output()
    };

    let resolved = match output {
        Ok(out) if out.status.success() => {
            String::from_utf8_lossy(&out.stdout).trim().to_string()
        }
        Ok(out) => {
            let stderr = String::from_utf8_lossy(&out.stderr);
            tracing::warn!("token 命令执行失败 (exit={}): {}",
                out.status.code().unwrap_or(-1), stderr.trim());
            value.to_string()
        }
        Err(e) => {
            tracing::warn!("token 命令无法启动: {}", e);
            value.to_string()
        }
    };

    if resolved != value {
        tracing::info!("token 通过命令解析: {} -> {} 字节", cmd_str, resolved.len());
    }

    if !resolved.is_empty() {
        let mut cache = cache.lock().unwrap();
        cache.push((value.to_string(), resolved.clone()));
        if cache.len() > 32 {
            cache.remove(0);
        }
    }

    resolved
}

// ── 文件尾部逐行反读 ──────────────────────────────────────

/// 单次向文件头方向读取的块大小
const REVERSE_LINES_CHUNK: u64 = 64 * 1024;

/// 从文件末尾向前逐行读取的迭代器。
///
/// 用于「日志查看只要最后 N 行」这类场景：只回扫需要的字节，
/// 不把整份文件读进内存。`max_bytes` 是回扫字节上限，用尽后迭代结束
/// （调用方可通过 [`ReverseLines::read_bytes`] 判断是否因上限被截断）。
pub struct ReverseLines {
    file: std::fs::File,
    /// 下一次向文件头方向读取的截止位置
    pos: u64,
    /// 尚未切分成行的尾部字节（顺序与文件一致）
    buf: Vec<u8>,
    read_bytes: u64,
    max_bytes: u64,
    finished: bool,
}

impl ReverseLines {
    /// 打开文件并从末尾开始反向读行。文件为空时迭代立即结束。
    pub fn open(path: &Path, max_bytes: u64) -> std::io::Result<Self> {
        let file = std::fs::File::open(path)?;
        let len = file.metadata()?.len();
        Ok(Self {
            file,
            pos: len,
            buf: Vec::new(),
            read_bytes: 0,
            max_bytes,
            finished: false,
        })
    }

    /// 已经从文件中实际读取的字节数
    pub fn read_bytes(&self) -> u64 {
        self.read_bytes
    }

    /// 是否已经退到文件开头（false = 还有更早的内容未读）
    pub fn at_start(&self) -> bool {
        self.pos == 0
    }

    /// 向文件头方向再读一块，拼到 `buf` 前面。返回 false 表示没有更多可读数据。
    fn fill(&mut self) -> bool {
        use std::io::{Read, Seek, SeekFrom};
        if self.pos == 0 || self.read_bytes >= self.max_bytes {
            return false;
        }
        let n = self.pos
            .min(REVERSE_LINES_CHUNK)
            .min(self.max_bytes - self.read_bytes);
        let start = self.pos - n;
        let mut chunk = vec![0u8; n as usize];
        if self.file.seek(SeekFrom::Start(start)).is_err() {
            return false;
        }
        if self.file.read_exact(&mut chunk).is_err() {
            return false;
        }
        self.pos = start;
        self.read_bytes += n;
        // 新块在前、旧残留（未成行的行尾）在后
        chunk.extend_from_slice(&self.buf);
        self.buf = chunk;
        true
    }
}

impl Iterator for ReverseLines {
    type Item = String;

    fn next(&mut self) -> Option<String> {
        loop {
            if self.finished {
                return None;
            }
            // 从 `buf` 尾部切出一整行（不包含行尾的 '\n'）
            if let Some(idx) = self.buf.iter().rposition(|&b| b == b'\n') {
                let line = self.buf.split_off(idx + 1);
                self.buf.pop(); // 丢掉 '\n'
                if line.is_empty() {
                    // 文件末尾的换行 / 连续空行，交给上层的 trim 判断
                    continue;
                }
                return Some(String::from_utf8_lossy(&line).into_owned());
            }
            // `buf` 里没有换行：继续向文件头读取；读不到（到文件头或字节上限）就收尾
            if self.pos == 0 || !self.fill() {
                self.finished = true;
                let line = std::mem::take(&mut self.buf);
                if line.is_empty() {
                    return None;
                }
                // 到达字节上限时这里可能是一行被截断的前半段，仍返回给调用方
                return Some(String::from_utf8_lossy(&line).into_owned());
            }
        }
    }
}

/// 检查给定的 IP 字符串是否匹配任一 CIDR 子网。
///
/// 支持 IPv4 和 IPv6 CIDR 表示法（如 `192.168.1.0/24`、`10.0.0.0/8`、`::1/128`）。
/// 若 CIDR 省略前缀长度，视为 `/32`（IPv4）或 `/128`（IPv6）。
/// 若子网列表为空，始终返回 `false`（不信任任何 IP）。
pub fn ip_matches_subnets(ip: &str, subnets: &[String]) -> bool {
    use std::net::IpAddr;

    let ip_addr: IpAddr = match ip.parse() {
        Ok(addr) => addr,
        Err(_) => return false,
    };

    subnets.iter().any(|cidr| {
        let cidr = cidr.trim();
        // 尝试按 IPv4 CIDR 解析
        if let Ok(net) = cidr.parse::<ipnet::Ipv4Net>() {
            if let IpAddr::V4(ip4) = ip_addr {
                return net.contains(&ip4);
            }
        }
        // 尝试按 IPv6 CIDR 解析
        if let Ok(net) = cidr.parse::<ipnet::Ipv6Net>() {
            if let IpAddr::V6(ip6) = ip_addr {
                return net.contains(&ip6);
            }
        }
        // 尝试作为无前缀长度的 IP 解析（视为 /32 或 /128）
        if let Ok(addr) = cidr.parse::<IpAddr>() {
            return ip_addr == addr;
        }
        false
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `!cat <path>` 应直接读文件并去掉首尾空白（LLM 密钥等敏感配置用这种写法）
    #[test]
    fn resolve_token_reads_file_reference() {
        let path = std::env::temp_dir().join("okb_resolve_token_test.txt");
        std::fs::write(&path, "  secret-value\n").unwrap();

        let resolved = resolve_token(&format!("!cat {}", path.display()));
        assert_eq!(resolved, "secret-value");

        // 文件不存在时保持原样（便于在日志/配置回显中看出未解析）
        let missing = format!("!cat {}", path.with_extension("missing").display());
        assert_eq!(resolve_token(&missing), missing);

        // 非 `!` 前缀的普通值原样返回
        assert_eq!(resolve_token("plain-key"), "plain-key");
        let _ = std::fs::remove_file(&path);
    }

    /// 反向读行：与「正序读全文再取尾部」结果一致
    #[test]
    fn reverse_lines_reads_tail() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tail.log");

        // 带行尾换行
        std::fs::write(&path, "a\nb\nc\n").unwrap();
        let got: Vec<String> = ReverseLines::open(&path, 1024).unwrap().collect();
        assert_eq!(got, vec!["c", "b", "a"]);

        // 不带行尾换行
        std::fs::write(&path, "a\nb\nc").unwrap();
        let got: Vec<String> = ReverseLines::open(&path, 1024).unwrap().collect();
        assert_eq!(got, vec!["c", "b", "a"]);

        // 只有一行且没有换行
        std::fs::write(&path, "only").unwrap();
        let got: Vec<String> = ReverseLines::open(&path, 1024).unwrap().collect();
        assert_eq!(got, vec!["only"]);

        // 空文件
        std::fs::write(&path, "").unwrap();
        assert_eq!(ReverseLines::open(&path, 1024).unwrap().count(), 0);

        // 空行被跳过（与原「读全文再过滤空行」的行为一致）
        std::fs::write(&path, "a\n\n\nb\n").unwrap();
        let got: Vec<String> = ReverseLines::open(&path, 1024).unwrap().collect();
        assert_eq!(got, vec!["b", "a"]);
    }

    /// 跨块边界（单行长度 > CHUNK）与字节上限截断
    #[test]
    fn reverse_lines_handles_large_lines_and_budget() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("big.log");

        let long = "x".repeat(REVERSE_LINES_CHUNK as usize + 100);
        std::fs::write(&path, format!("{}\nsecond\n", long)).unwrap();
        let mut it = ReverseLines::open(&path, 4 * 1024 * 1024).unwrap();
        assert_eq!(it.next().unwrap(), "second");
        assert_eq!(it.next().unwrap().len(), long.len());
        assert_eq!(it.next(), None);

        // 字节预算只够读到最后一行的前半段
        let mut it = ReverseLines::open(&path, 8).unwrap();
        let first = it.next().unwrap();
        assert_eq!(it.read_bytes(), 8);
        assert!(first.ends_with("second"));
        assert_eq!(it.next(), None);
    }
}
