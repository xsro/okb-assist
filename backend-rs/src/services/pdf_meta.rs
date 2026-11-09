//! 轻量级 PDF 元数据（含 DOI）提取。
//!
//! 当启用 `mupdf` feature 时，使用内置的 `mupdf` 库提取 Info 字典和页面文本。
//! 未启用时，回退为调用外部 `mutool` 工具完成相同功能。
//!
//! **公共入口只接受文件路径**：调用方（上传、解析、PDF 信息页）都不必把整份 PDF
//! 读进内存；mutool 路径也直接读原文件，不再为每次抽取写临时副本。

use std::path::Path;

#[cfg(feature = "mupdf")]
use mupdf::{Document, MetadataName, TextExtractOptions};

use std::process::Command;

// ── 公共 API ──────────────────────────────────────────────

/// 规范化 DOI
pub fn normalize_doi(raw: &str) -> Option<String> {
    let s = raw.trim();
    if s.is_empty() {
        return None;
    }
    let mut s = s.to_string();
    for prefix in &[
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
    s = s.trim_end_matches(|c: char| c == '.' || c == ',' || c == ';' || c == ')').to_string();
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

/// 异步从磁盘上的 PDF 提取元数据（在后台线程执行，不把整份 PDF 读进内存）
pub async fn extract_pdf_metadata_from_path_async(
    path: String,
    filename: Option<String>,
    doc_id: Option<i64>,
) -> serde_json::Value {
    tokio::task::spawn_blocking(move || {
        extract_pdf_metadata_from_path(Path::new(&path), filename.as_deref(), doc_id)
    })
    .await
    .unwrap_or_else(|_| serde_json::Value::Object(serde_json::Map::new()))
}

/// 从磁盘上的 PDF 提取元数据（按运行时引擎分发）
pub fn extract_pdf_metadata_from_path(
    path: &Path,
    filename: Option<&str>,
    doc_id: Option<i64>,
) -> serde_json::Value {
    match crate::services::pdf_engine::active() {
        #[cfg(feature = "mupdf")]
        crate::services::pdf_engine::PdfEngine::Mupdf => {
            extract_pdf_metadata_from_path_mupdf(path, filename, doc_id)
        }
        _ => extract_pdf_metadata_from_path_mutool(path, filename, doc_id),
    }
}

/// 从磁盘上的 PDF 提取元数据（内置 mupdf）
///
/// mupdf 路径拿不到逐行字号，因此版面推断退化为「行序启发式」；
/// Info 字典只用来补空字段（它的 Title 常常是排版软件留下的文件名）。
#[cfg(feature = "mupdf")]
fn extract_pdf_metadata_from_path_mupdf(
    path: &Path,
    filename: Option<&str>,
    doc_id: Option<i64>,
) -> serde_json::Value {
    let mut entries: Vec<(String, String)> = Vec::new();
    let mut plain = String::new();

    if let Ok(doc) = Document::open(path) {
        // 1. Info 字典（只补空字段，CreationDate 作为年份最后一层兑底）
        for name in [
            MetadataName::Title,
            MetadataName::Author,
            MetadataName::Subject,
            MetadataName::Keywords,
            MetadataName::CreationDate,
        ] {
            if let Ok(val) = doc.metadata(name) {
                let val = val.trim().to_string();
                if !val.is_empty() {
                    let key = match name {
                        MetadataName::Title => "Title",
                        MetadataName::Author => "Author",
                        MetadataName::Subject => "Subject",
                        MetadataName::Keywords => "Keywords",
                        MetadataName::CreationDate => "CreationDate",
                        _ => continue,
                    };
                    entries.push((key.to_string(), val));
                }
            }
        }

        // 2. 页面文本（前 5 页）→ 行序启发式
        let text = extract_full_text(&doc, doc_id);
        plain = normalize_fullwidth(text.as_deref().unwrap_or_default());
    }

    let info = info_dict_from_entries(&entries);
    let mut result = analyze_metadata(
        &lines_from_plain_text(&plain),
        &plain,
        filename,
        info_year_of(&entries),
    );

    if let Some(info) = info.as_ref() {
        fill_from_info_dict(info, filename, &mut result);
    }

    serde_json::Value::Object(result)
}

/// 从磁盘上的 PDF 提取元数据（外部 mutool：直接读原文件，无需临时副本）
///
/// 两次 mutool 调用：`stext` 取首页逐行字号/坐标（题名、作者、刊名靠版面判定），
/// `text` 取前 5 页纯文本（引用格式、DOI、摘要、关键词、年份）。
fn extract_pdf_metadata_from_path_mutool(
    path: &Path,
    filename: Option<&str>,
    doc_id: Option<i64>,
) -> serde_json::Value {
    let info = extract_info_dict_from_path(path);
    let info_year = info_year_of_info(&info);

    let plain = extract_full_text_from_path(path, doc_id)
        .map(|t| normalize_fullwidth(&t))
        .unwrap_or_default();

    let lines = extract_layout_from_path(path);
    let mut result = analyze_metadata(&lines, &plain, filename, info_year);

    // Info 字典只补空字段（Title/Author 常常是排版软件留下的垃圾）
    if let Some(info) = info.as_ref() {
        fill_from_info_dict(info, filename, &mut result);
    }

    serde_json::Value::Object(result)
}

// ── PDF 元信息报告（Info 字典 + 页数 + 启发式结果） ────────

/// 异步生成 PDF 元信息报告（在后台线程执行，不把整份 PDF 读进内存）
pub async fn extract_pdf_report_from_path_async(
    path: String,
    filename: Option<String>,
    doc_id: Option<i64>,
) -> serde_json::Value {
    tokio::task::spawn_blocking(move || {
        extract_pdf_report_from_path(Path::new(&path), filename.as_deref(), doc_id)
    })
    .await
    .unwrap_or_else(|_| serde_json::json!({"error": "PDF 元信息提取任务崩溃"}))
}

/// 生成 PDF 元信息报告，结构：
/// ```json
/// {
///   "file_size": 123456,
///   "pdf_version": "1.7",
///   "page_count": 17,
///   "encrypted": false,
///   "info": { "Title": "...", "Author": "..." },
///   "heuristics": { "doi": "...", "volume": "..." },
///   "error": "仅在解析失败时出现"
/// }
/// ```
/// `info` 为 PDF Info 字典原始键值；`heuristics` 为现有启发式抽取结果。
pub fn extract_pdf_report_from_path(
    path: &Path,
    filename: Option<&str>,
    doc_id: Option<i64>,
) -> serde_json::Value {
    match crate::services::pdf_engine::active() {
        #[cfg(feature = "mupdf")]
        crate::services::pdf_engine::PdfEngine::Mupdf => {
            extract_pdf_report_from_path_mupdf(path, filename, doc_id)
        }
        _ => extract_pdf_report_from_path_mutool(path, filename, doc_id),
    }
}

/// 生成 PDF 元信息报告（内置 mupdf）
#[cfg(feature = "mupdf")]
fn extract_pdf_report_from_path_mupdf(
    path: &Path,
    filename: Option<&str>,
    doc_id: Option<i64>,
) -> serde_json::Value {
    let mut report = base_report_from_path(path);

    let doc = match Document::open(path) {
        Ok(d) => d,
        Err(e) => {
            report.insert("error".to_string(), serde_json::json!(e.to_string()));
            return serde_json::Value::Object(report);
        }
    };

    report.insert("page_count".to_string(), serde_json::json!(doc.page_count().ok()));
    report.insert("encrypted".to_string(), serde_json::json!(doc.needs_password().unwrap_or(false)));

    // Info 字典（mupdf 只能取到标准字段）
    let mut info = serde_json::Map::new();
    for (name, key) in [
        (MetadataName::Format, "Format"),
        (MetadataName::Encryption, "Encryption"),
        (MetadataName::Title, "Title"),
        (MetadataName::Author, "Author"),
        (MetadataName::Subject, "Subject"),
        (MetadataName::Keywords, "Keywords"),
        (MetadataName::Creator, "Creator"),
        (MetadataName::Producer, "Producer"),
        (MetadataName::CreationDate, "CreationDate"),
        (MetadataName::ModDate, "ModDate"),
    ] {
        if let Ok(val) = doc.metadata(name) {
            let val = val.trim().to_string();
            if !val.is_empty() {
                info.insert(key.to_string(), serde_json::Value::String(val));
            }
        }
    }
    let heuristic_entries: Vec<(String, String)> = info
        .iter()
        .map(|(k, v)| (k.clone(), v.as_str().unwrap_or_default().to_string()))
        .collect();
    report.insert("info".to_string(), serde_json::Value::Object(info));

    // 启发式抽取（与元数据抽取同一套逻辑：版面/行序 → Info 字典补空）
    let plain = extract_full_text(&doc, doc_id)
        .map(|t| normalize_fullwidth(&t))
        .unwrap_or_default();
    let mut heuristic = analyze_metadata(
        &lines_from_plain_text(&plain),
        &plain,
        filename,
        info_year_of(&heuristic_entries),
    );
    if let Some(parsed) = info_dict_from_entries(&heuristic_entries) {
        fill_from_info_dict(&parsed, filename, &mut heuristic);
    }
    report.insert("heuristics".to_string(), serde_json::Value::Object(heuristic));

    serde_json::Value::Object(report)
}

/// 生成 PDF 元信息报告（外部 mutool：直接读原文件）
fn extract_pdf_report_from_path_mutool(
    path: &Path,
    filename: Option<&str>,
    doc_id: Option<i64>,
) -> serde_json::Value {
    let mut report = base_report_from_path(path);

    // Info 字典原始键值
    let entries = mutool_show(path, &["trailer.Info"])
        .map(|out| parse_pdf_dict_entries(&out))
        .unwrap_or_default();

    let mut info = serde_json::Map::new();
    for (k, v) in &entries {
        info.insert(k.clone(), serde_json::Value::String(v.clone()));
    }
    report.insert("info".to_string(), serde_json::Value::Object(info));
    report.insert("page_count".to_string(), serde_json::json!(mutool_page_count(path)));
    report.insert("encrypted".to_string(), serde_json::json!(mutool_encrypted(path)));

    // 启发式抽取（与元数据抽取同一套逻辑：版面/行序 → Info 字典补空）
    let plain = extract_full_text_from_path(path, doc_id)
        .map(|t| normalize_fullwidth(&t))
        .unwrap_or_default();
    let mut heuristic = analyze_metadata(
        &lines_from_plain_text(&plain),
        &plain,
        filename,
        info_year_of(&entries),
    );
    if let Some(parsed) = info_dict_from_entries(&entries) {
        fill_from_info_dict(&parsed, filename, &mut heuristic);
    }
    report.insert("heuristics".to_string(), serde_json::Value::Object(heuristic));

    serde_json::Value::Object(report)
}

/// 报告公共字段：文件大小与 PDF 版本
fn base_report_from_path(path: &Path) -> serde_json::Map<String, serde_json::Value> {
    let mut report = serde_json::Map::new();
    let size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    report.insert("file_size".to_string(), serde_json::json!(size));
    report.insert(
        "pdf_version".to_string(),
        serde_json::json!(pdf_version_from_file(path)),
    );
    report
}

/// 从文件头（前 1 KB）读取 PDF 版本号
fn pdf_version_from_file(path: &Path) -> Option<String> {
    use std::io::Read;
    let mut file = std::fs::File::open(path).ok()?;
    let mut head = vec![0u8; 1024];
    let n = file.read(&mut head).ok()?;
    head.truncate(n);
    pdf_version_from_header(&head)
}

/// 从文件头读取 PDF 版本号（如 `1.7`）
fn pdf_version_from_header(content: &[u8]) -> Option<String> {
    let head = &content[..content.len().min(1024)];
    let text = String::from_utf8_lossy(head);
    let pos = text.find("%PDF-")?;
    let ver: String = text[pos + 5..]
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.')
        .collect();
    if ver.is_empty() {
        None
    } else {
        Some(ver)
    }
}

// ── mutool 路径：Info 字典提取 ────────────────────────────

struct InfoDict {
    title: Option<String>,
    author: Option<String>,
    subject: Option<String>,
    keywords: Option<String>,
    creation_date: Option<String>,
}

fn extract_info_dict_from_path(path: &Path) -> Option<InfoDict> {
    let stdout = mutool_show(path, &["trailer.Info"])?;
    let entries = parse_pdf_dict_entries(&stdout);
    info_dict_from_entries(&entries)
}

/// 调用 `mutool show <pdf> <path...>` 并返回 stdout
fn mutool_show(path: &std::path::Path, args: &[&str]) -> Option<String> {
    let mutool_path = crate::settings::get_settings().mutool_path();
    let output = Command::new(&mutool_path)
        .arg("show")
        .arg(path.to_str()?)
        .args(args)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&output.stdout).to_string())
}

