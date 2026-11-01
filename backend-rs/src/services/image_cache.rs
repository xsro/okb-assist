//! 文档图片资源包（`images.zip`）的进程内缓存。
//!
//! 阅读页的一篇文章通常引用十几到几十张图片，而图片是按需求值的
//! （`GET /assist/api/documents/:id/image/:filename`）。若每个请求都重新把整份
//! `images.zip` 读进内存再解析一遍索引，一次浏览就会产生「图片数 × zip 大小」
//! 的磁盘 I/O 与内存抖动（实测单包最大 1.6 MB，20 张图就是 32 MB 的瞬时分配）。
//!
//! 这里按 **doc_id** 缓存 zip 字节，带两层约束：
//! - 总字节预算 [`CACHE_BUDGET_BYTES`]，超出后按 LRU 淘汰；
//! - 单包超过 [`MAX_ENTRY_BYTES`] 不进入缓存（避免一个巨型包把预算吃光）。
//!
//! 失效判断用「修改时间（纳秒）+ 文件大小」，因此 `replace-asset` 换包后
//! 会自然失效；如需立即失效可调用 [`invalidate`]。

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::UNIX_EPOCH;

/// 缓存总字节预算（64 MB；测试用小值以便验证淘汰）
#[cfg(not(test))]
const CACHE_BUDGET_BYTES: usize = 64 * 1024 * 1024;
#[cfg(test)]
const CACHE_BUDGET_BYTES: usize = 4 * 1024;
/// 单个 zip 超过该大小不缓存（16 MB）
const MAX_ENTRY_BYTES: u64 = 16 * 1024 * 1024;

struct Entry {
    /// 文件修改时间（纳秒），与 `size` 一起用于失效判断
    mtime_nanos: u64,
    size: u64,
    /// 最近一次访问的逻辑时钟，用于 LRU 淘汰
    tick: u64,
    /// 原始 zip 字节（`Arc<Vec<u8>>`：从读盘结果零拷贝转移所有权）
    bytes: Arc<Vec<u8>>,
}

#[derive(Default)]
struct Store {
    entries: HashMap<i64, Entry>,
    total_bytes: usize,
    tick: u64,
}

fn store() -> &'static Mutex<Store> {
    static STORE: OnceLock<Mutex<Store>> = OnceLock::new();
    STORE.get_or_init(|| Mutex::new(Store::default()))
}

/// 读取某个文档的 `images.zip` 内容（优先命中缓存）。
///
/// 文件不存在、不可读或超过单包上限时返回 `None`（超过上限时退化为每次读盘，不缓存）。
pub async fn load(doc_id: i64, zip_path: &str) -> Option<Arc<Vec<u8>>> {
    let meta = tokio::fs::metadata(zip_path).await.ok()?;
    if !meta.is_file() {
        return None;
    }
    let size = meta.len();
    let mtime_nanos = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);

    if let Some(bytes) = lookup(doc_id, mtime_nanos, size) {
        tracing::debug!("图片包缓存命中 (doc_id={}, {} 字节)", doc_id, size);
        return Some(bytes);
    }
    tracing::debug!("图片包缓存未命中，读盘 (doc_id={}, {})", doc_id, zip_path);

    // 未命中：读盘放进 spawn_blocking，避免阻塞运行时
    let path = zip_path.to_string();
    let read = tokio::task::spawn_blocking(move || std::fs::read(&path))
        .await
        .ok()?
        .ok()?;
    let bytes = Arc::new(read);

    if size <= MAX_ENTRY_BYTES {
        insert(doc_id, mtime_nanos, size, bytes.clone());
    } else {
        tracing::debug!(
            "图片包过大不缓存 (doc_id={}, {} 字节 > {})",
            doc_id,
            size,
            MAX_ENTRY_BYTES
        );
    }
    Some(bytes)
}

/// 丢弃某个文档的缓存（替换/删除资源包时调用）
pub fn invalidate(doc_id: i64) {
    let mut s = store().lock().unwrap();
    if let Some(entry) = s.entries.remove(&doc_id) {
        s.total_bytes = s.total_bytes.saturating_sub(entry.bytes.len());
    }
}

/// 清空全部缓存（仅测试用）
#[cfg(test)]
pub fn clear() {
    let mut s = store().lock().unwrap();
    s.entries.clear();
    s.total_bytes = 0;
}

/// 当前缓存的 (文档数, 字节数)，仅测试用
#[cfg(test)]
pub fn stats() -> (usize, usize) {
    let s = store().lock().unwrap();
    (s.entries.len(), s.total_bytes)
}

