# 图片资源读取路径改造（第一步：seek 读 + 条目缓存）

> 状态：**✅ 已实施（2026-11-20）** ｜ 撰写：2026-11-19 ｜ 实施记录与实测数据见 §10
> 涉及文件：`backend-rs/src/services/image_cache.rs`（重写）、`backend-rs/src/routers/documents.rs`（`get_image_from_zip`）
> 前置结论：图片存档方式（**一篇文档一个 `images.zip`**）保持不变 —— 这是为规避 U 盘（exFAT/FAT32）上
> 大量小文件带来的簇浪费、目录项上限与 FAT 链碎片而做的选择，方向正确。本文只改**读路径**。

## 1. 目标与非目标

### 1.1 目标

| # | 目标 | 判定依据 |
|---|---|---|
| G1 | 读一张图**不再整包读入内存** | 读 1 张图的内存增量 ≈ 该图字节数，与包大小无关 |
| G2 | 每篇文档的**中央目录只解析一次** | 同一文档连续读 N 张图，中央目录解析次数 = 1 |
| G3 | 命中缓存时**零磁盘 I/O** | `stats()` 计数不增长；`RUST_LOG=debug` 命中日志 |
| G4 | 失败原因可区分（包缺失 / 包损坏 / 图不存在） | HTTP 404 / 500 文案分开 |

### 1.2 非目标（本次**不做**）

- ✋ 不改 `images.zip` 的**写入**布局与压缩方式（现在已是 `CompressionMethod::Stored`，见 `services/mineru.rs:1633`）；
- ✋ 不引入大 pack 文件 / SQLite BLOB / 内容寻址存储（那是"第二步"，见 §9）；
- ✋ 不加 `Range` / `ETag` / `sendfile`（Stored 已具备条件，但独立于本次改造）；
- ✋ 不改前端（`<img>` 的 URL 形状、`MarkdownViewer` 的懒加载都不变）；
- ✋ 不改路径模板（`markdown_asset_path`）、不改数据库结构。

## 2. 现状走查

### 2.1 调用链

```
<img src="/assist/api/documents/{id}/image/{filename}/">
  └─ main.rs:637            token_middleware：路径含 "/image/" → 免鉴权，注入 Role::ViewOnly
  └─ documents.rs:312-313   路由（带/不带尾斜杠）→ get_image_from_zip
       ├─ auth::assert_role(["admin","view-only","view-upload"])   // 恒通过
       ├─ fetch_doc(db, id)                                        // 每次请求一次 SQLite 查询
       ├─ paths::get_asset_path()  → data/pdfs/{id}/{id}.zip
       ├─ image_cache::load(id, zip_path)                          // ← 整包 fs::read 进内存
       └─ spawn_blocking(image_cache::read_image(&raw, filename))   // ← 每次线性扫中央目录
  └─ 200 + Content-Type + Cache-Control: public, max-age=86400
```

### 2.2 现有实现的问题

`services/image_cache.rs`（145 行）：

| 位置 | 现状 | 问题 |
|---|---|---|
| `load()` :52 | `tokio::fs::metadata` → 查缓存 → 未命中 `spawn_blocking(std::fs::read(path))` **整包读入** | 包多大就分配多大；>16 MB（`MAX_ENTRY_BYTES`）时**完全不缓存**，每张图都重读整包 |
| `read_image()` :181 | 每次 `ZipArchive::new()` + `for i in 0..len` **线性**按 basename 找目标 | 每次请求都重解一遍中央目录，O(条目数)；图片多时纯浪费 |
| `lookup()` :115 | 指纹 = `mtime(纳秒) + size` | 这部分设计正确，保留 |

一句话：**zip 作为容器是对的，错的只是"把整包当缓存单位"**。实际应该缓存"解出来的那张图"和"目录索引"。

### 2.3 先量化（实施前建议跑一次，决定是否需要后续步骤）

