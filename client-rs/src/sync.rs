//! 子命令 `sync`：本地目录与用户网盘的同步（个人网盘 + 公有文件夹，三方比对 + 状态文件）

use std::collections::{BTreeMap, HashMap, HashSet};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;

use anyhow::{bail, Context, Result};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use tokio_stream::StreamExt;

use crate::util::*;

// 设计要点：
//   - 只碰**个人网盘**（`scope=private`），不涉及公有文件夹；
//   - 所有配置来自 `~/.okb/client.json` 的 `sync` 段，命令本身无参数；
//   - 三方比对：本地（size + mtime）、网盘（size + `modified`）、上次同步记录
//     （`~/.okb/client/sync-state.json`），据此判定该上传/下载/跳过；
//   - 不传播删除：一侧删掉的文件会在下次同步时从另一侧补回（删除请用网盘页面）；
//   - 空目录：双向/上传会在网盘上创建，双向/下载会在本地创建。

/// 同步方向
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SyncDirection {
    /// 双向（默认）：本地 ⇄ 网盘
    TwoWay,
    /// 单向上传：以本地为准，只写网盘
    Upload,
    /// 单向下载：以网盘为准，只写本地
    Download,
}

impl SyncDirection {
    fn parse(raw: Option<&str>) -> Result<Self> {
        let Some(v) = raw
            .map(|s| s.trim().to_lowercase())
            .filter(|s| !s.is_empty())
        else {
            return Ok(SyncDirection::TwoWay);
        };
        Ok(match v.as_str() {
            "two-way" | "two_way" | "twoway" | "both" | "bidirectional" | "双向" => {
                SyncDirection::TwoWay
            }
            "upload" | "up" | "push" | "to-remote" | "one-way-upload" | "上传" | "单向上传" => {
                SyncDirection::Upload
            }
            "download" | "down" | "pull" | "to-local" | "one-way-download" | "下载"
            | "单向下载" => SyncDirection::Download,
            other => bail!(
                "sync.direction 取值非法: {}（可用 two-way / upload / download，缺省 two-way）",
                other
            ),
        })
    }

    fn label(&self) -> &'static str {
        match self {
            SyncDirection::TwoWay => "双向同步（本地 ⇄ 网盘）",
            SyncDirection::Upload => "单向上传（本地 → 网盘，以本地为准）",
            SyncDirection::Download => "单向下载（网盘 → 本地，以网盘为准）",
        }
    }

    /// 可以把本地文件写到网盘上
    fn can_upload(&self) -> bool {
        !matches!(self, SyncDirection::Download)
    }

    /// 可以把网盘文件写到本地
    fn can_download(&self) -> bool {
        !matches!(self, SyncDirection::Upload)
    }
}

/// 冲突策略（两侧都改过 / 首次同步无法判断时怎么办）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SyncConflictPolicy {
    /// 取 mtime 较新的一方（默认，自动处理）
    Newer,
    /// 严格模式：只报告、不动文件，交给人工决定
    Strict,
}

impl SyncConflictPolicy {
    fn parse(raw: Option<&str>) -> Result<Self> {
        let Some(v) = raw
            .map(|s| s.trim().to_lowercase())
            .filter(|s| !s.is_empty())
        else {
            return Ok(SyncConflictPolicy::Newer);
        };
        Ok(match v.as_str() {
            "newer" | "newest" | "latest" | "mtime" | "auto" | "最新" | "较新" => {
                SyncConflictPolicy::Newer
            }
            "strict" | "report" | "skip" | "manual" | "none" | "严格" | "只报告" | "手动" => {
                SyncConflictPolicy::Strict
            }
            other => bail!(
                "sync.conflict 取值非法: {}（可用 newer / strict，缺省 newer）",
                other
            ),
        })
    }

    fn label(&self) -> &'static str {
        match self {
            SyncConflictPolicy::Newer => "取较新一方（自动处理）",
            SyncConflictPolicy::Strict => "严格模式（只报告，不改动文件）",
        }
    }
}

/// 本地文件签名（大小 + mtime 毫秒）
#[derive(Debug, Clone, PartialEq)]
struct LocalSig {
    size: u64,
    mtime_ms: u64,
}

/// 网盘文件签名（大小 + 服务端 `modified` 时间串）
#[derive(Debug, Clone, PartialEq)]
struct RemoteSig {
    size: u64,
    modified: Option<String>,
}

impl RemoteSig {
    /// `modified` 解析成毫秒时间戳（解析失败当 0），用于「双方都改过时取较新一方」
    fn modified_ms(&self) -> u64 {
        self.modified
            .as_deref()
            .and_then(parse_rfc3339_ms)
            .unwrap_or(0)
    }
}

/// 上次同步完成时两侧的样子
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct SyncStateEntry {
    local_size: u64,
    local_mtime_ms: u64,
    remote_size: u64,
    #[serde(default)]
    remote_modified: Option<String>,
}

impl SyncStateEntry {
    fn record(local: &LocalSig, remote: &RemoteSig) -> Self {
        SyncStateEntry {
            local_size: local.size,
            local_mtime_ms: local.mtime_ms,
            remote_size: remote.size,
            remote_modified: remote.modified.clone(),
        }
    }

    fn local_changed(&self, local: &LocalSig) -> bool {
        self.local_size != local.size || self.local_mtime_ms != local.mtime_ms
    }

    fn remote_changed(&self, remote: &RemoteSig) -> bool {
        self.remote_size != remote.size || self.remote_modified != remote.modified
    }
}

/// 对单个文件的处理动作
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SyncDecision {
    Skip,
    Upload,
    Download,
    /// 两侧都有改动（严格模式下不碰任何一侧）
    Conflict,
}

/// 判定结果之外，冲突还需要一句说明（两侧都改过 / 首次同步无法判断）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ConflictKind {
    /// 状态里有记录，且本地与网盘都变了
    BothChanged,
    /// 没有状态记录、大小也不一样（首次同步无法判断谁新谁旧）
    NoBaseline,
}

/// 冲突判定的完整结果
fn judge(
    direction: SyncDirection,
    policy: SyncConflictPolicy,
    local: Option<&LocalSig>,
    remote: Option<&RemoteSig>,
    saved: Option<&SyncStateEntry>,
) -> (SyncDecision, Option<ConflictKind>) {
    let (Some(l), Some(r)) = (local, remote) else {
        return (decide_action(direction, local, remote, saved), None);
    };
    if direction != SyncDirection::TwoWay {
        // 单向同步没有「冲突」：以指定的一侧为准
        return (decide_action(direction, local, remote, saved), None);
    }
    let conflict = match saved {
        Some(s) => {
            if s.local_changed(l) && s.remote_changed(r) {
                Some(ConflictKind::BothChanged)
            } else {
                None
            }
        }
        // 首次同步：大小相同视为同一份内容（不算冲突），大小不同则无法判断
        None => {
            if l.size == r.size {
                None
            } else {
                Some(ConflictKind::NoBaseline)
            }
        }
    };
    match conflict {
        Some(kind) if policy == SyncConflictPolicy::Strict => (SyncDecision::Conflict, Some(kind)),
        Some(kind) => (
            // 非严格模式：按 mtime 较新一方自动处理
            if r.modified_ms() > l.mtime_ms {
                SyncDecision::Download
            } else {
                SyncDecision::Upload
            },
            Some(kind),
        ),
        None => (decide_action(direction, local, remote, saved), None),
    }
}

