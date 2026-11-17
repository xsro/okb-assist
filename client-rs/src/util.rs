//! 通用工具：客户端配置、DOI→哈希记录、HTTP 客户端、服务端 API 调用、上传节流与进度显示

use std::collections::{HashMap, HashSet};
use std::io::{self, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;

use anyhow::{Context, Result};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio_stream::StreamExt;

// ==================== 上传节流参数 ====================

/// 上传节流参数（`zotero` / `dir` 两个子命令共用）
#[derive(clap::Args, Debug, Clone)]
pub(crate) struct UploadIntervalArgs {
    /// 每次上传之间的间隔秒数（默认 1 秒；可写小数，0 表示不等待）
    #[arg(long, value_name = "SECONDS", default_value_t = 1.0)]
    pub(crate) upload_interval: f64,

    /// 单次上传请求的超时秒数（0 = 自动：300s + 每 MB 30s，上限 2 小时）；
    /// 慢链路或服务端卡顿（表现为 `error sending request`）时调大
    #[arg(long, value_name = "SECONDS", default_value_t = 0.0)]
    pub(crate) upload_timeout: f64,

    /// 不显示上传进度（默认在传输时实时刷新「已发送 / 总大小、百分比、速度、剩余时间」）
    #[arg(long)]
    pub(crate) no_progress: bool,

    /// 上传时让服务端做完整的 PDF 元数据抽取（`extract_meta=full`：Info 字典 DOI → Crossref，
    /// 没有 DOI 时再跑启发式抽取题名/作者/年份/摘要等）
    ///
    /// 默认**不开**（请求里带 `extract_meta=false`）：抽取会起几个 mutool 子进程把整份文档正文过一遍，
    /// 批量上传大文件时这是「等待服务端处理」的主要耗时。
    /// 不抽也能先把记录建好（列表里显示文件名），之后在文档管理页 / 
    /// `POST /assist/api/pipeline/extract-pdf-meta/:id` 补即可。
    #[arg(long)]
    pub(crate) extract_meta: bool,
}

impl UploadIntervalArgs {
    /// 传给 `upload_client` 的超时覆盖值（<= 0 表示自动）
    pub(crate) fn timeout_override(&self) -> Option<f64> {
        Some(self.upload_timeout).filter(|s| *s > 0.0)
    }

    /// 第 `seq` 次上传（从 0 开始）前的等待：首个不等待，间隔 <= 0 也不等待
    pub(crate) async fn throttle(&self, seq: usize) {        if seq > 0 && self.upload_interval > 0.0 {
            tokio::time::sleep(std::time::Duration::from_secs_f64(self.upload_interval)).await;
        }
    }

    /// 头部提示用的可读描述
    pub(crate) fn describe(&self) -> String {
        if self.upload_interval > 0.0 {
            format!("{} 秒", self.upload_interval)
        } else {
            "无间隔".to_string()
        }
    }
}

// ==================== 数据类型 ====================

/// 文档元数据（上传后 PUT 更新用，字段名与服务端一致）
#[derive(Debug, Clone, Serialize)]
pub(crate) struct DocMetadata {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) authors: Option<String>, // JSON array string
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) year: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) doi: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) journal: Option<String>,
    #[serde(rename = "abstract", skip_serializing_if = "Option::is_none")]
    pub(crate) abstract_text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) keywords: Option<String>, // JSON array string
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) doc_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) language: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) source: Option<String>,
}

/// diff-hashes 接口返回
///
/// `invalid` / `present_count` 目前没有消费方（`dir` 改成逐个比对后不再依赖汇总字段），
/// 但保留结构以便接口变更时有迹可循，故允许 dead_code。
#[allow(dead_code)]
#[derive(Debug, Deserialize)]
pub(crate) struct HashDiffResult {
    pub(crate) missing: Vec<String>,
    pub(crate) invalid: Vec<String>,
    #[serde(default)]
    pub(crate) present_count: Option<usize>,
}

/// doi-hash-index 接口返回的单条记录
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct ServerEntry {
    #[serde(default)]
    pub(crate) id: u64,
    #[serde(default)]
    pub(crate) doi: Option<String>,
    #[serde(default)]
    pub(crate) doi_key: Option<String>,
    #[serde(default)]
    pub(crate) file_hash: Option<String>,
    #[serde(default)]
    pub(crate) pdf_health: Option<String>,
}

impl ServerEntry {
    /// 服务端文件哈希（空串 = 服务端没有 PDF）
    pub(crate) fn hash(&self) -> &str {
        self.file_hash.as_deref().unwrap_or("")
    }

    /// PDF 健康状态（空串 = 从未检查过）
    pub(crate) fn health(&self) -> &str {
        self.pdf_health.as_deref().unwrap_or("")
    }

    /// 从 `by-doi` 返回的文档 JSON 构造（服务端索引不可用时的逐条查询回退路径）
    pub(crate) fn from_doc_json(doc: &serde_json::Value) -> Self {
        let text = |key: &str| {
            doc.get(key)
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
        };
        ServerEntry {
            id: doc.get("id").and_then(|v| v.as_u64()).unwrap_or(0),
            doi: text("doi"),
            doi_key: None,
            file_hash: text("file_hash"),
            pdf_health: text("pdf_health"),
        }
    }
}

/// doi-hash-index 接口返回
#[derive(Debug, Deserialize)]
pub(crate) struct ServerIndexResponse {
    #[serde(default)]
    pub(crate) count: Option<usize>,
    #[serde(default)]
    pub(crate) documents: Vec<ServerEntry>,
}

/// 服务端索引的本地视图：一次拉全量，后续比对全在本地完成。
pub(crate) struct ServerIndex {
    /// 服务端文献总数（含没有 DOI / 没有 PDF 的）
    pub(crate) count: usize,
    /// DOI 归一化键 → 记录
    pub(crate) by_doi: HashMap<String, ServerEntry>,
    /// 服务端已有文件的哈希（小写）
    pub(crate) hashes: HashSet<String>,
}