```bash
# 包数量 / 平均大小 / 总量
find data/pdfs -maxdepth 2 -name "*.zip" -printf "%s\n" \
  | awk '{s+=$1;n++} END{printf "%d 个包, 平均 %.2f MB, 合计 %.2f GB\n", n, s/n/1048576, s/1048576/1024}'

# 包内图片数量分布（抽 20 个样本）
find data/pdfs -maxdepth 2 -name "*.zip" | head -20 | while read z; do
  printf "%s %s\n" "$(unzip -Z1 "$z" 2>/dev/null | wc -l)" "$z"
done

# U 盘簇大小（会随卷容量变化，决定"要不要继续压文件数"）
cd /media/<你的U盘挂载点> && : > probe.bin && du -h probe.bin && rm -f probe.bin
```

> 若"平均 < 3 MB、每包几十张图"，本次改造就足以把读放大降到最低；若"平均 > 50 MB"，`>16 MB 不缓存` 这条退化路径
> 才是主要瓶颈（本次改造后它消失：不再有包级上限）。

## 3. 设计

### 3.1 数据结构

```rust
/// 缓存总字节预算（按"图片字节"计，不再按"包字节"计）
const CACHE_BUDGET_BYTES: usize = 64 * 1024 * 1024;   // test: 4 KB
/// 单张图片超过该大小不进缓存（8 MB：正常文献配图远小于此）
const MAX_IMAGE_BYTES: u64 = 8 * 1024 * 1024;
/// 单条目解压上限（防 zip 炸弹，32 MB）
const MAX_ENTRY_BYTES: u64 = 32 * 1024 * 1024;
/// 中央目录索引最多缓存多少篇文档（每篇约 数十字节 × 图片数）
const MAX_INDEX_DOCS: usize = 512;

struct IndexEntry {            // 包指纹 + basename → 条目序号
    mtime_nanos: u64,
    size: u64,
    tick: u64,                 // 索引自己的 LRU 时钟
    names: HashMap<String, usize>,
}

struct ImageEntry {            // 包指纹 + 解出的图片字节
    mtime_nanos: u64,
    size: u64,
    tick: u64,                 // 全局字节预算的 LRU 时钟
    bytes: Arc<[u8]>,          // Arc<[u8]> 而非 Arc<Vec<u8>>：可零拷贝转 Bytes（见 4.2）
}

#[derive(Default)]
struct Store {
    indices: HashMap<i64, IndexEntry>,
    images: HashMap<(i64, String), ImageEntry>,
    total_bytes: usize,
    tick: u64,
    index_tick: u64,
}
```

要点：

- **指纹语义不变**：仍是「包文件的 `mtime(纳秒)` + `size`」，与 `images` / `indices` 双向绑定；指纹不符即视为失效并删除。
- **两种 LRU 分离**：图片按**字节**预算（64 MB，命中价值最高），索引按**文档数**上限（512，字节量极小）。
- **键是 `(doc_id, filename)`**：不同文档的同名图（`images/figure-1.png`）互不干扰。
- **缓存元素用 `Arc<[u8]>`**：`bytes 1.12` 已在依赖树里（axum 也重导出为 `axum::body::Bytes`，`documents.rs:8` 已经在用），
  `Bytes::from_owner(arc)` 可以直接把缓存里的 `Arc<[u8]>` 变成响应体，**不复制图片字节**。

### 3.2 对外 API

```rust
pub enum ImageLookup {
    Found(Arc<[u8]>),         // 200（零拷贝转 Bytes::from_owner）
    Missing,                  // 404「图片不存在」（包打开了，但没有这个文件名）
    NoPackage,                // 404「图片包不存在」（文件缺失/不是普通文件）
    Corrupt(String),          // 500「图片包损坏或无法读取」（打不开、条目损坏、超上限）
}

/// 读取一张图片：只读中央目录 + 目标条目；命中缓存时不碰磁盘。
pub async fn read_image(doc_id: i64, zip_path: &str, filename: &str) -> ImageLookup;

/// 丢弃该文档全部缓存（索引 + 图片）。语义不变，调用点不变。
pub fn invalidate(doc_id: i64);

#[cfg(test)]
pub fn clear();
#[cfg(test)]
pub fn stats() -> (usize /*图片数*/, usize /*图片字节*/, usize /*索引文档数*/);
```

> 旧的 `pub async fn load(...) -> Option<Arc<Vec<u8>>>` 与 `pub fn read_image(&[u8], &str)` **删除**；
> 唯一外部调用点是 `documents.rs::get_image_from_zip`（另一个是测试）。

