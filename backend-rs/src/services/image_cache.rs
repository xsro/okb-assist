//! 文档图片资源包（`images.zip`）的**按条目**读取与缓存。
//!
//! 背景（为什么图片还是打成 zip，而不是散落在目录里）：文献库经常放在 U 盘（exFAT/FAT32）上，
//! 每篇文档几十张图片的松散文件会产生严重簇浪费、目录项上限与 FAT 链碎片问题，所以
//! **一篇文档一个 `images.zip`** 的存档粒度保持不变（条目是 `CompressionMethod::Stored`，
//! 见 `services/mineru.rs`）。这个选择是对的，需要改的只是**读路径**。
//!
//! 阅读页的一篇文章通常引用十几到几十张图片，而图片是按需取值的
//! （`GET /assist/api/documents/:id/image/:filename`）。旧实现把**整包**读进内存当缓存单位，
//! 于是「包多大就分配多大」，超过 16 MB 的包甚至完全不缓存、每张图都重读整包；并且每个请求
//! 都要重新线性扫一遍 zip 中央目录。
//!
//! 现在的策略是缓存两种更小的东西：
//! - **中央目录索引**（每篇文档一份，`basename → 条目下标`，只解析一次）；
//! - **解出来的单张图片**（按字节做 LRU，[`CACHE_BUDGET_BYTES`] 预算）。
//!
//! 未命中时只 `File::open` + 解析中央目录（KB 级）+ seek 读目标条目，**内存增量 ≈ 单张图片大小**，
//! 与包大小无关。命中时零磁盘 I/O。
//!
//! 失效判断仍用「修改时间（纳秒）+ 文件大小」，因此 `replace-asset` 换包后会自然失效；
//! 需要立即失效可调用 [`invalidate`]。

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::UNIX_EPOCH;

/// 缓存总字节预算（按**图片字节**计；测试用小值以便验证淘汰）
#[cfg(not(test))]
const CACHE_BUDGET_BYTES: usize = 64 * 1024 * 1024;
#[cfg(test)]
const CACHE_BUDGET_BYTES: usize = 1024 * 1024;

/// 单张图片超过该大小不进缓存（正常文献配图远小于此）
#[cfg(not(test))]
const MAX_IMAGE_BYTES: u64 = 8 * 1024 * 1024;
#[cfg(test)]
const MAX_IMAGE_BYTES: u64 = 256 * 1024;

/// 单条目解压上限（防 zip 炸弹）
const MAX_ENTRY_BYTES: u64 = 32 * 1024 * 1024;

/// 中央目录索引最多缓存多少篇文档（每篇约 数十字节 × 图片数，字节量极小）
#[cfg(not(test))]
const MAX_INDEX_DOCS: usize = 512;
#[cfg(test)]
const MAX_INDEX_DOCS: usize = 4;

/// 读取一张图片的结果。
pub enum ImageLookup {
    /// 200：图片字节（`Arc<[u8]>` 可用 `Bytes::from_owner` 零拷贝转成响应体）
    Found(Arc<[u8]>),
    /// 404：包能打开，但没有这个文件名
    Missing,
    /// 404：包文件缺失 / 不是普通文件
    NoPackage,
    /// 500：包打不开、条目损坏或超过大小上限
    Corrupt(String),
}

/// 一篇文档的中央目录索引
struct IndexEntry {
    /// 包文件修改时间（纳秒），与 `size` 一起用于失效判断
    mtime_nanos: u64,
    size: u64,
    /// 最近一次访问的逻辑时钟，用于按文档数的 LRU 淘汰
    tick: u64,
    /// `basename → 中央目录下标`
    names: HashMap<String, usize>,
}

/// 缓存的单张图片
struct ImageEntry {
    /// 包文件修改时间（纳秒），与 `size` 一起用于失效判断
    mtime_nanos: u64,
    size: u64,
    /// 最近一次访问的逻辑时钟，用于按字节预算的 LRU 淘汰
    tick: u64,
    /// 解出来的图片字节（`Arc<[u8]>`：可零拷贝转成 `Bytes`）
    bytes: Arc<[u8]>,
}

