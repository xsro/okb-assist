//! OKB-Assist 客户端工具
//!
//! 子命令:
//!   zotero <csv_file>   — 从 Zotero 导出的 CSV 导入文献
//!   dir    <dir>        — 扫描目录中的 PDF，与服务器比对后交互上传

use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::{self, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;

use anyhow::{bail, Context, Result};
use clap::Parser;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio_stream::StreamExt;
use walkdir::WalkDir;

// ==================== CLI 参数 ====================

#[derive(Parser, Debug)]
#[command(
    name = "okb-client",
    about = "OKB-Assist 客户端工具",
    version,
    subcommand_required = true,
    arg_required_else_help = true,
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Parser, Debug)]
enum Command {
    /// 从 Zotero 导出的 CSV 导入文献
    Zotero(ZoteroArgs),
    /// 扫描目录中的 PDF，与服务器比对后交互上传
    Dir(DirArgs),
    /// 按 ~/.okb/client.json 的 sync 配置同步网盘（个人网盘 + 公有文件夹，无命令行参数）
    Sync(SyncArgs),
}

/// `sync` 子命令：所有配置（本地目录、网盘目录、同步方向、令牌）均来自客户端配置文件
#[derive(Parser, Debug)]
struct SyncArgs {}

/// 上传节流参数（`zotero` / `dir` 两个子命令共用）
#[derive(clap::Args, Debug, Clone)]
struct UploadIntervalArgs {
    /// 每次上传之间的间隔秒数（默认 1 秒；可写小数，0 表示不等待）
    #[arg(long, value_name = "SECONDS", default_value_t = 1.0)]
    upload_interval: f64,

    /// 单次上传请求的超时秒数（0 = 自动：300s + 每 MB 30s，上限 2 小时）；
    /// 慢链路或服务端卡顿（表现为 `error sending request`）时调大
    #[arg(long, value_name = "SECONDS", default_value_t = 0.0)]
    upload_timeout: f64,

    /// 不显示上传进度（默认在传输时实时刷新「已发送 / 总大小、百分比、速度、剩余时间」）
    #[arg(long)]
    no_progress: bool,

    /// 上传时让服务端做 PDF 元数据启发式抽取（题名/作者/年份/DOI/摘要等）
    ///
    /// 默认**不开**：抽取会起几个 mutool 子进程把整份文档正文过一遍，
    /// 批量上传大文件时这是「等待服务端处理」的主要耗时。
    /// 不抽也能先把记录建好（列表里显示文件名），之后在文档管理页 / 
    /// `POST /assist/api/pipeline/extract-pdf-meta/:id` 补即可。
    #[arg(long)]
    extract_meta: bool,
}

impl UploadIntervalArgs {
    /// 传给 `upload_client` 的超时覆盖值（<= 0 表示自动）
    fn timeout_override(&self) -> Option<f64> {
        Some(self.upload_timeout).filter(|s| *s > 0.0)
    }

    /// 第 `seq` 次上传（从 0 开始）前的等待：首个不等待，间隔 <= 0 也不等待
    async fn throttle(&self, seq: usize) {        if seq > 0 && self.upload_interval > 0.0 {
            tokio::time::sleep(std::time::Duration::from_secs_f64(self.upload_interval)).await;
        }
    }

    /// 头部提示用的可读描述
    fn describe(&self) -> String {
        if self.upload_interval > 0.0 {
            format!("{} 秒", self.upload_interval)
        } else {
            "无间隔".to_string()
        }
    }
}

/// Zotero CSV 导入参数
#[derive(Parser, Debug)]
struct ZoteroArgs {
    /// Zotero 导出的 CSV 文件路径
    csv_file: String,

    /// OKB-Assist 服务地址（缺省取 `~/.okb/client.json` 的 `url`）
    #[arg(long)]
    base_url: Option<String>,

    /// 访问令牌（缺省取 `~/.okb/client.json` 的 `token`；也可用 OKB_ASSIST_TOKEN 环境变量）
    #[arg(long)]
    token: Option<String>,

    /// Zotero storage 文件夹的本地路径，用于把 Windows 附件路径映射到本地（缺省取 `~/.okb/client.json` 的 `storage_root`）
    #[arg(long)]
    storage_root: Option<String>,

    /// 试运行，不实际上传
    #[arg(long)]
    dry_run: bool,

    /// 上传后自动更新元数据（默认关闭，需显式指定）
    #[arg(long, default_value_t = false)]
    update_meta: bool,

    /// DOI 已存在时的处理方式：skip 直接跳过（默认）；hash 比对文件哈希，不同则覆盖已有 PDF；
    /// health-hash 先看已有 PDF 是否健康（`ok` 或从未检查过都算健康），健康则跳过，不健康才比对哈希（不同则覆盖）
    #[arg(long, value_enum, default_value_t = DupDoiAction::Skip)]
    dup_doi_action: DupDoiAction,

    /// 只处理这些 Zotero 条目（CSV 的 Key 列），可重复或用逗号分隔，大小写不敏感
    #[arg(long, value_delimiter = ',', value_name = "KEY")]
    keys: Vec<String>,

    /// 显示解析出的元数据详情
    #[arg(long)]
    debug: bool,

    /// 复用 `~/.okb/client/doi-hash.json` 里记录的 DOI→哈希，命中就不再重新读整份 PDF 算哈希
    /// （仅当本地文件自上次记录后没被替换/修改过时才可靠）
    #[arg(long, default_value_t = false)]
    reuse_hash: bool,

    #[command(flatten)]
    interval: UploadIntervalArgs,
}

/// DOI 重复时的处理策略（`--dup-doi-action`）
#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
enum DupDoiAction {
    /// 跳过（默认）：DOI 已存在就什么都不做
    Skip,
    /// 比对哈希：哈希不一致（或服务端没有 PDF）则覆盖已有文档的 PDF
    Hash,
    /// 健康优先：已有 PDF 健康（含从未检查过 / NULL）则跳过；只有明确不健康才比对哈希，不一致则覆盖
    HealthHash,
}

/// 目录扫描参数
#[derive(Parser, Debug)]
struct DirArgs {
    /// 要扫描的目录路径（包含 PDF 文件）
    dir: String,

    /// OKB-Assist 服务地址（缺省取 `~/.okb/client.json` 的 `url`）
    #[arg(long)]
    base_url: Option<String>,

    /// 访问令牌（缺省取 `~/.okb/client.json` 的 `token`）
    #[arg(long)]
    token: Option<String>,

    /// 试运行，不实际上传
    #[arg(long)]
    dry_run: bool,

    /// 递归搜索子目录
    #[arg(long, short)]
    recursive: bool,

    /// 上传后触发解析流水线（需要显式指定）
    #[arg(long)]
    process: bool,

    #[command(flatten)]
    interval: UploadIntervalArgs,
}

// ==================== 数据类型 ====================

/// Zotero CSV 行（动态列名，用 HashMap 接收）
type ZoteroRow = HashMap<String, String>;

/// 文档元数据（上传后 PUT 更新用，字段名与服务端一致）
#[derive(Debug, Clone, Serialize)]
struct DocMetadata {
    #[serde(skip_serializing_if = "Option::is_none")]
    title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    authors: Option<String>, // JSON array string
    #[serde(skip_serializing_if = "Option::is_none")]
    year: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    doi: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    journal: Option<String>,
    #[serde(rename = "abstract", skip_serializing_if = "Option::is_none")]
    abstract_text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    keywords: Option<String>, // JSON array string
    #[serde(skip_serializing_if = "Option::is_none")]
    doc_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    language: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    source: Option<String>,
}

/// diff-hashes 接口返回
///
/// `invalid` / `present_count` 目前没有消费方（`dir` 改成逐个比对后不再依赖汇总字段），
/// 但保留结构以便接口变更时有迹可循，故允许 dead_code。
#[allow(dead_code)]
#[derive(Debug, Deserialize)]
struct HashDiffResult {
    missing: Vec<String>,
    invalid: Vec<String>,
    #[serde(default)]
    present_count: Option<usize>,
}

/// doi-hash-index 接口返回的单条记录
#[derive(Debug, Clone, Deserialize)]
struct ServerEntry {
    #[serde(default)]
    id: u64,
    #[serde(default)]
    doi: Option<String>,
    #[serde(default)]
    doi_key: Option<String>,
    #[serde(default)]
    file_hash: Option<String>,
    #[serde(default)]
    pdf_health: Option<String>,
}

impl ServerEntry {
    /// 服务端文件哈希（空串 = 服务端没有 PDF）
    fn hash(&self) -> &str {
        self.file_hash.as_deref().unwrap_or("")
    }

    /// PDF 健康状态（空串 = 从未检查过）
    fn health(&self) -> &str {
        self.pdf_health.as_deref().unwrap_or("")
    }
}

/// doi-hash-index 接口返回
#[derive(Debug, Deserialize)]
struct ServerIndexResponse {
    #[serde(default)]
    count: Option<usize>,
    #[serde(default)]
    documents: Vec<ServerEntry>,
}

/// 服务端索引的本地视图：一次拉全量，后续比对全在本地完成。
struct ServerIndex {
    /// 服务端文献总数（含没有 DOI / 没有 PDF 的）
    count: usize,
    /// DOI 归一化键 → 记录
    by_doi: HashMap<String, ServerEntry>,
    /// 服务端已有文件的哈希（小写）
    hashes: HashSet<String>,
}