### 3.3 读取时序

```
read_image(doc_id, zip_path, filename)
 ├─ tokio::fs::metadata(zip_path)
 │    ├─ 失败 / 非文件 → NoPackage
 │    └─ ok → (mtime_nanos, size)
 ├─ lookup_image(doc_id, filename, mtime, size)      // 纯内存，O(1)
 │    └─ 命中 → Found(bytes)                          // ← G3：零 I/O
 ├─ names = lookup_index(doc_id, mtime, size)        // 纯内存，可能 None
 └─ spawn_blocking::
      File::open → ZipArchive::new(BufReader<File>) // 只解析中央目录（KB 级）
      names = names.unwrap_or_else(collect_names)   // basename → 下标，不解压数据
      names.get(filename) → 无 → Ok((None, names))
      archive.by_index(i) → size() 校验 → read_to_end  // 只解这一条
 ├─ insert_index(...)                                // 顺带按 MAX_INDEX_DOCS 淘汰
 ├─ bytes.len() <= MAX_IMAGE_BYTES → insert_image(...)  // 按 64 MB 预算淘汰
 └─ Found / Missing / Corrupt
```

关键实现细节：

1. **指纹复核**：`File::metadata()`（打开后的 fd）与之前 `save` 时的指纹再比一次，避免 `stat` 与 `open` 之间被 `replace-asset` 替换；不一致就重新走一遍或直接返回 `Corrupt("图片包已被替换")`（择一，日志说明即可）。
2. **名字匹配语义保持**：按 **basename** 匹配（`enclosed_name()` 拒绝 `../`），同名取**第一个**（与现状 `break` 一致）；跳过目录条目（`is_dir()`）。
3. **不解压数据地建索引**：`zip 2.x` 用 `by_index_raw(i)`（或 `file_names()` + `index_for_name()`）取名字，`entry.size()` 取解压后大小。
4. **不做 single-flight**：同一张图并发两次请求会各解析一次（浏览器对同一 origin 最多 6 并发，量级可忽略）。缓存击穿留给第二步。
5. **持锁范围**：`Mutex<Store>` 只在查找/插入时短暂持有，**不跨 `spawn_blocking` 持有**（与现状一致）。

### 3.4 HTTP 语义（`get_image_from_zip`）

| `ImageLookup` | 状态码 | 响应体 |
|---|---|---|
| `Found` | 200 | 图片字节 + `Content-Type`（`image_mime`，不变）+ `Cache-Control: public, max-age=86400` |
| `Missing` | 404 | `{"detail": "图片不存在"}` |
| `NoPackage` | 404 | `{"detail": "图片包不存在"}` |
| `Corrupt(msg)` | 500 | `{"detail": "图片包损坏或无法读取", "error": msg}` + `warn!` 日志 |

`auth::assert_role(["admin","view-only","view-upload"])`、`fetch_doc` 存在性校验、`paths::get_asset_path()` 计算、日志埋点全部**不动**；
唯一被删掉的是 `std::path::Path::new(&zip_path).exists()` 预检（`read_image` 内部的 `metadata` 已经覆盖，且能区分「不是普通文件」）。

## 4. 逐文件改动清单

### 4.1 `backend-rs/src/services/image_cache.rs`（重写，约 240 行含测试）

- 模块文档改成"按条目、seek 读取"的说明（保留"为什么用 zip"的一段背景，防止以后被误改回松散文件）。
- 常量：删 `MAX_ENTRY_BYTES`（旧含义=单包上限）→ 新增 `MAX_IMAGE_BYTES` / `MAX_ENTRY_BYTES`(解压上限) / `MAX_INDEX_DOCS`；`CACHE_BUDGET_BYTES` 保留 64 MB。
- 新增 `ImageLookup`、`IndexEntry`、`ImageEntry`；`Store` 增加 `indices` / `index_tick`。
- 新增 `read_image(doc_id, zip_path, filename)`；`lookup_image` / `insert_image` / `lookup_index` / `insert_index` / `collect_names` / `read_entry`。
- `invalidate(doc_id)`：同时清 `indices` 与 `images`（过滤 `images` 的键前缀）。
- 测试辅助 `stats()` 返回三元组（图片数、图片字节、索引文档数），并新增一个 `#[cfg(test)] static READ_BYTES: AtomicU64`（记录"读盘字节数"与"中央目录解析次数"）用于断言 G1/G2。

