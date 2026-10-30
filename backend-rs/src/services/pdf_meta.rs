//! 轻量级 PDF 元数据（含 DOI）提取。
//!
//! 当启用 `mupdf` feature 时，使用内置的 `mupdf` 库提取 Info 字典和页面文本。
//! 未启用时，回退为调用外部 `mutool` 工具完成相同功能。

#[cfg(feature = "mupdf")]
use mupdf::{Document, MetadataName, TextExtractOptions};

#[cfg(not(feature = "mupdf"))]
use std::process::Command;
#[cfg(not(feature = "mupdf"))]
use tempfile::TempDir;

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

/// 异步提取 PDF 元数据（在后台线程执行）
pub async fn extract_pdf_metadata_async(content: Vec<u8>, filename: Option<String>, doc_id: Option<i64>) -> serde_json::Value {
    tokio::task::spawn_blocking(move || {
        extract_pdf_metadata(&content, filename.as_deref(), doc_id)
    })
    .await
    .unwrap_or_else(|_| serde_json::Value::Object(serde_json::Map::new()))
}

/// 从 PDF 字节内容中提取元数据
#[cfg(feature = "mupdf")]
pub fn extract_pdf_metadata(content: &[u8], filename: Option<&str>, doc_id: Option<i64>) -> serde_json::Value {
    let mut result = serde_json::Map::new();

    if let Ok(doc) = Document::from_bytes(content, "pdf") {
        // 1. 从 Info 字典提取
        extract_from_info_dict(&doc, filename, &mut result);

        // 2. 从页面文本提取（补充 Info 字典未覆盖的字段）
        let full_text = extract_full_text(&doc, doc_id);
        if let Some(ref text) = full_text {
            // 全宽字符归一化（中文 PDF 常见）
            let normalized = normalize_fullwidth(text);

            extract_from_text(&normalized, filename, &mut result);
        }
    }

    serde_json::Value::Object(result)
}

/// 从 PDF 字节内容中提取元数据（回退方案：调用外部 mutool）
#[cfg(not(feature = "mupdf"))]
pub fn extract_pdf_metadata(content: &[u8], filename: Option<&str>, doc_id: Option<i64>) -> serde_json::Value {
    let mut result = serde_json::Map::new();

    // 1. 通过 mutool 提取 Info 字典
    if let Some(info) = extract_info_dict_via_mutool(content) {
        extract_from_info_dict_parsed(&info, filename, &mut result);
    }

    // 2. 通过 mutool 提取页面文本
    if let Some(text) = extract_full_text_via_mutool(content, doc_id) {
        let normalized = normalize_fullwidth(&text);
        extract_from_text(&normalized, filename, &mut result);
    }

    serde_json::Value::Object(result)
}

// ── PDF 元信息报告（Info 字典 + 页数 + 启发式结果） ────────

