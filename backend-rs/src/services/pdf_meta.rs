//! 轻量级 PDF 元数据（含 DOI）提取。
//!
//! 当启用 `mupdf` feature 时，使用内置的 `mupdf` 库提取 Info 字典和页面文本。
//! 未启用时，回退为调用外部 `mutool` 工具完成相同功能。

#[cfg(feature = "mupdf")]
use mupdf::{Document, MetadataName, TextExtractOptions};

use std::process::Command;
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

/// 从 PDF 字节内容中提取元数据
#[cfg(feature = "mupdf")]
pub fn extract_pdf_metadata(content: &[u8], filename: Option<&str>) -> serde_json::Value {
    let mut result = serde_json::Map::new();

    if let Ok(doc) = Document::from_bytes(content, "pdf") {
        // 1. 从 Info 字典提取
        extract_from_info_dict(&doc, filename, &mut result);

        // 2. 从页面文本提取（补充 Info 字典未覆盖的字段）
        let full_text = extract_full_text(&doc);
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
pub fn extract_pdf_metadata(content: &[u8], filename: Option<&str>) -> serde_json::Value {
    let mut result = serde_json::Map::new();

    // 1. 通过 mutool 提取 Info 字典
    if let Some(info) = extract_info_dict_via_mutool(content) {
        extract_from_info_dict_parsed(&info, filename, &mut result);
    }

    // 2. 通过 mutool 提取页面文本
    if let Some(text) = extract_full_text_via_mutool(content) {
        let normalized = normalize_fullwidth(&text);
        extract_from_text(&normalized, filename, &mut result);
    }

    serde_json::Value::Object(result)
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

    let mutool_path = crate::settings::get_settings().mutool_path();
    let output = Command::new(&mutool_path)
        .args(["show", temp_path.to_str()?, "trailer.Info"])
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    parse_mutool_info(&stdout)
}

#[cfg(not(feature = "mupdf"))]
fn parse_mutool_info(output: &str) -> Option<InfoDict> {
    let mut title = None;
    let mut author = None;
    let mut subject = None;
    let mut keywords = None;
    let mut creation_date = None;

    for line in output.lines() {
        let line = line.trim();
        if let Some(val) = line.strip_prefix("/Title") {
            title = parse_pdf_string(val);
        } else if let Some(val) = line.strip_prefix("/Author") {
            author = parse_pdf_string(val);
        } else if let Some(val) = line.strip_prefix("/Subject") {
            subject = parse_pdf_string(val);
        } else if let Some(val) = line.strip_prefix("/Keywords") {
            keywords = parse_pdf_string(val);
        } else if let Some(val) = line.strip_prefix("/CreationDate") {
            creation_date = parse_pdf_string(val);
        }
    }

    Some(InfoDict { title, author, subject, keywords, creation_date })
}

#[cfg(not(feature = "mupdf"))]
fn parse_pdf_string(s: &str) -> Option<String> {
    let s = s.trim();
    if s.starts_with('(') && s.ends_with(')') {
        let inner = &s[1..s.len() - 1];
        Some(inner.to_string())
    } else {
        None
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
fn extract_full_text(doc: &Document) -> Option<String> {
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
        Some(text)
    }
}

#[cfg(not(feature = "mupdf"))]
fn extract_full_text_via_mutool(content: &[u8]) -> Option<String> {
    let temp_dir = TempDir::new().ok()?;
    let temp_path = temp_dir.path().join("extract_text.pdf");
    std::fs::write(&temp_path, content).ok()?;

    let mutool_path = crate::settings::get_settings().mutool_path();
    let output = Command::new(&mutool_path)
        .args(["draw", "-F", "text", temp_path.to_str()?])
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    // 去掉 mutool 输出的页面头信息（如 "page /path/file.pdf 1"）
    let text: String = stdout
        .lines()
        .filter(|line| !line.starts_with("page ") && !line.starts_with("system error"))
        .collect::<Vec<&str>>()
        .join("\n");

    if text.trim().is_empty() {
        None
    } else {
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

    // 年份
    if !result.contains_key("year") {
        if let Some(year) = scan_year_from_text(text) {
            result.insert("year".to_string(), serde_json::Value::Number(year.into()));
        }
    }

    // 期刊名
    if !result.contains_key("journal") {
        if let Some(journal) = scan_journal(text) {
            result.insert("journal".to_string(), serde_json::Value::String(journal));
        }
    }

    // 标题（从引用格式或正文提取）
    if !result.contains_key("title") {
        if let Some(title) = extract_title_from_text(text, filename) {
            result.insert("title".to_string(), serde_json::Value::String(title));
        }
    }

    // 作者
    if !result.contains_key("authors") {
        if let Some(authors) = extract_authors_from_text(text) {
            result.insert("authors".to_string(), serde_json::json!(authors));
        }
    }

    // 卷/期/页码
    if !result.contains_key("volume") {
        if let Some(vol) = scan_volume(text) {
            result.insert("volume".to_string(), serde_json::Value::String(vol));
        }
    }
    if !result.contains_key("issue") {
        if let Some(issue) = scan_issue(text) {
            result.insert("issue".to_string(), serde_json::Value::String(issue));
        }
    }
    if !result.contains_key("pages") {
        if let Some(pages) = scan_pages(text) {
            result.insert("pages".to_string(), serde_json::Value::String(pages));
        }
    }
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