参考骨架（实现时按 zip 2.4.2 的实际签名微调）：

```rust
fn read_entry(
    path: &str,
    want: &str,
    names: Option<HashMap<String, usize>>,
    stats: &ReadProbe,                 // #[cfg(test)] 计数；release 为 ZST
) -> Result<(Option<Vec<u8>>, HashMap<String, usize>), String> {
    use std::io::{BufReader, Read, Seek};

    let file = std::fs::File::open(path).map_err(|e| format!("打开图片包失败: {e}"))?;
    let mut archive = zip::ZipArchive::new(BufReader::new(file))
        .map_err(|e| format!("解析图片包失败: {e}"))?;

    let names = match names {
        Some(n) => n,
        None => {
            stats.count_index_build();
            let mut map = HashMap::new();
            for i in 0..archive.len() {
                let Ok(f) = archive.by_index_raw(i) else { continue };
                if f.is_dir() { continue }
                let Some(p) = f.enclosed_name() else { continue };
                let Some(base) = p.file_name().map(|n| n.to_string_lossy().to_string()) else { continue };
                map.entry(base).or_insert(i);   // 同名取第一个，与旧实现一致
            }
            map
        }
    };

    let Some(&idx) = names.get(want) else { return Ok((None, names)) };

    let mut entry = archive.by_index(idx).map_err(|e| format!("读取条目失败: {e}"))?;
    if entry.size() > MAX_ENTRY_BYTES {
        return Err(format!("图片过大: {} 字节 > 上限 {}", entry.size(), MAX_ENTRY_BYTES));
    }
    let mut buf = Vec::with_capacity(entry.size() as usize);
    entry.read_to_end(&mut buf).map_err(|e| format!("解压条目失败: {e}"))?;
    stats.count_entry_read(buf.len() as u64);
    Ok((Some(buf), names))
}

pub async fn read_image(doc_id: i64, zip_path: &str, filename: &str) -> ImageLookup {
    let meta = match tokio::fs::metadata(zip_path).await {
        Ok(m) if m.is_file() => m,
        _ => return ImageLookup::NoPackage,
    };
    let size = meta.len();
    let mtime = meta.modified().ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_nanos() as u64).unwrap_or(0);

    if let Some(bytes) = lookup_image(doc_id, filename, mtime, size) {
        tracing::debug!("图片缓存命中 (doc_id={doc_id}, {filename}, {} 字节)", bytes.len());
        return ImageLookup::Found(bytes);
    }
    let names = lookup_index(doc_id, mtime, size);

    let path = zip_path.to_string();
    let want = filename.to_string();
    let res = tokio::task::spawn_blocking(move || read_entry(&path, &want, names, ReadProbe::new())).await;
    match res {
        Ok(Ok((Some(bytes), names))) => {
            insert_index(doc_id, mtime, size, names);
            let bytes: Arc<[u8]> = bytes.into();      // Vec<u8> → Arc<[u8]>，一次分配转移所有权
            if (bytes.len() as u64) <= MAX_IMAGE_BYTES {
                insert_image(doc_id, filename, mtime, size, bytes.clone());
            } else {
                tracing::debug!("单图过大不缓存 (doc_id={doc_id}, {filename}, {} 字节)", bytes.len());
            }
            ImageLookup::Found(bytes)
        }
        Ok(Ok((None, names))) => { insert_index(doc_id, mtime, size, names); ImageLookup::Missing }
        Ok(Err(msg)) => { tracing::warn!("读取图片失败 (doc_id={doc_id}, {filename}): {msg}"); ImageLookup::Corrupt(msg) }
        Err(e) => ImageLookup::Corrupt(format!("读取任务失败: {e}")),
    }
}
```

### 4.2 `backend-rs/src/routers/documents.rs`（`get_image_from_zip` :4884，约 -20/+25 行）