impl ServerIndex {
    pub(crate) fn from_response(resp: ServerIndexResponse) -> Self {
        let count = resp.count.unwrap_or(resp.documents.len());
        let mut by_doi: HashMap<String, ServerEntry> = HashMap::new();
        let mut hashes: HashSet<String> = HashSet::new();
        for entry in resp.documents {
            if let Some(h) = entry.file_hash.as_deref().filter(|h| !h.is_empty()) {
                hashes.insert(h.to_lowercase());
            }
            // 服务端已给归一化键就直接用；旧版服务端没这个字段时本地补算
            let key = entry.doi_key.clone().or_else(|| {
                entry
                    .doi
                    .as_deref()
                    .and_then(normalize_doi)
                    .map(|d| d.to_lowercase())
            });
            if let Some(k) = key {
                by_doi.entry(k).or_insert(entry);
            }
        }
        ServerIndex {
            count,
            by_doi,
            hashes,
        }
    }

    /// 按 DOI 查服务端记录（与服务端同一套归一化）
    pub(crate) fn lookup(&self, doi: &str) -> Option<&ServerEntry> {
        let key = normalize_doi(doi)?.to_lowercase();
        self.by_doi.get(&key)
    }

    pub(crate) fn has_hash(&self, hash: &str) -> bool {
        self.hashes.contains(&hash.to_lowercase())
    }
}

/// 上传响应（`doi` / `file_hash` 用于记进 DOI→哈希记录，服务端没返回就为空）
#[derive(Debug, Deserialize)]
pub(crate) struct UploadResult {
    pub(crate) id: u64,
    /// 服务端对这次上传的处置：新建时没有这个字段，去重分支回 `attached` / `skipped` / `replaced`
    #[serde(default)]
    pub(crate) status: Option<String>,
    #[serde(default)]
    pub(crate) doi: Option<String>,
    #[serde(default)]
    pub(crate) file_hash: Option<String>,
}

// ==================== 客户端配置（~/.okb/client.json） ====================

/// 没有配置文件时使用的默认服务地址
pub(crate) const DEFAULT_BASE_URL: &str = "http://192.168.1.162:5001";

/// `~/.okb/client.json` 的内容。字段全部可选，缺什么就用默认/命令行值。
#[derive(Debug, Default, Deserialize)]
pub(crate) struct ClientConfig {
    /// 服务地址（`url` 与 `base_url` 两种写法都接受）
    #[serde(default, alias = "base_url")]
    pub(crate) url: Option<String>,
    /// 访问令牌
    #[serde(default)]
    pub(crate) token: Option<String>,
    /// Zotero storage 根目录（可被 `--storage-root` 覆盖）
    #[serde(default)]
    pub(crate) storage_root: Option<String>,
    /// 个人网盘同步配置（`sync` 子命令用）
    #[serde(default)]
    pub(crate) sync: Option<SyncConfig>,
}

/// `sync` 子命令的配置（`~/.okb/client.json` 的 `sync` 段）
///
/// 两种写法都支持（推荐用与 `sync` 一起的嵌套写法）：
/// ```json
/// "sync": { "personal": { "disk": "D:\\\\okb-disk", "remote": "sync", "direction": "two-way" } }
/// "sync": { "local": "D:\\\\okb-disk", "remote": "sync", "direction": "two-way" }
/// ```
#[derive(Debug, Default, Clone, Deserialize)]
pub(crate) struct SyncConfig {
    /// 本地目录（必填；等价于 `personal.disk`）
    #[serde(default)]
    pub(crate) local: Option<String>,
    /// 个人网盘里的子目录（缺省 = 根目录）
    #[serde(default)]
    pub(crate) remote: Option<String>,
    /// 同步方向：`two-way`（缺省）/ `upload` / `download`
    #[serde(default)]
    pub(crate) direction: Option<String>,
    /// 冲突策略：`newer`（缺省，取较新一方）/ `strict`（只报告，不改动文件）
    #[serde(default)]
    pub(crate) conflict: Option<String>,
    /// 个人网盘的嵌套写法（与平铺字段等价，嵌套优先）
    #[serde(default)]
    pub(crate) personal: Option<SyncPersonalConfig>,
    /// 公有文件夹同步（数组，可配多个；只读的文件夹只能单向下载）
    #[serde(default)]
    pub(crate) public: Vec<SyncPublicConfig>,
}

/// `sync.public[]`：某个公有文件夹的同步目标
///
/// ```json
/// "public": [
///   { "folder": "团队资料", "disk": "D:\okb-public\team", "direction": "download" }
/// ]
/// ```
#[derive(Debug, Default, Clone, Deserialize)]
pub(crate) struct SyncPublicConfig {
    /// 公有文件夹的名称或 id（`pf_xxx`）
    #[serde(default)]
    pub(crate) folder: Option<String>,
    /// 本地目录（必填）
    #[serde(default)]
    pub(crate) disk: Option<String>,
    /// 该文件夹内的子目录（缺省 = 文件夹根）
    #[serde(default)]
    pub(crate) remote: Option<String>,
    /// 同步方向：缺省 `download`（公有文件夹是共享的，默认只读镜像）；
    /// 显式写 `upload` / `two-way` 才会上传（且需要对该文件夹有写权限）
    #[serde(default)]
    pub(crate) direction: Option<String>,
    /// 冲突策略：`newer`（缺省）/ `strict`
    #[serde(default)]
    pub(crate) conflict: Option<String>,
}

/// `sync.personal`：个人网盘同步的目标
#[derive(Debug, Default, Clone, Deserialize)]
pub(crate) struct SyncPersonalConfig {
    /// 本地目录（对应个人网盘根目录）
    #[serde(default)]
    pub(crate) disk: Option<String>,
    /// 个人网盘里的子目录（缺省 = 根目录）
    #[serde(default)]
    pub(crate) remote: Option<String>,
    /// 同步方向：`two-way`（缺省）/ `upload` / `download`
    #[serde(default)]
    pub(crate) direction: Option<String>,
    /// 冲突策略：`newer`（缺省，取较新一方）/ `strict`（只报告，不改动文件）
    #[serde(default)]
    pub(crate) conflict: Option<String>,
}

impl SyncConfig {
    pub(crate) fn personal(&self) -> Option<&SyncPersonalConfig> {
        self.personal.as_ref()
    }

    pub(crate) fn local(&self) -> Option<&str> {
        self.local
            .as_deref()
            .or_else(|| self.personal().and_then(|p| p.disk.as_deref()))
    }