#[derive(Default)]
struct Store {
    indices: HashMap<i64, IndexEntry>,
    images: HashMap<(i64, String), ImageEntry>,
    /// 仅统计图片字节（索引字节量可忽略）
    total_bytes: usize,
    tick: u64,
    index_tick: u64,
}

fn store() -> &'static Mutex<Store> {
    static STORE: OnceLock<Mutex<Store>> = OnceLock::new();
    STORE.get_or_init(|| Mutex::new(Store::default()))
}

/// 读取一张图片：只读中央目录 + 目标条目；命中缓存时不碰磁盘。
pub async fn read_image(doc_id: i64, zip_path: &str, filename: &str) -> ImageLookup {
    let meta = match tokio::fs::metadata(zip_path).await {
        Ok(m) if m.is_file() => m,
        _ => return ImageLookup::NoPackage,
    };
    let size = meta.len();
    let mtime_nanos = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);

    if let Some(bytes) = lookup_image(doc_id, filename, mtime_nanos, size) {
        tracing::debug!(
            "图片缓存命中 (doc_id={}, {}, {} 字节)",
            doc_id,
            filename,
            bytes.len()
        );
        return ImageLookup::Found(bytes);
    }

    let names = lookup_index(doc_id, mtime_nanos, size);

    // 解析中央目录 + 解压单条目是 CPU/阻塞操作，放后台线程
    let path = zip_path.to_string();
    let want = filename.to_string();
    let res = tokio::task::spawn_blocking(move || read_entry(&path, &want, names, mtime_nanos, size)).await;

    match res {
        Ok(Ok((Some(bytes), names))) => {
            insert_index(doc_id, mtime_nanos, size, names);
            let bytes: Arc<[u8]> = bytes.into();
            if (bytes.len() as u64) <= MAX_IMAGE_BYTES {
                insert_image(doc_id, filename, mtime_nanos, size, bytes.clone());
            } else {
                tracing::debug!(
                    "单图过大不缓存 (doc_id={}, {}, {} 字节 > {})",
                    doc_id,
                    filename,
                    bytes.len(),
                    MAX_IMAGE_BYTES
                );
            }
            ImageLookup::Found(bytes)
        }
        Ok(Ok((None, names))) => {
            insert_index(doc_id, mtime_nanos, size, names);
            ImageLookup::Missing
        }
        Ok(Err(msg)) => {
            tracing::warn!("读取图片失败 (doc_id={}, {}): {}", doc_id, filename, msg);
            ImageLookup::Corrupt(msg)
        }
        Err(e) => {
            tracing::warn!("读取图片任务失败 (doc_id={}, {}): {}", doc_id, filename, e);
            ImageLookup::Corrupt(format!("读取任务失败: {e}"))
        }
    }
}

/// 丢弃某个文档的全部缓存（索引 + 图片；替换/删除资源包时调用）
pub fn invalidate(doc_id: i64) {
    let mut s = store().lock().unwrap();
    s.indices.remove(&doc_id);
    let mut freed = 0usize;
    s.images.retain(|(id, _), e| {
        if *id == doc_id {
            freed += e.bytes.len();
            false
        } else {
            true
        }
    });
    s.total_bytes = s.total_bytes.saturating_sub(freed);
}

/// 清空全部缓存（仅测试用）
#[cfg(test)]
pub fn clear() {
    let mut s = store().lock().unwrap();
    s.indices.clear();
    s.images.clear();
    s.total_bytes = 0;
    s.tick = 0;
    s.index_tick = 0;
}

/// 当前缓存的 (图片数, 图片字节数, 索引文档数)，仅测试用
#[cfg(test)]
pub fn stats() -> (usize, usize, usize) {
    let s = store().lock().unwrap();
    (s.images.len(), s.total_bytes, s.indices.len())
}

// ===== 测试探针：统计「读盘字节数」与「中央目录解析次数」（release 下完全不存在） =====

#[cfg(test)]
static READ_BYTES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
#[cfg(test)]
static INDEX_BUILDS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

#[cfg(test)]
fn read_bytes() -> u64 {
    READ_BYTES.load(std::sync::atomic::Ordering::Relaxed)
}