```rust
    let zip_path = paths::get_asset_path(&settings, id);
    // 不再需要 `Path::exists()` 预检：`read_image` 自己 stat 一次并回 `NoPackage`（少一次 syscall）
    match image_cache::read_image(id, &zip_path, &filename).await {
        image_cache::ImageLookup::Found(data) => {
            let mime = image_mime(&filename);
            // 零拷贝：Arc<[u8]> 直接变成响应体（bytes 1.12 的 from_owner）
            let body = Body::from(Bytes::from_owner(data));
            (StatusCode::OK,
             [(header::CONTENT_TYPE, mime),
              (header::CACHE_CONTROL, "public, max-age=86400")],
             body).into_response()
        }
        image_cache::ImageLookup::Missing => {
            (StatusCode::NOT_FOUND, Json(json!({"detail": "图片不存在"}))).into_response()
        }
        image_cache::ImageLookup::NoPackage => {
            (StatusCode::NOT_FOUND, Json(json!({"detail": "图片包不存在"}))).into_response()
        }
        image_cache::ImageLookup::Corrupt(msg) => {
            tracing::warn!("图片包损坏 (doc_id={id}, {filename}): {msg}");
            (StatusCode::INTERNAL_SERVER_ERROR,
             Json(json!({"detail": "图片包损坏或无法读取", "error": msg}))).into_response()
        }
    }
```

`Body` 与 `Bytes` 已在 `documents.rs:8` 导入（`use axum::body::{Body, Bytes};`），无需新增依赖。
若 `Bytes::from_owner` 因泛型约束编译不过（`Arc<[u8]>: AsRef<[u8]>` 由 `impl<T: ?Sized> AsRef<T> for Arc<T>` 提供，正常可用），
退化为 `Body::from(data.to_vec())` —— 一次「等于单图大小」的复制，仍与现状的开销同级，**不要**退化成复制整包。

### 4.3 调用点核对（`invalidate` 语义不变，逐处确认仍有效）

| 位置 | 场景 | 动作 |
|---|---|---|
| `routers/documents.rs:1231` | 删除文档 | 保留 |
| `routers/documents.rs:1368` | 合并去重删源文档 | 保留 |
| `routers/documents.rs:5132` | `replace-asset` 换包 | 保留（mtime 精度不足时仍靠它） |
| `routers/pipeline.rs:2650` | `pipeline reset` 清资源包 | 保留 |
| `routers/admin.rs:122` | 管理端按 id 清理 | 保留 |

> 由于指纹里带 `size`，换包后即使漏调 `invalidate` 也基本不会读到旧图；`replace-asset` 已显式失效，不需要额外处理。

### 4.4 文档同步（按仓库约定「新增/修改机制要落文档」）

- `AGENTS.md`
  - 目录表 `services/image_cache.rs` 的描述：`图片资源包按 doc 的 LRU 缓存` → `图片按条目 LRU 缓存（seek 读取，不整包读入）`；
  - 「文档管理」一节里的 `/image/:filename` 说明：「64 MB 预算、单包 16 MB 上限」→「64 MB 图片字节预算、单图 8 MB 上限、索引 512 篇、只读中央目录 + 目标条目」；
- 本文档（`document/dev/image-read-path-plan.md`）作为实施记录，完成后把「状态」改成 ✅ 并补一节「实测数据」；
- `document/dev/` 目前没有进 `_sidebar.md`（`dev/todo.md` 也没进），**本次不新增导航项**；如需对外可读再单独加。

## 5. 测试计划

`image_cache.rs` 内 `#[cfg(test)] mod tests`（用已有依赖 `tempfile = "3"` 造真实 zip 文件；`zip` 已是依赖）：