    pub(crate) fn remote(&self) -> Option<&str> {
        self.remote
            .as_deref()
            .or_else(|| self.personal().and_then(|p| p.remote.as_deref()))
    }

    pub(crate) fn direction(&self) -> Option<&str> {
        self.direction
            .as_deref()
            .or_else(|| self.personal().and_then(|p| p.direction.as_deref()))
    }

    pub(crate) fn conflict(&self) -> Option<&str> {
        self.conflict
            .as_deref()
            .or_else(|| self.personal().and_then(|p| p.conflict.as_deref()))
    }
}

impl ClientConfig {
    pub(crate) fn base_url(&self) -> String {
        self.url
            .clone()
            .filter(|u| !u.trim().is_empty())
            .unwrap_or_else(|| DEFAULT_BASE_URL.to_string())
    }
}

/// `~/.okb/client.json` 路径
pub(crate) fn config_path() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".okb").join("client.json"))
}

/// `~/.okb/client/docs.json` 路径（服务端 doi-hash 索引的本地缓存）
pub(crate) fn docs_cache_path() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".okb").join("client").join("docs.json"))
}

/// 读取配置；文件不存在直接返回默认值，解析失败只打警告（不阻断运行）
pub(crate) fn load_config() -> ClientConfig {
    let Some(path) = config_path() else {
        return ClientConfig::default();
    };
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) => {
            if e.kind() != io::ErrorKind::NotFound {
                eprintln!("警告: 无法读取 {}: {}", path.display(), e);
            }
            return ClientConfig::default();
        }
    };
    match serde_json::from_str::<ClientConfig>(&text) {
        Ok(cfg) => cfg,
        Err(e) => {
            eprintln!("警告: {} 解析失败（按默认配置继续）: {}", path.display(), e);
            ClientConfig::default()
        }
    }
}

/// 把服务端索引的原始响应写入 `~/.okb/client/docs.json`（原子写：先写 `.tmp` 再 rename）
pub(crate) fn save_docs_cache(raw: &str) -> Option<PathBuf> {
    let path = docs_cache_path()?;
    if let Some(parent) = path.parent() {
        if let Err(e) = std::fs::create_dir_all(parent) {
            eprintln!("警告: 无法创建目录 {}: {}", parent.display(), e);
            return None;
        }
    }
    let tmp = path.with_extension("json.tmp");
    if let Err(e) = std::fs::write(&tmp, raw) {
        eprintln!("警告: 无法写入 {}: {}", tmp.display(), e);
        return None;
    }
    if let Err(e) = std::fs::rename(&tmp, &path) {
        eprintln!("警告: 无法保存 {}: {}", path.display(), e);
        let _ = std::fs::remove_file(&tmp);
        return None;
    }
    Some(path)
}

// ==================== DOI → 哈希记录（~/.okb/client/doi-hash.json） ====================

/// `~/.okb/client/doi-hash.json` 路径（DOI → 文件 sha256 的本地记录，供 `--reuse-hash` 复用）
pub(crate) fn hash_doi_path() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".okb").join("client").join("doi-hash.json"))
}

/// DOI → sha256 记录（文件内容是一个 JSON 对象，key 为归一化小写 DOI，value 为 sha256 hex）
///
/// 目的：整库 PDF 动辄几 GB，每次运行都把每份文件重读一遍算哈希很慢，
/// 记录一次后可用 `--reuse-hash` 直接复用。**文件被替换后记录会过期**，
/// 所以只有确定本地文件没换过时才该开 `--reuse-hash`。
pub(crate) struct HashDoiStore {
    pub(crate) path: Option<PathBuf>,
    pub(crate) map: HashMap<String, String>,
    pub(crate) dirty: bool,
}

impl HashDoiStore {
    /// 读取记录；文件不存在 = 空记录，解析失败只警告（不阻断运行）
    pub(crate) fn load() -> Self {
        let path = hash_doi_path();
        let mut map = HashMap::new();
        if let Some(p) = &path {
            match std::fs::read_to_string(p) {
                Ok(text) => match serde_json::from_str::<HashMap<String, String>>(&text) {
                    Ok(m) => map = m,
                    Err(e) => eprintln!("警告: {} 解析失败（按空记录继续）: {}", p.display(), e),
                },
                Err(e) => {
                    if e.kind() != io::ErrorKind::NotFound {
                        eprintln!("警告: 无法读取 {}: {}", p.display(), e);
                    }
                }
            }
        }
        Self {
            path,
            map,
            dirty: false,
        }
    }

    /// DOI → 记录用的 key（去 `https://doi.org/` 等前缀后转小写）
    pub(crate) fn key(doi: &str) -> Option<String> {
        normalize_doi(doi).map(|d| d.to_lowercase())
    }

    pub(crate) fn get(&self, doi: &str) -> Option<&str> {
        let key = Self::key(doi)?;
        self.map.get(&key).map(|s| s.as_str())
    }

    pub(crate) fn len(&self) -> usize {
        self.map.len()
    }

    /// 记录一条 DOI → 哈希（DOI 无效或哈希为空时忽略；已记录的相同值不重复写盘）
    pub(crate) fn record(&mut self, doi: &str, hash: &str) {
        let hash = hash.trim();
        if hash.is_empty() {
            return;
        }
        let Some(key) = Self::key(doi) else {
            return;
        };
        if self.map.get(&key).map(|h| h.as_str()) == Some(hash) {
            return;
        }
        self.map.insert(key, hash.to_string());
        self.dirty = true;
    }

    /// 有改动时原子写回（先写 `doi-hash.json.tmp` 再 rename）
    pub(crate) fn flush(&mut self) {
        if !self.dirty {
            return;
        }
        let Some(path) = self.path.clone() else {
            eprintln!("警告: 无法定位 ~/.okb/client/doi-hash.json，哈希记录未保存");
            return;
        };
        if let Some(parent) = path.parent() {
            if let Err(e) = std::fs::create_dir_all(parent) {
                eprintln!("警告: 无法创建目录 {}: {}", parent.display(), e);
                return;
            }
        }
        let body = match serde_json::to_string_pretty(&self.map) {
            Ok(b) => b,
            Err(e) => {
                eprintln!("警告: 哈希记录序列化失败: {}", e);
                return;
            }
        };
        let tmp = path.with_extension("rs.tmp");
        if let Err(e) = std::fs::write(&tmp, &body) {
            eprintln!("警告: 无法写入 {}: {}", tmp.display(), e);
            return;
        }
        if let Err(e) = std::fs::rename(&tmp, &path) {
            eprintln!("警告: 无法保存 {}: {}", path.display(), e);
            let _ = std::fs::remove_file(&tmp);
            return;
        }
        self.dirty = false;
    }
}