/// 根据「本地签名 / 网盘签名 / 上次状态 / 方向」判定动作（纯函数，便于单测）
/// 冲突已由 `judge` 处理，这里只处理「能确定方向」的场景
fn decide_action(
    direction: SyncDirection,
    local: Option<&LocalSig>,
    remote: Option<&RemoteSig>,
    saved: Option<&SyncStateEntry>,
) -> SyncDecision {
    let (Some(l), Some(r)) = (local, remote) else {
        return match (local, remote) {
            (Some(_), None) => {
                if direction.can_upload() {
                    SyncDecision::Upload
                } else {
                    SyncDecision::Skip
                }
            }
            (None, Some(_)) => {
                if direction.can_download() {
                    SyncDecision::Download
                } else {
                    SyncDecision::Skip
                }
            }
            _ => SyncDecision::Skip,
        };
    };

    let same_size = l.size == r.size;
    match direction {
        // 单向上传：只要不是「两侧都没动过」就覆盖网盘（本地为准）
        SyncDirection::Upload => match saved {
            Some(s) if !s.local_changed(l) && !s.remote_changed(r) => SyncDecision::Skip,
            Some(_) => SyncDecision::Upload,
            // 首次见到：大小相同就当已同步，避免无谓的全量重传
            None => {
                if same_size {
                    SyncDecision::Skip
                } else {
                    SyncDecision::Upload
                }
            }
        },
        // 单向下载：对称
        SyncDirection::Download => match saved {
            Some(s) if !s.local_changed(l) && !s.remote_changed(r) => SyncDecision::Skip,
            Some(_) => SyncDecision::Download,
            None => {
                if same_size {
                    SyncDecision::Skip
                } else {
                    SyncDecision::Download
                }
            }
        },
        // 双向：三方比对；双方都改过时取 mtime 较新的一方（网盘 mtime = 上传时间）
        SyncDirection::TwoWay => match saved {
            Some(s) => match (s.local_changed(l), s.remote_changed(r)) {
                (false, false) => SyncDecision::Skip,
                (true, false) => SyncDecision::Upload,
                (false, true) => SyncDecision::Download,
                (true, true) => {
                    if r.modified_ms() > l.mtime_ms {
                        SyncDecision::Download
                    } else {
                        SyncDecision::Upload
                    }
                }
            },
            None => {
                if same_size {
                    SyncDecision::Skip
                } else if r.modified_ms() > l.mtime_ms {
                    SyncDecision::Download
                } else {
                    SyncDecision::Upload
                }
            }
        },
    }
}

/// 把 RFC3339（`2026-11-09T10:29:05.123456789+00:00` / `...Z`）解析成 Unix 毫秒
///
/// 不引 chrono：只需要给「谁更新」做比较，手写 civil-date 换算即可。
fn parse_rfc3339_ms(raw: &str) -> Option<u64> {
    let s = raw.trim();
    let (date, rest) = s.split_once('T')?;
    let mut dp = date.split('-');
    let y: i64 = dp.next()?.parse().ok()?;
    let mo: i64 = dp.next()?.parse().ok()?;
    let d: i64 = dp.next()?.parse().ok()?;

    let tz_start = rest.find(['+', 'Z']).unwrap_or(rest.len());
    let time = &rest[..tz_start];
    let tz = &rest[tz_start..];
    let mut tp = time.split(':');
    let h: i64 = tp.next()?.parse().ok()?;
    let mi: i64 = tp.next()?.parse().ok()?;
    let sec_raw = tp.next()?;
    let (sec_s, frac_s) = sec_raw.split_once('.').unwrap_or((sec_raw, ""));
    let sec: i64 = sec_s.parse().ok()?;
    let frac_ms: i64 = {
        let digits: String = frac_s.chars().take(3).collect();
        let padded = format!("{:0<3}", digits);
        padded.parse::<i64>().unwrap_or(0)
    };

    let offset_sec: i64 = if tz.is_empty() || tz == "Z" {
        0
    } else {
        let sign = if tz.starts_with('-') { -1 } else { 1 };
        let body = &tz[1..];
        let (oh, om) = body.split_once(':').unwrap_or((body, "0"));
        let oh: i64 = oh.parse().unwrap_or(0);
        let om: i64 = om.parse().unwrap_or(0);
        sign * (oh * 3600 + om * 60)
    };

    let days = days_from_civil(y, mo, d);
    let epoch_ms = (days * 86_400 + h * 3_600 + mi * 60 + sec - offset_sec) * 1000 + frac_ms;
    if epoch_ms < 0 {
        None
    } else {
        Some(epoch_ms as u64)
    }
}

/// 公历日期 → Unix 纪元后的天数（Howard Hinnant 的 days_from_civil）
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// 同步状态文件（`~/.okb/client/sync-state.json`），v2 起按「同步目标」分组：
///
/// ```json
/// { "version": 2, "targets": {
///     "private:sync": { "a/b.pdf": { "local_size": 1, ... } },
///     "public:pf_1a2b3c:" : { ... } } }
/// ```
#[derive(Debug, Default, Serialize, Deserialize)]
struct SyncStateFile {
    #[serde(default = "sync_state_version")]
    version: u32,
    /// 目标键（`private:<子目录>` / `public:<folder_id>:<子目录>`）→ 相对路径 → 上次同步记录
    #[serde(default)]
    targets: BTreeMap<String, BTreeMap<String, SyncStateEntry>>,
}

/// v1 的状态文件（单目标：`{version:1, remote, entries}`），读出后并到 v2 的 `private:<remote>`
#[derive(Debug, Default, Deserialize)]
struct SyncStateV1 {
    #[serde(default)]
    remote: String,
    #[serde(default)]
    entries: BTreeMap<String, SyncStateEntry>,
}

fn sync_state_version() -> u32 {
    2
}

fn sync_state_path() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".okb").join("client").join("sync-state.json"))
}

/// 读整份状态文件（含 v1 → v2 迁移）；读不到/解析失败返回空表
fn load_sync_state_file() -> SyncStateFile {
    let Some(path) = sync_state_path() else {
        return SyncStateFile {
            version: sync_state_version(),
            targets: BTreeMap::new(),
        };
    };
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) => {
            if e.kind() != io::ErrorKind::NotFound {
                eprintln!("警告: 无法读取 {}: {}", path.display(), e);
            }
            return SyncStateFile {
                version: sync_state_version(),
                targets: BTreeMap::new(),
            };
        }
    };
    let value: serde_json::Value = match serde_json::from_str(&text) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("警告: {} 解析失败（按空状态重建）: {}", path.display(), e);
            return SyncStateFile {
                version: sync_state_version(),
                targets: BTreeMap::new(),
            };
        }
    };
    let version = value.get("version").and_then(|v| v.as_u64()).unwrap_or(1);
    if version >= 2 {
        let file: SyncStateFile = serde_json::from_value(value).unwrap_or_default();
        return SyncStateFile {
            version: sync_state_version(),
            targets: file.targets,
        };
    }
    // v1 迁移：老文件只有一个 remote 子目录，键按个人网盘算
    let old: SyncStateV1 = serde_json::from_value(value).unwrap_or_default();
    let mut targets = BTreeMap::new();
    let remote = old.remote.trim().trim_matches('/').to_string();
    if !old.entries.is_empty() {
        targets.insert(format!("private:{}", remote), old.entries);
    }
    SyncStateFile {
        version: sync_state_version(),
        targets,
    }
}

/// 取某个目标的记录
fn state_entries_for(file: &SyncStateFile, key: &str) -> BTreeMap<String, SyncStateEntry> {
    file.targets.get(key).cloned().unwrap_or_default()
}

/// 原子写状态文件（先写 `.tmp` 再 rename）
fn save_sync_state(file: &SyncStateFile) {
    let Some(path) = sync_state_path() else {
        eprintln!("警告: 无法定位 ~/.okb/client/sync-state.json，同步状态未保存");
        return;
    };
    if let Some(parent) = path.parent() {
        if let Err(e) = std::fs::create_dir_all(parent) {
            eprintln!("警告: 无法创建目录 {}: {}", parent.display(), e);
            return;
        }
    }
    let body = match serde_json::to_string_pretty(file) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("警告: 同步状态序列化失败: {}", e);
            return;
        }
    };
    let tmp = path.with_extension("json.tmp");
    if let Err(e) = std::fs::write(&tmp, body) {
        eprintln!("警告: 无法写入 {}: {}", tmp.display(), e);
        return;
    }
    if let Err(e) = std::fs::rename(&tmp, &path) {
        eprintln!("警告: 无法保存 {}: {}", path.display(), e);
    }
}

/// 网盘目录列表里的一项
#[derive(Debug, Clone)]
struct RemoteEntry {
    name: String,
    is_dir: bool,
    size: u64,
    modified: Option<String>,
}

/// 本地目录列表里的一项
struct LocalEntry {
    name: String,
    is_dir: bool,
    sig: LocalSig,
}