impl ServerIndex {
    fn from_response(resp: ServerIndexResponse) -> Self {
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
    fn lookup(&self, doi: &str) -> Option<&ServerEntry> {
        let key = normalize_doi(doi)?.to_lowercase();
        self.by_doi.get(&key)
    }

    fn has_hash(&self, hash: &str) -> bool {
        self.hashes.contains(&hash.to_lowercase())
    }
}

/// 上传响应（`doi` / `file_hash` 用于记进 DOI→哈希记录，服务端没返回就为空）
#[derive(Debug, Deserialize)]
struct UploadResult {
    id: u64,
    #[serde(default)]
    doi: Option<String>,
    #[serde(default)]
    file_hash: Option<String>,
}

/// 本地可上传文档（Zotero）
#[derive(Clone)]
struct LocalDoc {
    doi: String,
    file_path: String,
    /// 该条目全部可用的 PDF 附件（`file_path` 是从中挑出来的一个；用于 409 时提示用户换附件）
    attachments: Vec<String>,
    title: Option<String>,
    meta: DocMetadata,
    info: HashMap<String, String>,
}



// ==================== 客户端配置（~/.okb/client.json） ====================

/// 没有配置文件时使用的默认服务地址
const DEFAULT_BASE_URL: &str = "http://192.168.1.162:5001";

/// `~/.okb/client.json` 的内容。字段全部可选，缺什么就用默认/命令行值。
#[derive(Debug, Default, Deserialize)]
struct ClientConfig {
    /// 服务地址（`url` 与 `base_url` 两种写法都接受）
    #[serde(default, alias = "base_url")]
    url: Option<String>,
    /// 访问令牌
    #[serde(default)]
    token: Option<String>,
    /// Zotero storage 根目录（可被 `--storage-root` 覆盖）
    #[serde(default)]
    storage_root: Option<String>,
    /// 个人网盘同步配置（`sync` 子命令用）
    #[serde(default)]
    sync: Option<SyncConfig>,
}

/// `sync` 子命令的配置（`~/.okb/client.json` 的 `sync` 段）
///
/// 两种写法都支持（推荐用与 `sync` 一起的嵌套写法）：
/// ```json
/// "sync": { "personal": { "disk": "D:\\\\okb-disk", "remote": "sync", "direction": "two-way" } }
/// "sync": { "local": "D:\\\\okb-disk", "remote": "sync", "direction": "two-way" }
/// ```
#[derive(Debug, Default, Clone, Deserialize)]
struct SyncConfig {
    /// 本地目录（必填；等价于 `personal.disk`）
    #[serde(default)]
    local: Option<String>,
    /// 个人网盘里的子目录（缺省 = 根目录）
    #[serde(default)]
    remote: Option<String>,
    /// 同步方向：`two-way`（缺省）/ `upload` / `download`
    #[serde(default)]
    direction: Option<String>,
    /// 冲突策略：`newer`（缺省，取较新一方）/ `strict`（只报告，不改动文件）
    #[serde(default)]
    conflict: Option<String>,
    /// 个人网盘的嵌套写法（与平铺字段等价，嵌套优先）
    #[serde(default)]
    personal: Option<SyncPersonalConfig>,
    /// 公有文件夹同步（数组，可配多个；只读的文件夹只能单向下载）
    #[serde(default)]
    public: Vec<SyncPublicConfig>,
}

/// `sync.public[]`：某个公有文件夹的同步目标
///
/// ```json
/// "public": [
///   { "folder": "团队资料", "disk": "D:\okb-public\team", "direction": "download" }
/// ]
/// ```
#[derive(Debug, Default, Clone, Deserialize)]
struct SyncPublicConfig {
    /// 公有文件夹的名称或 id（`pf_xxx`）
    #[serde(default)]
    folder: Option<String>,
    /// 本地目录（必填）
    #[serde(default)]
    disk: Option<String>,
    /// 该文件夹内的子目录（缺省 = 文件夹根）
    #[serde(default)]
    remote: Option<String>,
    /// 同步方向：缺省 `download`（公有文件夹是共享的，默认只读镜像）；
    /// 显式写 `upload` / `two-way` 才会上传（且需要对该文件夹有写权限）
    #[serde(default)]
    direction: Option<String>,
    /// 冲突策略：`newer`（缺省）/ `strict`
    #[serde(default)]
    conflict: Option<String>,
}

/// `sync.personal`：个人网盘同步的目标
#[derive(Debug, Default, Clone, Deserialize)]
struct SyncPersonalConfig {
    /// 本地目录（对应个人网盘根目录）
    #[serde(default)]
    disk: Option<String>,
    /// 个人网盘里的子目录（缺省 = 根目录）
    #[serde(default)]
    remote: Option<String>,
    /// 同步方向：`two-way`（缺省）/ `upload` / `download`
    #[serde(default)]
    direction: Option<String>,
    /// 冲突策略：`newer`（缺省，取较新一方）/ `strict`（只报告，不改动文件）
    #[serde(default)]
    conflict: Option<String>,
}

impl SyncConfig {
    fn personal(&self) -> Option<&SyncPersonalConfig> {
        self.personal.as_ref()
    }

    fn local(&self) -> Option<&str> {
        self.local
            .as_deref()
            .or_else(|| self.personal().and_then(|p| p.disk.as_deref()))
    }

    fn remote(&self) -> Option<&str> {
        self.remote
            .as_deref()
            .or_else(|| self.personal().and_then(|p| p.remote.as_deref()))
    }

    fn direction(&self) -> Option<&str> {
        self.direction
            .as_deref()
            .or_else(|| self.personal().and_then(|p| p.direction.as_deref()))
    }

    fn conflict(&self) -> Option<&str> {
        self.conflict
            .as_deref()
            .or_else(|| self.personal().and_then(|p| p.conflict.as_deref()))
    }
}

impl ClientConfig {
    fn base_url(&self) -> String {
        self.url
            .clone()
            .filter(|u| !u.trim().is_empty())
            .unwrap_or_else(|| DEFAULT_BASE_URL.to_string())
    }
}

/// `~/.okb/client.json` 路径
fn config_path() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".okb").join("client.json"))
}

/// `~/.okb/client/docs.json` 路径（服务端 doi-hash 索引的本地缓存）
fn docs_cache_path() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".okb").join("client").join("docs.json"))
}

/// 读取配置；文件不存在直接返回默认值，解析失败只打警告（不阻断运行）
fn load_config() -> ClientConfig {
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
fn save_docs_cache(raw: &str) -> Option<PathBuf> {
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
fn hash_doi_path() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".okb").join("client").join("doi-hash.json"))
}

/// DOI → sha256 记录（文件内容是一个 JSON 对象，key 为归一化小写 DOI，value 为 sha256 hex）
///
/// 目的：整库 PDF 动辄几 GB，每次运行都把每份文件重读一遍算哈希很慢，
/// 记录一次后可用 `--reuse-hash` 直接复用。**文件被替换后记录会过期**，
/// 所以只有确定本地文件没换过时才该开 `--reuse-hash`。
struct HashDoiStore {
    path: Option<PathBuf>,
    map: HashMap<String, String>,
    dirty: bool,
}

