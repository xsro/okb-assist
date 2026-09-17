use std::collections::{HashMap, HashSet};
use std::io::{self, Write};
use std::path::Path;

use anyhow::{bail, Context, Result};
use clap::Parser;
use reqwest::Client;
use serde::{Deserialize, Serialize};

// ==================== CLI 参数 ====================

#[derive(Parser, Debug)]
#[command(
    name = "import-zotero",
    about = "读取 Zotero 导出的 CSV，逐条上传到 OKB-Assist 服务",
    after_help = r"用法示例:
    import-zotero zotero_export.csv
    import-zotero zotero_export.csv --storage-root /path/to/storage
    import-zotero data\我的文库.csv --base-url http://192.168.1.100:5001 --token change-me"
)]
struct Args {
    /// Zotero 导出的 CSV 文件路径
    csv_file: String,

    /// OKB-Assist 服务地址
    #[arg(long, default_value = "http://192.168.1.122:5001")]
    base_url: String,

    /// 访问令牌（也可通过 OKB_ASSIST_TOKEN 环境变量或 ~/.okb_assist_token 文件设置）
    #[arg(long)]
    token: Option<String>,

    /// Zotero storage 文件夹的本地路径，用于把 Windows 附件路径映射到本地
    #[arg(long)]
    storage_root: Option<String>,

    /// 试运行，不实际上传
    #[arg(long)]
    dry_run: bool,

    /// 上传后自动更新元数据（默认开启）
    #[arg(long, default_value_t = true)]
    update_meta: bool,

    /// 不更新元数据，仅上传文件
    #[arg(long = "no-update-meta")]
    no_update_meta: bool,

    /// 显示解析出的元数据详情
    #[arg(long)]
    debug: bool,
}

// ==================== 数据类型 ====================

/// Zotero CSV 行（动态列名，用 HashMap 接收）
type ZoteroRow = HashMap<String, String>;

/// 文档元数据（上传后 PUT 更新用，字段名与服务端一致）
#[derive(Debug, Serialize)]
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

/// diff-dois 接口返回
#[derive(Debug, Deserialize)]
struct DiffResult {
    missing: Vec<String>,
    #[serde(default)]
    present_count: Option<usize>,
}

/// 上传响应
#[derive(Debug, Deserialize)]
struct UploadResult {
    id: u64,
}

/// 本地可上传文档
struct LocalDoc {
    doi: String,
    file_path: String,
    title: Option<String>,
    meta: DocMetadata,
    info: HashMap<String, String>,
}

// ==================== 列名映射（兼容不同导出格式） ====================

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

// ==================== 类型映射 ====================

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
        _ => zotero_type, // 未知类型保留原始值
    };
    mapped.to_string()
}

// ==================== 语言映射 ====================

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
    // 尝试取前两位
    if l.chars().count() >= 2 {
        let prefix: String = l.chars().take(2).collect();
        if let Some(m) = lang_map(&prefix) {
            return Some(m.to_string());
        }
    }
    None
}

// ==================== Token 读取 ====================

/// 按优先级读取 token: 参数 > 环境变量 > 文件
fn read_token(arg_token: Option<&str>) -> Option<String> {
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
    let token_file = dirs::home_dir()?.join(".okb_assist_token");
    let content = std::fs::read_to_string(&token_file).ok()?;
    let t = content.trim().to_string();
    if t.is_empty() {
        None
    } else {
        Some(t)
    }
}

// ==================== 文件路径解析 ====================

/// 去掉 Zotero 附加在路径后的 ":mime:size" 后缀。
/// 路径本身最多只有一个 ':'（Windows 盘符），从第二个 ':' 起截断。
fn strip_zotero_suffix(p: &str) -> &str {
    if let Some(first) = p.find(':') {
        if let Some(second_rel) = p[first + 1..].find(':') {
            return &p[..first + 1 + second_rel];
        }
    }
    p
}

/// 将 Windows Zotero storage 路径映射到本地 storage_root。
/// 例如 C:\Users\X\Zotero\storage\HASH\file.pdf -> {storage_root}/HASH/file.pdf
fn map_storage_path(path: &str, root: &str) -> Option<String> {
    let comps: Vec<&str> = path.split(['/', '\\']).collect();
    if comps.len() > 5 {
        let rel = comps[5..].join(std::path::MAIN_SEPARATOR_STR);
        let root = root.trim_end_matches(['/', '\\']);
        Some(format!(
            "{}{}{}",
            root,
            std::path::MAIN_SEPARATOR,
            rel
        ))
    } else {
        None
    }
}

/// 解析 Zotero file 字段，返回存在的 PDF 文件路径列表。
/// 支持分号分隔的多个路径，以及可选的 storage_root 映射。
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

/// 返回修改时间最新的文件路径
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

// ==================== Zotero 元数据解析 ====================

/// 解析 Zotero 作者字段，支持多种格式。
/// "Last, First" → "First Last"
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

/// 从日期字符串中提取第一个 4 位连续数字年份
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