| # | 用例 | 断言 |
|---|---|---|
| T1 | `reads_only_target_entry`（一个包 20 张各 200 KB） | 读 1 张：`READ_BYTES` ≈ 该图字节 + 中央目录，**远小于包大小**；`stats().0 == 1` |
| T2 | `index_built_once_per_doc` | 连续读 3 张不同图：`INDEX_BUILDS == 1` |
| T3 | `cache_hit_avoids_disk` | 重复读同一张：`READ_BYTES` 不再增长 |
| T4 | `entry_lru_evicts_by_bytes` | 预算调小（test 用 4 KB）后，最久未用的图被淘汰，总字节 ≤ 预算 |
| T5 | `missing_entry_returns_missing` | 包里没这个文件名 → `Missing`（不是 `Corrupt`） |
| T6 | `corrupt_zip_returns_corrupt` | 写一个非 zip 字节的文件 → `Corrupt`；空文件 → `Corrupt` |
| T7 | `no_package` | 文件不存在 → `NoPackage` |
| T8 | `basename_matching_and_traversal` | zip 内 `images/figure-1.png` 可用 `figure-1.png` 命中；`../figure-1.png` / `foo/../figure-1.png` 不命中 |
| T9 | `dir_entry_skipped` | zip 内有 `images/`（目录条目）时不参与匹配 |
| T10 | `oversize_image_not_cached` | 单图 > `MAX_IMAGE_BYTES`（test 里调小）→ 返回内容但 `stats().1` 不增长 |
| T11 | `mtime_change_invalidates` | 重写同名包（内容变、大小变）→ 读到的必须是新内容 |
| T12 | `invalidate_clears_both` | `invalidate` 后 `stats() == (0,0,0)` |
| T13 | `zip_deflated_entry_still_works` | 写一个 `Deflated` 条目的 zip（模拟用户 `replace-asset` 上传的压缩包）→ 正常解出 |
| T14 | `concurrent_reads_same_image` | `join!` 两个 `read_image` 都返回正确字节（不 panic、不死锁） |

现有两个用例（`read_image_matches_basename`、`lru_evicts_and_invalidates`）语义由 T8/T4/T12 覆盖，**删除**。

> 设备（OrangePi）上只跑 `cargo check`（仓库约定：嵌入式设备不编译后端）；`cargo test image_cache --lib` 在开发机上跑。

## 6. 验收标准

### 6.1 自动化

```bash
cd backend-rs
cargo check                # 设备上跑这条
cargo test image_cache --lib   # 开发机；全绿，且 T1/T2/T3 是关键回归哨兵
```

### 6.2 端到端冒烟（独立实例 + 独立 SQLite，避免污染正式库）

```bash
# 1) 起实例（debug 日志，方便看命中/未命中）
cd backend-rs && RUST_LOG=okb_assist=debug cargo run -- --port 5099

# 2) 连续取同一篇文档的 5 张图，记 PID 的 RSS
PID=$(pgrep -f "okb_assist.*5099")
for n in 1 2 3 4 5; do
  curl -s -o /dev/null -w "img$n: %{http_code} %{size_download}B %{time_total}s\n" \
    "http://127.0.0.1:5099/assist/api/documents/<id>/image/<name$n>/"
  ps -o rss= -p $PID | awk '{printf "  RSS %.1f MB\n", $1/1024}'
done

# 3) 异常分支
curl -s -o /dev/null -w "%{http_code}\n" ".../documents/<id>/image/不存在.png/"   # 期望 404 图片不存在
curl -s ".../documents/<不存在id>/image/a.png/"                                  # 期望 404 文档不存在
# 手工把 asset zip 覆盖成随机字节后再取 → 期望 500 图片包损坏或无法读取
```

### 6.3 量化对照（用一个大包造场景，最能体现收益）

```bash
# 造 200 张 200 KB 的图 + 一个 ~40 MB 的 zip（旧实现在这种包上完全不缓存）
uv run --no-project python - <<'PY'
import os, zipfile, random
os.makedirs("/tmp/imgtest", exist_ok=True)
with zipfile.ZipFile("/tmp/imgtest/big.zip", "w", zipfile.ZIP_STORED) as z:
    for i in range(200):
        z.writestr(f"images/fig-{i}.png", bytes(random.getrandbits(8) for _ in range(204_800)))
print("ok")
PY
# 放到 data/pdfs/<id>/<id>.zip（先备份原包），然后按 6.2 逐个取图
```

| 指标 | 改前（40 MB 包） | 改后（目标） |
|---|---|---|
| 第 1 张图 RSS 增量 | ≈ +40 MB（整包） | ≈ +0.2 MB（单图） |
| 第 N 张（N≥2） | 各 +40 MB 或纯内存但整包常驻 | 纯内存，0 I/O |
| 每请求 zip 中央目录解析 | 每次 1 次（线性扫） | 每篇 1 次 |
| 单包 > 16 MB 时 | **每张图都重读整包** | 无此退化 |