/// 不参与同步的文件名（临时文件与 Windows/macOS 垃圾文件）
fn is_ignored_name(name: &str) -> bool {
    name.ends_with(".okbsync-part")
        || matches!(name, "Thumbs.db" | "desktop.ini" | ".DS_Store")
}

/// 相对路径拼接（统一用 `/`）
fn join_rel(rel: &str, name: &str) -> String {
    if name.is_empty() {
        rel.to_string()
    } else if rel.is_empty() {
        name.to_string()
    } else {
        format!("{}/{}", rel, name)
    }
}

/// 网盘内的目标路径 = `sync.remote` + 相对路径
fn remote_join(root: &str, rel: &str) -> String {
    join_rel(root, rel)
}

/// 查询串里的路径百分号编码（只保留 unreserved 与 `/`）
fn encode_query_path(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' => {
                out.push(*b as char)
            }
            _ => out.push_str(&format!("%{:02X}", b)),
        }
    }
    out
}

/// `GET /assist/api/drive/list?scope=private&path=…`
async fn drive_list(
    client: &Client,
    base_url: &str,
    token: &Option<String>,
    scope: &str,
    rel: &str,
) -> Result<Vec<RemoteEntry>> {
    let url = format!(
        "{}/assist/api/drive/list?scope={}&path={}",
        base_url,
        encode_query_path(scope),
        encode_query_path(rel)
    );
    let mut req = client.get(&url);
    if let Some(t) = token {
        req = req.header("X-Token", t.as_str());
    }
    let resp = req
        .send()
        .await
        .with_context(|| format!("列网盘目录失败: /{}", rel))?;
    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        bail!(
            "列网盘目录失败（{} /{}）: {}",
            status,
            rel,
            truncate_body(&body, 300)
        );
    }
    let value: serde_json::Value = resp.json().await.context("列网盘目录响应解析失败")?;
    let mut out = Vec::new();
    for item in value
        .get("entries")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default()
    {
        let name = item
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();
        if name.is_empty() || is_ignored_name(&name) {
            continue;
        }
        out.push(RemoteEntry {
            name,
            is_dir: item.get("is_dir").and_then(|v| v.as_bool()).unwrap_or(false),
            size: item.get("size").and_then(|v| v.as_u64()).unwrap_or(0),
            modified: item
                .get("modified")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string()),
        });
    }
    Ok(out)
}

/// `POST /assist/api/drive/mkdir`（已存在视为成功）
async fn drive_mkdir(
    client: &Client,
    base_url: &str,
    token: &Option<String>,
    scope: &str,
    parent: &str,
    name: &str,
) -> Result<()> {
    let url = format!("{}/assist/api/drive/mkdir", base_url);
    let mut req = client
        .post(&url)
        .json(&serde_json::json!({"scope": scope, "path": parent, "name": name}));
    if let Some(t) = token {
        req = req.header("X-Token", t.as_str());
    }
    let resp = req.send().await.context("建网盘目录请求失败")?;
    let status = resp.status();
    if status.is_success() || status == reqwest::StatusCode::CONFLICT {
        return Ok(());
    }
    let body = resp.text().await.unwrap_or_default();
    bail!(
        "建网盘目录失败（{} /{}/{}）: {}",
        status,
        parent,
        name,
        truncate_body(&body, 200)
    )
}

/// 按层级创建网盘目录（`a/b/c`），已存在的层级静默跳过
async fn ensure_remote_dir(
    client: &Client,
    base_url: &str,
    token: &Option<String>,
    scope: &str,
    rel: &str,
) -> Result<()> {
    let mut acc = String::new();
    for seg in rel.split('/').filter(|s| !s.is_empty()) {
        drive_mkdir(client, base_url, token, scope, &acc, seg).await?;
        acc = join_rel(&acc, seg);
    }
    Ok(())
}

/// 上传一个文件到网盘目录（`overwrite=true`，同名直接覆盖而不是生成 `(1)`）
async fn drive_upload_file(
    base_url: &str,
    token: &Option<String>,
    scope: &str,
    dir_rel: &str,
    file_path: &Path,
) -> Result<u64> {
    let size = tokio::fs::metadata(file_path)
        .await
        .with_context(|| format!("无法读取文件信息: {}", file_path.display()))?
        .len();
    let name = file_path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "file".to_string());
    let file = tokio::fs::File::open(file_path)
        .await
        .with_context(|| format!("无法读取文件: {}", file_path.display()))?;

    let progress = TransferProgress::start(size);
    let counter = progress.counter();
    let part = reqwest::multipart::Part::stream_with_length(
        reqwest::Body::wrap_stream(tokio_util::io::ReaderStream::new(file).map(move |chunk| {
            if let Ok(bytes) = &chunk {
                counter.fetch_add(bytes.len() as u64, Ordering::Relaxed);
            }
            chunk
        })),
        size,
    )
    .file_name(name)
    .mime_str("application/octet-stream")?;

    // 字段顺序敏感：网盘上传处理器边收边处理，`overwrite` 必须在 `file` 之前
    let form = reqwest::multipart::Form::new()
        .text("scope", scope.to_string())
        .text("path", dir_rel.to_string())
        .text("overwrite", "true")
        .part("file", part);

    let client = upload_client(size, None)?;
    let url = format!("{}/assist/api/drive/upload", base_url);
    let mut req = client.post(&url).multipart(form);
    if let Some(t) = token {
        req = req.header("X-Token", t.as_str());
    }
    let result = req.send().await;
    progress.finish(result.is_ok());
    let resp = result?;

    let status = resp.status();
    let body = resp.text().await.unwrap_or_default();
    let parsed: serde_json::Value = serde_json::from_str(&body).unwrap_or(serde_json::Value::Null);
    let file_ok = parsed
        .pointer("/files/0/ok")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    if !status.is_success() || !file_ok {
        let detail = parsed
            .pointer("/files/0/detail")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        bail!(
            "上传失败（{}）: {}{}",
            status,
            truncate_body(&body, 200),
            if detail.is_empty() {
                String::new()
            } else {
                format!(" - {}", detail)
            }
        );
    }
    Ok(size)
}

/// 从网盘下载一个文件到本地（先写 `.okbsync-part` 再改名，不会留下半截文件）
async fn drive_download_file(
    client: &Client,
    base_url: &str,
    token: &Option<String>,
    scope: &str,
    rel: &str,
    dest: &Path,
    expect_size: u64,
) -> Result<u64> {
    let url = format!(
        "{}/assist/api/drive/download?scope={}&path={}",
        base_url,
        encode_query_path(scope),
        encode_query_path(rel)
    );
    let mut req = client.get(&url);
    if let Some(t) = token {
        req = req.header("X-Token", t.as_str());
    }
    let mut resp = req
        .send()
        .await
        .with_context(|| format!("下载请求失败: {}", rel))?;
    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        bail!(
            "下载失败（{}）: {}",
            status,
            truncate_body(&body, 200)
        );
    }

    let total = resp.content_length().filter(|n| *n > 0).unwrap_or(expect_size);
    let progress = TransferProgress::start_download(total);
    let counter = progress.counter();

    if let Some(parent) = dest.parent() {
        tokio::fs::create_dir_all(parent).await.ok();
    }
    let tmp = dest.with_file_name(format!(
        ".{}.okbsync-part",
        dest.file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "download".to_string())
    ));

    let written: Result<u64> = async {
        use tokio::io::AsyncWriteExt;
        let mut file = tokio::fs::File::create(&tmp)
            .await
            .with_context(|| format!("无法写入临时文件: {}", tmp.display()))?;
        let mut written: u64 = 0;
        loop {
            match resp.chunk().await {
                Ok(Some(chunk)) => {
                    written += chunk.len() as u64;
                    counter.fetch_add(chunk.len() as u64, Ordering::Relaxed);
                    file.write_all(&chunk).await.context("写入本地文件失败")?;
                }
                Ok(None) => break,
                Err(e) => return Err(e).context("下载中断"),
            }
        }
        file.flush().await.context("刷新本地文件失败")?;
        Ok(written)
    }
    .await;
    progress.finish(written.is_ok());

    let written = match written {
        Ok(n) => n,
        Err(e) => {
            let _ = tokio::fs::remove_file(&tmp).await;
            return Err(e);
        }
    };

    // 覆盖目标：Windows 上目标被占用时 rename 会失败，先删再改名
    let _ = tokio::fs::remove_file(dest).await;
    tokio::fs::rename(&tmp, dest)
        .await
        .with_context(|| format!("保存到 {} 失败", dest.display()))?;
    Ok(written)
}