// ==================== Token 读取 ====================

/// 按优先级读取 token: 参数 > 环境变量 > `~/.okb/client.json` 的 `token` > 旧版 `~/.okb_assist_token`
pub(crate) fn read_token(arg_token: Option<&str>, config_token: Option<&str>) -> Option<String> {
    if let Some(t) = arg_token {
        if !t.is_empty() {
            return Some(t.to_string());
        }
    }
    if let Ok(t) = std::env::var("OKB_ASSIST_TOKEN") {
        if !t.is_empty() {
            return Some(t);
        }
    }
    if let Some(t) = config_token.map(|t| t.trim()).filter(|t| !t.is_empty()) {
        return Some(t.to_string());
    }
    let token_file = dirs::home_dir()?.join(".okb_assist_token");
    let content = std::fs::read_to_string(&token_file).ok()?;
    let t = content.trim().to_string();
    if t.is_empty() {
        None
    } else {
        Some(t)
    }
}

// ==================== HTTP 客户端 ====================

/// 构造带超时（120s）且跟随重定向的 HTTP 客户端
pub(crate) fn http_client() -> Result<Client> {
    Client::builder()
        .timeout(std::time::Duration::from_secs(120))
        .redirect(reqwest::redirect::Policy::default())
        .build()
        .context("无法创建 HTTP 客户端")
}

pub(crate) fn auth_headers(token: &Option<String>) -> reqwest::header::HeaderMap {
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert(
        reqwest::header::CONTENT_TYPE,
        "application/json".parse().unwrap(),
    );
    if let Some(t) = token {
        headers.insert("X-Token", t.parse().unwrap());
    }
    headers
}

// ==================== SHA256 哈希 ====================

/// 流式计算文件 SHA256（不把整份文件读进内存）
pub(crate) fn sha256_file(path: &Path) -> Result<String> {
    let file = std::fs::File::open(path).context(format!("无法打开文件: {}", path.display()))?;
    let mut reader = std::io::BufReader::with_capacity(64 * 1024, file);
    let mut hasher = Sha256::new();
    std::io::copy(&mut reader, &mut hasher)
        .context(format!("无法读取文件: {}", path.display()))?;
    Ok(hex::encode(hasher.finalize()))
}

/// 在后台线程算哈希（`sha256_file` 是阻塞 IO，不要直接卡在 async 任务里）
pub(crate) async fn sha256_file_async(path: PathBuf) -> Result<String> {
    tokio::task::spawn_blocking(move || sha256_file(&path))
        .await
        .map_err(|e| anyhow::anyhow!("哈希任务崩溃: {}", e))?
}

// ==================== API 调用 ====================

/// 批量比对文件哈希，返回服务器缺失的哈希列表
pub(crate) async fn api_diff_hashes(
    client: &Client,
    base_url: &str,
    hashes: &[String],
    token: &Option<String>,
) -> Option<HashDiffResult> {
    let url = format!("{}/assist/api/documents/diff-hashes", base_url);
    let body = serde_json::json!({ "hashes": hashes });
    match client
        .post(&url)
        .headers(auth_headers(token))
        .json(&body)
        .send()
        .await
    {
        Ok(resp) if resp.status() == reqwest::StatusCode::OK => resp.json().await.ok(),
        Ok(resp) => {
            eprintln!("错误: diff-hashes 请求失败: {}", resp.status());
            None
        }
        Err(e) => {
            eprintln!("错误: 无法连接服务器进行 diff-hashes: {}", e);
            None
        }
    }
}

/// 一次性拉取服务端全部文献的 DOI / 文件哈希（本地比对，避免逐条 by-doi 往返）
pub(crate) async fn api_fetch_server_index(
    client: &Client,
    base_url: &str,
    token: &Option<String>,
) -> Option<ServerIndex> {
    let url = format!("{}/assist/api/documents/doi-hash-index", base_url);
    let mut req = client.get(&url);
    if let Some(t) = token {
        req = req.header("X-Token", t.as_str());
    }
    match req.send().await {
        Ok(resp) if resp.status() == reqwest::StatusCode::OK => {
            let raw = match resp.text().await {
                Ok(t) => t,
                Err(e) => {
                    eprintln!("  服务端索引读取失败: {}", e);
                    return None;
                }
            };
            let body = match serde_json::from_str::<ServerIndexResponse>(&raw) {
                Ok(b) => b,
                Err(e) => {
                    eprintln!("  服务端索引解析失败: {}", e);
                    return None;
                }
            };
            // 自动落盘一份原始索引（~/.okb/client/docs.json），方便离线查看/被其他工具复用
            match save_docs_cache(&raw) {
                Some(p) => println!("  服务端索引已保存: {}", p.display()),
                None => eprintln!("  警告: 服务端索引未能保存到本地缓存"),
            }
            Some(ServerIndex::from_response(body))
        }
        Ok(resp) => {
            eprintln!("  服务端索引请求失败: {}", resp.status());
            None
        }
        Err(e) => {
            eprintln!("  服务端索引请求失败: {}", e);
            None
        }
    }
}

/// 人性化耗时（<1 秒用毫秒，否则秒）
pub(crate) fn format_duration(d: std::time::Duration) -> String {
    let ms = d.as_millis();
    if ms < 1000 {
        format!("{} ms", ms)
    } else {
        format!("{:.1} s", d.as_secs_f64())
    }
}

/// 人性化文件大小（与上传进度里的单位一致）
pub(crate) fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut idx = 0usize;
    while value >= 1024.0 && idx + 1 < UNITS.len() {
        value /= 1024.0;
        idx += 1;
    }
    if idx == 0 {
        format!("{} B", bytes)
    } else {
        format!("{:.1} {}", value, UNITS[idx])
    }
}