/// 从归一化后的 Zotero 行提取尽可能完整的元数据。
fn parse_zotero_row(row: &ZoteroRow) -> DocMetadata {
    // ── 基本信息 ──
    let title = row.get("title").filter(|s| !s.is_empty()).cloned();

    // 作者
    let authors = row
        .get("author")
        .map(|s| parse_authors(s))
        .filter(|v| !v.is_empty())
        .map(|v| serde_json::to_string(&v).unwrap_or_default());

    // 年份：优先 Publication Year，其次从 Date 提取
    let year = row
        .get("year")
        .and_then(|s| s.trim().parse::<i32>().ok())
        .or_else(|| row.get("date").and_then(|d| extract_year_from_date(d)));

    let doi = row.get("doi").filter(|s| !s.is_empty()).cloned();

    // ── 期刊 / 来源 ──
    // Zotero CSV 中 "Item Type" 才是文献类型，"Type" 是另一字段（常为空或自定义），
    // 因此优先取 "Item Type"，回退到 "Type"。
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

    // ── 标签 → 关键词 ──
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

    // ── 文档类型 ──
    let doc_type = if item_type.is_empty() {
        None
    } else {
        Some(map_doc_type(&item_type))
    };

    // ── 语言 ──
    let language = row.get("language").and_then(|s| normalize_language(s));

    // ── 来源/出版信息 ──
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

    // ── 卷号/期号/页码 → 拼接到 journal 末尾 ──
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

    // ── ISBN/ISSN → 存入 source 末尾 ──
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

    // ── URL ──（无 DOI 时把 URL 存入 source）
    if let Some(url) = row.get("url").filter(|s| !s.is_empty()) {
        if doi.is_none() {
            source = Some(match source {
                Some(existing) => format!("{} URL:{}", existing, url),
                None => format!("URL:{}", url),
            });
        }
    }

    // ── 编辑/译者 → 存入 source ──
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

// ==================== API 调用 ====================

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

/// 批量比对 DOI，返回服务器缺失的 DOI 列表
async fn diff_dois(
    client: &Client,
    base_url: &str,
    dois: &[String],
    token: &Option<String>,
) -> Option<DiffResult> {
    let url = format!("{}/assist/api/documents/diff-dois", base_url);
    let body = serde_json::json!({ "dois": dois });
    match client
        .post(&url)
        .headers(auth_headers(token))
        .json(&body)
        .send()
        .await
    {
        Ok(resp) if resp.status() == reqwest::StatusCode::OK => resp.json().await.ok(),
        Ok(resp) => {
            eprintln!("错误: diff-dois 请求失败: {}", resp.status());
            None
        }
        Err(e) => {
            eprintln!("错误: 无法连接服务器进行 diff: {}", e);
            None
        }
    }
}

/// 上传 PDF 文件到服务器
async fn upload_document(
    client: &Client,
    base_url: &str,
    file_path: &str,
    token: &Option<String>,
) -> Option<UploadResult> {
    let path = Path::new(file_path);
    let filename = path
        .file_name()
        .map(|f| f.to_string_lossy().to_string())
        .unwrap_or_else(|| "unknown.pdf".to_string());

    let file_data = match tokio::fs::read(file_path).await {
        Ok(data) => data,
        Err(e) => {
            eprintln!("  上传失败: 无法读取文件 {}: {}", file_path, e);
            return None;
        }
    };

    let file_part = reqwest::multipart::Part::bytes(file_data)
        .file_name(filename)
        .mime_str("application/pdf")
        .unwrap();

    let form = reqwest::multipart::Form::new().part("file", file_part);

    let url = format!("{}/assist/api/documents/upload", base_url);
    let mut req = client.post(&url).multipart(form);
    if let Some(t) = token {
        req = req.header("X-Token", t.as_str());
    }

    match req.send().await {
        Ok(resp) if resp.status() == reqwest::StatusCode::OK => resp.json().await.ok(),
        Ok(resp) => {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            eprintln!("  上传失败: {} - {}", status, &body[..body.len().min(200)]);
            None
        }
        Err(e) => {
            eprintln!("  上传失败: {}", e);
            None
        }
    }
}

/// 更新文档元数据
async fn update_document_metadata(
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
async fn save_document_info(
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

// ==================== 展示 ====================

fn print_doc_detail(d: &LocalDoc) {
    let meta = &d.meta;
    println!(
        "  ── {} ──",
        d.title.clone().unwrap_or_else(|| "(无标题)".to_string())
    );
    if let Some(authors) = &meta.authors {
        if let Ok(a) = serde_json::from_str::<Vec<String>>(authors) {
            println!("  作者: {}", a.join(", "));
        }
    }
    if let Some(y) = meta.year {
        println!("  年份: {}", y);
    }
    if let Some(j) = &meta.journal {
        println!("  期刊/来源: {}", j);
    }
    if !d.doi.is_empty() {
        println!("  DOI: {}", d.doi);
    }
    if let Some(l) = &meta.language {
        println!("  语言: {}", l);
    }
    if let Some(dt) = &meta.doc_type {
        println!("  文献类型: {}", dt);
    }
    if let Some(ab) = &meta.abstract_text {
        let s = if ab.chars().count() > 300 {
            format!("{}...", ab.chars().take(300).collect::<String>())
        } else {
            ab.clone()
        };
        println!("  摘要: {}", s);
    }
    println!("  文件: {}", d.file_path);
}

/// 上传单篇文献。返回是否成功。
async fn upload_one(
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
    let res = match upload_document(client, base_url, &d.file_path, token).await {
        Some(r) => r,
        None => {
            println!("  上传失败 {}: 无返回结果", d.doi);
            return false;
        }
    };
    let doc_id = res.id;
    if update_meta {
        update_document_metadata(client, base_url, doc_id, &d.meta, token).await;
        save_document_info(client, base_url, doc_id, &d.info, token).await;
        println!("  已保存信息到 json: id={}", doc_id);
    }
    println!("  已上传: {} -> id={}", d.doi, doc_id);
    true
}

// ==================== 主流程 ====================

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    let token = read_token(args.token.as_deref());
    let update_meta = args.update_meta && !args.no_update_meta;

    println!("服务地址: {}", args.base_url);
    println!("CSV 文件: {}", args.csv_file);
    println!("Storage root: {}", args.storage_root.clone().unwrap_or_default());
    println!("Token: {}", if token.is_some() { "********" } else { "未设置" });
    println!();

    let csv_path = Path::new(&args.csv_file);
    if !csv_path.is_file() {
        bail!("错误: CSV 文件不存在: {}", args.csv_file);
    }

    // 读取 CSV（自动处理 UTF-8 BOM，列名归一化为 canonical key）
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

    // 1. 读取 CSV，构建本地可上传文档列表
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

        let doi = norm.get("doi").cloned().unwrap_or_default();
        if doi.is_empty() {
            continue; // DOI-based flow; skip rows w/o DOI
        }
        let file_field = norm.get("file").cloned().unwrap_or_default();
        let paths = parse_file_paths(&file_field, &args.storage_root);
        let file_path = match get_newest_file(&paths) {
            Some(f) => f,
            None => continue, // not locally uploadable
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

    // 2. 与服务器做 diff
    let client = client()?;
    let dois: Vec<String> = local_docs.iter().map(|d| d.doi.clone()).collect();
    let diff = match diff_dois(&client, &args.base_url, &dois, &token).await {
        Some(d) => d,
        None => {
            println!("错误: diff 失败，无法继续。");
            return Ok(());
        }
    };

    let missing: HashSet<String> = diff.missing.into_iter().collect();
    let to_upload: Vec<&LocalDoc> =
        local_docs.iter().filter(|d| missing.contains(&d.doi)).collect();

    let total = local_docs.len();
    let vacant_count = to_upload.len();
    let present_count = diff.present_count.unwrap_or(total - vacant_count);

    println!("本地可上传（含 DOI 且有文件）: {}", total);
    println!("服务器已存在: {}", present_count);
    println!("服务器空缺（待上传）: {}", vacant_count);
    println!();
    for (i, d) in to_upload.iter().enumerate() {
        println!(
            "  [{}] {}  {}",
            i + 1,
            d.doi,
            d.title.clone().unwrap_or_default()
        );
    }

    if to_upload.is_empty() {
        println!("无空缺文献，无需上传。");
        return Ok(());
    }

    // 3. 交互选择
    print!("请选择 [a] 全部上传  [s] 逐个上传  [q] 退出: ");
    io::stdout().flush().ok();
    let mut choice = String::new();
    io::stdin().read_line(&mut choice).ok();
    let choice = choice.trim().to_lowercase();

    let mut uploaded = 0usize;
    let mut failed = 0usize;

    if choice == "q" {
        return Ok(());
    } else if choice == "a" {
        for d in &to_upload {
            if upload_one(d, &args.base_url, update_meta, args.dry_run, &client, &token).await {
                uploaded += 1;
            } else {
                failed += 1;
            }
        }
    } else if choice == "s" {
        for d in &to_upload {
            print_doc_detail(d);
            print!("上传这篇？[y/N]: ");
            io::stdout().flush().ok();
            let mut ans = String::new();
            io::stdin().read_line(&mut ans).ok();
            if ans.trim().to_lowercase() == "y" {
                if upload_one(d, &args.base_url, update_meta, args.dry_run, &client, &token).await {
                    uploaded += 1;
                } else {
                    failed += 1;
                }
            } else {
                println!("  跳过。");
            }
        }
    } else {
        println!("无效选择，退出。");
        return Ok(());
    }

    // 4. 最终统计
    println!();
    println!("==================================================");
    println!("已上传: {}", uploaded);
    println!("失败: {}", failed);

    Ok(())
}

/// 构造带超时（120s）且跟随重定向的 HTTP 客户端
fn client() -> Result<Client> {
    Client::builder()
        .timeout(std::time::Duration::from_secs(120))
        .redirect(reqwest::redirect::Policy::default())
        .build()
        .context("无法创建 HTTP 客户端")
}