## 7. 风险与回滚

| 风险 | 评估 | 处置 |
|---|---|---|
| `zip 2.4.2` API 名称与预期不符（`by_index_raw` / `file_names` / `index_for_name`） | 中 | 只用「打开 → 按名取条目 → 读」三个能力，任一可用即可；实施时先 `cargo doc -p zip --no-deps` 或看 `~/.cargo/registry` 源码确认 |
| 每请求 `open` 一次 fd | 低 | 浏览器同域并发 ≤ 6，且 fd 用完即关；**不缓存打开的 `ZipArchive`**（缓存句柄会带来锁串行化与 fd 泄漏风险，收益仅省一次 open） |
| 缓存击穿（同一张图并发 2 次） | 低 | 接受；第二步再考虑 single-flight |
| 指纹撞车（换包后 mtime 相同） | 低 | 已有 5 处 `invalidate` 显式失效；指纹含 `size` |
| 返回体从 `Vec<u8>` 变 `Arc<[u8]>` 导致类型不通过 | 低 | 用 `Bytes::from_owner(data)`；不通则 `Body::from(data.to_vec())`（单图大小的一次复制）；**不要**改成复制整包 |
| 内存预算从"包字节"改"图片字节"后总占用反而变大 | 低 | 64 MB 上限不变；单图上限 8 MB 防止少数巨图吃光预算 |

**回滚**：改动集中在 `services/image_cache.rs`（单文件重写）与 `get_image_from_zip`（约 25 行），`git revert` 一个提交即可；无数据迁移、无配置变更、无前端改动。

## 8. 工作量

| 项 | 预估 |
|---|---|
| `image_cache.rs` 重写 + 实现细节打通 | 3～4 h |
| `get_image_from_zip` 改造 + 错误分支 | 0.5 h |
| 14 个单测（含造 zip 的辅助函数） | 2 h |
| 端到端冒烟 + 大包对照 | 1 h |
| 文档（AGENTS.md 两处 + 本文状态） | 0.5 h |
| **合计** | **约 1 人日** |

## 9. 后续（第二步，按需触发）

先跑 §2.3 的量化，只有出现下列信号才值得继续：

1. **包数量上万、`du` 显示簇浪费明显**，或需要"只替换/追加单张图而不重传整包"；
2. 需要 `Range` / `ETag` / `sendfile` 直出（Stored 条目已具备条件，届时用 `ZipFile::data_start()` + `compressed_size()` 读原始字节，或干脆换成自建 pack）；
3. 需要跨文档图片去重（同一张图被多篇引用）。

届时的方案是「pack 文件 + SQLite 偏移索引」，**保留"每篇文档一个容器"的粒度**（或每 512 MB 一个 pack，只装若干篇），
而不是全库一个大文件 —— 单文件损坏的影响面从"一篇"变成"几十篇"，这是 per-doc zip 现在最大的隐形优点。

反面选项（不推荐）：图片进 SQLite BLOB（7 500 篇 × 1.6 MB ≈ 12 GB，WAL checkpoint / 备份 / 单文件风险都会变差）；
退回松散文件（回到簇浪费与目录项上限的原点）。

## 10. 实施记录与实测数据（2026-11-20）

改动落地后与计划的两处偏差：

1. **索引构建不解压、也不 seek**：用 `ZipArchive::file_names()`（中央目录解析时已经在内存里的名字）
   + `Path::file_name()` 取 basename，目录条目靠「名字以 `/` 结尾」识别（与 `ZipFile::is_dir()` 同义），
   因此建索引**零额外 I/O**，`by_index_raw(i)` 实际没被用到（它每个条目还要 seek 一次到局部头）。
   名字安全判定 `safe_entry_name()`（拒绝绝对路径 / 盘符 / 任何 `..` 段）替代 `ZipFile::enclosed_name()`
   —— 后者需要先拿到 `ZipFile`，而这里只需要名字。真正返回内容时仍**只按 basename 匹配**，
   请求名里的 `../`、`images/x.png` 都不会命中。