/// 终端显示宽度（中文/全角字符算 2 列，用于擦除上一行残留）
pub(crate) fn display_width(s: &str) -> usize {
    s.chars()
        .map(|c| if (c as u32) >= 0x1100 { 2 } else { 1 })
        .sum()
}

/// 进度条宽度（字符数）
pub(crate) const PROGRESS_BAR_WIDTH: usize = 16;

/// 进度采样间隔
pub(crate) const PROGRESS_TICK: std::time::Duration = std::time::Duration::from_millis(200);

/// 是否显示上传进度（`--no-progress` 置为 false，在 `run_dir` / `run_zotero` 开头设置）
pub(crate) static PROGRESS_ENABLED: AtomicBool = AtomicBool::new(true);

pub(crate) fn percent_of(sent: u64, total: u64) -> u64 {
    if total == 0 {
        100
    } else {
        (sent.min(total) * 100 / total).min(100)
    }
}

pub(crate) fn speed_bps(sent: u64, elapsed: std::time::Duration) -> u64 {
    let secs = elapsed.as_secs_f64().max(0.001);
    (sent as f64 / secs) as u64
}

/// 进度行的箭头（上传 ⬆ / 下载 ⬇）
pub(crate) fn arrow_of(verb: &str) -> &'static str {
    if verb == "下载" {
        "⬇"
    } else {
        "⬆"
    }
}

/// 交互模式下的进度行（单行、原地刷新）
pub(crate) fn progress_line(verb: &str, sent: u64, total: u64, elapsed: std::time::Duration) -> String {
    let pct = percent_of(sent, total);
    let filled = (pct as usize * PROGRESS_BAR_WIDTH / 100).min(PROGRESS_BAR_WIDTH);
    let bar = format!(
        "{}{}",
        "█".repeat(filled),
        "░".repeat(PROGRESS_BAR_WIDTH - filled)
    );
    let speed = speed_bps(sent, elapsed);
    let tail = if sent >= total {
        if verb == "下载" {
            "传输完成，正在写入本地…".to_string()
        } else {
            "传输完成，等待服务端处理…".to_string()
        }
    } else if speed > 0 {
        let eta = (total - sent) as f64 / speed as f64;
        format!(
            "{}/s · 剩余约 {}",
            format_bytes(speed),
            format_duration(std::time::Duration::from_secs_f64(eta))
        )
    } else {
        String::new()
    };
    let head = format!(
        "{} {:>3}% [{}] {} / {}",
        arrow_of(verb),
        pct,
        bar,
        format_bytes(sent),
        format_bytes(total)
    );
    if tail.is_empty() {
        head
    } else {
        format!("{} · {}", head, tail)
    }
}

/// 非交互模式（重定向到文件/管道）下的进度行，按 10% 粒度打印
pub(crate) fn progress_line_plain(verb: &str, sent: u64, total: u64) -> String {
    format!(
        "{} {} {}% {} / {}",
        arrow_of(verb),
        verb,
        percent_of(sent, total),
        format_bytes(sent),
        format_bytes(total)
    )
}

/// 传输进度：传输时只做一次 `fetch_add`，后台任务每 200ms 采样并画一行进度。
///
/// - 终端（stderr 是 TTY）：`\r` 原地刷新，进度独占一行（调用方的半行提示会先补一个换行）；
/// - 非终端：每 10% 打一行，避免 carriage return 把日志文件刷成一团。
///
/// 结束时（`finish`）打印一行汇总：已传输 / 总大小 / 用时 / 平均速度，失败时标注已传输量。
pub(crate) struct TransferProgress {
    /// 已从本地文件读出发送的字节数（上传递增）/ 已写入本地的字节数（下载递增）
    pub(crate) sent: Arc<AtomicU64>,
    /// 进度文案里的动作（“上传” / “下载”）
    pub(crate) verb: &'static str,
    pub(crate) total: u64,
    pub(crate) started: std::time::Instant,
    pub(crate) handle: Option<tokio::task::JoinHandle<()>>,
    pub(crate) enabled: bool,
    pub(crate) interactive: bool,
    /// 上一行已画出的显示宽度（用于擦除残留字符）
    pub(crate) last_width: Arc<AtomicUsize>,
    /// 进度是否已经独占了一行（首次输出前需要补换行）
    pub(crate) line_started: Arc<AtomicBool>,
}

impl TransferProgress {
    /// 上传进度（`⬆ 上传`）
    pub(crate) fn start(total: u64) -> Self {
        Self::start_with(total, "上传")
    }

    /// 下载进度（`⬇ 下载`）
    pub(crate) fn start_download(total: u64) -> Self {
        Self::start_with(total, "下载")
    }

    pub(crate) fn start_with(total: u64, verb: &'static str) -> Self {
        let enabled = PROGRESS_ENABLED.load(Ordering::Relaxed) && total > 0;
        let interactive = io::stderr().is_terminal();
        let sent = Arc::new(AtomicU64::new(0));
        let last_width = Arc::new(AtomicUsize::new(0));
        let line_started = Arc::new(AtomicBool::new(false));
        let mut this = TransferProgress {
            sent: sent.clone(),
            verb,
            total,
            started: std::time::Instant::now(),
            handle: None,
            enabled,
            interactive,
            last_width: last_width.clone(),
            line_started: line_started.clone(),
        };
        if !enabled {
            return this;
        }

        let started = this.started;
        this.handle = Some(tokio::spawn(async move {
            let mut ticker = tokio::time::interval(PROGRESS_TICK);
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            let mut last_milestone = 0u64;
            loop {
                ticker.tick().await;
                let current = sent.load(Ordering::Relaxed).min(total);
                if interactive {
                    let line = progress_line(verb, current, total, started.elapsed());
                    let mut w = io::stderr();
                    if !line_started.swap(true, Ordering::Relaxed) {
                        // 调用方（`上传 ... ` 之类）往往没有换行，先收尾再独占一行
                        let _ = writeln!(w);
                    }
                    let width = display_width(&line);
                    let pad = last_width.load(Ordering::Relaxed).saturating_sub(width);
                    let _ = write!(w, "\r{}{}", line, " ".repeat(pad));
                    let _ = w.flush();
                    last_width.store(width, Ordering::Relaxed);
                } else {
                    let pct = percent_of(current, total);
                    if pct / 10 > last_milestone / 10 {
                        last_milestone = pct;
                        eprintln!("{}", progress_line_plain(verb, current, total));
                    }
                }
            }
        }));
        this
    }

