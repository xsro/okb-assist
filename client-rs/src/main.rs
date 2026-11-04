//! OKB-Assist 客户端工具
//!
//! 子命令:
//!   zotero <csv_file>   — 从 Zotero 导出的 CSV 导入文献
//!   dir    <dir>        — 扫描目录中的 PDF，与服务器比对后交互上传

use std::collections::{HashMap, HashSet};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use clap::Parser;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
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
}

/// 上传节流参数（`zotero` / `dir` 两个子命令共用）
#[derive(clap::Args, Debug, Clone)]
struct UploadIntervalArgs {
    /// 每次上传之间的间隔秒数（默认 1 秒；可写小数，0 表示不等待）
    #[arg(long, value_name = "SECONDS", default_value_t = 1.0)]
    upload_interval: f64,
}

impl UploadIntervalArgs {
    /// 第 `seq` 次上传（从 0 开始）前的等待：首个不等待，间隔 <= 0 也不等待
    async fn throttle(&self, seq: usize) {
        if seq > 0 && self.upload_interval > 0.0 {
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

/// 上传响应
#[derive(Debug, Deserialize)]
struct UploadResult {
    id: u64,
}

/// 本地可上传文档（Zotero）
#[derive(Clone)]
struct LocalDoc {
    doi: String,
    file_path: String,
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

/// 上传类请求的超时：基础 120s + 每 MB 2s（上限 30 分钟），
/// 避免大文件走慢链路时被固定 120s 提前掐断。
fn upload_client(file_size: u64) -> Result<Client> {
    let extra_secs = (file_size / (1024 * 1024)).saturating_mul(2);
    let secs = (120 + extra_secs).min(30 * 60);
    Client::builder()
        .timeout(std::time::Duration::from_secs(secs))
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

    // 流式 multipart：必须带长度，否则 reqwest 会改成 chunked
    let part = reqwest::multipart::Part::stream_with_length(
        reqwest::Body::wrap_stream(tokio_util::io::ReaderStream::new(file)),
        size,
    )
    .file_name(filename)
    .mime_str("application/pdf")?;

    let mut form = reqwest::multipart::Form::new().part("file", part);
    for (k, v) in extra_fields {
        form = form.text((*k).to_string(), (*v).to_string());
    }

    let client = upload_client(size)?;
    let mut req = client.post(url).multipart(form);
    if let Some(t) = token {
        req = req.header("X-Token", t.as_str());
    }
    Ok(req.send().await?)
}

/// 上传 PDF 文件到服务器（新建文献，默认不触发自动处理）
async fn api_upload(
    base_url: &str,
    file_path: &Path,
    doi: Option<&str>,
    token: &Option<String>,
) -> Option<UploadResult> {
    let url = format!("{}/assist/api/documents/upload", base_url);
    // 显式带 DOI：服务端不必解析 PDF 就能做 DOI 去重（命中时返回 409 `kind=doi`）
    let mut fields: Vec<(&str, &str)> = vec![("auto_process", "false")];
    if let Some(doi) = doi {
        fields.push(("doi", doi));
    }
    let resp = match post_pdf(&url, file_path, token, &fields).await {
        Ok(r) => r,
        Err(e) => {
            eprintln!("  上传失败: {}", e);
            return None;
        }
    };

    let status = resp.status();
    if status == reqwest::StatusCode::OK {
        resp.json().await.ok()
    } else {
        let body = resp.text().await.unwrap_or_default();
        if status == reqwest::StatusCode::CONFLICT {
            // 服务端去重命中（kind=hash|doi）：报出已有文档 id，方便人工确认后重跑
            let parsed: Option<serde_json::Value> = serde_json::from_str(&body).ok();
            let kind = parsed
                .as_ref()
                .and_then(|v| v.get("kind"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let existing_id = parsed
                .as_ref()
                .and_then(|v| v.get("existing_id"))
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            eprintln!(
                "  上传被服务端去重拦下（kind={}）: 已有文档 id={}（可改用 --dup-doi-action hash 覆盖）",
                if kind.is_empty() { "unknown" } else { kind },
                existing_id
            );
        } else {
            eprintln!(
                "  上传失败: {} - {}",
                status,
                truncate_body(&body, 200)
            );
        }
        None
    }
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
) -> bool {
    let url = format!("{}/assist/api/documents/{}/pdf", base_url, doc_id);
    let resp = match post_pdf(&url, file_path, token, &[]).await {
        Ok(r) => r,
        Err(e) => {
            eprintln!("  更新 PDF 失败: {}", e);
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

    println!("扫描目录: {}", args.dir);
    println!("递归搜索: {}", if args.recursive { "是" } else { "否" });
    println!("上传间隔: {}", args.interval.describe());
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
        print!("        上传 ... ");
        io::stdout().flush().ok();
        match api_upload(&base_url, path, None, &token).await {
            Some(result) => {
                println!("成功 -> id={}", result.id);
                uploaded += 1;
                // 触发解析流水线
                if process && api_trigger_pipeline(&client, &base_url, result.id, &token).await {
                    println!("        -> 已触发流水线");
                }
            }
            None => {
                println!("失败");
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

fn get_newest_file(paths: &[String]) -> Option<String> {
    paths
        .iter()
        .filter(|p| Path::new(p).exists())
        .max_by_key(|p| {
            std::fs::metadata(p)
                .and_then(|m| m.modified())
                .unwrap_or(std::time::SystemTime::UNIX_EPOCH)
        })
        .cloned()
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
async fn compute_local_hash(d: &LocalDoc) -> Option<String> {
    let file_name = Path::new(&d.file_path)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| d.file_path.clone());
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
            Some(h)
        }
        Err(e) => {
            println!("失败 ({}): {}", format_duration(started.elapsed()), e);
            None
        }
    }
}

async fn upload_one_zotero(
    d: &LocalDoc,
    base_url: &str,
    update_meta: bool,
    dry_run: bool,
    client: &Client,
    token: &Option<String>,
) -> bool {
    if dry_run {
        println!("  [dry-run] 跳过上传: {} ({})", d.doi, d.file_path);
        return true;
    }
    let res = match api_upload(base_url, Path::new(&d.file_path), Some(d.doi.as_str()), token).await {
        Some(r) => r,
        None => {
            println!("  上传失败 {}: 无返回结果", d.doi);
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

    println!("服务地址: {}", base_url);
    println!("CSV 文件: {}", args.csv_file);
    println!("Storage root: {}", storage_root.clone().unwrap_or_default());
    println!("Token: {}", if token.is_some() { "********" } else { "未设置" });
    println!("更新元数据: {}", if update_meta { "是" } else { "否 (默认)" });
    println!("上传间隔: {}", args.interval.describe());
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
        let file_path = match get_newest_file(&paths) {
            Some(f) => f,
            None => {
                if !key_filter.is_empty() {
                    filtered_out.push((item_key.clone(), "没有可用的 PDF 附件".to_string()));
                }
                continue;
            }
        };
        let meta = parse_zotero_row(&norm);
        let info: ZoteroRow = raw
            .iter()
            .filter(|(_, v)| !v.trim().is_empty())
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        local_docs.push(LocalDoc {
            doi,
            file_path,
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
            match compute_local_hash(d).await {
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
        print!("  新建 [{}] {} ... ", i + 1, dw.doi);
        io::stdout().flush().ok();
        if upload_one_zotero(dw, &base_url, update_meta, false, &client, &token).await {
            println!("✓");
            uploaded += 1;
        } else {
            println!("失败");
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
        print!("  更新 PDF [{}] ID={} ({}) ... ", i + 1, server_id, dw.doi);
        io::stdout().flush().ok();
        if api_replace_pdf(&base_url, *server_id, Path::new(&dw.file_path), &token).await {
            if update_meta {
                api_update_metadata(&client, &base_url, *server_id, &dw.meta, &token).await;
                api_save_info(&client, &base_url, *server_id, &dw.info, &token).await;
            }
            println!("✓");
            updated_pdf += 1;
        } else {
            println!("失败");
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

// ==================== 入口 ====================

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    match &cli.command {
        Command::Zotero(args) => run_zotero(args).await,
        Command::Dir(args) => run_dir(args).await,
    }
}