2. **测试态常量**：`MAX_IMAGE_BYTES` 测试值 256 KB、`CACHE_BUDGET_BYTES` 1 MB、`MAX_INDEX_DOCS` 4
   （取 256 KB 是为了让 T1 的 20 × 200 KB 样本仍能进缓存，同时 T4/T10 能造出淘汰与超大单图场景）。
   测试共享进程级缓存，14 个用例统一用 `test_guard()` 串行 + `clear()` + `reset_probe()`。

### 10.1 自动化

```bash
cd backend-rs
cargo check                       # 通过（设备上跑的这条）
cargo test --bin okb_assist image_cache   # 14 passed（注意本仓库无 lib target，是 --bin）
```

T1～T14 全部按计划落地（`reads_only_target_entry` / `index_built_once_per_doc` / `cache_hit_avoids_disk` /
`entry_lru_evicts_by_bytes` / `missing_entry_returns_missing` / `corrupt_zip_returns_corrupt` / `no_package` /
`basename_matching_and_traversal` / `dir_entry_skipped` / `oversize_image_not_cached` / `mtime_change_invalidates` /
`invalidate_clears_both` / `zip_deflated_entry_still_works` / `concurrent_reads_same_image`），
旧的两个用例（`read_image_matches_basename`、`lru_evicts_and_invalidates`）已删除。

### 10.2 本机真实库量化（§2.3）

```
10 个包, 平均 0.94 MB, 合计 0.01 GB, 最大 1.65 MB
抽样 30 包（实际 10 个）条目数: 中位 48, 最大 80, 坏包 0
>16 MB 的包（旧实现完全不缓存）: 0
```

结论：真实库目前全是「平均 < 1 MB、每包几十张图」的小包，旧实现的退化路径（>16 MB 不缓存）
**在当前数据上不触发**；本次改造的收益主要在「每请求重扫中央目录」（48 条目 × 每张图）这一项上消除，
同时把未来的大包场景提前兜住。

### 10.3 端到端冒烟（独立实例 + DB 副本，port 5099）

| 场景 | 结果 |
|---|---|
| 正常图（doc 1，`1.zip` 55 条目） | 200，字节与 zip 内条目 sha256 一致 |
| 同一张图重复取 | 200，日志 `图片缓存命中 (doc_id=1, …, 19057 字节)`，响应更快（1.9ms → 1.1ms） |
| 文档存在、包有、文件名不存在 | 404 `{"detail":"图片不存在"}` |
| 文档存在、没有包文件 | 404 `{"detail":"图片包不存在"}` |
| 文档不存在 | 404 `{"detail":"文档不存在"}`（未变） |
| 包被换成随机字节 | 500 `{"detail":"图片包损坏或无法读取","error":"解析图片包失败: … Could not find EOCD"}` |

### 10.4 大包对照（§6.3，200 张 200 KB 图 / ~40 MB 的 Stored zip）

| 指标 | 改前（旧实现） | 改后（实测） |
|---|---|---|
| 第 1 张图后 RSS | ≈ +40 MB（整包 `fs::read`） | 34.1 → 34.3 MB（**+0.2 MB = 单图**） |
| 第 2～5 张（各取不同图） | 每张都重读整包（>16 MB 不缓存） | 各 +0.2 MB（单图进缓存，0 次整包读取） |
| 单张耗时 | 随包大小增长 | 2.0～2.6 ms（首张）/ 1.2 ms（命中缓存） |
| 中央目录解析次数 | 每请求 1 次 | 每篇 1 次 |

### 10.5 文档同步

- `AGENTS.md`：目录表 `services/image_cache.rs` 描述改为「图片按条目 LRU 缓存（seek 读中央目录 + 单条目）」；
  「文档管理」里 `/image/:filename` 那段改为「64 MB 图片字节预算、单图 8 MB 上限、中央目录索引 512 篇」，
  并补 `ImageLookup` 四种结果的 HTTP 语义；
- `document/dev/` 仍未进 `_sidebar.md`（按计划不新增导航项）。

未改动（本次非目标，仍按 §9 需要时再议）：`images.zip` 写入布局、`Range`/`ETag`/`sendfile`、
缓存击穿 single-flight、前端与路径模板。
