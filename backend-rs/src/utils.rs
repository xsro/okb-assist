//! 工具函数。

use std::path::{Path, PathBuf};

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

/// 返回当前 UTC 时间的 SQLite DateTime 字符串（微秒级，与 Python SQLAlchemy 存储格式一致）
pub fn now_datetime() -> String {
    chrono::Utc::now().format("%Y-%m-%d %H:%M:%S%.6f").to_string()
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
