//! 子命令 `zotero`：解析 Zotero 导出的 CSV，比对服务端后批量导入文献

use std::collections::{HashMap, HashSet};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;

use anyhow::{bail, Context, Result};
use clap::Parser;
use reqwest::Client;

use crate::util::*;

// ==================== CLI 参数 ====================

/// Zotero CSV 导入参数
#[derive(Parser, Debug)]
pub(crate) struct ZoteroArgs {
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

// ==================== 数据类型 ====================

/// Zotero CSV 行（动态列名，用 HashMap 接收）
type ZoteroRow = HashMap<String, String>;

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
    extract_meta: ExtractMeta,
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

pub(crate) async fn run_zotero(args: &ZoteroArgs) -> Result<()> {
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
    println!("PDF 元数据抽取: {}", args.interval.extract_meta.describe());
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
                (true, Some(doc)) => Some(ServerEntry::from_doc_json(&doc)),
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
        if args.interval.yes {
            new_choices = vec![true; new_entries.len()];
            println!("\n新建策略：-y 自动确认，全部新建。");
        } else {
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
        if args.interval.yes {
            update_choices = vec![true; to_update_pdf.len()];
            println!("\n更新策略：-y 自动确认，全部更新。");
        } else {
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
    if args.interval.yes {
        println!("\n已用 -y 自动确认。");
    } else {
        print!("\n确认执行？[Y/n]: ");
        io::stdout().flush().ok();
        let mut confirm = String::new();
        io::stdin().read_line(&mut confirm).ok();
        if confirm.trim().to_lowercase() == "n" {
            println!("已取消。");
            return Ok(());
        }
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

#[cfg(test)]
mod attachment_tests {
    use super::*;

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