/// 同步上下文（配置 + 进度统计 + 状态）
struct SyncCtx {
    /// 小请求用（列目录/建目录），带 120s 超时
    client: Client,
    /// 下载用：不设总超时，否则大文件必然被 120s 掐断
    download_client: Client,
    base_url: String,
    token: Option<String>,
    /// 网盘作用域：`private`（个人网盘）或 `public:<folder_id>`（公有文件夹）
    scope: String,
    local_root: PathBuf,
    /// 目标在网盘里的子目录（`sync.personal.remote` / `sync.public[].remote`）
    remote_root: String,
    direction: SyncDirection,
    /// 冲突策略（`newer` / `strict`）
    conflict_policy: SyncConflictPolicy,
    /// 本次运行开始时刻（毫秒），用来把 mtime 显示成「N 分钟前」
    now_ms: u64,
    /// 该目标上次同步的记录（相对路径 → 两侧签名）
    state: BTreeMap<String, SyncStateEntry>,
    uploaded: usize,
    downloaded: usize,
    skipped: usize,
    conflicts: usize,
    failed: usize,
    uploaded_bytes: u64,
    downloaded_bytes: u64,
    /// 状态是否有变化（决定是否重写状态文件）
    state_dirty: bool,
}

impl SyncCtx {
    fn local_dir(&self, rel: &str) -> PathBuf {
        let mut path = self.local_root.clone();
        for seg in rel.split('/').filter(|s| !s.is_empty()) {
            path.push(seg);
        }
        path
    }

    /// 该相对路径在网盘里的绝对（相对网盘根）路径
    fn remote_of(&self, rel: &str) -> String {
        remote_join(&self.remote_root, rel)
    }
}

/// 列本地目录（跳过符号链接与同步临时文件）
async fn local_entries(dir: &Path) -> Result<Vec<LocalEntry>> {
    let mut out = Vec::new();
    let mut rd = match tokio::fs::read_dir(dir).await {
        Ok(rd) => rd,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(out),
        Err(e) => return Err(e).with_context(|| format!("无法读取目录: {}", dir.display())),
    };
    while let Some(item) = rd.next_entry().await? {
        let name = item.file_name().to_string_lossy().to_string();
        if is_ignored_name(&name) {
            continue;
        }
        let meta = match tokio::fs::symlink_metadata(item.path()).await {
            Ok(m) => m,
            Err(_) => continue,
        };
        if meta.file_type().is_symlink() {
            continue;
        }
        let is_dir = meta.is_dir();
        let mtime_ms = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        out.push(LocalEntry {
            name,
            is_dir,
            sig: LocalSig {
                size: if is_dir { 0 } else { meta.len() },
                mtime_ms,
            },
        });
    }
    Ok(out)
}

/// 本地文件签名（同步结束后写状态用）
async fn local_sig(path: &Path) -> Option<LocalSig> {
    let meta = tokio::fs::metadata(path).await.ok()?;
    if meta.is_dir() {
        return None;
    }
    let mtime_ms = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    Some(LocalSig {
        size: meta.len(),
        mtime_ms,
    })
}