/// 读取 PDF 页数（优先 Page tree 的 /Count，回退到 `mutool pages` 计数）
fn mutool_page_count(path: &std::path::Path) -> Option<i64> {
    if let Some(out) = mutool_show(path, &["trailer/Root/Pages/Count"]) {
        if let Ok(n) = out.trim().parse::<i64>() {
            if n >= 0 {
                return Some(n);
            }
        }
    }
    let mutool_path = crate::settings::get_settings().mutool_path();
    let output = Command::new(&mutool_path)
        .arg("pages")
        .arg(path.to_str()?)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let n = text.lines().filter(|l| l.trim_start().starts_with("<page ")).count() as i64;
    if n > 0 {
        Some(n)
    } else {
        None
    }
}

/// 是否加密（trailer 中存在 /Encrypt）
fn mutool_encrypted(path: &std::path::Path) -> bool {
    match mutool_show(path, &["trailer/Encrypt"]) {
        Some(out) => {
            let t = out.trim();
            !t.is_empty() && t != "null"
        }
        None => false,
    }
}

/// 从解析出的 Info 字典条目构造结构化 `InfoDict`（键名大小写不敏感）
fn info_dict_from_entries(entries: &[(String, String)]) -> Option<InfoDict> {
    if entries.is_empty() {
        return None;
    }
    let get = |name: &str| {
        entries
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.clone())
    };
    Some(InfoDict {
        title: get("Title"),
        author: get("Author"),
        subject: get("Subject"),
        keywords: get("Keywords"),
        creation_date: get("CreationDate"),
    })
}

/// Info 字典里的 CreationDate 年份（作为年份的最后一层兑底）
fn info_year_of(entries: &[(String, String)]) -> Option<i64> {
    info_dict_from_entries(entries)
        .and_then(|i| i.creation_date)
        .and_then(|d| parse_year_from_date(&d))
}

fn info_year_of_info(info: &Option<InfoDict>) -> Option<i64> {
    info.as_ref()
        .and_then(|i| i.creation_date.as_deref())
        .and_then(parse_year_from_date)
}

// ── PDF 字典 / 字符串解析（mutool 路径共用） ──────────────

#[allow(dead_code)]
fn is_pdf_delimiter(c: char) -> bool {
    c.is_whitespace()
        || matches!(
            c,
            '/' | '(' | ')' | '<' | '>' | '[' | ']' | '{' | '}' | '%'
        )
}

/// 解析 `mutool show` 输出的 PDF 字典，返回 (键, 解码后的值) 列表。
///
/// 支持字面量字符串 `(..)`、十六进制字符串 `<..>`、名字 `/Name`、
/// 数字与布尔字面量；嵌套字典的键值会被跳过（Info 字典里极少出现）。
fn parse_pdf_dict_entries(output: &str) -> Vec<(String, String)> {
    let chars: Vec<char> = output.chars().collect();
    let n = chars.len();
    let mut entries = Vec::new();
    let mut i = 0usize;

    while i < n {
        if chars[i] != '/' {
            i += 1;
            continue;
        }
        // 键名
        i += 1;
        let key_start = i;
        while i < n && !is_pdf_delimiter(chars[i]) {
            i += 1;
        }
        let key: String = chars[key_start..i].iter().collect();
        if key.is_empty() {
            continue;
        }
        // 跳过空白后读取值
        while i < n && chars[i].is_whitespace() {
            i += 1;
        }
        if i >= n {
            break;
        }
        let (value, next) = read_pdf_value(&chars, i);
        i = next;
        if let Some(v) = value {
            entries.push((key, v));
        }
    }

    entries
}

/// 读取一个 PDF 值，返回 (解码后的字符串, 下一个位置)
#[allow(dead_code)]
fn read_pdf_value(chars: &[char], start: usize) -> (Option<String>, usize) {
    let n = chars.len();
    let mut i = start;

    match chars[i] {
        // 字面量字符串
        '(' => {
            let mut depth = 0i32;
            let mut raw = String::new();
            while i < n {
                let c = chars[i];
                if c == '\\' {
                    raw.push(c);
                    i += 1;
                    if i < n {
                        raw.push(chars[i]);
                        i += 1;
                    }
                    continue;
                }
                if c == '(' {
                    depth += 1;
                    if depth == 1 {
                        i += 1;
                        continue;
                    }
                }
                if c == ')' {
                    depth -= 1;
                    if depth == 0 {
                        i += 1;
                        break;
                    }
                }
                raw.push(c);
                i += 1;
            }
            let bytes = unescape_pdf_literal(raw.as_bytes());
            (Some(decode_pdf_bytes(&bytes)), i)
        }
        // 嵌套字典：跳过（不展示）
        '<' if i + 1 < n && chars[i + 1] == '<' => {
            let mut depth = 0i32;
            while i + 1 < n {
                if chars[i] == '<' && chars[i + 1] == '<' {
                    depth += 1;
                    i += 2;
                    continue;
                }
                if chars[i] == '>' && chars[i + 1] == '>' {
                    depth -= 1;
                    i += 2;
                    if depth == 0 {
                        break;
                    }
                    continue;
                }
                i += 1;
            }
            (None, i)
        }
        // 十六进制字符串
        '<' => {
            i += 1;
            let hex_start = i;
            while i < n && chars[i] != '>' {
                i += 1;
            }
            let hex: String = chars[hex_start..i].iter().collect();
            if i < n {
                i += 1;
            }
            (Some(decode_pdf_bytes(&decode_pdf_hex(&hex))), i)
        }
        // 名字 / 数字 / 布尔
        _ => {
            let token_start = i;
            while i < n && !is_pdf_delimiter(chars[i]) {
                i += 1;
            }
            let token: String = chars[token_start..i].iter().collect();
            (Some(token), i)
        }
    }
}

/// 解析 PDF 字面量字符串的转义字符，返回原始字节
#[allow(dead_code)]
fn unescape_pdf_literal(raw: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(raw.len());
    let mut i = 0usize;
    while i < raw.len() {
        let b = raw[i];
        if b != b'\\' {
            out.push(b);
            i += 1;
            continue;
        }
        i += 1;
        if i >= raw.len() {
            break;
        }
        match raw[i] {
            b'n' => {
                out.push(b'\n');
                i += 1;
            }
            b'r' => {
                out.push(b'\r');
                i += 1;
            }
            b't' => {
                out.push(b'\t');
                i += 1;
            }
            b'b' => {
                out.push(8);
                i += 1;
            }
            b'f' => {
                out.push(12);
                i += 1;
            }
            b'(' | b')' | b'\\' => {
                out.push(raw[i]);
                i += 1;
            }
            // 续行：反斜杠 + 换行
            b'\r' => {
                i += 1;
                if i < raw.len() && raw[i] == b'\n' {
                    i += 1;
                }
            }
            b'\n' => {
                i += 1;
            }
            d if (b'0'..=b'7').contains(&d) => {
                // 八进制转义（最多 3 位）
                let mut code: u32 = 0;
                let mut count = 0;
                while count < 3 && i < raw.len() && (b'0'..=b'7').contains(&raw[i]) {
                    code = code * 8 + (raw[i] - b'0') as u32;
                    i += 1;
                    count += 1;
                }
                out.push((code & 0xFF) as u8);
            }
            _ => {
                out.push(raw[i]);
                i += 1;
            }
        }
    }
    out
}

/// 解码 PDF 十六进制字符串为字节
#[allow(dead_code)]
fn decode_pdf_hex(hex: &str) -> Vec<u8> {
    let nibbles: Vec<u8> = hex
        .chars()
        .filter(|c| !c.is_whitespace())
        .filter_map(|c| c.to_digit(16).map(|d| d as u8))
        .collect();
    nibbles
        .chunks(2)
        .map(|pair| if pair.len() == 2 { (pair[0] << 4) | pair[1] } else { pair[0] << 4 })
        .collect()
}

/// 解码 PDF 字符串字节：优先 UTF-16BE（带 BOM），否则按 UTF-8 / Latin-1 处理
#[allow(dead_code)]
fn decode_pdf_bytes(bytes: &[u8]) -> String {
    if bytes.len() >= 2 && bytes[0] == 0xFE && bytes[1] == 0xFF {
        let units: Vec<u16> = bytes[2..]
            .chunks_exact(2)
            .map(|c| u16::from_be_bytes([c[0], c[1]]))
            .collect();
        return String::from_utf16_lossy(&units);
    }
    match std::str::from_utf8(bytes) {
        Ok(s) => s.to_string(),
        Err(_) => bytes.iter().map(|&b| b as char).collect(),
    }
}

fn fill_from_info_dict(
    info: &InfoDict,
    filename: Option<&str>,
    result: &mut serde_json::Map<String, serde_json::Value>,
) {
    if !result.contains_key("title") {
        if let Some(ref val) = info.title {
            let val = val.trim().to_string();
            if !val.is_empty() && !looks_like_filename(&val, filename) && !looks_like_placeholder(&val) {
                result.insert("title".to_string(), serde_json::Value::String(val));
            }
        }
    }
    if !result.contains_key("authors") {
        if let Some(ref val) = info.author {
            let val = val.trim().to_string();
            if !val.is_empty() {
                let (names, ok) = parse_names(&val);
                let authors = if ok > 0 && !names.is_empty() {
                    names
                } else {
                    split_authors(&val)
                        .into_iter()
                        .map(|a| collapse_spaces(&strip_name_marks(&a)))
                        .filter(|a| !a.is_empty())
                        .collect()
                };
                if !authors.is_empty() {
                    result.insert("authors".to_string(), serde_json::json!(authors));
                }
            }
        }
    }
    if !result.contains_key("keywords") {
        if let Some(ref val) = info.keywords {
            let val = val.trim().to_string();
            if !val.is_empty() {
                result.insert("keywords".to_string(), serde_json::json!(split_keywords(&val)));
            }
        }
    }
    if !result.contains_key("abstract") {
        if let Some(ref val) = info.subject {
            let val = val.trim().to_string();
            // subject 常被排版软件拿来塞整段摘要，太短的（口号、版权声明）不算
            if val.chars().count() > 80 {
                result.insert("abstract".to_string(), serde_json::Value::String(val));
            }
        }
    }
    if !result.contains_key("year") {
        if let Some(ref val) = info.creation_date {
            if let Some(year) = parse_year_from_date(val) {
                result.insert("year".to_string(), serde_json::Value::Number(year.into()));
            }
        }
    }
}

// ── 页面文本提取 ─────────────────────────────────────────

#[cfg(feature = "mupdf")]
fn extract_full_text(doc: &Document, doc_id: Option<i64>) -> Option<String> {
    let page_count = doc.page_count().ok()?;
    let opts = TextExtractOptions::default();
    let mut text = String::new();

    for i in 0..page_count.min(5) {
        let page = doc.load_page(i).ok()?;
        let page_text = page.text(opts.clone()).ok()?;
        text.push_str(&page_text);
        text.push('\n');
    }

    if text.is_empty() {
        None
    } else {
        // 调试：保存提取的文本到临时文件
        let id_str = doc_id.map(|d| d.to_string()).unwrap_or_else(|| "unknown".to_string());
        let debug_dir = std::env::temp_dir().join("okb_assist");
        let _ = std::fs::create_dir_all(&debug_dir);
        let debug_path = debug_dir.join(format!("mupdf_text_{}.txt", id_str));
        if let Ok(mut f) = std::fs::File::create(&debug_path) {
            use std::io::Write;
            let _ = f.write_all(text.as_bytes());
            tracing::debug!("mupdf 提取文本已保存到: {:?}", debug_path);
        }
        tracing::debug!("mupdf 提取文本内容:\n{}", text);
        Some(text)
    }
}