#[cfg(test)]
fn index_builds() -> u64 {
    INDEX_BUILDS.load(std::sync::atomic::Ordering::Relaxed)
}

#[cfg(test)]
fn reset_probe() {
    READ_BYTES.store(0, std::sync::atomic::Ordering::Relaxed);
    INDEX_BUILDS.store(0, std::sync::atomic::Ordering::Relaxed);
}

fn lookup_image(doc_id: i64, filename: &str, mtime_nanos: u64, size: u64) -> Option<Arc<[u8]>> {
    let key = (doc_id, filename.to_string());
    let mut s = store().lock().unwrap();
    s.tick += 1;
    let tick = s.tick;

    let stale = match s.images.get(&key) {
        Some(e) => e.mtime_nanos != mtime_nanos || e.size != size,
        None => return None,
    };
    if stale {
        // 包已被替换：丢掉旧条目，交给调用方重新读取
        if let Some(old) = s.images.remove(&key) {
            s.total_bytes = s.total_bytes.saturating_sub(old.bytes.len());
        }
        return None;
    }

    let entry = s.images.get_mut(&key)?;
    entry.tick = tick;
    Some(entry.bytes.clone())
}

fn insert_image(doc_id: i64, filename: &str, mtime_nanos: u64, size: u64, bytes: Arc<[u8]>) {
    let key = (doc_id, filename.to_string());
    let mut s = store().lock().unwrap();
    s.tick += 1;
    let tick = s.tick;

    if let Some(old) = s.images.remove(&key) {
        s.total_bytes = s.total_bytes.saturating_sub(old.bytes.len());
    }
    s.total_bytes += bytes.len();
    s.images.insert(
        key,
        ImageEntry {
            mtime_nanos,
            size,
            tick,
            bytes,
        },
    );

    // 按 LRU 淘汰到预算以内
    while s.total_bytes > CACHE_BUDGET_BYTES && !s.images.is_empty() {
        let victim = s
            .images
            .iter()
            .min_by_key(|(_, e)| e.tick)
            .map(|(k, _)| k.clone());
        match victim {
            Some(k) => {
                if let Some(old) = s.images.remove(&k) {
                    s.total_bytes = s.total_bytes.saturating_sub(old.bytes.len());
                    tracing::debug!(
                        "图片缓存淘汰 (doc_id={}, {}, 释放 {} 字节, 剩余 {} 字节)",
                        k.0,
                        k.1,
                        old.bytes.len(),
                        s.total_bytes
                    );
                }
            }
            None => break,
        }
    }
}

fn lookup_index(doc_id: i64, mtime_nanos: u64, size: u64) -> Option<HashMap<String, usize>> {
    let mut s = store().lock().unwrap();
    s.index_tick += 1;
    let tick = s.index_tick;

    let stale = match s.indices.get(&doc_id) {
        Some(e) => e.mtime_nanos != mtime_nanos || e.size != size,
        None => return None,
    };
    if stale {
        s.indices.remove(&doc_id);
        return None;
    }

    let entry = s.indices.get_mut(&doc_id)?;
    entry.tick = tick;
    Some(entry.names.clone())
}

fn insert_index(doc_id: i64, mtime_nanos: u64, size: u64, names: HashMap<String, usize>) {
    let mut s = store().lock().unwrap();
    s.index_tick += 1;
    let tick = s.index_tick;

    s.indices.insert(
        doc_id,
        IndexEntry {
            mtime_nanos,
            size,
            tick,
            names,
        },
    );

    // 索引按文档数淘汰（字节量极小，不值得按字节算）
    while s.indices.len() > MAX_INDEX_DOCS {
        let victim = s
            .indices
            .iter()
            .min_by_key(|(_, e)| e.tick)
            .map(|(id, _)| *id);
        match victim {
            Some(id) => {
                s.indices.remove(&id);
            }
            None => break,
        }
    }
}