/// 同步一个目录（非递归：子目录压入队列）
async fn sync_one_dir(ctx: &mut SyncCtx, rel: &str, queue: &mut Vec<String>) -> Result<()> {
    let local_dir = ctx.local_dir(rel);
    let remote_rel = ctx.remote_of(rel);
    if let Err(e) = tokio::fs::create_dir_all(&local_dir).await {
        if ctx.direction.can_download() {
            bail!("无法创建本地目录 {}: {}", local_dir.display(), e);
        }
    }

    let remote_entries =
        drive_list(&ctx.client, &ctx.base_url, &ctx.token, &ctx.scope, &remote_rel).await?;
    let local = local_entries(&local_dir).await?;

    // 记录本层最终的「两侧都有」的文件，用于刷新状态
    let mut written: HashSet<String> = HashSet::new();
    // 严格模式下判定为冲突的文件：本次不写状态，这样下次仍能识别为冲突
    let mut conflicted: HashSet<String> = HashSet::new();
    let mut transferred = false;

    // ── 目录：缺失的一侧补齐 ───────────────────────────────
    let local_dirs: HashMap<String, bool> = local
        .iter()
        .filter(|e| e.is_dir)
        .map(|e| (e.name.clone(), true))
        .collect();
    let local_files: Vec<&LocalEntry> = local.iter().filter(|e| !e.is_dir).collect();
    let mut remote_dirs: HashMap<String, bool> = HashMap::new();
    let mut remote_files: HashMap<String, RemoteSig> = HashMap::new();
    for entry in &remote_entries {
        if entry.is_dir {
            remote_dirs.insert(entry.name.clone(), true);
        } else {
            remote_files.insert(
                entry.name.clone(),
                RemoteSig {
                    size: entry.size,
                    modified: entry.modified.clone(),
                },
            );
        }
    }

    for name in local_dirs.keys() {
        // 本地是目录、网盘同名的是文件：无法当成目录用，报冲突跳过
        if remote_files.contains_key(name) {
            println!(
                "  ⚠ 类型冲突（本地目录 / 网盘同名文件）: {}/{}，跳过",
                rel, name
            );
            ctx.skipped += 1;
            continue;
        }
        let remote_has_dir = remote_dirs.contains_key(name);
        if !remote_has_dir && ctx.direction.can_upload() {
            match drive_mkdir(
                &ctx.client,
                &ctx.base_url,
                &ctx.token,
                &ctx.scope,
                &remote_rel,
                name,
            )
            .await
            {
                Ok(()) => {
                    remote_dirs.insert(name.clone(), true);
                }
                Err(e) => {
                    ctx.failed += 1;
                    println!("  建网盘目录失败 {}/{}: {}", remote_rel, name, e);
                }
            }
        }
        // 单向下载时，网盘上没有的目录不必下去扫（扫了也只能拿到空列表）
        if ctx.direction.can_upload() || remote_has_dir {
            queue.push(join_rel(rel, name));
        }
    }
    for name in remote_dirs.keys() {
        if local_dirs.contains_key(name) {
            continue;
        }
        if local.iter().any(|e| !e.is_dir && &e.name == name) {
            println!(
                "  ⚠ 类型冲突（本地同名文件 / 网盘同名目录）: {}/{}，跳过",
                rel, name
            );
            ctx.skipped += 1;
            continue;
        }
        if !ctx.direction.can_download() {
            continue;
        }
        let dir = ctx.local_dir(&join_rel(rel, name));
        match tokio::fs::create_dir_all(&dir).await {
            Ok(()) => queue.push(join_rel(rel, name)),
            Err(e) => {
                ctx.failed += 1;
                println!("  建本地目录失败 {}: {}", dir.display(), e);
            }
        }
    }

    // ── 文件 ────────────────────────────────────────────────
    for entry in &local_files {
        let file_rel = join_rel(rel, &entry.name);
        let local_path = ctx.local_dir(&file_rel);
        if remote_dirs.contains_key(&entry.name) {
            println!(
                "  ⚠ 类型冲突（本地同名文件 / 网盘同名目录）: {}，跳过",
                file_rel
            );
            ctx.skipped += 1;
            continue;
        }
        let saved = ctx.state.get(&file_rel).cloned();
        let remote_sig = remote_files.get(&entry.name).cloned();
        let (action, conflict) = judge(
            ctx.direction,
            ctx.conflict_policy,
            Some(&entry.sig),
            remote_sig.as_ref(),
            saved.as_ref(),
        );
        // 冲突提示：严格模式下不动文件；非严格模式说明按哪一侧处理
        if let Some(kind) = conflict {
            ctx.conflicts += 1;
            let reason = match kind {
                ConflictKind::BothChanged => "两侧都改过".to_string(),
                ConflictKind::NoBaseline => {
                    "首次同步且两侧大小不同（无法判断谁更新）".to_string()
                }
            };
            let detail = match remote_sig.as_ref() {
                Some(r) => format!(
                    "本地 {}，{}修改 · 网盘 {}，{}上传",
                    format_bytes(entry.sig.size),
                    human_age(entry.sig.mtime_ms, ctx.now_ms),
                    format_bytes(r.size),
                    human_age(r.modified_ms(), ctx.now_ms)
                ),
                None => format!("本地 {}", format_bytes(entry.sig.size)),
            };
            if action == SyncDecision::Conflict {
                println!(
                    "  ⚠ 冲突（严格模式，未改动）{}: {}｜{}",
                    file_rel, reason, detail
                );
                // 保留旧状态：下次仍能识别为冲突，也不会被本层清理掉
                written.insert(file_rel.clone());
                conflicted.insert(file_rel.clone());
                continue;
            }
            println!(
                "  ⚠ 冲突 {}: {}，采用较新的一方（{}）｜{}",
                file_rel,
                reason,
                if action == SyncDecision::Download {
                    "网盘"
                } else {
                    "本地"
                },
                detail
            );
        }
        match action {
            SyncDecision::Skip => {
                ctx.skipped += 1;
                if let Some(r) = remote_sig.as_ref() {
                    ctx.remember(&file_rel, &entry.sig, r);
                    written.insert(file_rel.clone());
                }
            }
            SyncDecision::Upload => {
                println!(
                    "  上传 {}（{}）",
                    file_rel,
                    format_bytes(entry.sig.size)
                );
                match drive_upload_file(
                    &ctx.base_url,
                    &ctx.token,
                    &ctx.scope,
                    &remote_rel,
                    &local_path,
                )
                .await
                {
                    Ok(size) => {
                        ctx.uploaded += 1;
                        ctx.uploaded_bytes += size;
                        transferred = true;
                    }
                    Err(e) => {
                        ctx.failed += 1;
                        println!("  上传失败 {}: {}\n", file_rel, e);
                    }
                }
            }
            SyncDecision::Download => {
                println!(
                    "  下载 {}（{}）",
                    file_rel,
                    format_bytes(remote_sig.as_ref().map(|r| r.size).unwrap_or(0))
                );
                let dest = ctx.local_dir(&file_rel);
                match drive_download_file(
                    &ctx.download_client,
                    &ctx.base_url,
                    &ctx.token,
                    &ctx.scope,
                    &ctx.remote_of(&file_rel),
                    &dest,
                    remote_sig.as_ref().map(|r| r.size).unwrap_or(0),
                )
                .await
                {
                    Ok(size) => {
                        ctx.downloaded += 1;
                        ctx.downloaded_bytes += size;
                        transferred = true;
                    }
                    Err(e) => {
                        ctx.failed += 1;
                        println!("  下载失败 {}: {}\n", file_rel, e);
                    }
                }
            }
            // `judge` 已把冲突转成 Conflict（这里理论上到不了），稳妥起见仍保持不动
            SyncDecision::Conflict => {
                ctx.conflicts += 1;
                written.insert(file_rel.clone());
                conflicted.insert(file_rel.clone());
                println!("  ⚠ 冲突（未改动）{}", file_rel);
            }
        }
    }

    for (name, remote_sig) in &remote_files {
        if local_files.iter().any(|e| &e.name == name) {
            continue;
        }
        let file_rel = join_rel(rel, name);
        if !ctx.direction.can_download() {
            ctx.skipped += 1;
            continue;
        }
        println!("  下载 {}（{}）", file_rel, format_bytes(remote_sig.size));
        let dest = ctx.local_dir(&file_rel);
        match drive_download_file(
            &ctx.download_client,
            &ctx.base_url,
            &ctx.token,
            &ctx.scope,
            &ctx.remote_of(&file_rel),
            &dest,
            remote_sig.size,
        )
        .await
        {
            Ok(size) => {
                ctx.downloaded += 1;
                ctx.downloaded_bytes += size;
                transferred = true;
            }
            Err(e) => {
                ctx.failed += 1;
                println!("  下载失败 {}: {}\n", file_rel, e);
            }
        }
    }

    // ── 刷新状态（本层）────────────────────────────────────
    // 有传输时两侧都要重新取：网盘的 `modified` 变了、下载的文件也没在初始列表里
    let local_after = if transferred {
        local_entries(&local_dir).await.unwrap_or_default()
    } else {
        local
    };
    let remote_after = if transferred {
        drive_list(&ctx.client, &ctx.base_url, &ctx.token, &ctx.scope, &remote_rel)
            .await
            .unwrap_or(remote_entries)
    } else {
        remote_entries
    };
    let mut remote_files_after: HashMap<String, RemoteSig> = HashMap::new();
    for entry in &remote_after {
        if !entry.is_dir {
            remote_files_after.insert(
                entry.name.clone(),
                RemoteSig {
                    size: entry.size,
                    modified: entry.modified.clone(),
                },
            );
        }
    }
    for entry in local_after.iter().filter(|e| !e.is_dir) {
        let key = join_rel(rel, &entry.name);
        if conflicted.contains(&key) {
            // 冲突未处理：保留上次同步的状态，下一次仍按冲突处理
            continue;
        }
        let Some(r) = remote_files_after.get(&entry.name) else {
            continue;
        };
        let sig = if transferred {
            match local_sig(&ctx.local_dir(&key)).await {
                Some(s) => s,
                None => continue,
            }
        } else {
            entry.sig.clone()
        };
        ctx.remember(&key, &sig, r);
        written.insert(key);
    }

    // 清掉本层里已经不存在的记录（文件被删/改名了）
    let prefix = if rel.is_empty() {
        String::new()
    } else {
        format!("{}/", rel)
    };
    let stale: Vec<String> = ctx
        .state
        .keys()
        .filter(|key| match key.strip_prefix(&prefix) {
            Some(rest) => !rest.contains('/') && !written.contains(key.as_str()),
            None => false,
        })
        .cloned()
        .collect();
    for key in stale {
        ctx.state.remove(&key);
        ctx.state_dirty = true;
    }

    Ok(())
}

impl SyncCtx {
    fn remember(&mut self, rel: &str, local: &LocalSig, remote: &RemoteSig) {
        let entry = SyncStateEntry::record(local, remote);
        if self.state.get(rel) != Some(&entry) {
            self.state.insert(rel.to_string(), entry);
            self.state_dirty = true;
        }
    }
}

/// 公有文件夹（来自 `GET /assist/api/drive` 的 `public_folders`）
#[derive(Debug, Clone)]
struct PublicFolder {
    id: String,
    name: String,
    can_write: bool,
}

/// 读网盘概览里的公有文件夹清单（按角色阈值过滤后的「我能访问的」）
async fn drive_public_folders(
    client: &Client,
    base_url: &str,
    token: &Option<String>,
) -> Result<Vec<PublicFolder>> {
    let url = format!("{}/assist/api/drive", base_url);
    let mut req = client.get(&url);
    if let Some(t) = token {
        req = req.header("X-Token", t.as_str());
    }
    let resp = req.send().await.context("读取网盘概览失败")?;
    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        bail!(
            "读取网盘概览失败（{}）: {}",
            status,
            truncate_body(&body, 300)
        );
    }
    let value: serde_json::Value = resp.json().await.context("网盘概览响应解析失败")?;
    let enabled = value
        .get("enabled")
        .and_then(|v| v.as_bool())
        .unwrap_or(true);
    if !enabled {
        bail!("网盘功能未启用（config.json 的 drive.enabled = false）");
    }
    let mut out = Vec::new();
    for item in value
        .get("public_folders")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default()
    {
        let Some(id) = item.get("id").and_then(|v| v.as_str()) else {
            continue;
        };
        out.push(PublicFolder {
            id: id.to_string(),
            name: item
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or(id)
                .to_string(),
            can_write: item
                .get("can_write")
                .and_then(|v| v.as_bool())
                .unwrap_or(false),
        });
    }
    Ok(out)
}

/// 按名称（不区分大小写）或 id 匹配公有文件夹
fn match_public_folder<'a>(folders: &'a [PublicFolder], key: &str) -> Option<&'a PublicFolder> {
    let key = key.trim();
    if key.is_empty() {
        return None;
    }
    folders
        .iter()
        .find(|f| f.id == key || f.name.eq_ignore_ascii_case(key))
}