fn extract_full_text_from_path(path: &Path, doc_id: Option<i64>) -> Option<String> {
    let mutool_path = crate::settings::get_settings().mutool_path();
    let output = Command::new(&mutool_path)
        .args(["draw", "-q", "-F", "text", path.to_str()?])
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    // 过滤可能残留的错误信息行
    let text: String = stdout
        .lines()
        .filter(|line| !line.starts_with("system error") && !line.starts_with("cannot "))
        .collect::<Vec<&str>>()
        .join("\n");

    if text.trim().is_empty() {
        None
    } else {
        // 调试：保存提取的文本到临时文件
        let id_str = doc_id.map(|d| d.to_string()).unwrap_or_else(|| "unknown".to_string());
        let debug_dir = std::env::temp_dir().join("okb_assist");
        let _ = std::fs::create_dir_all(&debug_dir);
        let debug_path = debug_dir.join(format!("mutool_text_{}.txt", id_str));
        if let Ok(mut f) = std::fs::File::create(&debug_path) {
            use std::io::Write;
            let _ = f.write_all(text.as_bytes());
            tracing::debug!("mutool 提取文本已保存到: {:?}", debug_path);
        }
        tracing::debug!("mutool 提取文本内容:\n{}", text);
        Some(text)
    }
}

// ── 版面感知的元数据抽取 ─────────────────────────────────
//
// 输入两路信息：
//   - `lines`：**首页**逐行文本（含字号与坐标；纯文本引擎时字号为 0，退化为行序）
//   - `plain`：前 5 页纯文本（引用格式、DOI、摘要、关键词、年份兜底都靠它）
//
// 抽取优先级（可靠性从高到低），先到先得、后面的只补空：
//   1. `引用格式：作者，等．题名[J]．刊名，年，卷（期）：起-止`（结构化，最准）
//   2. 显式字段标签 `题目：` / `作者：` / `DOI：`（期刊「网络首发」封面版式）
//   3. 版面推断：首页最大字号行 = 题名；次大且形似人名的行 = 作者（按与题名的距离取最近）；
//      短中文行 + 相邻英文行 = 刊名（中文刊名 / 英文刊名）
//   4. 行序兜底：没有字号信息时按文本行顺序猜
//
// 与旧实现的区别：旧版只看「第一行像不像刊头」和「有没有引用格式」，遇到
// （a）网络首发封面、（b）题名排在摘要之后、（c）多行题名/作者被拆行 就会抽错。
// 新版改为「按字号分档 + 与题名的版面距离」定位，必要时回落到标签/引用格式。

use serde_json::Value;
use std::sync::OnceLock;

/// 首页的一行文本（合并了同一基线上的多段文字）
#[derive(Debug, Clone, Default)]
pub(crate) struct TextLine {
    text: String,
    /// 该行最大字号；`0.0` 表示「未知」（纯文本引擎）
    size: f32,
    /// PDF 坐标（越大越靠上），只用于比较相对距离
    y: f32,
}

/// 编译一次、复用多次的正则
fn re<'a>(cell: &'a OnceLock<regex::Regex>, pattern: &str) -> &'a regex::Regex {
    cell.get_or_init(|| regex::Regex::new(pattern).expect("内置正则应可编译"))
}

static RE_TITLE_LABEL: OnceLock<regex::Regex> = OnceLock::new();
static RE_AUTHOR_LABEL: OnceLock<regex::Regex> = OnceLock::new();
static RE_DOI_LABEL: OnceLock<regex::Regex> = OnceLock::new();
static RE_CIT_LABEL: OnceLock<regex::Regex> = OnceLock::new();
static RE_CIT_BREAK: OnceLock<regex::Regex> = OnceLock::new();
static RE_CIT_DONE: OnceLock<regex::Regex> = OnceLock::new();
static RE_DOI_HEAD: OnceLock<regex::Regex> = OnceLock::new();
static RE_CIT_JOURNAL: OnceLock<regex::Regex> = OnceLock::new();
static RE_CIT_VOL: OnceLock<regex::Regex> = OnceLock::new();
static RE_CIT_VOL2: OnceLock<regex::Regex> = OnceLock::new();
static RE_CIT_PAGES: OnceLock<regex::Regex> = OnceLock::new();
static RE_CIT_VOL_DONE: OnceLock<regex::Regex> = OnceLock::new();
static RE_CIT_AUTHOR_TITLE: OnceLock<regex::Regex> = OnceLock::new();
static RE_CIT_NAME_TAIL: OnceLock<regex::Regex> = OnceLock::new();
static RE_DOI_PLAIN: OnceLock<regex::Regex> = OnceLock::new();
static RE_DOI_SPACED: OnceLock<regex::Regex> = OnceLock::new();
static RE_DOI_GLUE: OnceLock<regex::Regex> = OnceLock::new();
static RE_DOI_EXACT: OnceLock<regex::Regex> = OnceLock::new();
static RE_MASK_LINE: OnceLock<regex::Regex> = OnceLock::new();
static RE_MASK_DOI: OnceLock<regex::Regex> = OnceLock::new();
static RE_MASK_CODE: OnceLock<regex::Regex> = OnceLock::new();
static RE_YEAR_MONTH: OnceLock<regex::Regex> = OnceLock::new();
static RE_YEAR_DASH: OnceLock<regex::Regex> = OnceLock::new();
static RE_YEAR_PAREN: OnceLock<regex::Regex> = OnceLock::new();
static RE_YEAR_COPY: OnceLock<regex::Regex> = OnceLock::new();
static RE_YEAR_EN: OnceLock<regex::Regex> = OnceLock::new();
static RE_EN_JOURNAL: OnceLock<regex::Regex> = OnceLock::new();
static RE_ABS_CN: OnceLock<regex::Regex> = OnceLock::new();
static RE_ABS_EN: OnceLock<regex::Regex> = OnceLock::new();
static RE_KW_CN: OnceLock<regex::Regex> = OnceLock::new();
static RE_KW_EN: OnceLock<regex::Regex> = OnceLock::new();
static RE_STOP_LINE: OnceLock<regex::Regex> = OnceLock::new();
static RE_NAME_CHUNK: OnceLock<regex::Regex> = OnceLock::new();
static RE_NAME_REJECT: OnceLock<regex::Regex> = OnceLock::new();
static RE_LATIN_NAME: OnceLock<regex::Regex> = OnceLock::new();
static RE_LATIN_JOURNAL: OnceLock<regex::Regex> = OnceLock::new();
static RE_REFERENCE_HEAD: OnceLock<regex::Regex> = OnceLock::new();

/// 月份英文名（用于 `December 2024` / `(2026)` 这类日期行）
const MONTHS: &str = "January|February|March|April|May|June|July|August|September|October|November|December|Jan|Feb|Mar|Apr|Jun|Jul|Aug|Sep|Sept|Oct|Nov|Dec";

const NAME_BAD: &[&str] = &[
    "大学",
    "学院",
    "研究所",
    "研究院",
    "实验室",
    "公司",
    "集团",
    "部队",
    "中心",
    "学会",
    "编辑部",
    "委员会",
    "前沿",
    "专题",
    "报道",
    "综述",
    "论文",
    "报告",
    "观点",
    "论坛",
    "聚焦",
    "关注",
    "观察",
    "评论",
    "摘要",
    "关键词",
    "引言",
    "本章",
    "目录",
    "期刊",
    "学报",
    "作者",
    "通讯",
    "通信",
    "编辑",
    "责任",
    "投稿",
    "引用",
    "日期",
    "收稿",
    "题目",
    "标题",
    "英文",
    "中文",
    "图表",
    "万方",
    "学号",
    "密级",
    "导师",
    "教师",
    "姓名",
    "申请",
    "学科",
    "专业",
    "学位",
    "教授",
    "学校",
    "代码",
    "编号",
    "分类",
    "引言",
    "编委",
    "责任编辑",
    "博士研究生",
    "硕士研究生",
    "在线",
    "满足",
    "公开",
    "中国",
    "职称",
    "名称",
    "工学",
    "硕士",
    "博士",
    "研究方向",
    "完成时间",
    "培养",
    "学制",
    "系别",
    "保密",
    "卷",
    "期",
    "页码",
    "附表",
    "附图",
    "研究",
    "评述",
    "姓名",
    "工作单位",
    "所在单位",
    "单位",
    "答辩",
    "委员",
    "角色",
    "目前",
    "国家",
    "电子",
    "发表",
    "cnki",
    "asus",
    "知网",
    "微信",
    "二维码",
];

const LATIN_STOP: &[&str] = &[
    "abstract",
    "keywords",
    "review",
    "introduction",
    "editorial",
    "article",
    "research",
    "summary",
    "conclusion",
    "references",
    "corresponding",
    "received",
    "accepted",
    "citation",
    "doi",
    "issn",
    "university",
    "institute",
    "college",
    "laboratory",
    "academy",
    "school",
    "department",
    "faculty",
    "acta",
    "sinica",
    "aerial",
    "vehicles",
    "engagement",
    "technology",
    "science",
    "engineering",
];

/// 明显不是题名的整行（学位论文封面、版权页、栏目标签等）。匹配时会先去掉空格
const JUNK_TITLES: &[&str] = &[
    "学位论文",
    "网络首发论文",
    "专题报道",
    "研究与探索",
    "科工前沿",
    "目录",
    "稿约",
    "征稿",
    "版权声明",
    "作者简介",
    "主办单位",
    "征订",
    "万方数据",
    "分类号",
    "学号",
    "密级",
    "单位代码",
    "学校代码",
    "学校编码",
    "论文题目",
    "专业名称",
    "学科专业",
    "指导教师",
    "申请学位",
    "图书分类号",
    "在线阅读",
    "viewonline",
    "全文下载",
    "masterthesis",
    "mastersthesis",
    "doctoraldissertation",
    "dissertation",
    "学术学位",
    "专业学位",
    "递交日期",
    "培养方式",
    "研究方向",
    "完成时间",
    "图书在版编目",
    "udc",
    "引言",
    "英文论文题目",
];

const KEYWORD_BAD: &[&str] = &[
    "作者",
    "通信",
    "基金",
    "收稿",
    "录用",
    "大学",
    "学院",
    "研究所",
    "研究院",
    "E-mail",
    "http",
    "DOI",
    "中图法",
    "中图分类",
    "文献标志",
];

pub(crate) fn is_han(c: char) -> bool {
    matches!(
        c as u32,
        0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xF900..=0xFAFF
    )
}

pub(crate) fn contains_han(s: &str) -> bool {
    s.chars().any(is_han)
}

/// 去掉所有空白（中文版式里经常出现「硕 士 学 位 论 文」这种字间距）
/// 以及私有区占位字符（U+E000..U+F8FF，字体映射残留，不可见但会把关键词打断）
fn compact(s: &str) -> String {
    s.chars()
        .filter(|c| !c.is_whitespace() && !matches!(*c as u32, 0xE000..=0xF8FF))
        .collect()
}

/// 文本里「正常字符」的占比。字体没带 ToUnicode 的 PDF 会抽出成片乱码，
/// 这类行既不能当题名也不能当作者。
/// 文本里「实义字符」（汉字 / 英文字母 / 数字）的占比。
/// 字体没带 ToUnicode 的 PDF 会抽出成片乱码（大量符号码位），
/// 这类行既不能当题名也不能当作者。
fn content_char_ratio(s: &str) -> f32 {
    let total = char_count(s);
    if total == 0 {
        return 0.0;
    }
    let content = s
        .chars()
        .filter(|c| is_han(*c) || c.is_ascii_alphanumeric())
        .count();
    content as f32 / total as f32
}