/// 条目名是否可安全当作路径（与 `ZipFile::enclosed_name` 的拒绝规则保持同义）：
/// 拒绝绝对路径与任何 `..` 段。真正返回内容时只按 basename 匹配，不会落到磁盘上。
fn safe_entry_name(name: &str) -> bool {
    if name.starts_with('/') || name.starts_with('\\') {
        return false;
    }
    // Windows 风格的盘符绝对路径
    let bytes = name.as_bytes();
    if bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' {
        return false;
    }
    !name.split(['/', '\\']).any(|c| c == "..")
}

/// 建索引：只读中央目录里已有的名字，**不解压任何数据、也不 seek 到条目头**。
fn collect_names<R: std::io::Read + std::io::Seek>(
    archive: &zip::ZipArchive<R>,
) -> HashMap<String, usize> {
    let mut map = HashMap::new();
    for (i, name) in archive.file_names().enumerate() {
        // 目录条目（zip 约定以 `/` 结尾）
        if name.ends_with('/') || name.ends_with('\\') {
            continue;
        }
        if !safe_entry_name(name) {
            continue;
        }
        let Some(base) = std::path::Path::new(name).file_name() else {
            continue;
        };
        let base = base.to_string_lossy().to_string();
        if base.is_empty() {
            continue;
        }
        // 同名取第一个，与旧实现（线性扫 + break）一致
        map.entry(base).or_insert(i);
    }
    map
}