    /// 传输流用的计数器（`ReaderStream` 的 `map` 里 `fetch_add`）
    pub(crate) fn counter(&self) -> Arc<AtomicU64> {
        self.sent.clone()
    }

    /// 收尾：停掉采样任务，画一行最终结果（成功显示总量/用时/平均速度，失败显示已发送量）
    pub(crate) fn finish(mut self, ok: bool) {
        if let Some(handle) = self.handle.take() {
            handle.abort();
        }
        if !self.enabled {
            return;
        }
        let sent = self.sent.load(Ordering::Relaxed).min(self.total);
        let elapsed = self.started.elapsed();
        let speed = speed_bps(sent, elapsed);
        let arrow = arrow_of(self.verb);
        let (done, interrupted) = if self.verb == "下载" {
            ("已下载", "下载中断：已接收")
        } else {
            ("已发送", "上传中断：已发送")
        };
        let line = if ok {
            format!(
                "{} {} {} / {}（用时 {}，平均 {}/s）",
                arrow,
                done,
                format_bytes(sent),
                format_bytes(self.total),
                format_duration(elapsed),
                format_bytes(speed)
            )
        } else {
            format!(
                "{} {} {} / {}（{}%），用时 {}",
                arrow,
                interrupted,
                format_bytes(sent),
                format_bytes(self.total),
                percent_of(sent, self.total),
                format_duration(elapsed)
            )
        };

        if self.interactive {
            let mut w = io::stderr();
            if !self.line_started.swap(true, Ordering::Relaxed) {
                let _ = writeln!(w);
            }
            let pad = self
                .last_width
                .load(Ordering::Relaxed)
                .saturating_sub(display_width(&line));
            let _ = writeln!(w, "\r{}{}", line, " ".repeat(pad));
            let _ = w.flush();
        } else {
            eprintln!("{}", line);
        }
    }
}

impl Drop for TransferProgress {
    fn drop(&mut self) {
        // 未走到 `finish`（提前 return / panic）时别把采样任务留在后台
        if let Some(handle) = self.handle.take() {
            handle.abort();
        }
    }
}

/// 上传类请求的超时：默认 300s + 每 MB 30s（上限 2 小时），
/// 可用 `--upload-timeout` 覆盖。
///
/// 基线从旧的 120s 提到 300s：实测慢链路/服务端卡顿时「2 MB 传了 6 分钟」，
/// 旧基线会在服务端已经收完/正在写入时先把请求掰断（表现为 `error sending request`）。
pub(crate) fn upload_client(file_size: u64, timeout_override: Option<f64>) -> Result<Client> {
    let secs = match timeout_override.filter(|s| *s > 0.0) {
        Some(t) => t,
        None => {
            let extra = (file_size / (1024 * 1024)).saturating_mul(30);
            (300 + extra).min(2 * 3600) as f64
        }
    };
    Client::builder()
        .timeout(std::time::Duration::from_secs_f64(secs))
        .redirect(reqwest::redirect::Policy::default())
        .build()
        .context("无法创建 HTTP 客户端")
}

/// 截断响应体用于错误提示（按字符边界切，避免中文被切成非法 UTF-8）
pub(crate) fn truncate_body(body: &str, max_chars: usize) -> String {
    if body.chars().count() <= max_chars {
        body.to_string()
    } else {
        let head: String = body.chars().take(max_chars).collect();
        format!("{}…", head)
    }
}

/// 流式发送一个 PDF（multipart 的 `file` 字段），不把整份文件读进内存。
///
/// `extra_fields` 为附加的表单字段（如 `on_duplicate=replace`）。
pub(crate) async fn post_pdf(
    url: &str,
    file_path: &Path,
    token: &Option<String>,
    extra_fields: &[(&str, &str)],
    timeout_override: Option<f64>,
) -> Result<reqwest::Response> {
    let size = tokio::fs::metadata(file_path)
        .await
        .with_context(|| format!("无法读取文件信息: {}", file_path.display()))?
        .len();
    let file = tokio::fs::File::open(file_path)
        .await
        .with_context(|| format!("无法读取文件: {}", file_path.display()))?;
    let filename = file_path
        .file_name()
        .map(|f| f.to_string_lossy().to_string())
        .unwrap_or_else(|| "unknown.pdf".to_string());

    // 流式 multipart：必须带长度，否则 reqwest 会改成 chunked。
    // 下游 map 只做一次 `fetch_add`（零拷贝），进度由 `UploadProgress` 的后台任务采样绘制。
    let progress = TransferProgress::start(size);
    let counter = progress.counter();
    let part = reqwest::multipart::Part::stream_with_length(
        reqwest::Body::wrap_stream(
            tokio_util::io::ReaderStream::new(file).map(move |chunk| {
                if let Ok(bytes) = &chunk {
                    counter.fetch_add(bytes.len() as u64, Ordering::Relaxed);
                }
                chunk
            }),
        ),
        size,
    )
    .file_name(filename)
    .mime_str("application/pdf")?;

    let mut form = reqwest::multipart::Form::new().part("file", part);
    for (k, v) in extra_fields {
        form = form.text((*k).to_string(), (*v).to_string());
    }

    let client = upload_client(size, timeout_override)?;
    let mut req = client.post(url).multipart(form);
    if let Some(t) = token {
        req = req.header("X-Token", t.as_str());
    }
    // `send()` 要等响应头，期间字节已发完但服务端还在查重/抽元数据，进度行会显示「等待服务端处理」
    let result = req.send().await;
    progress.finish(result.is_ok());
    Ok(result?)
}

/// 上传结果（区分「新建成功」与「409 去重命中」，后者需要把服务端已有文档的信息报给用户）
pub(crate) enum UploadOutcome {
    /// 新建成功
    Created(UploadResult),
    /// 409 去重命中；`kind` = `hash`（文件内容已在库中）/ `doi`（DOI 已在库中）
    Duplicate {
        kind: String,
        existing_id: u64,
        existing_doi: Option<String>,
        existing_filename: Option<String>,
        same_file: bool,
        message: String,
    },
    /// 其它失败（原因已打印）
    Failed,
}