pub(crate) fn is_garbage_text(s: &str) -> bool {
    // 用去空白后的文本算占比（中文版式的宽间距不应被当成乱码）
    content_char_ratio(&compact(s)) < 0.55
}

pub(crate) fn char_count(s: &str) -> usize {
    s.chars().count()
}

pub(crate) fn truncate_chars(s: &str, max: usize) -> String {
    if char_count(s) <= max {
        return s.to_string();
    }
    s.chars().take(max).collect()
}

pub(crate) fn collapse_spaces(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut last_space = false;
    for c in s.chars() {
        if c.is_whitespace() {
            if !last_space {
                out.push(' ');
            }
            last_space = true;
        } else {
            out.push(c);
            last_space = false;
        }
    }
    out.trim().to_string()
}

// ── 版面解析（mutool draw -F stext） ─────────────────────

/// 用 `mutool draw -F stext` 读首页逐行版面信息（出错时返回空表，后续自动退化）
fn extract_layout_from_path(path: &Path) -> Vec<TextLine> {
    let Some(path_str) = path.to_str() else {
        return Vec::new();
    };
    let mutool_path = crate::settings::get_settings().mutool_path();
    let output = match Command::new(&mutool_path)
        .args(["draw", "-q", "-F", "stext", "-o", "-", path_str, "1"])
        .output()
    {
        Ok(o) if o.status.success() => o,
        _ => return Vec::new(),
    };
    let xml = String::from_utf8_lossy(&output.stdout);
    parse_stext_layout(&xml)
}

/// 从 stext XML 里逐行取出「文本 + 最大字号 + bbox 左上角」
fn parse_stext_layout(xml: &str) -> Vec<TextLine> {
    let mut out = Vec::new();
    let mut cursor = 0usize;
    while let Some(rel) = xml[cursor..].find("<line") {
        let start = cursor + rel;
        // 本 `<line>` 元素的范围：到下一个 `<line` 之前
        let seg_end = match xml[start + 5..].find("<line") {
            Some(n) => start + 5 + n,
            None => xml.len(),
        };
        let seg = &xml[start..seg_end];
        cursor = seg_end.max(start + 5);

        let Some(raw_text) = attr_value(seg, "text") else {
            continue;
        };
        let text = normalize_fullwidth(&unescape_xml(&raw_text)).trim().to_string();
        if text.is_empty() {
            continue;
        }
        let y = attr_value(seg, "bbox")
            .and_then(|bbox| bbox.split_whitespace().nth(1).map(|v| v.to_string()))
            .and_then(|v| v.parse::<f32>().ok())
            .unwrap_or(0.0);

        // 一行里可能有多个 <font>，取最大字号（大字号决定它是不是题名）
        let mut size: f32 = 0.0;
        let mut from = 0usize;
        while let Some(pos) = seg[from..].find("size=\"") {
            let vstart = from + pos + "size=\"".len();
            let Some(vend_rel) = seg[vstart..].find('"') else {
                break;
            };
            if let Ok(v) = seg[vstart..vstart + vend_rel].parse::<f32>() {
                if v > size {
                    size = v;
                }
            }
            from = vstart + vend_rel + 1;
        }

        out.push(TextLine { text, size, y });
    }
    out
}

fn attr_value(s: &str, name: &str) -> Option<String> {
    let key = format!("{}=\"", name);
    let pos = s.find(&key)?;
    let vstart = pos + key.len();
    let vend = s[vstart..].find('"')?;
    Some(s[vstart..vstart + vend].to_string())
}