/// 一个同步目标（个人网盘 / 某个公有文件夹）
struct SyncTarget {
    /// 输出用名称
    label: String,
    /// 网盘作用域（`private` / `public:<id>`）
    scope: String,
    /// 状态文件里的键
    state_key: String,
    local_root: PathBuf,
    /// 目标在网盘内的子目录（空 = 根）
    remote_root: String,
    direction: SyncDirection,
    conflict: SyncConflictPolicy,
}

/// 各目标共用的连接信息
struct SyncShared {
    client: Client,
    download_client: Client,
    base_url: String,
    token: Option<String>,
    now_ms: u64,
}

/// 运行一个同步目标：返回带统计与最新状态的上下文
async fn sync_one_target(
    shared: &SyncShared,
    target: &SyncTarget,
    state: BTreeMap<String, SyncStateEntry>,
) -> SyncCtx {
    let mut ctx = SyncCtx {
        client: shared.client.clone(),
        download_client: shared.download_client.clone(),
        base_url: shared.base_url.clone(),
        token: shared.token.clone(),
        scope: target.scope.clone(),
        local_root: target.local_root.clone(),
        remote_root: target.remote_root.clone(),
        direction: target.direction,
        conflict_policy: target.conflict,
        now_ms: shared.now_ms,
        state,
        uploaded: 0,
        downloaded: 0,
        skipped: 0,
        conflicts: 0,
        failed: 0,
        uploaded_bytes: 0,
        downloaded_bytes: 0,
        state_dirty: false,
    };

    let mut queue: Vec<String> = vec![String::new()];
    while let Some(rel) = queue.pop() {
        println!(
            "[{}]",
            if rel.is_empty() {
                "/".to_string()
            } else {
                format!("/{}/", rel)
            }
        );
        if let Err(e) = sync_one_dir(&mut ctx, &rel, &mut queue).await {
            ctx.failed += 1;
            println!("  同步目录失败 {}: {}", rel, e);
        }
    }
    ctx
}

/// 网盘子目录归一化（去空白与首尾 `/`，空串 = 根目录）
fn normalize_remote_subdir(raw: Option<&str>) -> String {
    raw.map(|s| s.trim().trim_matches('/').to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_default()
}

/// `sync` 子命令入口：配置全部来自 `~/.okb/client.json`
pub(crate) async fn run_sync() -> Result<()> {
    let cfg = load_config();
    let sync_cfg = cfg.sync.clone().unwrap_or_default();
    let base_url = cfg.base_url();
    let token = cfg.token.clone();

    // ── 组装同步目标 ────────────────────────────────────────
    let mut targets: Vec<SyncTarget> = Vec::new();

    // 个人网盘（配了 local / personal.disk 才有）
    if let Some(local) = sync_cfg
        .local()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        let remote_root = normalize_remote_subdir(sync_cfg.remote());
        targets.push(SyncTarget {
            label: "个人网盘".to_string(),
            state_key: format!("private:{}", remote_root),
            scope: "private".to_string(),
            local_root: PathBuf::from(local),
            remote_root,
            direction: SyncDirection::parse(sync_cfg.direction())?,
            conflict: SyncConflictPolicy::parse(sync_cfg.conflict())?,
        });
    }

    // 公有文件夹（可配多个）
    if !sync_cfg.public.is_empty() {
        let client = http_client()?;
        let folders = drive_public_folders(&client, &base_url, &token).await?;
        for (idx, entry) in sync_cfg.public.iter().enumerate() {
            let folder_raw = entry
                .folder
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty());
            let Some(folder_raw) = folder_raw else {
                bail!(
                    "sync.public[{}] 缺少 folder（写公有文件夹名称或 id，如 \"团队资料\" / \"pf_1a2b3c4d\"）",
                    idx
                );
            };
            let disk_raw = entry
                .disk
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty());
            let Some(disk_raw) = disk_raw else {
                bail!("sync.public[{}]（{}）缺少 disk（本地目录）", idx, folder_raw);
            };
            let Some(folder) = match_public_folder(&folders, folder_raw) else {
                let names: Vec<String> = folders
                    .iter()
                    .map(|f| format!("{}（{}）", f.name, f.id))
                    .collect();
                bail!(
                    "找不到公有文件夹「{}」；当前账号可访问的公有文件夹：{}",
                    folder_raw,
                    if names.is_empty() {
                        "（无）".to_string()
                    } else {
                        names.join("、")
                    }
                );
            };

            // 公有文件夹是共享的：未显式指定方向时默认只读镜像（download）
            let mut direction = match entry.direction.as_deref() {
                Some(raw) => SyncDirection::parse(Some(raw))?,
                None => SyncDirection::Download,
            };
            let mut note = String::new();
            if direction.can_upload() && !folder.can_write {
                direction = SyncDirection::Download;
                note = "（只读文件夹，已改为单向下载）".to_string();
            }
            let remote_root = normalize_remote_subdir(entry.remote.as_deref());
            targets.push(SyncTarget {
                label: format!(
                    "公有文件夹「{}」{}{}",
                    folder.name,
                    if folder.can_write { "可写" } else { "只读" },
                    note
                ),
                state_key: format!("public:{}:{}", folder.id, remote_root),
                scope: format!("public:{}", folder.id),
                local_root: PathBuf::from(disk_raw),
                remote_root,
                direction,
                conflict: SyncConflictPolicy::parse(entry.conflict.as_deref())?,
            });
        }
    }

    if targets.is_empty() {
        bail!(
            "~/.okb/client.json 里没有可同步的目标。示例：\n\
             \"sync\": {{\n\
             \x20 \"personal\": {{ \"disk\": \"D:\\\\okb-disk\", \"remote\": \"sync\", \"direction\": \"two-way\" }},\n\
             \x20 \"public\": [ {{ \"folder\": \"团队资料\", \"disk\": \"D:\\\\okb-public\\\\team\", \"direction\": \"download\" }} ]\n\
             }}"
        );
    }

    // ── 表头 ────────────────────────────────────────────────
    println!("网盘同步（sync）");
    println!("  服务地址: {}", base_url);
    println!("  同步目标: {} 个", targets.len());
    if token.is_none() {
        println!("  ⚠ 未配置 token（仅受信任子网免鉴权时可用）");
    }
    println!("  说明: 删除不传播（一侧删掉的文件会从另一侧补回）；跳过 Thumbs.db / desktop.ini / .DS_Store");
    println!("        公有文件夹默认只读镜像（direction 缺省 = download）；要写入需显式配置且有写权限");
    println!();

    let shared = SyncShared {
        client: http_client()?,
        download_client: Client::builder()
            .redirect(reqwest::redirect::Policy::default())
            .build()
            .context("无法创建下载客户端")?,
        base_url,
        token,
        now_ms: now_epoch_ms(),
    };

    let mut state_file = load_sync_state_file();
    let started = std::time::Instant::now();
    let mut state_dirty = false;
    // 严格模式下的冲突数（多目标时各目标策略可能不同，需要分开统计）
    let mut strict_conflicts = 0usize;

    // 跨目标汇总（借 SyncCtx 装计数，避免再定义一套结构）
    let mut totals = (
        0usize, // 上传个数
        0u64,   // 上传字节
        0usize, // 下载个数
        0u64,   // 下载字节
        0usize, // 跳过
        0usize, // 冲突
        0usize, // 失败
    );

    for target in &targets {
        println!("── {} ──", target.label);
        println!("  本地目录: {}", target.local_root.display());
        println!(
            "  网盘目录: {}（scope={}）",
            if target.remote_root.is_empty() {
                "/".to_string()
            } else {
                format!("/{}", target.remote_root)
            },
            target.scope
        );
        println!("  同步方向: {}", target.direction.label());
        println!("  冲突策略: {}", target.conflict.label());

        if let Err(e) = tokio::fs::create_dir_all(&target.local_root).await {
            totals.6 += 1;
            println!("  无法创建本地目录 {}: {}", target.local_root.display(), e);
            println!();
            continue;
        }
        if target.direction.can_upload() {
            if let Err(e) = ensure_remote_dir(
                &shared.client,
                &shared.base_url,
                &shared.token,
                &target.scope,
                &target.remote_root,
            )
            .await
            {
                totals.6 += 1;
                println!("  准备网盘目录失败: {}", e);
                println!();
                continue;
            }
        }

        let entries = state_entries_for(&state_file, &target.state_key);
        let ctx = sync_one_target(&shared, target, entries).await;

        println!(
            "  → 上传 {}（{}）· 下载 {}（{}）· 跳过 {} · 冲突 {} · 失败 {}",
            ctx.uploaded,
            format_bytes(ctx.uploaded_bytes),
            ctx.downloaded,
            format_bytes(ctx.downloaded_bytes),
            ctx.skipped,
            ctx.conflicts,
            ctx.failed
        );
        println!();

        totals.0 += ctx.uploaded;
        totals.1 += ctx.uploaded_bytes;
        totals.2 += ctx.downloaded;
        totals.3 += ctx.downloaded_bytes;
        totals.4 += ctx.skipped;
        totals.5 += ctx.conflicts;
        if target.conflict == SyncConflictPolicy::Strict {
            strict_conflicts += ctx.conflicts;
        }
        totals.6 += ctx.failed;

        // 存回该目标的最新记录（冲突文件的旧记录由 ctx.state 保留）
        state_dirty |= ctx.state_dirty;
        state_file
            .targets
            .insert(target.state_key.clone(), ctx.state.clone());
    }

    if state_dirty {
        save_sync_state(&state_file);
    }

    println!("==================================================");
    println!("上传:   {} 个（{}）", totals.0, format_bytes(totals.1));
    println!("下载:   {} 个（{}）", totals.2, format_bytes(totals.3));
    println!("跳过:   {} 个（已同步 / 方向不适用）", totals.4);
    if totals.5 > 0 {
        println!(
            "冲突:   {} 个（{}）",
            totals.5,
            if strict_conflicts > 0 {
                format!("其中 {} 个严格模式未改动，需人工处理", strict_conflicts)
            } else {
                "已按较新一方处理".to_string()
            }
        );
    }
    println!("失败:   {} 个", totals.6);
    println!("用时:   {}", format_duration(started.elapsed()));
    println!("==================================================");

    if totals.6 > 0 {
        bail!("同步完成，但有 {} 项失败", totals.6);
    }
    if strict_conflicts > 0 {
        bail!(
            "同步完成，但有 {} 个文件在两侧都被改动（严格模式未改动任何一侧）。\n\
             处理方式：对比两版内容后让两侧一致，然后删掉 ~/.okb/client/sync-state.json（或其中对应条目）再跑一次；\n\
             也可以临时把 sync.conflict 改成 newer 跑一次，让较新的一方自动覆盖。",
            strict_conflicts
        );
    }
    Ok(())
}