/// 打开包 → （必要时）建索引 → 读目标条目。返回 (图片字节, 索引)，缺失时字节为 `None`。
fn read_entry(
    path: &str,
    want: &str,
    names: Option<HashMap<String, usize>>,
    expect_mtime: u64,
    expect_size: u64,
) -> Result<(Option<Vec<u8>>, HashMap<String, usize>), String> {
    use std::io::Read;

    let file = std::fs::File::open(path).map_err(|e| format!("打开图片包失败: {e}"))?;

    // 指纹复核：避免 `stat` 与 `open` 之间包被替换（rename 后 fd 指向旧 inode）
    let meta = file.metadata().map_err(|e| format!("读取图片包属性失败: {e}"))?;
    let actual_mtime = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    if actual_mtime != expect_mtime || meta.len() != expect_size {
        return Err("图片包在读取过程中被替换，请重试".to_string());
    }

    let mut archive = zip::ZipArchive::new(std::io::BufReader::new(file))
        .map_err(|e| format!("解析图片包失败: {e}"))?;

    let names = match names {
        Some(n) => n,
        None => {
            #[cfg(test)]
            INDEX_BUILDS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            collect_names(&archive)
        }
    };

    let Some(&idx) = names.get(want) else {
        return Ok((None, names));
    };

    let mut entry = archive
        .by_index(idx)
        .map_err(|e| format!("读取条目失败（{want}）: {e}"))?;
    let declared = entry.size();
    if declared > MAX_ENTRY_BYTES {
        return Err(format!("图片过大: {declared} 字节 > 上限 {MAX_ENTRY_BYTES}"));
    }
    let mut buf = Vec::with_capacity(declared.min(MAX_ENTRY_BYTES) as usize);
    // 用 take 兜住「声明大小造假」的包：实际最多读 MAX_ENTRY_BYTES + 1
    Read::take(&mut entry, MAX_ENTRY_BYTES + 1)
        .read_to_end(&mut buf)
        .map_err(|e| format!("解压条目失败（{want}）: {e}"))?;
    if buf.len() as u64 > MAX_ENTRY_BYTES {
        return Err(format!(
            "图片解压后超上限: >{} 字节（上限 {MAX_ENTRY_BYTES}）",
            MAX_ENTRY_BYTES
        ));
    }
    #[cfg(test)]
    READ_BYTES.fetch_add(buf.len() as u64, std::sync::atomic::Ordering::Relaxed);
    Ok((Some(buf), names))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// 全局缓存与探针是进程级共享的，测试必须串行，否则互相污染
    fn test_guard() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: Mutex<()> = Mutex::new(());
        let guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        clear();
        reset_probe();
        guard
    }

    /// 写一个 zip 文件；条目按顺序写入，`compression` 控制是否压缩
    fn write_zip(
        path: &str,
        entries: &[(&str, &[u8])],
        compression: zip::CompressionMethod,
    ) {
        let file = std::fs::File::create(path).unwrap();
        let mut w = zip::ZipWriter::new(file);
        let opts = zip::write::SimpleFileOptions::default()
            .compression_method(compression)
            .large_file(true);
        for (name, data) in entries {
            w.start_file(*name, opts).unwrap();
            w.write_all(data).unwrap();
        }
        w.finish().unwrap();
    }

    fn tmp_zip(dir: &tempfile::TempDir, name: &str) -> String {
        dir.path().join(name).to_string_lossy().to_string()
    }

    fn found_bytes(l: ImageLookup) -> Vec<u8> {
        match l {
            ImageLookup::Found(b) => b.to_vec(),
            ImageLookup::Missing => panic!("期望 Found，实际 Missing"),
            ImageLookup::NoPackage => panic!("期望 Found，实际 NoPackage"),
            ImageLookup::Corrupt(m) => panic!("期望 Found，实际 Corrupt: {m}"),
        }
    }

    fn is_missing(l: ImageLookup) -> bool {
        matches!(l, ImageLookup::Missing)
    }

    /// 200 KB 的伪图片内容（内容不同，长度可按序号区分）
    fn image_bytes(seed: u8, len: usize) -> Vec<u8> {
        (0..len).map(|i| seed.wrapping_add(i as u8)).collect()
    }

    // T1：读一张图只读那一张，不整包读入
    #[tokio::test]
    async fn reads_only_target_entry() {
        let _g = test_guard();
        let dir = tempfile::tempdir().unwrap();
        let path = tmp_zip(&dir, "a.zip");

        let blobs: Vec<Vec<u8>> = (0..20).map(|i| image_bytes(i as u8, 200 * 1024)).collect();
        let entries: Vec<(String, &[u8])> = blobs
            .iter()
            .enumerate()
            .map(|(i, b)| (format!("images/fig-{i}.png"), b.as_slice()))
            .collect();
        let refs: Vec<(&str, &[u8])> = entries.iter().map(|(n, d)| (n.as_str(), *d)).collect();
        write_zip(&path, &refs, zip::CompressionMethod::Stored);

        let got = found_bytes(read_image(1, &path, "fig-3.png").await);
        assert_eq!(got, blobs[3]);
        // 读盘字节 ≈ 目标图 + 中央目录，远小于整包（20 × 200 KB）
        assert!(
            read_bytes() < 2 * 200 * 1024,
            "读盘 {} 字节，应只读目标条目",
            read_bytes()
        );
        assert_eq!(stats().0, 1, "图片应进缓存");
    }

    // T2：同一篇文档连续读多张图，中央目录只解析一次
    #[tokio::test]
    async fn index_built_once_per_doc() {
        let _g = test_guard();
        let dir = tempfile::tempdir().unwrap();
        let path = tmp_zip(&dir, "b.zip");

        let blobs: Vec<Vec<u8>> = (0..5).map(|i| image_bytes(i as u8, 64 * 1024)).collect();
        let entries: Vec<(String, &[u8])> = blobs
            .iter()
            .enumerate()
            .map(|(i, b)| (format!("images/fig-{i}.png"), b.as_slice()))
            .collect();
        let refs: Vec<(&str, &[u8])> = entries.iter().map(|(n, d)| (n.as_str(), *d)).collect();
        write_zip(&path, &refs, zip::CompressionMethod::Stored);

        for i in 0..3 {
            let got = found_bytes(read_image(7, &path, &format!("fig-{i}.png")).await);
            assert_eq!(got, blobs[i]);
        }
        assert_eq!(index_builds(), 1, "中央目录应只解析一次");
        assert_eq!(stats().2, 1);
    }

    // T3：命中缓存时零磁盘 I/O
    #[tokio::test]
    async fn cache_hit_avoids_disk() {
        let _g = test_guard();
        let dir = tempfile::tempdir().unwrap();
        let path = tmp_zip(&dir, "c.zip");
        let data = image_bytes(1, 1000);
        write_zip(
            &path,
            &[("images/fig-1.png", data.as_slice())],
            zip::CompressionMethod::Stored,
        );

        assert_eq!(found_bytes(read_image(2, &path, "fig-1.png").await), data);
        let after_first = read_bytes();
        assert!(after_first >= data.len() as u64);

        assert_eq!(found_bytes(read_image(2, &path, "fig-1.png").await), data);
        assert_eq!(read_bytes(), after_first, "命中缓存后不应再读盘");
        assert_eq!(index_builds(), 1);
    }

    // T4：图片缓存按字节预算 LRU 淘汰
    #[tokio::test]
    async fn entry_lru_evicts_by_bytes() {
        let _g = test_guard();
        // 每张 400 KB，预算 1 MB → 第 3 张插入后必须淘汰最久未用的
        let blob: Arc<[u8]> = Arc::from(vec![0u8; 400 * 1024]);
        insert_image(1, "a.png", 0, 0, blob.clone());
        insert_image(1, "b.png", 0, 0, blob.clone());
        assert_eq!(stats().0, 2);
        insert_image(1, "c.png", 0, 0, blob.clone());

        let (count, bytes, _) = stats();
        assert!(bytes <= CACHE_BUDGET_BYTES, "总字节 {} 超预算", bytes);
        assert!(count < 3, "应淘汰最久未用的一张");
        assert!(lookup_image(1, "c.png", 0, 0).is_some(), "最新的必须留下");
    }

    // T5：包能打开但没有该文件名 → Missing（不是 Corrupt）
    #[tokio::test]
    async fn missing_entry_returns_missing() {
        let _g = test_guard();
        let dir = tempfile::tempdir().unwrap();
        let path = tmp_zip(&dir, "d.zip");
        write_zip(
            &path,
            &[("images/fig-1.png", b"x".as_slice())],
            zip::CompressionMethod::Stored,
        );

        assert!(is_missing(read_image(3, &path, "missing.png").await));
        // 索引仍应建好并缓存（下次命中就不用再解析）
        assert_eq!(stats().2, 1);
    }

    // T6：包损坏 → Corrupt
    #[tokio::test]
    async fn corrupt_zip_returns_corrupt() {
        let _g = test_guard();
        let dir = tempfile::tempdir().unwrap();

        let path = tmp_zip(&dir, "junk.zip");
        std::fs::write(&path, b"not a zip at all").unwrap();
        assert!(matches!(read_image(4, &path, "a.png").await, ImageLookup::Corrupt(_)));

        let empty = tmp_zip(&dir, "empty.zip");
        std::fs::write(&empty, b"").unwrap();
        assert!(matches!(read_image(4, &empty, "a.png").await, ImageLookup::Corrupt(_)));
    }

    // T7：包文件不存在 / 不是普通文件 → NoPackage
    #[tokio::test]
    async fn no_package() {
        let _g = test_guard();
        let dir = tempfile::tempdir().unwrap();
        let missing = tmp_zip(&dir, "nope.zip");
        assert!(matches!(
            read_image(5, &missing, "a.png").await,
            ImageLookup::NoPackage
        ));
        // 目录不是普通文件
        let dir_path = dir.path().to_string_lossy().to_string();
        assert!(matches!(
            read_image(5, &dir_path, "a.png").await,
            ImageLookup::NoPackage
        ));
    }

    // T8：按 basename 匹配；请求名里的路径段不参与匹配
    #[tokio::test]
    async fn basename_matching_and_traversal() {
        let _g = test_guard();
        let dir = tempfile::tempdir().unwrap();
        let path = tmp_zip(&dir, "e.zip");
        write_zip(
            &path,
            &[("images/figure-1.png", b"png-bytes".as_slice())],
            zip::CompressionMethod::Stored,
        );

        assert_eq!(found_bytes(read_image(6, &path, "figure-1.png").await), b"png-bytes");
        assert!(is_missing(read_image(6, &path, "../figure-1.png").await));
        assert!(is_missing(read_image(6, &path, "foo/../figure-1.png").await));
        assert!(is_missing(read_image(6, &path, "images/figure-1.png").await));
    }

    // T9：目录条目不参与匹配
    #[tokio::test]
    async fn dir_entry_skipped() {
        let _g = test_guard();
        let dir = tempfile::tempdir().unwrap();
        let path = tmp_zip(&dir, "f.zip");
        write_zip(
            &path,
            &[
                ("images/", b"".as_slice()),
                ("images/a.png", b"aaa".as_slice()),
            ],
            zip::CompressionMethod::Stored,
        );

        assert_eq!(found_bytes(read_image(8, &path, "a.png").await), b"aaa");
        // 目录条目名去掉尾部斜杠后是 "images"，但不能被当作图片命中
        assert!(is_missing(read_image(8, &path, "images").await));
    }

    // T10：单图超过上限时照常返回，但不进缓存
    #[tokio::test]
    async fn oversize_image_not_cached() {
        let _g = test_guard();
        let dir = tempfile::tempdir().unwrap();
        let path = tmp_zip(&dir, "g.zip");
        let big = vec![7u8; (MAX_IMAGE_BYTES + 1024) as usize];
        write_zip(
            &path,
            &[("images/big.png", big.as_slice())],
            zip::CompressionMethod::Stored,
        );

        let got = found_bytes(read_image(9, &path, "big.png").await);
        assert_eq!(got.len(), big.len());
        let (count, bytes, _) = stats();
        assert_eq!(count, 0, "超大单图不应进缓存");
        assert_eq!(bytes, 0);
    }

    // T11：包被替换（大小变化）后必须读到新内容
    #[tokio::test]
    async fn mtime_change_invalidates() {
        let _g = test_guard();
        let dir = tempfile::tempdir().unwrap();
        let path = tmp_zip(&dir, "h.zip");

        write_zip(
            &path,
            &[("images/fig-1.png", b"AAAA".as_slice())],
            zip::CompressionMethod::Stored,
        );
        assert_eq!(found_bytes(read_image(10, &path, "fig-1.png").await), b"AAAA");

        write_zip(
            &path,
            &[("images/fig-1.png", b"BBBBBBBB".as_slice())],
            zip::CompressionMethod::Stored,
        );
        assert_eq!(
            found_bytes(read_image(10, &path, "fig-1.png").await),
            b"BBBBBBBB",
            "换包后不能读到旧缓存"
        );
    }

    // T12：invalidate 同时清索引与图片
    #[tokio::test]
    async fn invalidate_clears_both() {
        let _g = test_guard();
        let dir = tempfile::tempdir().unwrap();
        let path = tmp_zip(&dir, "i.zip");
        write_zip(
            &path,
            &[("images/fig-1.png", b"zzz".as_slice())],
            zip::CompressionMethod::Stored,
        );

        found_bytes(read_image(11, &path, "fig-1.png").await);
        assert!(stats().0 > 0 && stats().2 > 0);
        invalidate(11);
        assert_eq!(stats(), (0, 0, 0));
    }

    // T13：Deflated 条目（用户 replace-asset 上传的压缩包）也能读
    #[tokio::test]
    async fn zip_deflated_entry_still_works() {
        let _g = test_guard();
        let dir = tempfile::tempdir().unwrap();
        let path = tmp_zip(&dir, "j.zip");
        let data = vec![b'a'; 40 * 1024];
        write_zip(
            &path,
            &[("images/fig-1.png", data.as_slice())],
            zip::CompressionMethod::Deflated,
        );

        assert_eq!(found_bytes(read_image(12, &path, "fig-1.png").await), data);
    }

    // T14：同一张图并发读取不出错
    #[tokio::test]
    async fn concurrent_reads_same_image() {
        let _g = test_guard();
        let dir = tempfile::tempdir().unwrap();
        let path = tmp_zip(&dir, "k.zip");
        let data = image_bytes(3, 32 * 1024);
        write_zip(
            &path,
            &[("images/fig-1.png", data.as_slice())],
            zip::CompressionMethod::Stored,
        );

        let (a, b) = tokio::join!(
            read_image(13, &path, "fig-1.png"),
            read_image(13, &path, "fig-1.png")
        );
        assert_eq!(found_bytes(a), data);
        assert_eq!(found_bytes(b), data);
    }
}