/// 还原 XML 实体（mutool 会把 `&` `<` 等转义）
fn unescape_xml(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(pos) = rest.find('&') {
        out.push_str(&rest[..pos]);
        let tail = &rest[pos..];
        let semi = match tail.find(';') {
            Some(p) if p <= 10 => p,
            _ => {
                out.push('&');
                rest = &tail[1..];
                continue;
            }
        };
        let entity = &tail[1..semi];
        let ch = match entity {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            _ => {
                if let Some(hex) = entity.strip_prefix("#x").or_else(|| entity.strip_prefix("#X")) {
                    u32::from_str_radix(hex, 16).ok().and_then(char::from_u32)
                } else if let Some(dec) = entity.strip_prefix('#') {
                    dec.parse::<u32>().ok().and_then(char::from_u32)
                } else {
                    None
                }
            }
        };
        match ch {
            Some(c) => {
                out.push(c);
                rest = &tail[semi + 1..];
            }
            None => {
                out.push('&');
                rest = &tail[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// 纯文本按行拆成 `TextLine`（无字号，用递减的 y 表示行序，便于复用“最近行”逻辑）
pub(crate) fn lines_from_plain_text(text: &str) -> Vec<TextLine> {
    let mut out = Vec::new();
    for (i, line) in text.lines().enumerate() {
        let t = line.trim();
        if t.is_empty() {
            continue;
        }
        out.push(TextLine {
            text: normalize_fullwidth(t),
            size: 0.0,
            y: -(i as f32),
        });
    }
    out
}

/// 把同一基线上的多段文字合并成一行（`题目：` 与它的值、被拆开的姓名、`摘`+`要：`）
fn merge_rows(lines: &[TextLine]) -> Vec<TextLine> {
    let mut rows: Vec<TextLine> = Vec::new();
    let mut main_len: Vec<usize> = Vec::new();

    for line in lines {
        let start = rows.len().saturating_sub(6);
        let mut hit: Option<usize> = None;
        for i in (start..rows.len()).rev() {
            if (rows[i].y - line.y).abs() <= 3.0 {
                hit = Some(i);
                break;
            }
        }
        match hit {
            Some(i) => {
                // 一律用空格分隔：中文姓名列表里「刘林 王散」这种被拆成两段的行，
                // 靠空格才能拆回两个名字（单个字姓名另有合并逻辑）
                // 字号取「主片段」（字符最多的那段）的字号，避免把页内装饰字符的大字号当成整行字号
                if char_count(&line.text) > main_len[i] {
                    rows[i].size = line.size;
                    main_len[i] = char_count(&line.text);
                }
                rows[i].text.push(' ');
                rows[i].text.push_str(&line.text);
            }
            None => {
                rows.push(line.clone());
                main_len.push(char_count(&line.text));
            }
        }
    }

    rows
}

/// 把被拆开的字段标签合回去（`摘` + `要：…`、`【摘` + `要】…`）
fn fix_split_markers(rows: &[TextLine]) -> Vec<TextLine> {
    let mut out: Vec<TextLine> = Vec::with_capacity(rows.len());
    let mut i = 0usize;
    while i < rows.len() {
        let cur = &rows[i];
        if i + 1 < rows.len() && cur.text.ends_with('摘') && rows[i + 1].text.starts_with('要') {
            let mut merged = rows[i + 1].clone();
            merged.text = format!("{}{}", cur.text, merged.text);
            merged.size = cur.size.max(rows[i + 1].size);
            merged.y = cur.y;
            out.push(merged);
            i += 2;
            continue;
        }
        out.push(cur.clone());
        i += 1;
    }
    out
}

// ── 摘要 / 关键词的文本块收集 ────────────────────────────

pub(crate) fn is_stop_line(text: &str) -> bool {
    re(
        &RE_STOP_LINE,
        r"^\s*[【\[(（]?\s*(?:中图分类号|中图法分类号|文献标志码|文献标识码|文章编号|DOI|Abstract|Key\s*-?\s*words?|Keywords|收稿日期|基金项目|作者简介|通信作者|引用格式|Citation|关\s*键\s*[词字]|引\s*言|[0-9]\s*引\s*言|一\s*、)",
    )
    .is_match(text)
}

/// 按行拼接成一段（中文直接相连，英文补空格，断词连字符去掉）
pub(crate) fn join_lines(parts: &[&str]) -> String {
    let mut out = String::new();
    for part in parts {
        let p = part.trim();
        if p.is_empty() {
            continue;
        }
        if out.is_empty() {
            out.push_str(p);
            continue;
        }
        let last = out.chars().last().unwrap_or(' ');
        if out.ends_with('-') && char_count(&out) > 1 {
            out.pop();
            out.push_str(p);
        } else if is_han(last) || matches!(last, '，' | '。' | '；' | '：' | '、' | '）' | '】' | '（') {
            out.push_str(p);
        } else {
            out.push(' ');
            out.push_str(p);
        }
    }
    out
}

fn collect_block(
    rows: &[TextLine],
    start: usize,
    max_lines: usize,
    max_len: usize,
    short_only: bool,
) -> String {
    let mut parts: Vec<&str> = Vec::new();
    let mut total = 0usize;
    for r in rows.iter().skip(start).take(max_lines) {
        let t = r.text.trim();
        if is_stop_line(t) {
            break;
        }
        if short_only && !(char_count(t) < 12 && !t.contains('。') && !t.contains('；') && !t.contains(';')) {
            break;
        }
        parts.push(t);
        total += char_count(t);
        if total >= max_len {
            break;
        }
    }
    join_lines(&parts)
}

fn collect_abstract_cn(rows: &[TextLine]) -> Option<String> {
    for (i, r) in rows.iter().enumerate() {
        let Some(caps) = re(&RE_ABS_CN, r"^\s*[【\[(（]?\s*摘\s*要\s*[】\])）]?\s*[:：]?\s*(.*)$")
            .captures(&r.text)
        else {
            continue;
        };
        let head = caps.get(1).map(|m| m.as_str().trim().to_string()).unwrap_or_default();
        // `摘要` 后面紧跟着另一个字段标签（双栏排版常见）就不要硬凑
        if head.is_empty() {
            if let Some(next) = rows.get(i + 1) {
                if is_stop_line(&next.text) {
                    return None;
                }
            }
        }
        let block = collect_block(rows, i + 1, 40, 6000, false);
        let rest = collapse_spaces(&format!("{} {}", head, block));
        if char_count(&rest) >= 40 {
            return Some(truncate_chars(&rest, 4000));
        }
        return None;
    }
    None
}

fn collect_abstract_en(rows: &[TextLine]) -> Option<String> {
    for (i, r) in rows.iter().enumerate() {
        let Some(caps) = re(&RE_ABS_EN, r"^\s*Abstract\s*[:：.]?\s*(.*)$").captures(&r.text) else {
            continue;
        };
        let head = caps.get(1).map(|m| m.as_str().trim().to_string()).unwrap_or_default();
        let block = collect_block(rows, i + 1, 40, 6000, false);
        let rest = collapse_spaces(&format!("{} {}", head, block));
        if char_count(&rest) >= 80 {
            return Some(truncate_chars(&rest, 4000));
        }
        return None;
    }
    None
}

pub(crate) fn split_keyword_list(body: &str, cn: bool) -> Vec<String> {
    // 先把“中文 空格 中文”的空格去掉（换行拼接留下的）
    let mut normalized = String::with_capacity(body.len());
    let chars: Vec<char> = body.chars().collect();
    for (i, c) in chars.iter().enumerate() {
        if *c == ' ' {
            let prev = if i > 0 { chars.get(i - 1) } else { None };
            let next = chars.get(i + 1);
            if matches!((prev, next), (Some(a), Some(b)) if is_han(*a) && is_han(*b)) {
                continue;
            }
        }
        normalized.push(*c);
    }
    let seps: &[char] = if cn {
        &['；', ';', '，', ',', '、', '|', '｜', '#', '！', '!', '＆', '&']
    } else {
        &['；', ';', ',']
    };
    let mut out = Vec::new();
    for item in normalized.split(|c| seps.contains(&c)) {
        let k = item
            .trim_matches(|c: char| c.is_whitespace() || c == '　')
            .trim_matches(|c: char| matches!(c, '：' | ':' | '·' | '•' | '#' | '!' | '&' | '＆'))
            .trim();
        if k.is_empty() || char_count(k) > 40 {
            continue;
        }
        if k.chars().any(|c| matches!(c, '。' | '：' | ':')) {
            continue;
        }
        if KEYWORD_BAD.iter().any(|b| k.contains(b)) {
            continue;
        }
        out.push(k.to_string());
        if out.len() >= 20 {
            break;
        }
    }
    out
}

fn collect_keywords_cn(rows: &[TextLine]) -> Option<Vec<String>> {
    for (i, r) in rows.iter().enumerate() {
        let Some(caps) = re(
            &RE_KW_CN,
            r"^\s*[【\[(（]?\s*(?:关\s*键\s*[词字])\s*[】\])）]?\s*[:：]?\s*(.*)$",
        )
        .captures(&r.text)
        else {
            continue;
        };
        let head = caps
            .get(1)
            .map(|m| m.as_str().trim().to_string())
            .unwrap_or_default();
        let tail = collect_block(rows, i + 1, 8, 300, true);
        let body = format!("{} {}", head, tail);
        let kws = split_keyword_list(&body, true);
        return if kws.is_empty() { None } else { Some(kws) };
    }
    None
}

fn collect_keywords_en(rows: &[TextLine]) -> Option<Vec<String>> {
    for (i, r) in rows.iter().enumerate() {
        let Some(caps) = re(&RE_KW_EN, r"^\s*(?:Key\s*-?\s*words?)\s*[:：.]?\s*(.*)$").captures(&r.text)
        else {
            continue;
        };
        let head = caps
            .get(1)
            .map(|m| m.as_str().trim().to_string())
            .unwrap_or_default();
        let tail = collect_block(rows, i + 1, 8, 300, true);
        let body = format!("{} {}", head, tail);
        let kws = split_keyword_list(&body, false);
        return if kws.is_empty() { None } else { Some(kws) };
    }
    None
}

// ── 人名 ────────────────────────────────────────────────

fn strip_name_marks(s: &str) -> String {
    s.chars()
        .filter(|c| !c.is_ascii_digit() && !matches!(c, '*' | '†' | '‡' | '§' | '※' | '#'))
        .collect::<String>()
        .trim()
        .to_string()
}

/// 解析作者串，返回 `(姓名列表, 合法姓名数)`；`合法姓名数 == 列表长度` 才算可信
fn parse_names(raw: &str) -> (Vec<String>, usize) {
    let mut tokens: Vec<String> = Vec::new();
    for chunk in re(&RE_NAME_CHUNK, r"[，,；;、]|\s+and\s+").split(raw) {
        let chunk = chunk.trim();
        if chunk.is_empty() {
            continue;
        }
        if contains_han(chunk) {
            // 中文名单可能用空格分隔多个作者（含「黄 侃」这种姓名中间的空格）
            for piece in chunk.split_whitespace() {
                let p = strip_name_marks(piece);
                if !p.is_empty() {
                    tokens.push(p);
                }
            }
        } else {
            // 英文名单按逗号分隔，内部空格是「名 姓」的一部分
            let p = collapse_spaces(&strip_name_marks(chunk));
            if !p.is_empty() {
                tokens.push(p);
            }
        }
    }

    // 合并被空格拆开的单字姓名（「黄 侃」→「黄侃」）
    let mut merged: Vec<String> = Vec::new();
    for t in tokens {
        let can_join = merged
            .last()
            .map(|prev| char_count(prev) == 1 && char_count(&t) == 1)
            .unwrap_or(false);
        if can_join {
            let prev = merged.last_mut().unwrap();
            let joined = format!("{}{}", prev, t);
            if joined.chars().all(is_han) {
                prev.push_str(&t);
                continue;
            }
        }
        merged.push(t);
    }

    let mut names = Vec::new();
    let mut bad = 0usize;
    let mut surname_ok = 0usize;
    let multi = merged.len() > 1;
    for t in merged {
        let n = char_count(&t);
        // 单个词成立时只认 2-4 字（5 字的「设计与实现」这类是题名续行，不是人名）
        let max_han = if multi { 5 } else { 4 };
        let han_shape = n >= 2 && n <= max_han && t.chars().all(is_han);
        let latin_name = re(&RE_LATIN_NAME, r"^[A-Z][a-z]+(?:(?:[ -][A-Z][a-z]+)){1,2}$").is_match(&t);
        if han_shape && has_common_surname(&t) {
            surname_ok += 1;
            names.push(t);
        } else if latin_name {
            surname_ok += 1;
            names.push(t);
        } else if han_shape {
            // 汉字形状对但姓氏不常见：先留着，但整行一个常见姓氏都没就当成版面碎片
            names.push(t);
        } else {
            bad += 1;
        }
    }
    let ok = if bad == 0 && surname_ok > 0 { names.len() } else { 0 };
    (names, ok)
}

/// 常见姓氏（百家姓 + 复姓）。中文姓名必须以常见姓氏开头，
/// 否则「障碍物」「与状」「评述」这种版面片段容易被当成作者。
const SURNAMES: &str = "\
王李张刘陈杨黄赵吴周徐孙马朱胡林郭何高罗郑梁谢宋唐许韩冯邓曹彭曾肖田董袁潘于蒋蔡余杜叶程苏魏吕丁任沈姚卢姜崔钟谭陆汪范金石廖贾夏韦付方白邹孟熊秦邱江尹薛闫段雷侯龙史陶黎贺顾毛郝龚邵万钱严覃武戴莫孔向汤常温康施文牛樊葛邢安齐易乔伍庞颜倪庄聂章鲁岳翟殷詹申敖韦巩聂覃单季裴霍涂成苗谷盛曲翁冉骆蓝路游辛靳管柴蒙鲍华喻祁蒲房滕屈饶解牟艾尤阳时穆农司卓古吉缪简车项连芦麦褚娄窦戚岑景党宫费卜冷晏席卫米柏宗瞿桂全佟应臧闵苟邬边卞姬师和仇栾隋商刁沙荣巫寇桑郎甄丛仲虞敖明佘池查麻苑迟邝封淡楚鞠欧阳司马上官诸葛夏侯东方皇甫尉迟公孙慕容长孙宇文司徒司空令狐端木独孤南宫西门轩辕呼延万俟闻人澹台公冶宗政濮阳淳于单于太叔申屠仲孙钟离拓跋赫连仉督晋楚闫法汝鄢涂钦归海羊舌微生梁丘左丘东门南门居买台智阳衣揭甲毋帅亢况郗汲逯扈郜蔚菅楼闻仝郏鄂钭粟冼藏栗浦盖訾阚隗逄都贲官区蔺匡位诸苟恽邸苑卿惠昝仇栾隋商刁沙荣巫寇桑郎甄丛仲虞敖明佘";

pub(crate) fn has_common_surname(name: &str) -> bool {
    let chars: Vec<char> = name.chars().collect();
    if chars.len() >= 3 {
        let two: String = chars.iter().take(2).collect();
        if SURNAMES.contains(&two) {
            return true;
        }
    }
    chars
        .first()
        .map(|c| SURNAMES.contains(*c))
        .unwrap_or(false)
}

pub(crate) fn looks_like_names(text: &str) -> bool {
    let s = text.trim();
    let n = char_count(s);
    if n == 0 || n > 150 {
        return false;
    }
    if is_garbage_text(s) {
        return false;
    }
    let packed = compact(s);
    if NAME_BAD.iter().any(|b| packed.contains(b)) {
        return false;
    }
    // 「姓名:张三」「工作单位:…」这类封面字段行不是作者行
    if s.contains(':') || s.contains('：') {
        return false;
    }
    let lower = s.to_lowercase();
    if LATIN_STOP.iter().any(|w| lower.contains(w)) {
        return false;
    }
    if re(&RE_NAME_REJECT, r"摘要|关键词|中图|文献标志|文章编号|DOI|http|www|@|\d{4}").is_match(s) {
        return false;
    }
    let (names, ok) = parse_names(s);
    ok > 0 && ok == names.len() && !names.is_empty() && names.len() <= 30
}

// ── 期刊名 ──────────────────────────────────────────────

fn looks_like_journal(text: &str) -> bool {
    let s = text.trim();
    let n = char_count(s);
    if !(2..=14).contains(&n) {
        return false;
    }
    if !contains_han(s) {
        return false;
    }
    if !s
        .chars()
        .all(|c| is_han(c) || matches!(c, '：' | ':' | '(' | ')' | '（' | '）' | '·'))
    {
        return false;
    }
    const NOT_JOURNAL: &[&str] = &[
        "编委", "责任", "编辑", "收稿", "致谢", "在线", "版权", "稿约", "征订", "主办",
    ];
    if NOT_JOURNAL.iter().any(|b| s.contains(b)) {
        return false;
    }
    const BAD: &[&str] = &[
        "前沿",
        "专题",
        "报道",
        "研究与探索",
        "摘要",
        "关键词",
        "中图分类号",
        "网络首发",
        "论文",
        "评述",
        "述评",
        "专论",
        "简报",
        "进展",
        "研究论文",
        "学术论文",
        "特约稿",
        "编者按",
        "导读",
        "卷首语",
        "封面",
    ];
    !BAD.iter().any(|b| s.contains(b))
}

/// 全宽字符归一化（中文期刊 PDF 常见：全角标点、全角数字、全角空格）
pub(crate) fn normalize_fullwidth(text: &str) -> String {
    text.chars()
        .map(|c| match c {
            '\u{3000}' | '\u{00A0}' => ' ',
            '\u{2217}' => '*',
            '\u{FF01}'..='\u{FF5E}' => ((c as u32 - 0xfee0) as u8) as char,
            _ => c,
        })
        .collect()
}

/// 取第一个捕获组（卷/期/页码兜底用，调用次数很少，直接编译正则）
/// 页码范围里的「–」「⁃」「～」统一成「-」
fn normalize_pages(s: &str) -> String {
    s.chars()
        .filter(|c| !c.is_whitespace())
        .map(|c| {
            if matches!(c, '－' | '–' | '—' | '‐' | '‑' | '−' | '⁃' | '~' | '～') {
                '-'
            } else {
                c
            }
        })
        .collect()
}

fn scan_one(text: &str, pattern: &str) -> Option<String> {
    let re = regex::Regex::new(pattern).ok()?;
    re.captures(text)
        .and_then(|c| c.get(1))
        .map(|m| m.as_str().trim().to_string())
}

/// 题名前后粘上的作者串/分隔符（「…研究综述朱彦伟!,张乘铭,杨乐平」「…systems$ Longwen Liu」）
fn strip_title_tail(title: &str) -> String {
    let mut t = title.to_string();
    // 「$」「|」 之后都是别的字段
    for sep in ['$', '|'] {
        if let Some(p) = t.find(sep) {
            t.truncate(p);
        }
    }
    // 末尾「姓名!」/「姓名*」（上标标记）；只在这种带标记时才到，避免误删「…研究方法」
    if let Ok(re_tail) = regex::Regex::new(r"([\p{Han}]{2,4})\s*[!！*†‡]\s*$") {
        if let Some(caps) = re_tail.captures(&t) {
            let name = caps.get(1).map(|m| m.as_str()).unwrap_or_default();
            let start = caps.get(0).map(|m| m.start()).unwrap_or(t.len());
            if has_common_surname(name) && start > 3 {
                t.truncate(start);
            }
        }
    }
    t = t.trim().to_string();
    // 「…研究,张三,李四」这种题名与作者同行的
    if let Some(p) = t.find(|c: char| c == ',' || c == '，') {
        let head = &t[..p];
        let tail = &t[p + 1..];
        if char_count(head) >= 6 && looks_like_names(tail) {
            t = head.to_string();
        }
    }
    t.trim().to_string()
}

pub(crate) fn clean_title(title: &str) -> String {
    // 去掉控制字符与私用区字符（期刊水印/图章常被抽成 U+E000..U+F8FF）
    let filtered: String = title
        .chars()
        .filter(|c| !c.is_control() && !matches!(*c as u32, 0xE000..=0xF8FF))
        .collect();
    // 同一行里粘上来的封面/页脚字段（「…研究作者姓名:张三专 业:…」）直接切掉
    let mut t: &str = filtered.trim();
    let compacted = compact(t);
    if let Some(pos) = COVER_FIELDS
        .iter()
        .filter_map(|f| compacted.find(f))
        .min()
    {
        if pos >= 4 {
            // 用去掉空白的偏移量反推原串位置（封面字段不含空白，按字符数近似足够）
            let mut seen = 0usize;
            let mut cut = t.len();
            for (idx, ch) in t.char_indices() {
                if !ch.is_whitespace() {
                    if seen == pos {
                        cut = idx;
                        break;
                    }
                    seen += 1;
                }
            }
            t = &t[..cut];
        }
    }
    let t = t
        .trim()
        .trim_start_matches(|c: char| matches!(c, ':' | '：' | '·' | '•'))
        .trim();
    let collapsed = collapse_spaces(t);
    let cleaned = collapsed.trim_matches(|c: char| {
        matches!(c, '.' | ',' | ';' | ':' | '，' | '。' | '；' | '：' | '*' | '†' | '‡')
    });
    strip_title_tail(cleaned)
}

// ── DOI ─────────────────────────────────────────────────

/// 从文本里扫 DOI，兼容「10. 19942/j. issn. …」这种被空格拆开的排版
pub(crate) fn scan_doi(text: &str) -> Option<String> {
    if let Some(m) = re(&RE_DOI_PLAIN, r"10\.\d{4,9}/[-._;()/:A-Za-z0-9]+").find(text) {
        return normalize_doi(m.as_str());
    }
    let lines: Vec<&str> = text.lines().collect();
    for (i, line) in lines.iter().enumerate() {
        if !line.contains("10.") {
            continue;
        }
        // 排版常把 DOI 从「/」「-」处折断（`doi: 10.1360/` + `SST-2023-0410`），先拼回一行再匹配
        let joined;
        let tail_open = {
            let t = line.trim_end();
            t.ends_with('/') || t.ends_with('-') || t.ends_with('_')
        };
        let next_starts_alnum = lines
            .get(i + 1)
            .and_then(|n| n.trim().chars().next())
            .map(|c| c.is_ascii_alphanumeric())
            .unwrap_or(false);
        let probe: &str = if tail_open || (line.trim_end().ends_with('/') && next_starts_alnum) {
            joined = format!("{}{}", line, lines.get(i + 1).copied().unwrap_or(""));
            &joined
        } else {
            line
        };
        let Some(m) = re(&RE_DOI_SPACED, r"10\.\s*\d{4,9}\s*/[-._;()/:A-Za-z0-9\s]*").find(probe) else {
            continue;
        };
        let mut doi: String = m.as_str().chars().filter(|c| !c.is_whitespace()).collect();
        // 被换行粘连上来的英文标题（`…54AClustering-based…`）：数字后紧跟的英文单词前面截断
        if let Some(glue) = re(&RE_DOI_GLUE, r"\d[A-Z][a-z]{3,}").find(&doi) {
            doi.truncate(glue.start() + 1);
        }
        let doi = doi
            .trim_end_matches(|c: char| c == '.' || c == ',' || c == ';' || c == ')')
            .to_string();
        if re(&RE_DOI_EXACT, r"^10\.\d{4,9}/[-._;()/:A-Za-z0-9]{4,100}$").is_match(&doi) {
            return normalize_doi(&doi);
        }
    }
    None
}

// ── 引用格式 ────────────────────────────────────────────

/// 取「引用格式：」之后的正文（跨行拼接，遇到下一个字段标签就停）
/// 引用标签的多种写法（中文期刊常见「引用本文」「引用格式」「本文引用格式」）
const CIT_LABEL_PATTERN: &str = r"(?:引用\s*本文|引用\s*格式|本文\s*引用\s*格式|引用\s*信息|如何\s*引用|Cite\s*this\s*article(?:\s*as)?|How\s*to\s*cite|To\s*cite(?:\s*this\s*article)?|Citation)\s*[:：]?";

/// 范围分隔符：- – — ‐ ‑ − ⁃ ~ ～
const RANGE_SEP: &str = r"[-－–—‐‑−⁃~～]";

pub(crate) fn find_citation(text: &str) -> Option<String> {
    let m = re(&RE_CIT_LABEL, CIT_LABEL_PATTERN).find(text)?;
    let rest = &text[m.end()..];
    let mut body = String::new();
    for line in rest.lines().take(6) {
        let t = line.trim();
        if t.is_empty() {
            continue;
        }
        if re(&RE_CIT_BREAK, r"^(网络首发|收稿日期|基金项目|作者简介|通信作者|E-?mail|DOI)").is_match(t) {
            break;
        }
        body.push_str(t);
        if re(
            &RE_CIT_DONE,
            r"[\[［]\s*J(?:/OL)?\s*[\]］]\s*[.．][^，,．.]{2,}[.．]",
        )
        .is_match(&body)
        {
            break;
        }
        // 没有 [J] 标记的引用（只有刊名 + 年卷期）也能用：看到「年，卷（期）」就收手
        if re(&RE_CIT_VOL_DONE, r"\d{4}\s*[，,]\s*\d+\s*[(（]\s*[A-Za-z0-9]+\s*[)）]").is_match(&body) {
            break;
        }
        if body.contains("http") || re(&RE_DOI_HEAD, r"10\.\d{4,9}/").is_match(&body) {
            break;
        }
    }
    if body.trim().is_empty() {
        return None;
    }
    Some(body)
}

pub(crate) fn parse_citation_fields(body: &str) -> Vec<(String, Value)> {
    let mut out: Vec<(String, Value)> = Vec::new();

    // ── 年 / 卷 / 期 / 页码 ──
    let vol_pattern = format!(
        r"(\d{{4}})\s*[，,]\s*(\d+)\s*[(（]\s*([A-Za-z0-9]+)\s*[)）]\s*[:：]\s*(\d+\s*{sep}\s*\d+)",
        sep = RANGE_SEP
    );
    let vol_re = re(&RE_CIT_VOL, &vol_pattern);
    let vol = vol_re.captures(body);
    let vol_pos = vol.as_ref().and_then(|c| c.get(0)).map(|m| m.start());
    if let Some(caps) = vol {
        if let Ok(year) = caps[1].parse::<i64>() {
            out.push(("year".to_string(), Value::Number(year.into())));
        }
        out.push(("volume".to_string(), Value::String(caps[2].to_string())));
        out.push(("issue".to_string(), Value::String(caps[3].to_string())));
        out.push((
            "pages".to_string(),
            Value::String(normalize_pages(&caps[4])),
        ));
    } else {
        if let Some(caps) = re(
            &RE_CIT_VOL2,
            r"(\d{4})\s*[，,]\s*(\d+)\s*[(（]\s*([A-Za-z0-9]+)\s*[)）]",
        )
        .captures(body)
        {
            if let Ok(year) = caps[1].parse::<i64>() {
                out.push(("year".to_string(), Value::Number(year.into())));
            }
            out.push(("volume".to_string(), Value::String(caps[2].to_string())));
            out.push(("issue".to_string(), Value::String(caps[3].to_string())));
        }
        let pages_pattern = format!(r"[:：]\s*(\d+\s*{}\s*\d+)", RANGE_SEP);
        if let Some(caps) = re(&RE_CIT_PAGES, &pages_pattern).captures(body) {
            out.push(("pages".to_string(), Value::String(normalize_pages(&caps[1]))));
        }
    }

    // ── 刊名：优先取 [J] 后的刊名，否则取「年，卷（期）」前面的那段 ──
    let mut journal_done = false;
    if let Some(caps) = re(&RE_CIT_JOURNAL, r"[\[［]\s*J(?:/OL)?\s*[\]］]\s*[.．]\s*([^，,．.]+)")
        .captures(body)
    {
        if let Some(m) = caps.get(1) {
            // 引用格式里刊名后面常粘着网址
            let j = m.as_str().trim();
            let j = match j.find("www").or_else(|| j.find("http")) {
                Some(p) => j[..p].trim(),
                None => j,
            };
            if !j.is_empty() && !is_garbage_text(j) {
                out.push(("journal".to_string(), Value::String(j.to_string())));
                journal_done = true;
            }
        }
    }
    let head: Option<&str> = vol_pos.map(|p| &body[..p]);
    let head = head.map(|h| {
        h.trim_end_matches(|c: char| c.is_whitespace() || matches!(c, ',' | '，' | '．' | '.'))
    });
    if !journal_done {
        if let Some(head) = head {
            if let Some(sep) = head.rfind(|c: char| matches!(c, '.' | '．')) {
                let j = head[sep + 1..].trim();
                if !j.is_empty() && !is_garbage_text(j) {
                    out.push(("journal".to_string(), Value::String(j.to_string())));
                }
            }
        }
    }

    // ── 作者 + 题名 ──
    if let Some(caps) = re(&RE_CIT_AUTHOR_TITLE, r"^(.*?)[.．](.*?)[\[［]\s*J").captures(body) {
        let author_part = re(&RE_CIT_NAME_TAIL, r"等[.．]|et\s+al[.．]")
            .split(caps.get(1).map(|m| m.as_str()).unwrap_or_default())
            .next()
            .unwrap_or_default();
        if looks_like_names(author_part) {
            let (names, ok) = parse_names(author_part);
            if ok > 0 && !names.is_empty() {
                out.push(("authors".to_string(), serde_json::json!(names)));
            }
        }
        let title = clean_title(caps.get(2).map(|m| m.as_str()).unwrap_or_default());
        if char_count(&title) >= 4 {
            out.push(("title".to_string(), Value::String(title)));
        }
    } else if let Some(head) = head {
        // 无 [J] 标记：作者．题名．刊名（刊名已在 head 末尾）
        let mut parts: Vec<&str> = head
            .split(|c| c == '.' || c == '．')
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .collect();
        if parts.len() >= 3 {
            if let Some(_journal_part) = parts.pop() {
                if let Some(title_part) = parts.pop() {
                    let authors_part = parts.join(" ");
                    if looks_like_names(&authors_part) {
                        let (names, ok) = parse_names(&authors_part);
                        if ok > 0 && !names.is_empty() {
                            out.push(("authors".to_string(), serde_json::json!(names)));
                        }
                    }
                    let title = clean_title(title_part);
                    if char_count(&title) >= 4 {
                        out.push(("title".to_string(), Value::String(title)));
                    }
                }
            }
        }
    }

    out
}

// ── 年份 ────────────────────────────────────────────────

/// 去掉 DOI / 刊号 / 文章编号这类会伪装成年份的数字
fn mask_noise(text: &str) -> String {
    let mut kept = String::with_capacity(text.len());
    for line in text.lines() {
        if re(
            &RE_MASK_LINE,
            r"(?i)(ISSN|ISBN|文章编号|中图分类|中图法分类|文献标[志识]码|CN\s*\d|https?://|www\.)",
        )
        .is_match(line)
        {
            continue;
        }
        kept.push_str(line);
        kept.push('\n');
    }
    let kept = re(&RE_MASK_DOI, r"10\.\d{4,9}/\S+").replace_all(&kept, " ").to_string();
    re(&RE_MASK_CODE, r"\d{4}\s*[-–—]\s*\d{4}")
        .replace_all(&kept, " ")
        .to_string()
}

fn plausible_year(y: i64) -> bool {
    use chrono::Datelike;
    let cap = chrono::Utc::now().year() as i64 + 1;
    (1900..=cap).contains(&y)
}

/// 页面上所有「像年份」的四位数（排除 `2096-4641` 这类刊号/编号里的数字）
fn years_in(text: &str) -> Vec<i64> {
    let chars: Vec<char> = text.chars().collect();
    let mut out = Vec::new();
    let mut i = 0usize;
    while i < chars.len() {
        if !chars[i].is_ascii_digit() {
            i += 1;
            continue;
        }
        let start = i;
        while i < chars.len() && chars[i].is_ascii_digit() {
            i += 1;
        }
        if i - start == 4 {
            let s: String = chars[start..i].iter().collect();
            if let Ok(y) = s.parse::<i64>() {
                let prev = if start > 0 { chars.get(start - 1) } else { None };
                let next = chars.get(i);
                let looks_like_code = matches!(prev, Some('-') | Some('–') | Some('—'))
                    || matches!(next, Some('-') | Some('–') | Some('—'));
                if plausible_year(y) && !looks_like_code {
                    out.push(y);
                }
            }
        }
        if i == start {
            i += 1;
        }
    }
    out
}

/// 年份：引用格式 > 首页日期行（取最大）> 全文出现过的年份（取最大）> Info 字典
fn find_year(page_lines: &[TextLine], plain: &str, info_year: Option<i64>) -> Option<i64> {
    let page_text = page_lines
        .iter()
        .map(|l| l.text.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    let masked = mask_noise(&page_text);

    let mut found: Vec<i64> = Vec::new();
    let patterns: [(&OnceLock<regex::Regex>, String); 5] = [
        (&RE_YEAR_MONTH, format!(r"((?:19|20)\d{{2}})\s*年")),
        (&RE_YEAR_DASH, r"((?:19|20)\d{2})\s*[.\-/]\s*\d{1,2}".to_string()),
        (&RE_YEAR_EN, format!(r"(?:{})\s*,?\s+((?:19|20)\d{{2}})", MONTHS)),
        (&RE_YEAR_PAREN, r"[（(]\s*((?:19|20)\d{2})\s*[)）]".to_string()),
        (&RE_YEAR_COPY, r"(?i)[©®]\s*((?:19|20)\d{2})".to_string()),
    ];
    for (cell, pattern) in patterns {
        for m in re(cell, &pattern).captures_iter(&masked) {
            if let Some(y) = m.get(1).and_then(|g| g.as_str().parse::<i64>().ok()) {
                if plausible_year(y) {
                    found.push(y);
                }
            }
        }
    }
    // 「第 9 卷 … 2026 年 6 月」这类行被上面的模式覆盖；剩下的靠裸年份兜底
    if found.is_empty() {
        found = years_in(&masked);
    }
    if let Some(max) = found.iter().max().copied() {
        return Some(pick_year(max, info_year));
    }

    let plain_years = years_in(&mask_noise(plain));
    if let Some(max) = plain_years.iter().max().copied() {
        return Some(pick_year(max, info_year));
    }
    info_year
}

/// 正文年份与 CreationDate 相差超过 5 年时，正文里的年份多半来自参考文献，改信 CreationDate
fn pick_year(text_year: i64, info_year: Option<i64>) -> i64 {
    match info_year {
        Some(i) if i - text_year > 5 => i,
        _ => text_year,
    }
}

/// 期刊名：中文刊名 + 相邻英文刊名（两侧行距/字号接近）
fn find_journal(raw_lines: &[TextLine], map: &mut serde_json::Map<String, Value>) {
    if map.contains_key("journal") {
        return;
    }
    for (i, r) in raw_lines.iter().enumerate() {
        if !looks_like_journal(&r.text) {
            continue;
        }
        for (j, r2) in raw_lines.iter().enumerate() {
            if i == j {
                continue;
            }
            if !re(&RE_LATIN_JOURNAL, r"^[A-Za-z][A-Za-z&:,\- ]{3,60}$").is_match(&r2.text) {
                continue;
            }
            if (r2.y - r.y).abs() > 26.0 {
                continue;
            }
            if r.size <= 0.0 || r2.size <= 0.0 {
                continue;
            }
            if !(r2.size >= r.size * 0.5 && r2.size <= r.size * 1.3) {
                continue;
            }
            map.insert("journal".to_string(), Value::String(r.text.clone()));
            map.insert("journal_en".to_string(), Value::String(r2.text.clone()));
            return;
        }
    }
    let text = raw_lines
        .iter()
        .map(|l| l.text.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    if let Some(m) = re(&RE_EN_JOURNAL, r"(Journal\s+of\s+[A-Z][A-Za-z&\- ]{3,60})").find(&text) {
        map.insert(
            "journal_en".to_string(),
            Value::String(m.as_str().trim().to_string()),
        );
    }
}

// ── 题名 / 作者 ─────────────────────────────────────────

/// 学位论文封面上的字段标签（题名里出现就意味着后面是别的字段）
const COVER_FIELDS: &[&str] = &[
    "作者姓名",
    "姓名",
    "作者学号",
    "指导教师",
    "导师",
    "学科专业",
    "专业名称",
    "申请学位",
    "培养方式",
    "研究方向",
    "完成时间",
    "递交日期",
    "工作单位",
    "所在单位",
    "答辩",
    "委员",
    "分类号",
    "单位代码",
    "学校代码",
    "密级",
];

/// 学位论文封面字段的前缀（去掉后剩下的就是值）
const COVER_LABEL_PREFIXES: &[&str] = &[
    "中文论文题目",
    "论文题目",
    "论文名称",
    "题目",
    "申请人姓名",
    "学生姓名",
    "作者姓名",
    "姓名",
    "指导教师",
    "合作导师",
    "导师",
    "学科专业",
    "专业学位类别",
    "专业学位领域",
    "专业名称",
    "所在学院",
    "学院",
    "学号",
    "密级",
    "分类号",
    "单位代码",
    "学校代码",
    "学校编码",
    "研究方向",
    "培养方式",
    "完成时间",
    "论文递交日期",
    "递交日期",
    "答辩日期",
];

/// 去掉「论文题目」「作者姓名:」这类封面字段前缀（后接冒号/空格都行）
fn strip_cover_prefix(text: &str) -> String {
    let t = text.trim_start();
    for prefix in COVER_LABEL_PREFIXES {
        if let Some(rest) = t.strip_prefix(prefix) {
            let rest = rest
                .trim_start_matches(|c: char| c.is_whitespace() || matches!(c, ':' | '：' | '.' | '·'));
            if !rest.is_empty() {
                return rest.to_string();
            }
        }
    }
    t.to_string()
}

/// 去掉行首的「作 者：」「题 目：」等标签（允许字间空格），返回剩下部分
fn strip_row_labels(text: &str) -> String {
    let t = strip_cover_prefix(text);
    for pattern in [
        r"^\s*作\s*者\s*[:：]?\s*",
        r"^\s*(?:论\s*文\s*)?(?:题\s*目|标\s*题|名\s*称)\s*[:：]?\s*",
    ] {
        if let Ok(re_label) = regex::Regex::new(pattern) {
            if let Some(m) = re_label.find(&t) {
                if m.start() == 0 && m.end() < t.len() {
                    return t[m.end()..].trim().to_string();
                }
            }
        }
    }
    t
}

fn is_title_candidate(text: &str) -> bool {
    let t = text.trim();
    // 单行超过 80 字的多半是正文段落，不是题名
    if char_count(t) < 4 || char_count(t) > 80 {
        return false;
    }
    if compact(t).chars().all(|c| c.is_ascii_digit()) {
        return false;
    }
    if is_garbage_text(t) {
        return false;
    }
    let c = compact(t).to_lowercase();
    if JUNK_TITLES.iter().any(|j| c.contains(&j.to_lowercase())) {
        return false;
    }
    // 封面字段行（「学号 1992...」「导师:陈钢」）与网址
    if COVER_FIELDS.iter().any(|f| c.contains(f)) {
        return false;
    }
    if c.contains("http") || c.contains("www.") || c.contains("doi:") {
        return false;
    }
    // 纯日期行（「2023 年12 月」）
    if let Ok(re_date) = regex::Regex::new(r"^\d{4}年\d{0,2}月(\d{0,2}日)?(\d{0,2}:\d{2})?$") {
        if re_date.is_match(&c) {
            return false;
        }
    }
    // 作者行也不是题名
    if looks_like_names(t) {
        return false;
    }
    // 学校/院所名不是题名（学位论文封面上它常常是最大字号）
    if char_count(&c) <= 15
        && (c.ends_with("大学")
            || c.ends_with("大學")
            || c.ends_with("学院")
            || c.ends_with("學院")
            || c.ends_with("研究院"))
    {
        return false;
    }
    // 全大写的英文行是刊头/校名，不是题名
    let has_lower = t.chars().any(|c| c.is_ascii_lowercase());
    let alpha = t.chars().filter(|c| c.is_ascii_alphabetic()).count();
    if !has_lower && alpha >= 6 {
        return false;
    }
    true
}

/// 版面推断题名：字号最大的那一档（同档多行按文本顺序拼接）
fn title_from_layout(rows: &[TextLine]) -> Option<(String, Vec<usize>)> {
    if !rows.iter().any(|r| r.size > 0.0) {
        return None;
    }
    // 按字号从大到小依次尝试：第一个「合格行」作题名起点，再向后续接同量级行
    // （多行题名常常差 0.x pt，按字号分档会被拆开，所以用 0.7 倍比例续接）
    let mut order: Vec<usize> = (0..rows.len()).collect();
    order.sort_by(|&a, &b| {
        rows[b]
            .size
            .partial_cmp(&rows[a].size)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    for &i in &order {
        if !is_title_candidate(&rows[i].text) {
            continue;
        }
        let mut idxs = vec![i];
        for j in (i + 1)..(i + 4).min(rows.len()) {
            if !is_title_candidate(&rows[j].text) {
                break;
            }
            if rows[j].size <= 0.0 || rows[j].size < rows[i].size * 0.7 {
                break;
            }
            // 重复排版（描边字/叠印）会给出同一行两遍
            if idxs.iter().any(|&k| rows[k].text.trim() == rows[j].text.trim()) {
                continue;
            }
            idxs.push(j);
        }
        let parts: Vec<&str> = idxs.iter().map(|&k| rows[k].text.as_str()).collect();
        let title = clean_title(&join_lines(&parts));
        if char_count(&title) < 4 {
            continue;
        }
        if !contains_han(&title) && title.split_whitespace().count() < 2 {
            continue;
        }
        return Some((title, idxs));
    }
    None
}

/// 版面推断作者：字号小于题名、形似人名，且离题名最近的那一行
fn authors_from_layout(rows: &[TextLine], title_idxs: &[usize]) -> Option<Vec<String>> {
    if title_idxs.is_empty() {
        return None;
    }
    let ty = title_idxs.iter().map(|&i| rows[i].y).sum::<f32>() / title_idxs.len() as f32;
    let tsize = title_idxs
        .iter()
        .map(|&i| rows[i].size)
        .fold(0.0f32, f32::max);
    // 中文文献的英文作者行比中文作者行更靠外，排序时给纯英文行一点惩罚
    let title_han = title_idxs
        .iter()
        .any(|&i| contains_han(&rows[i].text));
    let mut cands: Vec<(u8, f32, usize)> = Vec::new();
    for (i, r) in rows.iter().enumerate() {
        if title_idxs.contains(&i) {
            continue;
        }
        if tsize > 0.0 && !(r.size >= tsize * 0.30 && r.size < tsize * 0.99) {
            continue;
        }
        if looks_like_names(&r.text) {
            let latin_penalty = if title_han && !contains_han(&r.text) { 1 } else { 0 };
            cands.push((latin_penalty, (r.y - ty).abs(), i));
        }
    }
    cands.sort_by(|a, b| {
        a.0.cmp(&b.0)
            .then(a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))
    });
    for (_p, _d, i) in cands {
        let (names, ok) = parse_names(&rows[i].text);
        if ok > 0 && !names.is_empty() {
            return Some(names);
        }
    }
    None
}

/// 行序兜底题名：跳过刊头/日期/基金行，取最长的一段中文行
fn title_from_text_order(plain: &str, filename: Option<&str>) -> Option<String> {
    let lines: Vec<&str> = plain
        .lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty())
        .collect();
    let mut best: Option<(usize, String)> = None;
    for line in lines.iter().take(40) {
        let line = &strip_cover_prefix(line);
        if is_stop_line(line) || is_header_like(line) {
            continue;
        }
        let han = line.chars().filter(|c| is_han(*c)).count();
        if han < 4 || char_count(line) > 120 {
            continue;
        }
        if line.chars().any(|c| matches!(c, '。' | '；' | '，' | ',')) {
            continue;
        }
        if looks_like_placeholder(line) || looks_like_filename(line, filename) {
            continue;
        }
        if !is_title_candidate(line) {
            continue;
        }
        if best.as_ref().map(|(n, _)| han > *n).unwrap_or(true) {
            best = Some((han, line.to_string()));
        }
    }
    best.map(|(_, t)| clean_title(&t))
}

fn is_header_like(line: &str) -> bool {
    let lower = line.to_lowercase();
    if lower.contains("issn")
        || lower.contains("doi")
        || lower.contains("http")
        || lower.contains("journal")
        || lower.contains("vol")
    {
        return true;
    }
    const KEYS: &[&str] = &[
        "学报",
        "卷",
        "期",
        "网络首发",
        "收稿",
        "基金",
        "作者简介",
        "通信作者",
        "引用格式",
        "citation",
        "中图分类号",
        "文献标志码",
        "文章编号",
        "年",
        "月",
        "专题",
        "栏目",
    ];
    if KEYS.iter().any(|k| line.contains(k)) {
        return true;
    }
    // 纯数字 / 纯标点 / 页码
    line.chars().all(|c| c.is_ascii_digit() || c.is_ascii_punctuation() || c.is_whitespace())
}

/// 行序兜底作者：与题名行最近、且形似人名的一行
fn authors_from_text(plain: &str, title: Option<&str>) -> Option<Vec<String>> {
    let lines: Vec<&str> = plain
        .lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty())
        .collect();
    let anchor = title
        .filter(|t| !t.is_empty())
        .and_then(|t| {
            lines.iter().position(|l| {
                let l = l.trim();
                !l.is_empty() && (t.contains(l) || l.contains(t))
            })
        })
        .unwrap_or(0);
    let mut cands: Vec<(usize, usize)> = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        if Some(i) == Some(anchor) {
            continue;
        }
        if is_stop_line(line) || is_header_like(line) {
            continue;
        }
        if !looks_like_names(line) {
            continue;
        }
        cands.push((i.abs_diff(anchor), i));
    }
    cands.sort();
    for (_d, i) in cands {
        let (names, ok) = parse_names(lines[i]);
        if ok > 0 && !names.is_empty() {
            return Some(names);
        }
    }
    None
}

// ── 驱动 ────────────────────────────────────────────────

fn apply_labels(rows: &[TextLine], map: &mut serde_json::Map<String, Value>) {
    for r in rows {
        let t = r.text.as_str();
        if let Some(caps) = re(
            &RE_TITLE_LABEL,
            r"^(?:论\s*文\s*)?(?:题\s*目|标\s*题|名\s*称)\s*[:：]\s*(.+)$",
        )
        .captures(t)
        {
            if let Some(m) = caps.get(1) {
                let title = clean_title(m.as_str());
                if char_count(&title) >= 2 {
                    map.insert("title".to_string(), Value::String(title));
                }
            }
        }
        if !map.contains_key("authors") {
            if let Some(caps) = re(&RE_AUTHOR_LABEL, r"^作\s*者\s*[:：]\s*(.+)$").captures(t) {
                if let Some(m) = caps.get(1) {
                    if looks_like_names(m.as_str()) {
                        let (names, ok) = parse_names(m.as_str());
                        if ok > 0 && !names.is_empty() {
                            map.insert("authors".to_string(), serde_json::json!(names));
                        }
                    }
                }
            }
        }
        if let Some(caps) = re(&RE_DOI_LABEL, r"^(?:DOI|doi)\s*[:：]\s*(10\.\S+)$").captures(t) {
            if let Some(m) = caps.get(1) {
                if let Some(doi) = normalize_doi(m.as_str()) {
                    map.insert("doi".to_string(), Value::String(doi));
                }
            }
        }
    }
}

/// 截掉参考文献部分：参考文献里的 DOI / 年份很容易被误当成文献自己的
pub(crate) fn head_before_references(text: &str) -> &str {
    match re(&RE_REFERENCE_HEAD, r"(?mi)^\s*(?:参\s*考\s*文\s*献|references\b)").find(text) {
        Some(m) => &text[..m.start()],
        None => text,
    }
}

/// 主入口：`lines` 为首页行（可含字号），`plain` 为前 5 页纯文本
pub(crate) fn analyze_metadata(
    lines: &[TextLine],
    plain: &str,
    filename: Option<&str>,
    info_year: Option<i64>,
) -> serde_json::Map<String, Value> {
    let mut map = serde_json::Map::new();
    let plain_meta = head_before_references(plain);
    // 先去掉学位论文封面字段与「作 者：」「题 目：」这类行内标签，后面全部分析都受益
    let rows: Vec<TextLine> = fix_split_markers(&merge_rows(lines))
        .into_iter()
        .map(|mut r| {
            r.text = strip_row_labels(&r.text);
            r
        })
        .collect();
    let body = rows
        .iter()
        .map(|r| r.text.as_str())
        .collect::<Vec<_>>()
        .join("\n");

    // 1. DOI
    if let Some(doi) = scan_doi(&body).or_else(|| scan_doi(plain_meta)) {
        map.insert("doi".to_string(), Value::String(doi));
    }

    // 2. 引用格式（结构化程度最高，先来先占）
    if let Some(citation) = find_citation(&body).or_else(|| find_citation(plain_meta)) {
        for (k, v) in parse_citation_fields(&citation) {
            map.entry(k).or_insert(v);
        }
    }

    // 3. 显式字段标签（网络首发封面）
    apply_labels(&rows, &mut map);

    // 4. 题名：版面字号最大档；没有字号信息则按月/年行序兜底
    let layout_title = title_from_layout(&rows);
    let title_rows: Vec<usize> = match &layout_title {
        Some((_t, idxs)) => idxs.clone(),
        None => Vec::new(),
    };
    if !map.contains_key("title") {
        if let Some((title, _)) = layout_title.clone() {
            map.insert("title".to_string(), Value::String(title));
        } else if let Some(title) = title_from_text_order(plain_meta, filename) {
            map.insert("title".to_string(), Value::String(title));
        }
    }

    // 5. 作者：版面（次大字号 + 离题名最近）→ 行序兜底
    if !map.contains_key("authors") {
        let names = authors_from_layout(&rows, &title_rows).or_else(|| {
            let title = map.get("title").and_then(|v| v.as_str()).map(|s| s.to_string());
            authors_from_text(plain_meta, title.as_deref())
        });
        if let Some(names) = names {
            if !names.is_empty() {
                map.insert("authors".to_string(), serde_json::json!(names));
            }
        }
    }

    // 6. 年份：引用格式已给就用它；否则按首页日期行 → 全文年份 → Info 的 CreationDate
    let year = map
        .get("year")
        .and_then(|v| v.as_i64())
        .or_else(|| find_year(&rows, plain_meta, info_year));
    if let Some(y) = year {
        map.insert("year".to_string(), Value::Number(y.into()));
    }

    // 6.1 卷 / 期 / 页码（引用格式没给时从刊头行里找）
    if !map.contains_key("volume") {
        if let Some(v) = scan_one(&body, r"(?:^|\s)第\s*(\d+)\s*卷") {
            map.insert("volume".to_string(), Value::String(v));
        }
    }
    if !map.contains_key("issue") {
        if let Some(v) = scan_one(&body, r"(?:^|\s)第\s*(\d+)\s*期") {
            map.insert("issue".to_string(), Value::String(v));
        }
    }

    // 7. 刊名（中文 + 英文）
    find_journal(lines, &mut map);

    // 8. 摘要 / 关键词：先首页版面，再全文行序
    let plain_lines = fix_split_markers(&lines_from_plain_text(plain_meta));
    if !map.contains_key("abstract") {
        if let Some(a) = collect_abstract_cn(&rows).or_else(|| collect_abstract_cn(&plain_lines)) {
            map.insert("abstract".to_string(), Value::String(a));
        }
    }
    if !map.contains_key("abstract_en") {
        if let Some(a) = collect_abstract_en(&rows).or_else(|| collect_abstract_en(&plain_lines)) {
            map.insert("abstract_en".to_string(), Value::String(a));
        }
    }
    if !map.contains_key("keywords") {
        if let Some(k) = collect_keywords_cn(&rows).or_else(|| collect_keywords_cn(&plain_lines)) {
            map.insert("keywords".to_string(), serde_json::json!(k));
        }
    }
    if !map.contains_key("keywords_en") {
        if let Some(k) = collect_keywords_en(&rows).or_else(|| collect_keywords_en(&plain_lines)) {
            map.insert("keywords_en".to_string(), serde_json::json!(k));
        }
    }

    // 9. 题名兑底：扫描件/乱码 PDF 抽不到题名时，用文件名（期刊下载的文件名一般就是「题名_作者」）
    let bad_title = match map.get("title").and_then(|v| v.as_str()) {
        Some(t) => t.trim().is_empty() || is_garbage_text(t),
        None => true,
    };
    if bad_title {
        if let Some(t) = title_from_filename(filename) {
            map.insert("title".to_string(), Value::String(t));
        }
    }

    map
}

/// 从文件名推题名（只在抽不到题名时使用）
fn title_from_filename(filename: Option<&str>) -> Option<String> {
    let f = filename?;
    let mut base = f.trim().to_string();
    if base.to_lowercase().ends_with(".pdf") {
        base.truncate(base.len() - 4);
    }
    // 尾部序号「 (1)」
    if let Ok(re_seq) = regex::Regex::new(r"\s*[(（]\d+[)）]\s*$") {
        base = re_seq.replace(&base, "").to_string();
    }
    // 开头日期数字（「20220725基于…」）
    if let Ok(re_date) = regex::Regex::new(r"^\d{4,8}(?=\p{Han})") {
        base = re_date.replace(&base, "").to_string();
    }
    // 尾部「_作者」（只去掉看起来像人名的尾缀）
    if let Some((head, tail)) = base.rsplit_once('_') {
        let tail_han = char_count(tail) >= 2 && char_count(tail) <= 5 && tail.chars().all(is_han);
        if tail_han && char_count(head) >= 6 {
            base = head.to_string();
        }
    }
    let title = collapse_spaces(&base.replace('_', " "));
    if char_count(&title) >= 6 {
        Some(title)
    } else {
        None
    }
}

// ── 通用辅助函数 ────────────────────────────────────────

fn looks_like_filename(title: &str, filename: Option<&str>) -> bool {
    let t = title.trim().to_lowercase();
    if t.ends_with(".pdf") {
        return true;
    }
    if let Some(fname) = filename {
        let norm_f = fname.to_lowercase().replace(".pdf", "").replace(&['.', '_', '-'][..], "");
        let norm_t = t.replace(&['.', '_', '-'][..], "");
        if norm_t == norm_f && !title.contains(' ') {
            return true;
        }
    }
    if !title.contains(' ')
        && title.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_')
    {
        return true;
    }
    false
}

pub(crate) fn looks_like_placeholder(title: &str) -> bool {
    let t = title.trim().to_lowercase();
    if t.is_empty() {
        return true;
    }
    let placeholders = [
        "entire document", "untitled", "document", "title", "unknown",
        "n/a", "no title", "not available", "confidential", "draft",
        "new document", "new doc",
    ];
    placeholders.contains(&t.as_str())
}

fn split_authors(raw: &str) -> Vec<String> {
    regex::Regex::new(r";|,|\s+and\s+")
        .unwrap()
        .split(raw)
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

fn split_keywords(raw: &str) -> Vec<String> {
    regex::Regex::new(r";|,")
        .unwrap()
        .split(raw)
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

fn parse_year_from_date(date_str: &str) -> Option<i64> {
    let re = regex::Regex::new(r"D:(\d{4})(\d{2})(\d{2})").unwrap();
    if let Some(caps) = re.captures(date_str) {
        if let Some(year) = caps.get(1).and_then(|m| m.as_str().parse::<i64>().ok()) {
            if (1900..=2100).contains(&year) {
                return Some(year);
            }
        }
    }
    let re2 = regex::Regex::new(r"(19|20)\d{2}").unwrap();
    re2.find(date_str)
        .and_then(|m| m.as_str().parse::<i64>().ok())
}