/// 上传 PDF 文件到服务器（新建文献，默认不触发自动处理）
///
/// `extract_meta` 为 false 时请求里带 `extract_meta=false`（服务端跳过所有抽取），
/// 为 true 时带 `extract_meta=full`（客户端批量上传默认跳过：那是上传后最慢的一步，且可事后补）。
///
/// 链路或服务端卡顿导致请求失败时会自动重试一次；若重试后服务端返回 409 且
/// 已有记录的 DOI 与我们上传的一致，说明「上一次请求其实已经落库」，按成功处理。
pub(crate) async fn api_upload(
    base_url: &str,
    file_path: &Path,
    doi: Option<&str>,
    token: &Option<String>,
    timeout_override: Option<f64>,
    extract_meta: bool,
) -> UploadOutcome {
    let url = format!("{}/assist/api/documents/upload", base_url);
    // 显式带 DOI：服务端不必解析 PDF 就能做 DOI 去重（命中时返回 409 `kind=doi`）
    let mut fields: Vec<(&str, &str)> = vec![(
        "extract_meta",
        if extract_meta { "full" } else { "false" },
    )];
    if let Some(doi) = doi {
        fields.push(("doi", doi));
    }

    let mut resp: Option<reqwest::Response> = None;
    for attempt in 0..2 {
        match post_pdf(&url, file_path, token, &fields, timeout_override).await {
            Ok(r) => {
                resp = Some(r);
                break;
            }
            Err(e) => {
                if attempt == 0 {
                    eprintln!("    上传失败（{e:#}），3 秒后重试一次 ...");
                    tokio::time::sleep(std::time::Duration::from_secs(3)).await;
                } else {
                    eprintln!("    上传失败: {e:#}");
                    eprintln!(
                        "    提示: 多为链路过慢/服务端卡顿导致的超时（该请求可能已在服务端完成）。"
                    );
                    eprintln!(
                        "    重跑同一条命令即可核对：服务端若已有该 DOI 的同内容记录，会按成功处理；也可加 --upload-timeout <秒数> 放宽超时。"
                    );
                    return UploadOutcome::Failed;
                }
            }
        }
    }
    let resp = match resp {
        Some(r) => r,
        None => return UploadOutcome::Failed,
    };

    let status = resp.status();
    if status == reqwest::StatusCode::OK {
        return match resp.json().await {
            Ok(r) => UploadOutcome::Created(r),
            Err(e) => {
                eprintln!("    上传响应解析失败: {}", e);
                UploadOutcome::Failed
            }
        };
    }

    let body = resp.text().await.unwrap_or_default();
    if status == reqwest::StatusCode::CONFLICT {
        // 服务端去重命中（kind=hash|doi）：把已有文档的 DOI/文件名带回来，方便判断是「本地挂错附件」还是「库里记录有误」
        let parsed: Option<serde_json::Value> = serde_json::from_str(&body).ok();
        let get_str = |ptr: &str| -> Option<String> {
            parsed
                .as_ref()
                .and_then(|v| v.pointer(ptr))
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
        };
        let existing_doi = get_str("/existing/doi");
        // 同一个 DOI 的同内容记录 = 上一次请求（可能超时后才）真的落库了 → 当作成功，避免白重跑
        let same_doi = match (doi, existing_doi.as_deref()) {
            (Some(a), Some(b)) => normalize_doi(a)
                .zip(normalize_doi(b))
                .map(|(x, y)| x.eq_ignore_ascii_case(&y))
                .unwrap_or(false),
            _ => false,
        };
        if same_doi {
            let id = parsed
                .as_ref()
                .and_then(|v| v.get("existing_id"))
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            println!(
                "    服务端已有同一 DOI 的同内容记录 id={}（上一次请求应该是在超时后才写入的），按成功处理",
                id
            );
            return UploadOutcome::Created(UploadResult {
                id,
                // 409 响应体没有 `status`，这里给一个客户端侧标记，表示「库里本来就有同内容记录」
                status: Some("existing".to_string()),
                doi: existing_doi,
                file_hash: get_str("/existing/file_hash"),
            });
        }
        return UploadOutcome::Duplicate {
            kind: get_str("/kind").unwrap_or_else(|| "unknown".to_string()),
            existing_id: parsed
                .as_ref()
                .and_then(|v| v.get("existing_id"))
                .and_then(|v| v.as_u64())
                .unwrap_or(0),
            existing_doi,
            existing_filename: get_str("/existing/filename"),
            same_file: parsed
                .as_ref()
                .and_then(|v| v.get("same_file"))
                .and_then(|v| v.as_bool())
                .unwrap_or(false),
            message: get_str("/message").unwrap_or_default(),
        };
    }

    eprintln!(
        "    上传失败: {} - {}",
        status,
        truncate_body(&body, 200)
    );
    UploadOutcome::Failed
}

/// 通过 DOI 查询服务器上已有的文献。
/// 返回 `(found: bool, doc: Option<Value>)` 元组。
pub(crate) async fn api_lookup_by_doi(
    client: &Client,
    base_url: &str,
    doi: &str,
    token: &Option<String>,
) -> (bool, Option<serde_json::Value>) {
    let url = format!("{}/assist/api/documents/by-doi/{}", base_url, urlencoding(doi));
    let mut req = client.get(&url);
    if let Some(t) = token {
        req = req.header("X-Token", t.as_str());
    }
    match req.send().await {
        Ok(resp) if resp.status() == reqwest::StatusCode::OK => {
            match resp.json::<serde_json::Value>().await {
                Ok(body) => {
                    let found = body.get("found").and_then(|v| v.as_bool()).unwrap_or(false);
                    if found {
                        let doc = body.get("doc").cloned();
                        (true, doc)
                    } else {
                        (false, None)
                    }
                }
                Err(e) => {
                    eprintln!("  by-doi 解析响应失败: {}", e);
                    (false, None)
                }
            }
        }
        Ok(resp) => {
            eprintln!("  by-doi 查询失败: {}", resp.status());
            (false, None)
        }
        Err(e) => {
            eprintln!("  by-doi 查询失败: {}", e);
            (false, None)
        }
    }
}