fn lookup(doc_id: i64, mtime_nanos: u64, size: u64) -> Option<Arc<Vec<u8>>> {
    let mut s = store().lock().unwrap();
    s.tick += 1;
    let tick = s.tick;

    let stale = match s.entries.get(&doc_id) {
        Some(e) => e.mtime_nanos != mtime_nanos || e.size != size,
        None => return None,
    };
    if stale {
        // 文件已被替换：丢掉旧缓存，交给调用方重新读盘
        if let Some(old) = s.entries.remove(&doc_id) {
            s.total_bytes = s.total_bytes.saturating_sub(old.bytes.len());
        }
        return None;
    }

    let entry = s.entries.get_mut(&doc_id)?;
    entry.tick = tick;
    Some(entry.bytes.clone())
}

fn insert(doc_id: i64, mtime_nanos: u64, size: u64, bytes: Arc<Vec<u8>>) {
    let mut s = store().lock().unwrap();
    s.tick += 1;
    let tick = s.tick;

    if let Some(old) = s.entries.remove(&doc_id) {
        s.total_bytes = s.total_bytes.saturating_sub(old.bytes.len());
    }
    s.total_bytes += bytes.len();
    s.entries.insert(
        doc_id,
        Entry {
            mtime_nanos,
            size,
            tick,
            bytes,
        },
    );

    // 按 LRU 淘汰到预算以内
    while s.total_bytes > CACHE_BUDGET_BYTES && !s.entries.is_empty() {
        let victim = s
            .entries
            .iter()
            .min_by_key(|(_, e)| e.tick)
            .map(|(id, _)| *id);
        match victim {
            Some(id) => {
                if let Some(old) = s.entries.remove(&id) {
                    s.total_bytes = s.total_bytes.saturating_sub(old.bytes.len());
                    tracing::debug!(
                        "图片包缓存淘汰 (doc_id={}, 释放 {} 字节, 剩余 {} 字节)",
                        id,
                        old.bytes.len(),
                        s.total_bytes
                    );
                }
            }
            None => break,
        }
    }
}

/// 从 zip 字节中解出一张图片（按 basename 匹配，拒绝目录穿越名）。
pub fn read_image(zip_bytes: &[u8], filename: &str) -> Option<Vec<u8>> {
    use std::io::Read;

    let cursor = std::io::Cursor::new(zip_bytes);
    let mut archive = zip::ZipArchive::new(cursor).ok()?;

    // 先按 basename 找到 zip 内的安全路径（`enclosed_name` 会拒绝 `../` 之类的名字）
    let mut target: Option<std::path::PathBuf> = None;
    for i in 0..archive.len() {
        let entry = match archive.by_index(i) {
            Ok(e) => e,
            Err(_) => continue,
        };
        if let Some(name) = entry.enclosed_name() {
            let base = std::path::Path::new(&name)
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            if base == filename {
                target = Some(name.to_path_buf());
                break;
            }
        }
    }

    let mut img_data = Vec::new();
    let mut entry = archive.by_name(&target?.to_string_lossy()).ok()?;
    entry.read_to_end(&mut img_data).ok()?;
    Some(img_data)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_image_matches_basename() {
        use std::io::Write;

        let mut buf = Vec::new();
        {
            let mut w = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
            let opts = zip::write::SimpleFileOptions::default();
            w.start_file("images/figure-1.png", opts).unwrap();
            w.write_all(b"png-bytes").unwrap();
            w.finish().unwrap();
        }

        assert_eq!(
            read_image(&buf, "figure-1.png").as_deref(),
            Some(&b"png-bytes"[..])
        );
        assert!(read_image(&buf, "missing.png").is_none());
        assert!(read_image(b"not a zip", "figure-1.png").is_none());
    }

    #[test]
    fn lru_evicts_and_invalidates() {
        clear();
        // 同一个 doc 覆盖写：不应重复计入
        insert(1, 10, 4, Arc::new(b"aaaa".to_vec()));
        insert(1, 10, 4, Arc::new(b"bbbb".to_vec()));
        assert_eq!(stats(), (1, 4));
        assert_eq!(lookup(1, 10, 4).as_deref().map(|v| &v[..]), Some(&b"bbbb"[..]));
        // mtime 变化视为失效
        assert!(lookup(1, 11, 4).is_none());
        assert_eq!(stats(), (0, 0));

        // 超过预算（测试里 4KB）时淘汰最久未访问的条目
        insert(1, 0, 0, Arc::new(vec![0u8; 3 * 1024]));
        insert(2, 0, 0, Arc::new(vec![0u8; 3 * 1024]));
        assert_eq!(stats().0, 1);
        assert!(lookup(1, 0, 0).is_none());
        assert!(lookup(2, 0, 0).is_some());

        // 显式失效
        invalidate(2);
        assert_eq!(stats(), (0, 0));
        clear();
    }
}