/// 当前时间（Unix 毫秒）
fn now_epoch_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// 把某个 mtime 显示成相对当前的时长（不比绝对时间，避开时区问题）
fn human_age(ts_ms: u64, now_ms: u64) -> String {
    if ts_ms == 0 {
        return "时间未知".to_string();
    }
    let secs = now_ms.saturating_sub(ts_ms) / 1000;
    if secs < 60 {
        "刚刚".to_string()
    } else if secs < 3600 {
        format!("{} 分钟前", secs / 60)
    } else if secs < 86_400 {
        format!("{} 小时前", secs / 3600)
    } else {
        format!("{} 天前", secs / 86_400)
    }
}

#[cfg(test)]
mod sync_tests {
    use super::*;

    /// 默认（非严格）策略下的判定，便于复用旧断言
    fn decide(
        direction: SyncDirection,
        local: Option<&LocalSig>,
        remote: Option<&RemoteSig>,
        saved: Option<&SyncStateEntry>,
    ) -> SyncDecision {
        let (action, _) = judge(direction, SyncConflictPolicy::Newer, local, remote, saved);
        action
    }

    fn lsig(size: u64, mtime_ms: u64) -> LocalSig {
        LocalSig { size, mtime_ms }
    }
    fn rsig(size: u64, modified: &str) -> RemoteSig {
        RemoteSig {
            size,
            modified: Some(modified.to_string()),
        }
    }

    #[test]
    fn parses_rfc3339_to_epoch_ms() {
        // 2026-11-09T00:00:00Z = 1794182400 s
        assert_eq!(
            parse_rfc3339_ms("2026-11-09T00:00:00Z"),
            Some(1_794_182_400_000)
        );
        // 东八区同样时刻
        assert_eq!(
            parse_rfc3339_ms("2026-11-09T08:00:00+08:00"),
            Some(1_794_182_400_000)
        );
        // 小数秒保留到毫秒
        assert_eq!(
            parse_rfc3339_ms("2026-11-09T00:00:00.123456789+00:00"),
            Some(1_794_182_400_123)
        );
        assert_eq!(parse_rfc3339_ms("不是时间"), None);
    }

    #[test]
    fn direction_default_and_aliases() {
        assert_eq!(SyncDirection::parse(None).unwrap(), SyncDirection::TwoWay);
        assert_eq!(
            SyncDirection::parse(Some("  ")).unwrap(),
            SyncDirection::TwoWay
        );
        assert_eq!(
            SyncDirection::parse(Some("two_way")).unwrap(),
            SyncDirection::TwoWay
        );
        assert_eq!(
            SyncDirection::parse(Some("Push")).unwrap(),
            SyncDirection::Upload
        );
        assert_eq!(
            SyncDirection::parse(Some("pull")).unwrap(),
            SyncDirection::Download
        );
        assert!(SyncDirection::parse(Some("mirror")).is_err());
    }

    #[test]
    fn two_way_three_way_merge() {
        let saved = SyncStateEntry {
            local_size: 10,
            local_mtime_ms: 1_000,
            remote_size: 10,
            remote_modified: Some("2026-11-09T00:00:00+00:00".into()),
        };
        let remote_old = rsig(10, "2026-11-09T00:00:00+00:00");
        let remote_new = rsig(20, "2026-11-10T00:00:00+00:00");
        // 两侧都没动 → 跳过
        assert_eq!(
            decide(SyncDirection::TwoWay, Some(&lsig(10, 1_000)), Some(&remote_old), Some(&saved)),
            SyncDecision::Skip
        );
        // 只有本地改了 → 上传
        assert_eq!(
            decide(SyncDirection::TwoWay, Some(&lsig(12, 2_000)), Some(&remote_old), Some(&saved)),
            SyncDecision::Upload
        );
        // 只有网盘改了 → 下载
        assert_eq!(
            decide(SyncDirection::TwoWay, Some(&lsig(10, 1_000)), Some(&remote_new), Some(&saved)),
            SyncDecision::Download
        );
        // 两侧都改了 → 网盘的 mtime 更新，选下载
        assert_eq!(
            decide(SyncDirection::TwoWay, Some(&lsig(12, 2_000)), Some(&remote_new), Some(&saved)),
            SyncDecision::Download
        );
        // 两侧都改了 → 本地更新，选上传
        let remote_mid = rsig(20, "2026-11-09T00:00:30+00:00");
        // 本地 mtime 比网盘那次上传晚 1 秒 → 上传
        assert_eq!(
            decide(
                SyncDirection::TwoWay,
                Some(&lsig(12, 1_794_182_431_000)),
                Some(&remote_mid),
                Some(&saved)
            ),
            SyncDecision::Upload
        );
    }

    #[test]
    fn one_side_only_respects_direction() {
        let local = lsig(10, 1_000);
        let remote = rsig(10, "2026-11-09T00:00:00+00:00");
        // 双向：补齐缺失的一侧
        assert_eq!(
            decide(SyncDirection::TwoWay, Some(&local), None, None),
            SyncDecision::Upload
        );
        assert_eq!(
            decide(SyncDirection::TwoWay, None, Some(&remote), None),
            SyncDecision::Download
        );
        // 单向上传：本地独有的传上去，网盘独有的不动
        assert_eq!(
            decide(SyncDirection::Upload, Some(&local), None, None),
            SyncDecision::Upload
        );
        assert_eq!(
            decide(SyncDirection::Upload, None, Some(&remote), None),
            SyncDecision::Skip
        );
        // 单向下载：对称
        assert_eq!(
            decide(SyncDirection::Download, Some(&local), None, None),
            SyncDecision::Skip
        );
        assert_eq!(
            decide(SyncDirection::Download, None, Some(&remote), None),
            SyncDecision::Download
        );
    }