/// 异步生成 PDF 元信息报告（在后台线程执行）
pub async fn extract_pdf_report_async(
    content: Vec<u8>,
    filename: Option<String>,
    doc_id: Option<i64>,
) -> serde_json::Value {
    tokio::task::spawn_blocking(move || {
        extract_pdf_report(&content, filename.as_deref(), doc_id)
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
#[cfg(feature = "mupdf")]
pub fn extract_pdf_report(content: &[u8], filename: Option<&str>, doc_id: Option<i64>) -> serde_json::Value {
    let mut report = base_report(content);

    let doc = match Document::from_bytes(content, "pdf") {
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
    report.insert("info".to_string(), serde_json::Value::Object(info));

    // 启发式抽取（复用既有逻辑）
    let mut heuristic = serde_json::Map::new();
    extract_from_info_dict(&doc, filename, &mut heuristic);
    if let Some(text) = extract_full_text(&doc, doc_id) {
        extract_from_text(&normalize_fullwidth(&text), filename, &mut heuristic);
    }
    report.insert("heuristics".to_string(), serde_json::Value::Object(heuristic));

    serde_json::Value::Object(report)
}

/// 生成 PDF 元信息报告（回退方案：调用外部 mutool）
#[cfg(not(feature = "mupdf"))]
pub fn extract_pdf_report(content: &[u8], filename: Option<&str>, doc_id: Option<i64>) -> serde_json::Value {
    let mut report = base_report(content);

    let temp_dir = match TempDir::new() {
        Ok(d) => d,
        Err(e) => {
            report.insert("error".to_string(), serde_json::json!(format!("无法创建临时目录: {e}")));
            return serde_json::Value::Object(report);
        }
    };
    let temp_path = temp_dir.path().join("report.pdf");
    if let Err(e) = std::fs::write(&temp_path, content) {
        report.insert("error".to_string(), serde_json::json!(format!("无法写入临时文件: {e}")));
        return serde_json::Value::Object(report);
    }

    // Info 字典原始键值
    let entries = mutool_show(&temp_path, &["trailer.Info"])
        .map(|out| parse_pdf_dict_entries(&out))
        .unwrap_or_default();

    let mut info = serde_json::Map::new();
    for (k, v) in &entries {
        info.insert(k.clone(), serde_json::Value::String(v.clone()));
    }
    report.insert("info".to_string(), serde_json::Value::Object(info));
    report.insert("page_count".to_string(), serde_json::json!(mutool_page_count(&temp_path)));
    report.insert("encrypted".to_string(), serde_json::json!(mutool_encrypted(&temp_path)));

    // 启发式抽取（复用既有逻辑）
    let mut heuristic = serde_json::Map::new();
    if let Some(parsed) = info_dict_from_entries(&entries) {
        extract_from_info_dict_parsed(&parsed, filename, &mut heuristic);
    }
    if let Some(text) = extract_full_text_via_mutool(content, doc_id) {
        extract_from_text(&normalize_fullwidth(&text), filename, &mut heuristic);
    }
    report.insert("heuristics".to_string(), serde_json::Value::Object(heuristic));

    serde_json::Value::Object(report)
}

/// 报告公共字段：文件大小与 PDF 版本
fn base_report(content: &[u8]) -> serde_json::Map<String, serde_json::Value> {
    let mut report = serde_json::Map::new();
    report.insert("file_size".to_string(), serde_json::json!(content.len()));
    report.insert(
        "pdf_version".to_string(),
        serde_json::json!(pdf_version_from_header(content)),
    );
    report
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

// ── mupdf 路径：Info 字典提取 ─────────────────────────────

#[cfg(feature = "mupdf")]
fn extract_from_info_dict(
    doc: &Document,
    filename: Option<&str>,
    result: &mut serde_json::Map<String, serde_json::Value>,
) {
    for (name, key) in [
        (MetadataName::Title, "title"),
        (MetadataName::Author, "author"),
        (MetadataName::Subject, "subject"),
        (MetadataName::Keywords, "keywords"),
        (MetadataName::CreationDate, "creationdate"),
    ] {
        if let Ok(val) = doc.metadata(name) {
            let val = val.trim().to_string();
            if val.is_empty() {
                continue;
            }
            match key {
                "title" => {
                    if !looks_like_filename(&val, filename) && !looks_like_placeholder(&val) {
                        result.insert("title".to_string(), serde_json::Value::String(val));
                    }
                }
                "author" => {
                    let authors = split_authors(&val);
                    result.insert("authors".to_string(), serde_json::json!(authors));
                }
                "keywords" => {
                    result.insert("keywords".to_string(), serde_json::json!(split_keywords(&val)));
                }
                "subject" => {
                    if val.len() > 80 {
                        result.insert("abstract".to_string(), serde_json::Value::String(val));
                    }
                }
                "creationdate" => {
                    if let Some(year) = parse_year_from_date(&val) {
                        result.insert("year".to_string(), serde_json::Value::Number(year.into()));
                    }
                }
                _ => {}
            }
        }
    }
}

// ── mutool 路径：Info 字典提取 ────────────────────────────

#[cfg(not(feature = "mupdf"))]
struct InfoDict {
    title: Option<String>,
    author: Option<String>,
    subject: Option<String>,
    keywords: Option<String>,
    creation_date: Option<String>,
}

#[cfg(not(feature = "mupdf"))]
fn extract_info_dict_via_mutool(content: &[u8]) -> Option<InfoDict> {
    let temp_dir = TempDir::new().ok()?;
    let temp_path = temp_dir.path().join("extract_info.pdf");
    std::fs::write(&temp_path, content).ok()?;

    let stdout = mutool_show(&temp_path, &["trailer.Info"])?;
    let entries = parse_pdf_dict_entries(&stdout);
    info_dict_from_entries(&entries)
}

/// 调用 `mutool show <pdf> <path...>` 并返回 stdout
#[cfg(not(feature = "mupdf"))]
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
#[cfg(not(feature = "mupdf"))]
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
#[cfg(not(feature = "mupdf"))]
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
#[cfg(not(feature = "mupdf"))]
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
#[cfg(not(feature = "mupdf"))]
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
#[cfg(not(feature = "mupdf"))]
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

#[cfg(not(feature = "mupdf"))]
fn extract_from_info_dict_parsed(
    info: &InfoDict,
    filename: Option<&str>,
    result: &mut serde_json::Map<String, serde_json::Value>,
) {
    if let Some(ref val) = info.title {
        let val = val.trim().to_string();
        if !val.is_empty() && !looks_like_filename(&val, filename) && !looks_like_placeholder(&val) {
            result.insert("title".to_string(), serde_json::Value::String(val));
        }
    }
    if let Some(ref val) = info.author {
        let val = val.trim().to_string();
        if !val.is_empty() {
            let authors = split_authors(&val);
            result.insert("authors".to_string(), serde_json::json!(authors));
        }
    }
    if let Some(ref val) = info.keywords {
        let val = val.trim().to_string();
        if !val.is_empty() {
            result.insert("keywords".to_string(), serde_json::json!(split_keywords(&val)));
        }
    }
    if let Some(ref val) = info.subject {
        let val = val.trim().to_string();
        if !val.is_empty() && val.len() > 80 {
            result.insert("abstract".to_string(), serde_json::Value::String(val));
        }
    }
    if let Some(ref val) = info.creation_date {
        if let Some(year) = parse_year_from_date(val) {
            result.insert("year".to_string(), serde_json::Value::Number(year.into()));
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

#[cfg(not(feature = "mupdf"))]
fn extract_full_text_via_mutool(content: &[u8], doc_id: Option<i64>) -> Option<String> {
    let temp_dir = TempDir::new().ok()?;
    let temp_path = temp_dir.path().join("extract_text.pdf");
    std::fs::write(&temp_path, content).ok()?;

    let mutool_path = crate::settings::get_settings().mutool_path();
    let output = Command::new(&mutool_path)
        .args(["draw", "-q", "-F", "text", temp_path.to_str()?])
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

// ── 文本启发式提取 ──────────────────────────────────────

fn extract_from_text(
    text: &str,
    filename: Option<&str>,
    result: &mut serde_json::Map<String, serde_json::Value>,
) {
    // DOI（优先级最高，已有则跳过）
    if !result.contains_key("doi") {
        if let Some(doi) = scan_doi(text) {
            result.insert("doi".to_string(), serde_json::Value::String(doi));
        }
    }

    // 策略1：优先从引用格式完整解析所有字段
    let citation = parse_citation(text);

    // 年份
    if !result.contains_key("year") {
        if let Some(year) = citation.as_ref().and_then(|c| c.year) {
            result.insert("year".to_string(), serde_json::Value::Number(year.into()));
        } else if let Some(year) = scan_year_from_text(text) {
            result.insert("year".to_string(), serde_json::Value::Number(year.into()));
        }
    }

    // 期刊名
    if !result.contains_key("journal") {
        if let Some(journal) = citation.as_ref().and_then(|c| c.journal.clone()) {
            result.insert("journal".to_string(), serde_json::Value::String(journal));
        } else if let Some(journal) = scan_journal(text) {
            result.insert("journal".to_string(), serde_json::Value::String(journal));
        }
    }

    // 标题
    if !result.contains_key("title") {
        if let Some(title) = citation.as_ref().and_then(|c| c.title.clone()) {
            result.insert("title".to_string(), serde_json::Value::String(title));
        } else if let Some(title) = extract_title_from_text(text, filename) {
            result.insert("title".to_string(), serde_json::Value::String(title));
        }
    }

    // 作者
    if !result.contains_key("authors") {
        if let Some(authors) = citation.as_ref().and_then(|c| c.authors.clone()) {
            if !authors.is_empty() {
                result.insert("authors".to_string(), serde_json::json!(authors));
            }
        } else if let Some(authors) = extract_authors_from_text(text) {
            result.insert("authors".to_string(), serde_json::json!(authors));
        }
    }

    // 卷
    if !result.contains_key("volume") {
        if let Some(vol) = citation.as_ref().and_then(|c| c.volume.clone()) {
            result.insert("volume".to_string(), serde_json::Value::String(vol));
        } else if let Some(vol) = scan_volume(text) {
            result.insert("volume".to_string(), serde_json::Value::String(vol));
        }
    }

    // 期
    if !result.contains_key("issue") {
        if let Some(issue) = citation.as_ref().and_then(|c| c.issue.clone()) {
            result.insert("issue".to_string(), serde_json::Value::String(issue));
        } else if let Some(issue) = scan_issue(text) {
            result.insert("issue".to_string(), serde_json::Value::String(issue));
        }
    }

    // 页码
    if !result.contains_key("pages") {
        if let Some(pages) = citation.as_ref().and_then(|c| c.pages.clone()) {
            result.insert("pages".to_string(), serde_json::Value::String(pages));
        } else if let Some(pages) = scan_pages(text) {
            result.insert("pages".to_string(), serde_json::Value::String(pages));
        }
    }
}

// ── 引用格式完整解析 ────────────────────────────────────

/// 从引用格式完整解析所有文献字段。
///
/// 中文格式：引用格式：作者，作者，等．标题［J］．期刊名，年，卷（期）：页码．
/// 英文格式：Citation: Author, et al. Title [J]. Journal, Year, Vol(Issue): Pages.
struct CitationInfo {
    authors: Option<Vec<String>>,
    title: Option<String>,
    journal: Option<String>,
    year: Option<i64>,
    volume: Option<String>,
    issue: Option<String>,
    pages: Option<String>,
}

fn parse_citation(text: &str) -> Option<CitationInfo> {
    // 中文学术期刊引用格式
    // 格式：引用格式：作者，作者，等．标题［J］．期刊名，年，卷（期）：页码．
    let re_cn = regex::Regex::new(
        r"引用\s*格式\s*[：:]\s*(.+?)\s*［J］\s*([^,，]+?)\s*[，,]\s*(\d{4})\s*[，,]\s*(\d+)\s*（(\d+)）\s*[：:]\s*(\d+\s*[－\-]\s*\d+)"
    ).unwrap();

    if let Some(caps) = re_cn.captures(text) {
        let author_part = caps.get(1)?.as_str().trim();
        let title = caps.get(2)?.as_str().trim().to_string();
        let journal = caps.get(3)?.as_str().trim().to_string();
        let year: i64 = caps.get(4)?.as_str().parse().ok()?;
        let volume = caps.get(5)?.as_str().to_string();
        let issue = caps.get(6)?.as_str().to_string();
        let pages = caps.get(7)?.as_str().to_string();

        let authors = parse_authors_from_citation_part(author_part);

        return Some(CitationInfo {
            authors: Some(authors),
            title: Some(clean_title(&title)),
            journal: Some(journal),
            year: Some(year),
            volume: Some(volume),
            issue: Some(issue),
            pages: Some(pages),
        });
    }

    // 英文学术期刊引用格式
    // 格式：Citation: Author, et al. Title [J]. Journal, Year, Vol(Issue): Pages.
    let re_en = regex::Regex::new(
        r"(?i)Citation\s*:\s*(.+?)\s*\.\s*\[J\]\s*([^,]+?)\s*[，,]\s*(\d{4})\s*[，,]\s*(\d+)\s*\((\d+)\)\s*:\s*(\d+\s*[－\-]\s*\d+)"
    ).unwrap();

    if let Some(caps) = re_en.captures(text) {
        let author_part = caps.get(1)?.as_str().trim();
        let title = caps.get(2)?.as_str().trim().to_string();
        let journal = caps.get(3)?.as_str().trim().to_string();
        let year: i64 = caps.get(4)?.as_str().parse().ok()?;
        let volume = caps.get(5)?.as_str().to_string();
        let issue = caps.get(6)?.as_str().to_string();
        let pages = caps.get(7)?.as_str().to_string();

        let authors = parse_authors_from_citation_part_en(author_part);

        return Some(CitationInfo {
            authors: Some(authors),
            title: Some(clean_title(&title)),
            journal: Some(journal),
            year: Some(year),
            volume: Some(volume),
            issue: Some(issue),
            pages: Some(pages),
        });
    }

    None
}

/// 从引用格式的作者部分解析作者列表（中文）
fn parse_authors_from_citation_part(s: &str) -> Vec<String> {
    let s = s.trim();
    let s = if let Some(pos) = s.find("等") {
        &s[..pos]
    } else {
        s
    };
    let s = if let Some(pos) = s.find("et al.") {
        &s[..pos]
    } else {
        s
    };

    s.split(|c: char| c == '，' || c == ',' || c == '、')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

/// 从引用格式的作者部分解析作者列表（英文）
fn parse_authors_from_citation_part_en(s: &str) -> Vec<String> {
    let s = s.trim();
    let s = if let Some(pos) = s.find("et al.") {
        &s[..pos]
    } else {
        s
    };

    s.split(|c: char| c == ',' || c == '、')
        .map(|s| {
            s.trim()
                .to_string()
                .chars()
                .filter(|c| c.is_alphabetic() || *c == ' ')
                .collect::<String>()
                .trim()
                .to_string()
        })
        .filter(|s| !s.is_empty())
        .collect()
}

// ── 全宽字符归一化 ──────────────────────────────────────

/// 将全宽 ASCII 字符（U+FF01–U+FF5E）转为普通 ASCII（U+21–U+7E）
fn normalize_fullwidth(text: &str) -> String {
    text.chars()
        .map(|c| {
            if ('\u{FF01}'..='\u{FF5E}').contains(&c) {
                ((c as u32 - 0xfee0) as u8) as char
            } else {
                c
            }
        })
        .collect()
}

// ── DOI 扫描（支持全宽字符） ─────────────────────────────

fn scan_doi(text: &str) -> Option<String> {
    // 先尝试标准 ASCII DOI
    let re = regex::Regex::new(r"10\.\d{4,9}/[-._;()/:A-Za-z0-9]+").unwrap();
    if let Some(m) = re.find(text) {
        return normalize_doi(m.as_str());
    }
    // 再尝试全宽字符 DOI（归一化后再匹配）
    let normalized = normalize_fullwidth(text);
    if let Some(m) = re.find(&normalized) {
        return normalize_doi(m.as_str());
    }
    None
}

// ── 年份提取 ────────────────────────────────────────────

fn scan_year_from_text(text: &str) -> Option<i64> {
    // 优先匹配 "2025年" 或 "2025" 年份模式
    let re = regex::Regex::new(r"(19|20)\d{2}\s*年?").unwrap();
    if let Some(m) = re.find(text) {
        let year_str: String = m.as_str().chars().filter(|c| c.is_ascii_digit()).collect();
        if let Ok(y) = year_str.parse::<i64>() {
            if (1900..=2100).contains(&y) {
                return Some(y);
            }
        }
    }
    None
}

// ── 期刊名提取 ──────────────────────────────────────────

fn scan_journal(text: &str) -> Option<String> {
    // 中文学报模式
    let re_cn = regex::Regex::new(r"([\u4e00-\u9fa5]{2,20}学报)").unwrap();
    if let Some(m) = re_cn.find(text) {
        let j = m.as_str().to_string();
        if !j.contains("学报学报") {
            return Some(j);
        }
    }
    // 英文学报模式
    let re_en = regex::Regex::new(r"(Journal\s+of\s+[A-Z][\w\s]+)").unwrap();
    if let Some(m) = re_en.find(text) {
        return Some(m.as_str().trim().to_string());
    }
    None
}

// ── 标题提取 ────────────────────────────────────────────

fn extract_title_from_text(text: &str, filename: Option<&str>) -> Option<String> {
    // 策略1：从"引用格式"行提取（中文学术期刊标准格式）
    if let Some(title) = extract_title_from_citation(text) {
        return Some(title);
    }

    // 策略2：从"Citation:"行提取（英文格式）
    if let Some(title) = extract_title_from_english_citation(text) {
        return Some(title);
    }

    // 策略3：从首页大标题区域提取（跳过期刊 header）
    if let Some(title) = extract_title_from_header(text, filename) {
        return Some(title);
    }

    None
}

/// 从 "引用格式：" 行提取标题
/// 格式：引用格式：作者，作者，等．标题［J］．期刊名，年，卷（期）：页码．
fn extract_title_from_citation(text: &str) -> Option<String> {
    let re = regex::Regex::new(r"引用\s*格式\s*[：:]\s*(.+?)\s*\[J\]\s*[^,，]+").unwrap();
    if let Some(caps) = re.captures(text) {
        let part = caps.get(1)?.as_str().trim().to_string();
        // 去掉前面的作者部分（最后一个 "等．" 或 "等." 之后）
        if let Some(pos) = find_last_author_separator(&part) {
            let title = part[pos..].trim().to_string();
            if !title.is_empty() && !looks_like_placeholder(&title) {
                return Some(clean_title(&title));
            }
        }
    }
    None
}

/// 从 "Citation:" 行提取标题
fn extract_title_from_english_citation(text: &str) -> Option<String> {
    let re = regex::Regex::new(r"(?i)Citation\s*:\s*.+?\.\s*([A-Z][^.]+?)\s*\[J\]").unwrap();
    if let Some(caps) = re.captures(text) {
        let title = caps.get(1)?.as_str().trim().to_string();
        if !title.is_empty() && !looks_like_placeholder(&title) {
            return Some(title);
        }
    }
    None
}

/// 从首页正文提取标题（跳过期刊 header 行）
fn extract_title_from_header(text: &str, filename: Option<&str>) -> Option<String> {
    let lines: Vec<&str> = text.lines().collect();
    // 跳过前几行期刊 header（包含 "学报"、"Journal"、"卷"、"期" 等）
    let mut start = 0;
    for (i, line) in lines.iter().enumerate() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        // 跳过明显的期刊 header 行
        if trimmed.contains("学报")
            || trimmed.to_lowercase().contains("journal")
            || trimmed.contains("卷")
            || trimmed.contains("Vol")
            || trimmed.contains("No")
            || trimmed.contains("doi")
            || trimmed.contains("http")
            || trimmed.contains("收稿")
            || trimmed.contains("基金")
            || trimmed.contains("作者")
            || trimmed.contains("通信")
            || trimmed.contains("引用")
            || trimmed.contains("Citation")
            || trimmed.chars().all(|c| c.is_ascii_punctuation() || c.is_whitespace())
        {
            continue;
        }
        start = i;
        break;
    }

    // 收集接下来的 2-3 行作为候选标题
    let candidate_lines: Vec<&str> = lines.iter()
        .skip(start)
        .take(3)
        .map(|l| l.trim())
        .filter(|l| !l.is_empty())
        .collect();

    if candidate_lines.is_empty() {
        return None;
    }

    // 合并候选行
    let candidate = candidate_lines.join(" ");

    // 过滤掉明显不是标题的行
    if candidate.len() < 10 || candidate.len() > 200 {
        return None;
    }
    if candidate.chars().all(|c| c.is_ascii_punctuation() || c.is_whitespace()) {
        return None;
    }
    if looks_like_filename(&candidate, filename) || looks_like_placeholder(&candidate) {
        return None;
    }

    Some(clean_title(&candidate))
}

/// 查找作者分隔符（"等．" 或 "et al." 之后即为标题）
fn find_author_separator(s: &str) -> Option<usize> {
    for (i, c) in s.char_indices() {
        if c == '等' {
            // 检查后面是否紧跟 "．" 或 "."
            let rest = &s[i + c.len_utf8()..];
            if rest.starts_with('．') || rest.starts_with('.') {
                return Some(i + c.len_utf8() + 1);
            }
        }
    }
    // 英文 "et al." 之后
    if let Some(pos) = s.find("et al.") {
        return Some(pos + "et al.".len());
    }
    None
}

/// 查找最后一个作者分隔符
fn find_last_author_separator(s: &str) -> Option<usize> {
    let mut last = None;
    // 中文 "等．" 或 "等."
    for (i, c) in s.char_indices() {
        if c == '等' {
            let rest = &s[i + c.len_utf8()..];
            if rest.starts_with('．') || rest.starts_with('.') {
                last = Some(i + c.len_utf8() + 1);
            }
        }
    }
    if last.is_some() {
        return last;
    }
    // 英文 "et al."
    if let Some(pos) = s.find("et al.") {
        return Some(pos + "et al.".len());
    }
    None
}

fn clean_title(title: &str) -> String {
    title.trim()
        .trim_end_matches(|c: char| c == '.' || c == ',' || c == ';' || c == '．')
        .to_string()
}

// ── 作者提取 ────────────────────────────────────────────

fn extract_authors_from_text(text: &str) -> Option<Vec<String>> {
    // 策略1：从"引用格式"行提取
    if let Some(authors) = extract_authors_from_citation(text) {
        if !authors.is_empty() {
            return Some(authors);
        }
    }

    // 策略2：从正文作者行提取（标题下方、单位上方）
    if let Some(authors) = extract_authors_from_body(text) {
        if !authors.is_empty() {
            return Some(authors);
        }
    }

    None
}

/// 从引用格式提取作者
fn extract_authors_from_citation(text: &str) -> Option<Vec<String>> {
    let re = regex::Regex::new(r"引用\s*格式\s*[：:]\s*(.+?)\s*(?:等\.|等．|et\s+al\.)").unwrap();
    if let Some(caps) = re.captures(text) {
        let author_part = caps.get(1)?.as_str().trim();
        let authors: Vec<String> = author_part
            .split(|c: char| c == '，' || c == ',' || c == '、')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        if !authors.is_empty() {
            return Some(authors);
        }
    }
    None
}

/// 从正文提取作者
fn extract_authors_from_body(text: &str) -> Option<Vec<String>> {
    // 查找标题后的作者行：通常紧跟标题，包含中文姓名，可能带单位上标数字
    let lines: Vec<&str> = text.lines().collect();
    for (i, line) in lines.iter().enumerate() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        // 跳过 header 行
        if trimmed.contains("学报") || trimmed.to_lowercase().contains("journal")
            || trimmed.contains("卷") || trimmed.contains("Vol")
            || trimmed.contains("doi") || trimmed.contains("http")
            || trimmed.contains("收稿") || trimmed.contains("基金")
        {
            continue;
        }

        // 匹配作者行模式：中文姓名 + 可选单位上标数字
        // 例如："罗亚中１，２，杨 震１，２，王 华１，２，周建平３"
        if regex::Regex::new(r"^[\u4e00-\u9fa5]{2,4}[\d，,、\s]*[\u4e00-\u9fa5]{2,4}").unwrap().is_match(trimmed) {
            let authors: Vec<String> = trimmed
                .split(|c: char| c == '，' || c == ',' || c == '、')
                .map(|s| {
                    // 去掉单位上标数字和空格
                    s.chars().filter(|c| c.is_alphabetic() || *c == ' ' || *c == '　')
                        .collect::<String>()
                        .trim()
                        .to_string()
                })
                .filter(|s| !s.is_empty() && s.len() >= 2)
                .collect();
            if !authors.is_empty() && authors.len() <= 20 {
                return Some(authors);
            }
        }
    }
    None
}

// ── 卷/期/页码提取 ──────────────────────────────────────

fn scan_volume(text: &str) -> Option<String> {
    let re = regex::Regex::new(r"第\s*(\d+)\s*卷").unwrap();
    if let Some(caps) = re.captures(text) {
        return Some(caps.get(1)?.as_str().to_string());
    }
    None
}

fn scan_issue(text: &str) -> Option<String> {
    let re = regex::Regex::new(r"第\s*(\d+)\s*期").unwrap();
    if let Some(caps) = re.captures(text) {
        return Some(caps.get(1)?.as_str().to_string());
    }
    // "No. X" 或 "No X"
    let re_en = regex::Regex::new(r"(?i)No\.?\s*(\d+)").unwrap();
    if let Some(caps) = re_en.captures(text) {
        return Some(caps.get(1)?.as_str().to_string());
    }
    None
}

fn scan_pages(text: &str) -> Option<String> {
    // 中文页码："１－９" 或 "1-9"（文本已归一化）
    // 匹配模式：冒号后跟数字-数字，或括号后跟数字-数字
    let re = regex::Regex::new(r"[：:](\s*(\d+)\s*[－\-]\s*(\d+)\s*)").unwrap();
    if let Some(caps) = re.captures(text) {
        return Some(format!("{}-{}", caps.get(1)?.as_str(), caps.get(2)?.as_str()));
    }
    // 英文页码："pp. 1-9" 或 "1-9"
    let re_en = regex::Regex::new(r"(?i)pp?\.?\s*(\d+)\s*[－\-]\s*(\d+)").unwrap();
    if let Some(caps) = re_en.captures(text) {
        return Some(format!("{}-{}", caps.get(1)?.as_str(), caps.get(2)?.as_str()));
    }
    None
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