impl HashDoiStore {
    /// 读取记录；文件不存在 = 空记录，解析失败只警告（不阻断运行）
    fn load() -> Self {
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
    fn key(doi: &str) -> Option<String> {
        normalize_doi(doi).map(|d| d.to_lowercase())
    }

    fn get(&self, doi: &str) -> Option<&str> {
        let key = Self::key(doi)?;
        self.map.get(&key).map(|s| s.as_str())
    }

    fn len(&self) -> usize {
        self.map.len()
    }

    /// 记录一条 DOI → 哈希（DOI 无效或哈希为空时忽略；已记录的相同值不重复写盘）
    fn record(&mut self, doi: &str, hash: &str) {
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
    fn flush(&mut self) {
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
fn read_token(arg_token: Option<&str>, config_token: Option<&str>) -> Option<String> {
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
fn http_client() -> Result<Client> {
    Client::builder()
        .timeout(std::time::Duration::from_secs(120))
        .redirect(reqwest::redirect::Policy::default())
        .build()
        .context("无法创建 HTTP 客户端")
}

fn auth_headers(token: &Option<String>) -> reqwest::header::HeaderMap {
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
fn sha256_file(path: &Path) -> Result<String> {
    let file = std::fs::File::open(path).context(format!("无法打开文件: {}", path.display()))?;
    let mut reader = std::io::BufReader::with_capacity(64 * 1024, file);
    let mut hasher = Sha256::new();
    std::io::copy(&mut reader, &mut hasher)
        .context(format!("无法读取文件: {}", path.display()))?;
    Ok(hex::encode(hasher.finalize()))
}

/// 在后台线程算哈希（`sha256_file` 是阻塞 IO，不要直接卡在 async 任务里）
async fn sha256_file_async(path: PathBuf) -> Result<String> {
    tokio::task::spawn_blocking(move || sha256_file(&path))
        .await
        .map_err(|e| anyhow::anyhow!("哈希任务崩溃: {}", e))?
}

// ==================== API 调用 ====================

/// 批量比对文件哈希，返回服务器缺失的哈希列表
async fn api_diff_hashes(
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
async fn api_fetch_server_index(
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
fn format_duration(d: std::time::Duration) -> String {
    let ms = d.as_millis();
    if ms < 1000 {
        format!("{} ms", ms)
    } else {
        format!("{:.1} s", d.as_secs_f64())
    }
}

/// 人性化文件大小（与上传进度里的单位一致）
fn format_bytes(bytes: u64) -> String {
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
fn display_width(s: &str) -> usize {
    s.chars()
        .map(|c| if (c as u32) >= 0x1100 { 2 } else { 1 })
        .sum()
}

/// 进度条宽度（字符数）
const PROGRESS_BAR_WIDTH: usize = 16;
/// 进度采样间隔
const PROGRESS_TICK: std::time::Duration = std::time::Duration::from_millis(200);

/// 是否显示上传进度（`--no-progress` 置为 false，在 `run_dir` / `run_zotero` 开头设置）
static PROGRESS_ENABLED: AtomicBool = AtomicBool::new(true);

fn percent_of(sent: u64, total: u64) -> u64 {
    if total == 0 {
        100
    } else {
        (sent.min(total) * 100 / total).min(100)
    }
}

fn speed_bps(sent: u64, elapsed: std::time::Duration) -> u64 {
    let secs = elapsed.as_secs_f64().max(0.001);
    (sent as f64 / secs) as u64
}

/// 进度行的箭头（上传 ⬆ / 下载 ⬇）
fn arrow_of(verb: &str) -> &'static str {
    if verb == "下载" {
        "⬇"
    } else {
        "⬆"
    }
}

/// 交互模式下的进度行（单行、原地刷新）
fn progress_line(verb: &str, sent: u64, total: u64, elapsed: std::time::Duration) -> String {
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
fn progress_line_plain(verb: &str, sent: u64, total: u64) -> String {
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
struct TransferProgress {
    /// 已从本地文件读出发送的字节数（上传递增）/ 已写入本地的字节数（下载递增）
    sent: Arc<AtomicU64>,
    /// 进度文案里的动作（“上传” / “下载”）
    verb: &'static str,
    total: u64,
    started: std::time::Instant,
    handle: Option<tokio::task::JoinHandle<()>>,
    enabled: bool,
    interactive: bool,
    /// 上一行已画出的显示宽度（用于擦除残留字符）
    last_width: Arc<AtomicUsize>,
    /// 进度是否已经独占了一行（首次输出前需要补换行）
    line_started: Arc<AtomicBool>,
}

impl TransferProgress {
    /// 上传进度（`⬆ 上传`）
    fn start(total: u64) -> Self {
        Self::start_with(total, "上传")
    }

    /// 下载进度（`⬇ 下载`）
    fn start_download(total: u64) -> Self {
        Self::start_with(total, "下载")
    }

    fn start_with(total: u64, verb: &'static str) -> Self {
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
    fn counter(&self) -> Arc<AtomicU64> {
        self.sent.clone()
    }

    /// 收尾：停掉采样任务，画一行最终结果（成功显示总量/用时/平均速度，失败显示已发送量）
    fn finish(mut self, ok: bool) {
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
fn upload_client(file_size: u64, timeout_override: Option<f64>) -> Result<Client> {
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
fn truncate_body(body: &str, max_chars: usize) -> String {
    if body.chars().count() <= max_chars {
        body.to_string()
    } else {
        let head: String = body.chars().take(max_chars).collect();
        format!("{}…", head)
    }
}

/// 流式发送一个 PDF（multipart 的 `file` 字段），不把整份文件读进内存。
///
/// `extra_fields` 为附加的表单字段（如 `auto_process=false`）。
async fn post_pdf(
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
enum UploadOutcome {
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
/// `extract_meta` 为 false 时请求里带 `extract_meta=false`，服务端跳过 PDF 元数据启发式抽取
/// （客户端批量上传默认跳过：那是上传后最慢的一步，且可事后补）。
///
/// 链路或服务端卡顿导致请求失败时会自动重试一次；若重试后服务端返回 409 且
/// 已有记录的 DOI 与我们上传的一致，说明「上一次请求其实已经落库」，按成功处理。
async fn api_upload(
    base_url: &str,
    file_path: &Path,
    doi: Option<&str>,
    token: &Option<String>,
    timeout_override: Option<f64>,
    extract_meta: bool,
) -> UploadOutcome {
    let url = format!("{}/assist/api/documents/upload", base_url);
    // 显式带 DOI：服务端不必解析 PDF 就能做 DOI 去重（命中时返回 409 `kind=doi`）
    let mut fields: Vec<(&str, &str)> = vec![
        ("auto_process", "false"),
        ("extract_meta", if extract_meta { "true" } else { "false" }),
    ];
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
async fn api_lookup_by_doi(
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
async fn api_replace_pdf(
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
fn urlencoding(s: &str) -> String {
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
async fn api_trigger_pipeline(
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
async fn api_update_metadata(
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
async fn api_save_info(
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

// ==================== 子命令: dir ====================

/// 扫描目录，收集 PDF 文件列表
fn scan_pdfs(dir: &Path, recursive: bool) -> Result<Vec<PathBuf>> {
    let mut pdfs = Vec::new();

    if !dir.is_dir() {
        bail!("目录不存在: {}", dir.display());
    }

    let walker = if recursive {
        WalkDir::new(dir)
            .follow_links(true)
            .into_iter()
            .filter_entry(|e| {
                // 跳过隐藏目录和常见非文献目录
                let name = e.file_name().to_string_lossy();
                if e.depth() > 0 && e.file_type().is_dir() {
                    return !name.starts_with('.')
                        && name != "__pycache__"
                        && name != "node_modules"
                        && name != ".git";
                }
                true
            })
            .collect::<Vec<_>>()
    } else {
        // 非递归: 只读 dir 下直接文件
        let entries = std::fs::read_dir(dir)
            .with_context(|| format!("无法读取目录: {}", dir.display()))?;
        for entry in entries {
            let entry = entry?;
            let path = entry.path();
            if path.is_file() {
                let ext = path
                    .extension()
                    .map(|e| e.to_string_lossy().to_lowercase())
                    .unwrap_or_default();
                if ext == "pdf" {
                    pdfs.push(path);
                }
            }
        }
        pdfs.sort_by_key(|p| p.file_name().map(|n| n.to_string_lossy().to_string()));
        return Ok(pdfs);
    };

    for entry in walker {
        let entry = entry?;
        if !entry.file_type().is_file() {
            continue;
        }
        let path = entry.path().to_path_buf();
        let ext = path
            .extension()
            .map(|e| e.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        if ext == "pdf" {
            pdfs.push(path);
        }
    }

    pdfs.sort_by_key(|p| p.file_name().map(|n| n.to_string_lossy().to_string()));
    Ok(pdfs)
}

async fn run_dir(args: &DirArgs) -> Result<()> {
    let cfg = load_config();
    let token = read_token(args.token.as_deref(), cfg.token.as_deref());
    let base_url = args.base_url.clone().unwrap_or_else(|| cfg.base_url());
    let process = args.process;
    // 上传进度开关（`--no-progress`）
    PROGRESS_ENABLED.store(!args.interval.no_progress, Ordering::Relaxed);
    // `dir` 不知道 DOI（要等上传后服务端返回），所以只做记录：上传成功后写回 `~/.okb/client/doi-hash.json`
    let mut hash_store = HashDoiStore::load();

    println!("扫描目录: {}", args.dir);
    println!("递归搜索: {}", if args.recursive { "是" } else { "否" });
    println!("上传间隔: {}", args.interval.describe());
    println!(
        "PDF 元数据抽取: {}",
        if args.interval.extract_meta {
            "开启"
        } else {
            "跳过（默认，--extract-meta 开启）"
        }
    );
    println!("服务地址: {}", base_url);
    println!("Token: {}", if token.is_some() { "********" } else { "未设置" });
    println!();

    // 1. 扫描 PDF（这里**不**计算哈希：哈希与比对放到上传前逐个做，避免先扫全库再上传）
    let all_pdfs = scan_pdfs(Path::new(&args.dir), args.recursive)?;
    let total = all_pdfs.len();
    if total == 0 {
        println!("目录中没有找到 PDF 文件。");
        return Ok(());
    }
    println!("找到 {} 个 PDF 文件。", total);
    println!();

    // 2. 拉一次服务端全量哈希索引，供上传前逐个比对（1 次请求；失败则逐文件回退 diff-hashes）
    let client = http_client()?;
    println!("正在获取服务端索引（全部文件哈希）...");
    let server_index = api_fetch_server_index(&client, &base_url, &token).await;
    match &server_index {
        Some(idx) => println!(
            "  服务端索引：{} 篇文献，{} 个可比对哈希（1 次请求）",
            idx.count,
            idx.hashes.len()
        ),
        None => println!("  ⚠ 无法获取服务端索引，回退为逐个文件 diff-hashes 比对（较慢）"),
    }
    println!();

    // 3. 列出本地文件（文件太多时只列前 200 个，避免刷屏）
    const LIST_LIMIT: usize = 200;
    for (i, p) in all_pdfs.iter().take(LIST_LIMIT).enumerate() {
        println!("  [{:>3}] {}", i + 1, p.display());
    }
    if all_pdfs.len() > LIST_LIMIT {
        println!("  …（其余 {} 个未列出）", all_pdfs.len() - LIST_LIMIT);
    }
    println!();
    println!("提示：上传前会逐个计算哈希并与服务器比对，服务器已有的文件自动跳过。");
    println!();

    // 4. 交互选择
    print!("请选择 [a] 全部上传  [s] 逐个选择  [q] 退出: ");
    io::stdout().flush().ok();
    let mut choice = String::new();
    io::stdin().read_line(&mut choice).ok();
    let choice = choice.trim().to_lowercase();

    if choice == "q" {
        println!("已退出。");
        return Ok(());
    }

    let selected: Vec<&PathBuf> = if choice == "a" {
        all_pdfs.iter().collect()
    } else if choice == "s" {
        let mut sel = Vec::new();
        for p in &all_pdfs {
            print!("上传 {}？[y/N]: ", p.display());
            io::stdout().flush().ok();
            let mut ans = String::new();
            io::stdin().read_line(&mut ans).ok();
            if ans.trim().to_lowercase() == "y" {
                sel.push(p);
            } else {
                println!("  跳过。");
            }
        }
        sel
    } else {
        println!("无效选择，退出。");
        return Ok(());
    };

    if selected.is_empty() {
        println!("未选择任何文件。");
        return Ok(());
    }

    // 5. 逐个文件：计算哈希 -> 与服务端比对 -> 决定是否上传
    println!();
    println!("开始处理 {} 个文件 ...", selected.len());
    let mut uploaded = 0usize;
    let mut skipped = 0usize;
    let mut hash_failed = 0usize;
    let mut failed = 0usize;
    // 已执行的上传次数（用于按 `--upload-interval` 节流，首个不等待）
    let mut upload_seq = 0usize;
    let loop_started = std::time::Instant::now();

    for (i, path) in selected.iter().enumerate() {
        // 哈希是阻塞 IO：丢到后台线程，不卡住 tokio worker
        print!(
            "  [{}/{}] 计算哈希: {} ... ",
            i + 1,
            selected.len(),
            path.display()
        );
        io::stdout().flush().ok();
        let item_started = std::time::Instant::now();
        let hash = match sha256_file_async((*path).clone()).await {
            Ok(h) => {
                println!(
                    "{} ({})",
                    &h[..16.min(h.len())],
                    format_duration(item_started.elapsed())
                );
                h
            }
            Err(e) => {
                println!("失败: {} ({})", e, format_duration(item_started.elapsed()));
                hash_failed += 1;
                continue;
            }
        };

        // 上传前比对：服务器已有相同哈希则跳过
        match server_has_hash(&client, &base_url, &token, &server_index, &hash).await {
            Some(true) => {
                println!("        -> 服务器已存在该文件，跳过");
                skipped += 1;
                continue;
            }
            Some(false) => {}
            None => {
                println!("        -> ⚠ 哈希比对请求失败，本文件未上传（可重试）");
                failed += 1;
                continue;
            }
        }

        if args.dry_run {
            println!("        -> [dry-run] 服务器没有该文件，跳过上传");
            continue;
        }

        // 两次上传之间按 `--upload-interval` 等待（首个不等待）
        args.interval.throttle(upload_seq).await;
        upload_seq += 1;
        // 行尾先换行，进度行独占一行（见 `UploadProgress`）
        println!("        上传 ...");
        match api_upload(
            &base_url,
            path,
            None,
            &token,
            args.interval.timeout_override(),
            args.interval.extract_meta,
        )
        .await
        {
            UploadOutcome::Created(result) => {
                println!("        成功 -> id={}", result.id);
                // 记下 DOI → 哈希（服务端算的与本地一致，优先用服务端返回值），供 `zotero --reuse-hash` 复用
                if let Some(doi) = result.doi.as_deref().filter(|s| !s.trim().is_empty()) {
                    let h = result.file_hash.as_deref().unwrap_or(hash.as_str());
                    hash_store.record(doi, h);
                    hash_store.flush();
                }
                uploaded += 1;
                // 触发解析流水线
                if process && api_trigger_pipeline(&client, &base_url, result.id, &token).await {
                    println!("        -> 已触发流水线");
                }
            }
            UploadOutcome::Duplicate {
                kind,
                existing_id,
                existing_doi,
                existing_filename,
                same_file,
                message,
            } => {
                println!("        被服务端去重拦下");
                println!(
                    "        已有文档: id={} | DOI={} | 文件名={}",
                    existing_id,
                    existing_doi.as_deref().unwrap_or("（空）"),
                    existing_filename.as_deref().unwrap_or("（未知）")
                );
                if kind == "hash" {
                    println!(
                        "        原因: 这份 PDF 的内容（sha256）已在库中{}，与本次要登记的新文献是两条不同记录",
                        if same_file { "" } else { "（另一条记录）" }
                    );
                    println!(
                        "        处理: 多为库里那条记录的 PDF 装错（或你本地这份文件本来就是另一篇）——先确认 id={} 的真实内容再决定重传 PDF 或删掉它",
                        existing_id
                    );
                } else {
                    println!("        原因: {}", if message.is_empty() { "DOI 已存在" } else { message.as_str() });
                    println!("        处理: 用 --dup-doi-action hash 可改走「比对哈希、不同则覆盖已有 PDF」的路子");
                }
                failed += 1;
            }
            UploadOutcome::Failed => {
                println!("        失败");
                failed += 1;
            }
        }
    }

    // 6. 最终统计
    println!();
    println!("==================================================");
    println!("本地 PDF:     {}", total);
    println!("已上传:       {}", uploaded);
    println!("服务器已存在: {}", skipped);
    if hash_failed > 0 {
        println!("哈希失败:     {}", hash_failed);
    }
    println!("失败:         {}", failed);
    println!("用时:         {}", format_duration(loop_started.elapsed()));
    println!("==================================================");

    Ok(())
}

/// 与服务器比对单个文件哈希：`Some(true)` 已存在 / `Some(false)` 缺失 / `None` 比对失败。
///
/// 优先用一次性拉到的全量索引在本地比对；索引不可用时回退为单条 `diff-hashes` 请求。
async fn server_has_hash(
    client: &Client,
    base_url: &str,
    token: &Option<String>,
    server_index: &Option<ServerIndex>,
    hash: &str,
) -> Option<bool> {
    match server_index {
        Some(idx) => Some(idx.has_hash(hash)),
        None => {
            let diff = api_diff_hashes(client, base_url, &[hash.to_string()], token).await?;
            Some(!diff.missing.iter().any(|h| h.eq_ignore_ascii_case(hash)))
        }
    }
}

// ==================== 子命令: zotero ====================
// 以下代码从旧版 main.rs 迁移而来，功能不变

/// Zotero CSV 列名 → 内部 key（小写匹配）
fn canonical_key(col: &str) -> String {
    let c = col.trim().to_lowercase();
    let mapped = match c.as_str() {
        "title" => "title",
        "author" => "author",
        "publication year" => "year",
        "date" => "date",
        "doi" => "doi",
        "journal" => "journal",
        "publication title" => "journal",
        "abstract note" => "abstract",
        "abstract" => "abstract",
        "tags" => "tags",
        "type" => "type",
        "item type" => "item_type",
        "publisher" => "publisher",
        "place" => "place",
        "volume" => "volume",
        "issue" => "issue",
        "pages" => "pages",
        "isbn" => "isbn",
        "issn" => "issn",
        "url" => "url",
        "language" => "language",
        "edition" => "edition",
        "series" => "series",
        "series number" => "series_number",
        "conference name" => "conference_name",
        "proceedings title" => "proceedings_title",
        "book title" => "book_title",
        "editor" => "editor",
        "translator" => "translator",
        "access date" => "access_date",
        "rights" => "rights",
        "file attachments" => "file",
        "file" => "file",
        "link attachments" => "link",
        "extra" => "extra",
        "call number" => "call_number",
        "archive" => "archive",
        "archive location" => "archive_location",
        "library catalog" => "library_catalog",
        "retrieved" => "retrieved",
        _ => c.as_str(),
    };
    mapped.to_string()
}

fn map_doc_type(zotero_type: &str) -> String {
    let t = zotero_type.trim().to_lowercase();
    let mapped = match t.as_str() {
        "journalarticle" | "journal article" | "article" => "journalArticle",
        "book" => "book",
        "booksection" | "book section" => "bookSection",
        "conferencepaper" | "conference paper" | "conference proceedings" | "proceedingsarticle"
        | "proceedings article" => "conferencePaper",
        "thesis" | "dissertation" => "thesis",
        "report" | "technicalreport" | "technical report" => "report",
        "webpage" => "webpage",
        "document" => "document",
        "presentation" => "presentation",
        "manuscript" => "manuscript",
        "patent" => "patent",
        "newspaperarticle" | "newspaper article" => "journalArticle",
        "magazinearticle" | "magazine article" => "journalArticle",
        "preprint" => "preprint",
        "review" => "review",
        _ => zotero_type,
    };
    mapped.to_string()
}

fn lang_map(l: &str) -> Option<&'static str> {
    match l {
        "en" | "eng" | "english" => Some("en"),
        "zh" | "chi" | "chinese" | "中文" => Some("zh"),
        "ja" | "jpn" | "japanese" | "日文" => Some("ja"),
        "fr" | "fre" | "french" => Some("fr"),
        "de" | "ger" | "german" => Some("de"),
        "ru" | "rus" | "russian" => Some("ru"),
        "ko" | "kor" | "korean" | "韩文" => Some("ko"),
        "es" | "spa" | "spanish" => Some("es"),
        "it" | "ita" | "italian" => Some("it"),
        "pt" | "por" | "portuguese" => Some("pt"),
        "ar" | "ara" | "arabic" => Some("ar"),
        _ => None,
    }
}

fn normalize_language(lang: &str) -> Option<String> {
    if lang.is_empty() {
        return None;
    }
    let l = lang.trim().to_lowercase();
    if let Some(m) = lang_map(&l) {
        return Some(m.to_string());
    }
    if l.chars().count() >= 2 {
        let prefix: String = l.chars().take(2).collect();
        if let Some(m) = lang_map(&prefix) {
            return Some(m.to_string());
        }
    }
    None
}

/// 去掉 Zotero 附加在路径后的 ":mime:size" 后缀
fn strip_zotero_suffix(p: &str) -> &str {
    if let Some(first) = p.find(':') {
        if let Some(second_rel) = p[first + 1..].find(':') {
            return &p[..first + 1 + second_rel];
        }
    }
    p
}

/// 将 Windows Zotero storage 路径映射到本地 storage_root
fn map_storage_path(path: &str, root: &str) -> Option<String> {
    let comps: Vec<&str> = path.split(['/', '\\']).collect();
    if comps.len() > 5 {
        let rel = comps[5..].join(std::path::MAIN_SEPARATOR_STR);
        let root = root.trim_end_matches(['/', '\\']);
        Some(format!("{}{}{}", root, std::path::MAIN_SEPARATOR, rel))
    } else {
        None
    }
}

/// 解析 Zotero file 字段，返回存在的 PDF 文件路径列表
fn parse_file_paths(file_field: &str, storage_root: &Option<String>) -> Vec<String> {
    let mut paths = Vec::new();
    for part in file_field.split(';') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        let stripped = strip_zotero_suffix(part);
        let mut candidates: Vec<String> = vec![part.to_string()];
        if stripped != part {
            candidates.push(stripped.to_string());
        }

        if let Some(root) = storage_root {
            if !Path::new(part).exists() {
                if let Some(mapped) = map_storage_path(part, root) {
                    candidates.push(mapped.clone());
                    let ms = strip_zotero_suffix(&mapped);
                    if ms != mapped {
                        candidates.push(ms.to_string());
                    }
                }
            }
        }

        for c in &candidates {
            if !c.is_empty()
                && Path::new(c).exists()
                && c.to_ascii_lowercase().ends_with("pdf")
            {
                paths.push(c.clone());
                break;
            }
        }
    }
    paths
}

/// 从附件候选里挑一个 PDF。
///
/// 只有 1 个直接用；**多个时不再只按 mtime 选**（Zotero 条目挂错/多挂附件时会把别的论文传上去），
/// 而是按「文件名与条目标题/作者的词重合度」打分，同分再取最新 mtime；并打印全部候选与得分。
fn pick_attachment(paths: &[String], title: &str, authors: &str, label: &str) -> Option<String> {
    let existing: Vec<String> = paths
        .iter()
        .filter(|p| Path::new(p).exists())
        .cloned()
        .collect();
    match existing.len() {
        0 => None,
        1 => Some(existing[0].clone()),
        _ => {
            let tokens = title_tokens(title, authors);
            let mut scored: Vec<(usize, std::time::SystemTime, String)> = existing
                .iter()
                .map(|p| {
                    let stem = Path::new(p)
                        .file_stem()
                        .map(|s| s.to_string_lossy().to_lowercase())
                        .unwrap_or_default();
                    let score = tokens.iter().filter(|t| stem.contains(t.as_str())).count();
                    let mtime = std::fs::metadata(p)
                        .and_then(|m| m.modified())
                        .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
                    (score, mtime, p.clone())
                })
                .collect();
            println!(
                "        ⚠ {} 有 {} 个 PDF 附件，按标题/作者匹配挑选（不再只看修改时间）:",
                label,
                existing.len()
            );
            for (score, _, p) in &scored {
                println!(
                    "            [匹配 {} 词] {}",
                    score,
                    Path::new(p)
                        .file_name()
                        .map(|n| n.to_string_lossy().to_string())
                        .unwrap_or_else(|| p.clone())
                );
            }
            // 匹配词多者优先；同分取 mtime 新的（保持旧行为）
            scored.sort_by(|a, b| b.0.cmp(&a.0).then(b.1.cmp(&a.1)));
            let (score, _, picked) = scored[0].clone();
            if score == 0 {
                println!("            ⚠ 标题/作者未匹配到任何候选，按最新修改时间选择，可能选错——请在 Zotero 里确认附件");
            }
            println!(
                "            -> 选用: {}",
                Path::new(&picked)
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_else(|| picked.clone())
            );
            Some(picked)
        }
    }
}

/// 从标题与作者字段提取用于匹配文件名的词（小写、长度 >= 4、去停用词）
fn title_tokens(title: &str, authors: &str) -> Vec<String> {
    const STOP: [&str; 12] = [
        "the", "and", "for", "with", "from", "based", "using", "this", "that", "into",
        "via", "under",
    ];
    let mut out: Vec<String> = Vec::new();
    let mut push = |raw: &str| {
        for w in raw
            .to_lowercase()
            .split(|c: char| !c.is_alphanumeric())
            .filter(|w| w.len() >= 4 && !STOP.contains(w))
        {
            if !out.iter().any(|x| x == w) {
                out.push(w.to_string());
            }
        }
    };
    push(title);
    // 作者字段形如 `Last, First; Last2, First2`：取逗号前的姓
    for a in authors.split(';') {
        if let Some(last) = a.split(',').next() {
            push(last);
        }
    }
    out
}

fn parse_authors(author_field: &str) -> Vec<String> {
    if author_field.is_empty() {
        return vec![];
    }
    let mut authors = Vec::new();
    for a in author_field.split(';') {
        let a = a.trim();
        if a.is_empty() {
            continue;
        }
        if let Some((last, first)) = a.split_once(',') {
            let last = last.trim();
            let first = first.trim();
            if !first.is_empty() && !last.is_empty() {
                authors.push(format!("{} {}", first, last));
            } else if !last.is_empty() {
                authors.push(last.to_string());
            }
        } else {
            authors.push(a.to_string());
        }
    }
    authors
}

fn extract_year_from_date(date: &str) -> Option<i32> {
    let mut run = String::new();
    for c in date.chars() {
        if c.is_ascii_digit() {
            run.push(c);
            if run.len() == 4 {
                return run.parse::<i32>().ok();
            }
        } else {
            run.clear();
        }
    }
    None
}

/// 规范化 DOI：去掉 `https://doi.org/` / `doi:` 等前缀与尾部标点。
///
/// 与服务端 `services::pdf_meta::normalize_doi` 保持一致 —— 两边都归一化后，
/// `by-doi` 比对才能命中库里已存的 DOI（否则会误判为「新建」）。
fn normalize_doi(raw: &str) -> Option<String> {
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

fn parse_zotero_row(row: &ZoteroRow) -> DocMetadata {
    let title = row.get("title").filter(|s| !s.is_empty()).cloned();

    let authors = row
        .get("author")
        .map(|s| parse_authors(s))
        .filter(|v| !v.is_empty())
        .map(|v| serde_json::to_string(&v).unwrap_or_default());

    let year = row
        .get("year")
        .and_then(|s| s.trim().parse::<i32>().ok())
        .or_else(|| row.get("date").and_then(|d| extract_year_from_date(d)));

    let doi = row.get("doi").and_then(|s| normalize_doi(s));

    let item_type = row
        .get("item_type")
        .map(|s| s.trim().to_lowercase())
        .filter(|s| !s.is_empty())
        .or_else(|| row.get("type").map(|s| s.trim().to_lowercase()))
        .unwrap_or_default();

    let mut journal = row.get("journal").filter(|s| !s.is_empty()).cloned();
    if journal.is_none() && item_type.contains("conference") {
        journal = row
            .get("proceedings_title")
            .filter(|s| !s.is_empty())
            .cloned()
            .or_else(|| row.get("conference_name").filter(|s| !s.is_empty()).cloned());
    }
    if journal.is_none() {
        journal = row.get("book_title").filter(|s| !s.is_empty()).cloned();
    }

    let abstract_text = row.get("abstract").filter(|s| !s.is_empty()).cloned();

    let keywords = row
        .get("tags")
        .map(|s| {
            s.split(';')
                .map(|t| t.trim().to_string())
                .filter(|t| !t.is_empty())
                .collect::<Vec<String>>()
        })
        .filter(|v| !v.is_empty())
        .map(|v| serde_json::to_string(&v).unwrap_or_default());

    let doc_type = if item_type.is_empty() {
        None
    } else {
        Some(map_doc_type(&item_type))
    };

    let language = row.get("language").and_then(|s| normalize_language(s));

    let mut source_parts: Vec<String> = Vec::new();
    if let Some(p) = row.get("publisher").filter(|s| !s.is_empty()) {
        source_parts.push(p.clone());
    }
    if let Some(p) = row.get("place").filter(|s| !s.is_empty()) {
        source_parts.push(p.clone());
    }
    if let Some(e) = row.get("edition").filter(|s| !s.is_empty()) {
        source_parts.push(format!("Edition: {}", e));
    }
    if let Some(s) = row.get("series").filter(|s| !s.is_empty()) {
        let mut si = s.clone();
        if let Some(sn) = row.get("series_number").filter(|x| !x.is_empty()) {
            si.push_str(&format!(" #{}", sn));
        }
        source_parts.push(format!("Series: {}", si));
    }
    let mut source = if source_parts.is_empty() {
        None
    } else {
        Some(source_parts.join("; "))
    };

    let mut vip: Vec<String> = Vec::new();
    if let Some(v) = row.get("volume").filter(|s| !s.is_empty()) {
        vip.push(format!("Vol.{}", v));
    }
    if let Some(v) = row.get("issue").filter(|s| !s.is_empty()) {
        vip.push(format!("No.{}", v));
    }
    if let Some(v) = row.get("pages").filter(|s| !s.is_empty()) {
        vip.push(format!("pp.{}", v));
    }
    let journal = if vip.is_empty() {
        journal
    } else {
        let suffix = vip.join(", ");
        match &journal {
            Some(j) => Some(format!("{}, {}", j, suffix)),
            None => Some(suffix),
        }
    };

    let mut id_parts: Vec<String> = Vec::new();
    if let Some(v) = row.get("isbn").filter(|s| !s.is_empty()) {
        id_parts.push(format!("ISBN:{}", v));
    }
    if let Some(v) = row.get("issn").filter(|s| !s.is_empty()) {
        id_parts.push(format!("ISSN:{}", v));
    }
    if !id_parts.is_empty() {
        let s = id_parts.join(", ");
        source = Some(match source {
            Some(existing) => format!("{} ({})", existing, s),
            None => s,
        });
    }

    if let Some(url) = row.get("url").filter(|s| !s.is_empty()) {
        if doi.is_none() {
            source = Some(match source {
                Some(existing) => format!("{} URL:{}", existing, url),
                None => format!("URL:{}", url),
            });
        }
    }

    let mut contrib: Vec<String> = Vec::new();
    if let Some(ed) = row.get("editor").filter(|s| !s.is_empty()) {
        let eds: Vec<&str> = ed
            .split(';')
            .map(|e| e.trim())
            .filter(|e| !e.is_empty())
            .collect();
        if !eds.is_empty() {
            contrib.push(format!("Editors: {}", eds.join(", ")));
        }
    }
    if let Some(tr) = row.get("translator").filter(|s| !s.is_empty()) {
        contrib.push(format!("Translator: {}", tr));
    }
    if !contrib.is_empty() {
        let c = contrib.join("; ");
        source = Some(match source {
            Some(existing) => format!("{}; {}", existing, c),
            None => c,
        });
    }

    DocMetadata {
        title,
        authors,
        year,
        doi,
        journal,
        abstract_text,
        keywords,
        doc_type,
        language,
        source,
    }
}



/// 哈希前 12 位（便于日志里辨认，不含多字节字符问题）
fn preview_hash(h: &str) -> String {
    h.chars().take(12).collect()
}

/// 上传前按需计算本地 PDF 的 sha256（打印进度与耗时）；返回 `None` 表示读取失败。
///
/// `reuse` 为真时先查 DOI→哈希记录（`~/.okb/client/doi-hash.json`），命中就完全不读文件；
/// 真正算出哈希后会写回记录，方便下次 `--reuse-hash` 复用。
async fn compute_local_hash(
    d: &LocalDoc,
    reuse: bool,
    store: &mut HashDoiStore,
) -> Option<String> {
    let file_name = Path::new(&d.file_path)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| d.file_path.clone());
    if reuse {
        if let Some(h) = store.get(&d.doi) {
            println!(
                "    复用已记录哈希: {} | {} -> {}",
                d.doi,
                file_name,
                preview_hash(h)
            );
            return Some(h.to_string());
        }
    }
    print!("    计算哈希: {} | {} ... ", d.doi, file_name);
    io::stdout().flush().ok();
    let started = std::time::Instant::now();
    match sha256_file_async(PathBuf::from(&d.file_path)).await {
        Ok(h) => {
            println!(
                "{} ({})",
                &h[..16.min(h.len())],
                format_duration(started.elapsed())
            );
            store.record(&d.doi, &h);
            store.flush();
            Some(h)
        }
        Err(e) => {
            println!("失败 ({}): {}", format_duration(started.elapsed()), e);
            None
        }
    }
}

/// 把「409 去重命中」的动机讲清楚（旧版只说 id + 一个不相关的 `--dup-doi-action` 提示）
fn describe_duplicate(
    d: &LocalDoc,
    kind: &str,
    existing_id: u64,
    existing_doi: Option<&str>,
    existing_filename: Option<&str>,
    same_file: bool,
    message: &str,
) {
    let _ = same_file;
    println!(
        "    已有文档: id={} | DOI={} | 文件名={}",
        existing_id,
        existing_doi.unwrap_or("（空）"),
        existing_filename.unwrap_or("（未知）")
    );
    println!(
        "    本条记录: DOI={} | 文件={}",
        d.doi,
        Path::new(&d.file_path)
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| d.file_path.clone())
    );
    if kind == "hash" {
        println!(
            "    含义: 本地这份 PDF 的 sha256 与库中 id={} 的文件完全相同（内容一样，只是登记成了另一篇）。",
            existing_id
        );
        println!("    常见原因:");
        println!("      a) Zotero 条目挂错了附件（同一篇的 PDF 被挂到了别的条目上）；");
        println!("      b) 库里 id={} 这条记录的 PDF 装错了（DOI/元数据是 A，PDF 是 B）。", existing_id);
        if d.attachments.len() > 1 {
            println!("    该条目共有 {} 个 PDF 附件:", d.attachments.len());
            for (i, p) in d.attachments.iter().enumerate() {
                println!(
                    "      [{}]{}",
                    i + 1,
                    if *p == d.file_path { "（本次选用）" } else { "" }
                );
                println!("          {}", p);
            }
            println!("    处理: 在 Zotero 里删掉错的那个附件，或在 CSV 的「File Attachments」列只保留对的路径后重跑。");
        } else {
            println!("    处理: 先确认 id={} 那条记录的真实内容（详情页可下载 PDF），再决定是替换它还是换本地文件。", existing_id);
        }
    } else {
        println!(
            "    含义: {}",
            if message.is_empty() {
                "该 DOI 在库中已存在"
            } else {
                message
            }
        );
        println!("    处理: 加 --dup-doi-action hash 可改为「比对哈希，不同则覆盖已有 PDF」。");
    }
}

async fn upload_one_zotero(
    d: &LocalDoc,
    base_url: &str,
    update_meta: bool,
    dry_run: bool,
    client: &Client,
    token: &Option<String>,
    timeout_override: Option<f64>,
    extract_meta: bool,
) -> bool {
    if dry_run {
        println!("  [dry-run] 跳过上传: {} ({})", d.doi, d.file_path);
        return true;
    }
    let res = match api_upload(
        base_url,
        Path::new(&d.file_path),
        Some(d.doi.as_str()),
        token,
        timeout_override,
        extract_meta,
    )
    .await
    {
        UploadOutcome::Created(r) => r,
        UploadOutcome::Duplicate {
            kind,
            existing_id,
            existing_doi,
            existing_filename,
            same_file,
            message,
        } => {
            println!("被服务端去重拦下");
            describe_duplicate(
                d,
                &kind,
                existing_id,
                existing_doi.as_deref(),
                existing_filename.as_deref(),
                same_file,
                &message,
            );
            return false;
        }
        UploadOutcome::Failed => {
            println!("上传失败 {}", d.doi);
            return false;
        }
    };
    let doc_id = res.id;
    if update_meta {
        api_update_metadata(client, base_url, doc_id, &d.meta, token).await;
        api_save_info(client, base_url, doc_id, &d.info, token).await;
        println!("  已保存信息到 json: id={}", doc_id);
    }
    println!("  已上传: {} -> id={}", d.doi, doc_id);
    true
}

async fn run_zotero(args: &ZoteroArgs) -> Result<()> {
    let cfg = load_config();
    let token = read_token(args.token.as_deref(), cfg.token.as_deref());
    let base_url = args.base_url.clone().unwrap_or_else(|| cfg.base_url());
    let storage_root = args
        .storage_root
        .clone()
        .filter(|s| !s.trim().is_empty())
        .or_else(|| cfg.storage_root.clone());
    let update_meta = args.update_meta;
    // 上传进度开关（`--no-progress`）
    PROGRESS_ENABLED.store(!args.interval.no_progress, Ordering::Relaxed);

    println!("服务地址: {}", base_url);
    println!("CSV 文件: {}", args.csv_file);
    println!("Storage root: {}", storage_root.clone().unwrap_or_default());
    println!("Token: {}", if token.is_some() { "********" } else { "未设置" });
    println!("更新元数据: {}", if update_meta { "是" } else { "否 (默认)" });
    println!("上传间隔: {}", args.interval.describe());
    println!(
        "PDF 元数据抽取: {}",
        if args.interval.extract_meta {
            "开启"
        } else {
            "跳过（默认，--extract-meta 开启）"
        }
    );
    if !args.keys.is_empty() {
        println!("只处理 key: {}", args.keys.join(", "));
    }
    println!(
        "DOI 重复处理: {}",
        match args.dup_doi_action {
            DupDoiAction::Skip => "skip（跳过，默认）",
            DupDoiAction::Hash => "hash（比对哈希，不同则覆盖）",
            DupDoiAction::HealthHash => "health-hash（健康则跳过，否则比对哈希）",
        }
    );
    // DOI → 哈希记录：默认只写；开 `--reuse-hash` 时优先复用，省掉重新读整份 PDF
    let mut hash_store = HashDoiStore::load();
    println!(
        "哈希记录: {} 条（{}）",
        hash_store.len(),
        if args.reuse_hash {
            "复用开启，命中则跳过重新计算"
        } else {
            "仅记录，不复用"
        }
    );
    println!();

    let csv_path = Path::new(&args.csv_file);
    if !csv_path.is_file() {
        bail!("错误: CSV 文件不存在: {}", args.csv_file);
    }

    let mut rdr = csv::ReaderBuilder::new()
        .has_headers(true)
        .from_path(csv_path)
        .with_context(|| format!("无法打开 CSV 文件: {}", args.csv_file))?;

    let headers_record = rdr.headers()?.clone();
    let headers: Vec<String> = headers_record
        .iter()
        .map(|h| h.strip_prefix('\u{feff}').unwrap_or(h).to_string())
        .collect();
    let canon: Vec<String> = headers.iter().map(|h| canonical_key(h)).collect();

    // `--keys`：只处理指定 Zotero 条目（CSV 的 Key 列，大小写不敏感）
    let key_filter: HashSet<String> = args
        .keys
        .iter()
        .map(|k| k.trim().to_lowercase())
        .filter(|k| !k.is_empty())
        .collect();
    if !key_filter.is_empty() && !canon.iter().any(|c| c == "key") {
        bail!(
            "--keys 需要 CSV 中包含 Key 列（当前表头: {}）",
            headers.join(", ")
        );
    }
    // 命中过滤的 key（小写），用于事后报告「指定的 key 没在 CSV 里找到」
    let mut matched_keys: HashSet<String> = HashSet::new();
    // 命中过滤但被跳过的行（缺 DOI / 缺附件），带原因，方便排查
    let mut filtered_out: Vec<(String, String)> = Vec::new();

    let mut local_docs: Vec<LocalDoc> = Vec::new();
    for record in rdr.records() {
        let record = record?;
        let mut raw: ZoteroRow = ZoteroRow::new();
        let mut norm: ZoteroRow = ZoteroRow::new();
        for (i, h) in headers.iter().enumerate() {
            let v = record.get(i).unwrap_or("").trim().to_string();
            raw.insert(h.clone(), v.clone());
            norm.insert(canon[i].clone(), v);
        }

        // `--keys` 过滤：不匹配的行直接跳过（先于 DOI/附件判定，保证「命中的 key」报告准确）
        let item_key = norm
            .get("key")
            .map(|s| s.trim().to_lowercase())
            .unwrap_or_default();
        if !key_filter.is_empty() {
            if item_key.is_empty() || !key_filter.contains(&item_key) {
                continue;
            }
            matched_keys.insert(item_key.clone());
        }

        // DOI 归一化后再比对：去前缀/尾部标点，否则服务端的 by-doi 精确匹配会漏报
        let doi = match normalize_doi(norm.get("doi").map(|s| s.as_str()).unwrap_or("")) {
            Some(d) => d,
            None => {
                if !key_filter.is_empty() {
                    filtered_out.push((item_key.clone(), "缺少 DOI".to_string()));
                }
                continue;
            }
        };
        let file_field = norm.get("file").cloned().unwrap_or_default();
        let paths = parse_file_paths(&file_field, &storage_root);
        let row_title = norm.get("title").cloned().unwrap_or_default();
        let row_authors = norm.get("author").cloned().unwrap_or_default();
        let label = format!("{} ({})", item_key, doi);
        let file_path = match pick_attachment(&paths, &row_title, &row_authors, &label) {
            Some(f) => f,
            None => {
                if !key_filter.is_empty() {
                    filtered_out.push((item_key.clone(), "没有可用的 PDF 附件".to_string()));
                }
                continue;
            }
        };
        let available: Vec<String> = paths
            .iter()
            .filter(|p| Path::new(p.as_str()).exists())
            .cloned()
            .collect();
        let meta = parse_zotero_row(&norm);
        let info: ZoteroRow = raw
            .iter()
            .filter(|(_, v)| !v.trim().is_empty())
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        local_docs.push(LocalDoc {
            doi,
            file_path,
            attachments: available,
            title: meta.title.clone(),
            meta,
            info,
        });
    }

    // `--keys` 的事后报告：CSV 里没有的 key、命中但因故被跳过的行
    if !key_filter.is_empty() {
        let mut missing_keys: Vec<&String> = key_filter
            .iter()
            .filter(|k| !matched_keys.contains(*k))
            .collect();
        missing_keys.sort();
        if !missing_keys.is_empty() {
            println!();
            println!(
                "⚠ CSV 中没有这些 key（共 {} 个）: {}",
                missing_keys.len(),
                missing_keys
                    .iter()
                    .map(|s| s.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            );
        }
        if !filtered_out.is_empty() {
            println!();
            println!("⚠ 命中但被跳过（共 {} 条）:", filtered_out.len());
            for (k, reason) in &filtered_out {
                println!("  {} — {}", k, reason);
            }
        }
        println!();
        println!("指定 key 命中 {} 个，其中可上传 {} 个", matched_keys.len(), local_docs.len());
    }

    if local_docs.is_empty() {
        println!("没有本地可上传的文件（或全部缺少 DOI）。");
        return Ok(());
    }

    if args.debug {
        for d in &local_docs {
            let j = serde_json::to_string_pretty(&d.meta).unwrap_or_default();
            println!(
                "  [debug] {} | {}:\n{}",
                d.doi,
                d.title.clone().unwrap_or_default(),
                j
            );
        }
    }

    let client = http_client()?;

    // =========================================================
    // 第一阶段：与服务端比对，收集比对结果
    // =========================================================
    println!("第一阶段：与服务端比对 {} 个文献的 DOI ...", local_docs.len());
    println!();

    // 一次拉取服务端全量 DOI / 哈希索引，整库比对在本地完成（不再逐条 by-doi）
    println!("正在获取服务端索引（全部文献的 DOI / 哈希）...");
    let server_index = api_fetch_server_index(&client, &base_url, &token).await;
    match &server_index {
        Some(idx) => println!(
            "服务端索引：{} 篇文献，{} 个可比对 DOI（1 次请求）",
            idx.count,
            idx.by_doi.len()
        ),
        None => println!("⚠ 无法获取服务端索引，回退为逐个 DOI 查询（较慢）"),
    }
    println!();

    let mut new_entries: Vec<&LocalDoc> = Vec::new();
    // (服务端 id, 本地条目, 服务端哈希, 本地哈希)
    let mut to_update_pdf: Vec<(u64, &LocalDoc, String, Option<String>)> = Vec::new();
    // 跳过项连原因一起记，第二阶段直接列出来，避免用户疑惑「为什么没上传」
    let mut to_skip: Vec<(&LocalDoc, String)> = Vec::new();

    for d in &local_docs {
        // 优先查本地索引；索引不可用时回退到 by-doi 单条查询
        let server: Option<ServerEntry> = match &server_index {
            Some(idx) => idx.lookup(&d.doi).cloned(),
            None => match api_lookup_by_doi(&client, &base_url, &d.doi, &token).await {
                (true, Some(doc)) => Some(ServerEntry {
                    id: doc.get("id").and_then(|v| v.as_u64()).unwrap_or(0),
                    doi: doc.get("doi").and_then(|v| v.as_str()).map(|s| s.to_string()),
                    doi_key: None,
                    file_hash: doc
                        .get("file_hash")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string()),
                    pdf_health: doc
                        .get("pdf_health")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string()),
                }),
                // 查询失败或 found=true 但无 doc：安全起见视为新建
                _ => None,
            },
        };

        let Some(server) = server else {
            // 服务器没有这个 DOI：必然新建，不需要本地哈希
            new_entries.push(d);
            continue;
        };

        let server_id = server.id;
        // 服务端没有 PDF（哈希为空）时没法比，当成「需要补 PDF」
        let server_hash = server.hash().to_string();
        let hash_missing = server_hash.is_empty();

        // `pdf_health`：ok / warning / error / missing；空串（从未检查过）= 未知。
        // 未知状态按「健康」处理 —— 没证据说明它坏了就不折腾，只有明确的 warning/error/missing 才走哈希比对。
        let health = server.health();
        let health_unknown = health.is_empty();
        let healthy = health_unknown || health == "ok";

        // 本地哈希按需计算：只有「服务器已有 PDF」且策略确实要比哈希时才读文件，
        // 默认的 `skip` 策略下完全不读取本地文件（旧版会先把整库哈希算完，几 GB 的库要等很久）。
        let needs_hash = !hash_missing
            && match args.dup_doi_action {
                DupDoiAction::Skip => false,
                DupDoiAction::Hash => true,
                DupDoiAction::HealthHash => !healthy,
            };

        let local_hash = if needs_hash {
            match compute_local_hash(d, args.reuse_hash, &mut hash_store).await {
                Some(h) => Some(h),
                None => {
                    to_skip.push((d, "本地文件哈希计算失败".to_string()));
                    continue;
                }
            }
        } else {
            None
        };

        // 哈希一致 = 服务器与本地都有 PDF 且哈希相同
        let hash_same = matches!(&local_hash, Some(h) if *h == server_hash);
        let need_replace = hash_missing || !hash_same;

        match args.dup_doi_action {
            DupDoiAction::Skip => {
                to_skip.push((d, "DOI 已存在（--dup-doi-action=skip）".to_string()));
            }
            DupDoiAction::Hash => {
                if need_replace {
                    to_update_pdf.push((server_id, d, server_hash, local_hash));
                } else {
                    to_skip.push((d, "hash 一致".to_string()));
                }
            }
            DupDoiAction::HealthHash => {
                if healthy {
                    to_skip.push((
                        d,
                        if health_unknown {
                            "PDF 未做过健康检查，按健康处理，跳过".to_string()
                        } else {
                            "PDF 健康，跳过".to_string()
                        },
                    ));
                } else if need_replace {
                    to_update_pdf.push((server_id, d, server_hash, local_hash));
                } else {
                    to_skip.push((d, format!("PDF 不健康（{}），hash 一致", health)));
                }
            }
        }
    }

    // =========================================================
    // 第二阶段：展示汇总，让用户统一决策
    // =========================================================
    println!("──────────────────────────────────────────────────────");
    println!("比对完成");
    println!("  新建文献:      {}", new_entries.len());
    println!("  更新 PDF:      {}", to_update_pdf.len());
    println!("  跳过:          {}", to_skip.len());
    println!();

    // --- 新建文献 ---
    let mut new_choices: Vec<bool> = Vec::new();
    if !new_entries.is_empty() {
        println!("── 新建文献 ──");
        for (i, dw) in new_entries.iter().enumerate() {
            println!("  [{:>3}] {} | {}", i + 1, dw.doi, dw.title.clone().unwrap_or_default());
        }
        print!("\n新建策略 [a] 全部新建  [s] 逐个选择  [n] 全部跳过: ");
        io::stdout().flush().ok();
        let mut choice = String::new();
        io::stdin().read_line(&mut choice).ok();
        match choice.trim().to_lowercase().as_str() {
            "a" => new_choices = vec![true; new_entries.len()],
            "n" => new_choices = vec![false; new_entries.len()],
            "s" => {
                for (i, dw) in new_entries.iter().enumerate() {
                    print!("  新建 [{}] {} ？[y/N]: ", i + 1, dw.doi);
                    io::stdout().flush().ok();
                    let mut ans = String::new();
                    io::stdin().read_line(&mut ans).ok();
                    new_choices.push(ans.trim().to_lowercase() == "y");
                }
            }
            _ => new_choices = vec![false; new_entries.len()],
        }
        println!();
    }

    // --- 跳过（DOI 已存在）---
    if !to_skip.is_empty() {
        println!("── 跳过（DOI 已存在）──");
        for (i, (dw, reason)) in to_skip.iter().enumerate() {
            println!(
                "  [{:>3}] {} | {} — {}",
                i + 1,
                dw.doi,
                dw.title.clone().unwrap_or_default(),
                reason
            );
        }
        println!();
    }

    // --- 更新 PDF ---
    let mut update_choices: Vec<bool> = Vec::new();
    if !to_update_pdf.is_empty() {
        println!("── 更新 PDF ──");
        for (i, (server_id, dw, server_hash, local_hash)) in to_update_pdf.iter().enumerate() {
            if server_hash.is_empty() {
                println!("  [{:>3}] ID={} | {} | {}", i + 1, server_id, dw.doi, dw.title.clone().unwrap_or_default());
                println!("        文献存在但无 PDF 文件");
            } else {
                println!("  [{:>3}] ID={} | {} | {}", i + 1, server_id, dw.doi, dw.title.clone().unwrap_or_default());
                println!("        PDF 不同（服务器: {}..., 本地: {}...）",
                    preview_hash(server_hash),
                    preview_hash(local_hash.as_deref().unwrap_or("")));
            }
        }
        print!("\n更新策略 [a] 全部更新  [s] 逐个选择  [n] 全部跳过: ");
        io::stdout().flush().ok();
        let mut choice = String::new();
        io::stdin().read_line(&mut choice).ok();
        match choice.trim().to_lowercase().as_str() {
            "a" => update_choices = vec![true; to_update_pdf.len()],
            "n" => update_choices = vec![false; to_update_pdf.len()],
            "s" => {
                for (i, (server_id, dw, _, _)) in to_update_pdf.iter().enumerate() {
                    print!("  更新 PDF [{}] ID={} ({}) ？[y/N]: ", i + 1, server_id, dw.doi);
                    io::stdout().flush().ok();
                    let mut ans = String::new();
                    io::stdin().read_line(&mut ans).ok();
                    update_choices.push(ans.trim().to_lowercase() == "y");
                }
            }
            _ => update_choices = vec![false; to_update_pdf.len()],
        }
        println!();
    }

    // --- 确认 ---
    let new_count = new_choices.iter().filter(|&&c| c).count();
    let update_count = update_choices.iter().filter(|&&c| c).count();
    let skip_count = to_skip.len()
        + new_choices.iter().filter(|&&c| !c).count()
        + update_choices.iter().filter(|&&c| !c).count();

    if new_count == 0 && update_count == 0 {
        println!("没有需要执行的操作。");
        return Ok(());
    }

    println!("即将执行:");
    println!("  新建文献:  {}", new_count);
    println!("  更新 PDF:  {}", update_count);
    println!("  跳过:      {}", skip_count);
    print!("\n确认执行？[Y/n]: ");
    io::stdout().flush().ok();
    let mut confirm = String::new();
    io::stdin().read_line(&mut confirm).ok();
    if confirm.trim().to_lowercase() == "n" {
        println!("已取消。");
        return Ok(());
    }
    println!();

    // =========================================================
    // 第三阶段：批量执行
    // =========================================================
    println!("第三阶段：开始执行 ...");
    println!();

    let mut uploaded = 0usize;
    let mut updated_pdf = 0usize;
    let mut failed = 0usize;
    // 已执行的上传/替换次数（用于按 `--upload-interval` 节流，首个不等待）
    let mut upload_seq = 0usize;

    // 执行新建
    for (i, dw) in new_entries.iter().enumerate() {
        if !new_choices[i] {
            continue;
        }
        if args.dry_run {
            print!("  新建 [{}] {} ... ", i + 1, dw.doi);
            io::stdout().flush().ok();
            println!("[dry-run] 跳过");
            uploaded += 1;
            continue;
        }
        args.interval.throttle(upload_seq).await;
        upload_seq += 1;
        // 行尾先换行，进度行独占一行（见 `UploadProgress`）
        println!("  新建 [{}] {} ...", i + 1, dw.doi);
        if upload_one_zotero(
            dw,
            &base_url,
            update_meta,
            false,
            &client,
            &token,
            args.interval.timeout_override(),
            args.interval.extract_meta,
        )
        .await
        {
            println!("  ✓ 已上传");
            uploaded += 1;
        } else {
            println!("  ✗ 失败");
            failed += 1;
        }
    }

    // 执行更新 PDF
    for (i, (server_id, dw, _, _)) in to_update_pdf.iter().enumerate() {
        if !update_choices[i] {
            continue;
        }
        if args.dry_run {
            print!("  更新 PDF [{}] ID={} ({}) ... ", i + 1, server_id, dw.doi);
            io::stdout().flush().ok();
            println!("[dry-run] 跳过");
            updated_pdf += 1;
            continue;
        }
        args.interval.throttle(upload_seq).await;
        upload_seq += 1;
        println!("  更新 PDF [{}] ID={} ({}) ...", i + 1, server_id, dw.doi);
        if api_replace_pdf(
            &base_url,
            *server_id,
            Path::new(&dw.file_path),
            &token,
            args.interval.timeout_override(),
        )
        .await
        {
            if update_meta {
                api_update_metadata(&client, &base_url, *server_id, &dw.meta, &token).await;
                api_save_info(&client, &base_url, *server_id, &dw.info, &token).await;
            }
            println!("  ✓ 已更新");
            updated_pdf += 1;
        } else {
            println!("  ✗ 失败");
            failed += 1;
        }
    }

    println!();
    println!("==================================================");
    println!("新建上传: {}", uploaded);
    println!("更新 PDF: {}", updated_pdf);
    println!("跳过:     {}", skip_count);
    println!("失败:     {}", failed);

    Ok(())
}

// ==================== 网盘同步（sync 子命令） ====================
//
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
async fn run_sync() -> Result<()> {
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


// ==================== 入口 ====================

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    match &cli.command {
        Command::Zotero(args) => run_zotero(args).await,
        Command::Dir(args) => run_dir(args).await,
        Command::Sync(_) => run_sync().await,
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

    #[test]
    fn picks_attachment_matching_title_not_newest() {
        let dir = std::env::temp_dir().join(format!("okb-attach-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let wrong = dir.join("1-s2.0-S0016003223006038-main.pdf");
        let right = dir.join("Brogliato - 2016 - Nonsmooth Mechanics Models, Dynamics and Control.pdf");
        std::fs::write(&wrong, b"wrong").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(&right, b"right").unwrap();
        // 让「错的那个」成为最新文件，模拟原来的 bug 场景
        let newer = std::fs::File::options().write(true).open(&wrong).unwrap();
        newer.set_modified(std::time::SystemTime::now()).unwrap();
        drop(newer);
        let paths = vec![
            wrong.to_string_lossy().to_string(),
            right.to_string_lossy().to_string(),
        ];
        let picked = pick_attachment(
            &paths,
            "Nonsmooth mechanics: models, dynamics and control",
            "Brogliato, Bernard",
            "X3SM5EK9 (10.1007/978-3-319-28664-8)",
        )
        .unwrap();
        assert!(picked.ends_with("Brogliato - 2016 - Nonsmooth Mechanics Models, Dynamics and Control.pdf"), "picked: {picked}");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn title_tokens_skip_stopwords_and_keep_surnames() {
        let t = title_tokens("Nonsmooth mechanics: models, dynamics and control", "Brogliato, Bernard; Utkin, Vadim");
        assert!(t.contains(&"nonsmooth".to_string()));
        assert!(t.contains(&"brogliato".to_string()));
        assert!(t.contains(&"utkin".to_string()));
        assert!(!t.iter().any(|w| w == "and" || w == "the"));
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