    #[test]
    fn first_sync_same_size_is_skipped() {
        // 没有状态记录、大小一致 → 当成已同步（避免全量重传）
        let remote = rsig(10, "2026-11-09T00:00:00+00:00");
        assert_eq!(
            decide(SyncDirection::TwoWay, Some(&lsig(10, 1_000)), Some(&remote), None),
            SyncDecision::Skip
        );
        // 大小不同 → 看谁更新（网盘 2026 年 > 本地 1970 年）
        assert_eq!(
            decide(SyncDirection::TwoWay, Some(&lsig(10, 1_000)), Some(&rsig(20, "2026-11-09T00:00:00+00:00")), None),
            SyncDecision::Download
        );
        assert_eq!(
            decide(SyncDirection::TwoWay, Some(&lsig(10, 1_000)), Some(&rsig(20, "1970-01-01T00:00:01+00:00")), None),
            SyncDecision::Upload
        );
    }

    #[test]
    fn upload_direction_always_wins_over_remote_change() {
        let saved = SyncStateEntry {
            local_size: 10,
            local_mtime_ms: 1_000,
            remote_size: 10,
            remote_modified: Some("2026-11-09T00:00:00+00:00".into()),
        };
        // 网盘被人改过，但单向上传仍以本地为准
        assert_eq!(
            decide(
                SyncDirection::Upload,
                Some(&lsig(10, 1_000)),
                Some(&rsig(30, "2026-11-10T00:00:00+00:00")),
                Some(&saved)
            ),
            SyncDecision::Upload
        );
    }

    #[test]
    fn ignores_sync_temp_and_junk_files() {
        assert!(is_ignored_name(".a.pdf.okbsync-part"));
        assert!(is_ignored_name("Thumbs.db"));
        assert!(is_ignored_name("desktop.ini"));
        assert!(is_ignored_name(".DS_Store"));
        assert!(!is_ignored_name("paper.pdf"));
    }

    #[test]
    fn remote_path_join() {
        assert_eq!(remote_join("", ""), "");
        assert_eq!(remote_join("sync", ""), "sync");
        assert_eq!(remote_join("", "a.pdf"), "a.pdf");
        assert_eq!(remote_join("sync", "a/b.pdf"), "sync/a/b.pdf");
    }

    #[test]
    fn conflict_policy_parsing() {
        assert_eq!(
            SyncConflictPolicy::parse(None).unwrap(),
            SyncConflictPolicy::Newer
        );
        assert_eq!(
            SyncConflictPolicy::parse(Some(" latest ")).unwrap(),
            SyncConflictPolicy::Newer
        );
        assert_eq!(
            SyncConflictPolicy::parse(Some("Strict")).unwrap(),
            SyncConflictPolicy::Strict
        );
        assert_eq!(
            SyncConflictPolicy::parse(Some("手动")).unwrap(),
            SyncConflictPolicy::Strict
        );
        assert!(SyncConflictPolicy::parse(Some("overwrite")).is_err());
    }

    #[test]
    fn strict_mode_reports_instead_of_touching_files() {
        let saved = SyncStateEntry {
            local_size: 10,
            local_mtime_ms: 1_000,
            remote_size: 10,
            remote_modified: Some("2026-11-09T00:00:00+00:00".into()),
        };
        // 本地 2026-11-09T00:00:00Z 改过，网盘 00:00:30 改过（网盘更新）
        let local = lsig(12, 1_794_182_400_000);
        let remote = rsig(20, "2026-11-09T00:00:30+00:00");
        // 两侧都改过 → 冲突（不动文件），同时给出原因
        assert_eq!(
            judge(
                SyncDirection::TwoWay,
                SyncConflictPolicy::Strict,
                Some(&local),
                Some(&remote),
                Some(&saved)
            ),
            (SyncDecision::Conflict, Some(ConflictKind::BothChanged))
        );
        // 非严格模式：自动取较新一方（网盘）
        assert_eq!(
            judge(
                SyncDirection::TwoWay,
                SyncConflictPolicy::Newer,
                Some(&local),
                Some(&remote),
                Some(&saved)
            ),
            (SyncDecision::Download, Some(ConflictKind::BothChanged))
        );
        // 严格模式不影响「能确定方向」的变更：只有本地改 → 仍然上传
        assert_eq!(
            judge(
                SyncDirection::TwoWay,
                SyncConflictPolicy::Strict,
                Some(&lsig(12, 1_794_182_500_000)),
                Some(&rsig(10, "2026-11-09T00:00:00+00:00")),
                Some(&saved)
            ),
            (SyncDecision::Upload, None)
        );
        // 只有网盘改 → 仍然下载
        assert_eq!(
            judge(
                SyncDirection::TwoWay,
                SyncConflictPolicy::Strict,
                Some(&lsig(10, 1_000)),
                Some(&rsig(20, "2026-11-09T00:00:30+00:00")),
                Some(&saved)
            ),
            (SyncDecision::Download, None)
        );
    }

    #[test]
    fn strict_mode_first_sync_of_different_sizes_is_a_conflict() {
        let local = lsig(10, 1_794_182_500_000);
        // 无状态 + 大小不同 → 无法判断，严格模式下报冲突
        assert_eq!(
            judge(
                SyncDirection::TwoWay,
                SyncConflictPolicy::Strict,
                Some(&local),
                Some(&rsig(20, "2026-11-09T00:00:00+00:00")),
                None
            ),
            (SyncDecision::Conflict, Some(ConflictKind::NoBaseline))
        );
        // 无状态 + 大小相同 → 当作同一份内容，跳过（不算冲突）
        assert_eq!(
            judge(
                SyncDirection::TwoWay,
                SyncConflictPolicy::Strict,
                Some(&local),
                Some(&rsig(10, "2026-11-09T00:00:00+00:00")),
                None
            ),
            (SyncDecision::Skip, None)
        );
        // 单向同步从不报冲突（以指定一侧为准）
        assert_eq!(
            judge(
                SyncDirection::Upload,
                SyncConflictPolicy::Strict,
                Some(&local),
                Some(&rsig(20, "2026-11-09T00:00:00+00:00")),
                None
            ),
            (SyncDecision::Upload, None)
        );
    }

    #[test]
    fn public_folder_matches_by_name_or_id() {
        let folders = vec![
            PublicFolder {
                id: "pf_team".into(),
                name: "团队资料".into(),
                can_write: true,
            },
            PublicFolder {
                id: "pf_read".into(),
                name: "Public Docs".into(),
                can_write: false,
            },
        ];
        assert_eq!(
            match_public_folder(&folders, "团队资料").map(|f| f.id.as_str()),
            Some("pf_team")
        );
        assert_eq!(
            match_public_folder(&folders, " public docs ").map(|f| f.id.as_str()),
            Some("pf_read")
        );
        assert_eq!(
            match_public_folder(&folders, "pf_team").map(|f| f.name.as_str()),
            Some("团队资料")
        );
        assert!(match_public_folder(&folders, "不存在").is_none());
        assert!(match_public_folder(&folders, "   ").is_none());
    }

    #[test]
    fn remote_subdir_is_normalized() {
        assert_eq!(normalize_remote_subdir(None), "");
        assert_eq!(normalize_remote_subdir(Some("   ")), "");
        assert_eq!(normalize_remote_subdir(Some("/sync/")), "sync");
        assert_eq!(normalize_remote_subdir(Some(" a/b ")), "a/b");
    }

    #[test]
    fn human_age_renders_relative_time() {
        let now = 1_794_182_400_000u64;
        assert_eq!(human_age(now, now), "刚刚");
        assert_eq!(human_age(now - 90_000, now), "1 分钟前");
        assert_eq!(human_age(now - 2 * 3_600_000, now), "2 小时前");
        assert_eq!(human_age(now - 3 * 86_400_000, now), "3 天前");
        assert_eq!(human_age(0, now), "时间未知");
        // 时钟偏差（未来时间）不会 panics
        assert_eq!(human_age(now + 5_000, now), "刚刚");
    }
}