/// 为已有文档替换/上传 PDF 文件
pub(crate) async fn api_replace_pdf(
    base_url: &str,
    doc_id: u64,
    file_path: &Path,
    token: &Option<String>,
    timeout_override: Option<f64>,
) -> bool {
    let url = format!("{}/assist/api/documents/{}/pdf", base_url, doc_id);
    let resp = match post_pdf(&url, file_path, token, &[], timeout_override).await {
        Ok(r) => r,
        Err(e) => {
            eprintln!("  更新 PDF 失败: {e:#}");
            return false;
        }
    };

    let status = resp.status();
    if status == reqwest::StatusCode::OK {
        true
    } else {
        let body = resp.text().await.unwrap_or_default();
        eprintln!(
            "  更新 PDF 失败: {} - {}",
            status,
            truncate_body(&body, 200)
        );
        false
    }
}

/// URL 编码（用于 DOI 中的特殊字符）
pub(crate) fn urlencoding(s: &str) -> String {
    // 简单实现：只编码 / 和 ? 等必要字符
    let mut result = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '/' => result.push_str("%2F"),
            '?' => result.push_str("%3F"),
            '#' => result.push_str("%23"),
            '%' => result.push_str("%25"),
            ' ' => result.push_str("%20"),
            _ => result.push(c),
        }
    }
    result
}

/// 触发文档解析流水线
pub(crate) async fn api_trigger_pipeline(
    client: &Client,
    base_url: &str,
    doc_id: u64,
    token: &Option<String>,
) -> bool {
    let url = format!("{}/assist/api/pipeline/process/{}", base_url, doc_id);
    let mut req = client.post(&url).headers(auth_headers(token));
    if let Some(t) = token {
        req = req.header("X-Token", t.as_str());
    }
    match req.send().await {
        Ok(resp) if resp.status() == reqwest::StatusCode::OK => true,
        Ok(resp) => {
            eprintln!("  触发流水线失败: {}", resp.status());
            false
        }
        Err(e) => {
            eprintln!("  触发流水线失败: {}", e);
            false
        }
    }
}

/// 更新文档元数据
pub(crate) async fn api_update_metadata(
    client: &Client,
    base_url: &str,
    doc_id: u64,
    metadata: &DocMetadata,
    token: &Option<String>,
) -> bool {
    let url = format!("{}/assist/api/documents/{}", base_url, doc_id);
    match client
        .put(&url)
        .headers(auth_headers(token))
        .json(metadata)
        .send()
        .await
    {
        Ok(resp) if resp.status() == reqwest::StatusCode::OK => true,
        Ok(resp) => {
            eprintln!("  更新元数据失败: {}", resp.status());
            false
        }
        Err(e) => {
            eprintln!("  更新元数据失败: {}", e);
            false
        }
    }
}

/// 将完整的原始 CSV 行 POST 到服务器（持久化到 markdowns/{doc_id}.json）
pub(crate) async fn api_save_info(
    client: &Client,
    base_url: &str,
    doc_id: u64,
    info: &HashMap<String, String>,
    token: &Option<String>,
) -> bool {
    let url = format!("{}/assist/api/documents/{}/info", base_url, doc_id);
    let body = serde_json::json!({ "info": info });
    match client
        .post(&url)
        .headers(auth_headers(token))
        .json(&body)
        .send()
        .await
    {
        Ok(resp) if resp.status() == reqwest::StatusCode::OK => true,
        Ok(resp) => {
            eprintln!("  保存文档信息失败: {}", resp.status());
            false
        }
        Err(e) => {
            eprintln!("  保存文档信息失败: {}", e);
            false
        }
    }
}

/// 规范化 DOI：去掉 `https://doi.org/` / `doi:` 等前缀与尾部标点。
///
/// 与服务端 `services::pdf_meta::normalize_doi` 保持一致 —— 两边都归一化后，
/// `by-doi` 比对才能命中库里已存的 DOI（否则会误判为「新建」）。
pub(crate) fn normalize_doi(raw: &str) -> Option<String> {
    let s = raw.trim();
    if s.is_empty() {
        return None;
    }
    let mut s = s.to_string();
    for prefix in [
        "https://doi.org/",
        "http://doi.org/",
        "https://dx.doi.org/",
        "http://dx.doi.org/",
        "doi:",
    ] {
        if s.to_lowercase().starts_with(prefix) {
            s = s[prefix.len()..].trim().to_string();
            break;
        }
    }
    let s = s
        .trim_end_matches(|c: char| c == '.' || c == ',' || c == ';' || c == ')')
        .to_string();
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

#[cfg(test)]
mod hash_doi_tests {
    use super::*;

    #[test]
    fn reuse_by_normalized_doi() {
        let mut s = HashDoiStore { path: None, map: HashMap::new(), dirty: false };
        s.record("https://doi.org/10.1000/XYZ", "abc123");
        assert_eq!(s.get("10.1000/xyz"), Some("abc123"));
        assert_eq!(s.get("DOI:10.1000/xyz."), Some("abc123"));
        assert_eq!(s.get("10.1000/other"), None);
        // 空哈希 / 空 DOI 不记录
        s.record("10.1000/empty", "  ");
        s.record("", "deadbeef");
        assert_eq!(s.len(), 1);
        // 相同值不重复标记 dirty
        s.dirty = false;
        s.record("10.1000/xyz", "abc123");
        assert!(!s.dirty);
        // JSON 往返
        let body = serde_json::to_string_pretty(&s.map).unwrap();
        let back: HashMap<String, String> = serde_json::from_str(&body).unwrap();
        assert_eq!(back.get("10.1000/xyz").map(|v| v.as_str()), Some("abc123"));
    }

    #[test]
    fn flush_then_reload() {
        let dir = std::env::temp_dir().join(format!("okb-hashdoi-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("doi-hash.json");
        let _ = std::fs::remove_file(&path);
        let mut s = HashDoiStore { path: Some(path.clone()), map: HashMap::new(), dirty: false };
        s.record("10.2000/AAA", "cafebabe");
        s.flush();
        assert!(path.is_file());
        assert!(!path.with_extension("rs.tmp").exists());
        let text = std::fs::read_to_string(&path).unwrap();
        let m: HashMap<String, String> = serde_json::from_str(&text).unwrap();
        assert_eq!(m.get("10.2000/aaa").map(|v| v.as_str()), Some("cafebabe"));
        std::fs::remove_dir_all(&dir).ok();
    }
}
