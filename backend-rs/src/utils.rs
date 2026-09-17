//! 工具函数。

use std::path::{Path, PathBuf};

/// 计算文件 SHA256 哈希
pub fn calculate_file_hash(file_path: &str) -> std::io::Result<String> {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    let file = std::fs::File::open(file_path)?;
    let mut reader = std::io::BufReader::with_capacity(8192, file);
    std::io::copy(&mut reader, &mut hasher)?;
    Ok(hex::encode(hasher.finalize()))
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

/// 返回当前 UTC 时间 ISO8601 字符串（秒级精度，带 Z 后缀）
pub fn now_iso() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}
