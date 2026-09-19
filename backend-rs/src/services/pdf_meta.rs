//! 轻量级 PDF 元数据（含 DOI）提取。
//!
//! 优先使用 `lopdf` 提取 Info 字典；
//! 对于 XMP 元数据或 lopdf 无法解析的 PDF，回退到 `pdfcpu info` 命令行工具。

use lopdf::Document;
use std::process::Command;

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
pub fn extract_pdf_metadata(content: &[u8], filename: Option<&str>, pdfcpu_path: Option<&str>) -> serde_json::Value {
    let mut result = serde_json::Map::new();

    // 先尝试 lopdf
    if let Ok(doc) = Document::load_mem(content) {
        // 尝试从 Info 字典提取
        if let Ok(info_obj) = doc.trailer.get(b"Info") {
            if let Ok(info_dict) = info_obj.as_dict() {
                for (key, value) in info_dict.iter() {
                    let key_str = String::from_utf8_lossy(key).to_lowercase();
                    if let Ok(val_bytes) = value.as_str() {
                        let val = String::from_utf8_lossy(val_bytes).to_string();
                        match key_str.as_str() {
                            "title" => {
                                let t = val.trim().to_string();
                                if !t.is_empty()
                                    && !looks_like_filename(&t, filename)
                                    && !looks_like_placeholder(&t)
                                {
                                    result.insert("title".to_string(), serde_json::Value::String(t));
                                }
                            }
                            "author" => {
                                let a = val.trim().to_string();
                                if !a.is_empty() {
                                    let authors = split_authors(&a);
                                    result.insert("authors".to_string(), serde_json::json!(authors));
                                }
                            }
                            "keywords" => {
                                let k = val.trim().to_string();
                                if !k.is_empty() {
                                    result.insert("keywords".to_string(), serde_json::json!(split_keywords(&k)));
                                }
                            }
                            "subject" => {
                                let s = val.trim().to_string();
                                if !s.is_empty() && s.len() > 80 {
                                    result.insert("abstract".to_string(), serde_json::Value::String(s));
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
        }

        // 尝试从文档文本扫描 DOI
        if !result.contains_key("doi") {
            if let Ok(text) = extract_pdf_text(&doc) {
                if let Some(doi) = scan_doi(&text) {
                    result.insert("doi".to_string(), serde_json::Value::String(doi));
                }
            }
        }
    }

    // 如果 lopdf 没有提取到标题或作者，尝试 pdfcpu
    let has_title = result.contains_key("title");
    let has_authors = result.contains_key("authors");
    if !has_title || !has_authors {
        if let Some(pdfcpu_meta) = extract_pdf_metadata_pdfcpu(content, pdfcpu_path) {
            for (k, v) in pdfcpu_meta.as_object().unwrap() {
                if !result.contains_key(k) {
                    result.insert(k.clone(), v.clone());
                }
            }
        }
    }

    serde_json::Value::Object(result)
}

/// 通过 pdfcpu info 命令行工具提取 PDF 元数据（需要安装 pdfcpu）
fn extract_pdf_metadata_pdfcpu(content: &[u8], pdfcpu_path: Option<&str>) -> Option<serde_json::Value> {
    let pdfcpu_bin = pdfcpu_path.unwrap_or("pdfcpu");

    // 写入临时文件
    let temp_dir = std::env::temp_dir();
    let temp_path = temp_dir.join(format!("okb_meta_{}.pdf", std::process::id()));
    if std::fs::write(&temp_path, content).is_err() {
        return None;
    }

    let path_str = temp_path.to_str()?;
    let output = Command::new(pdfcpu_bin)
        .args(["info", path_str])
        .output()
        .ok()?;

    let _ = std::fs::remove_file(&temp_path);

    if !output.status.success() {
        return None;
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut result = serde_json::Map::new();

    let mut in_properties = false;
    let mut current_keywords: Vec<String> = Vec::new();

    for line in stdout.lines() {
        let trimmed = line.trim();

        // 检测 Properties 区域
        if trimmed == "Properties:" {
            in_properties = true;
            continue;
        }
        if trimmed.starts_with("━━━━") {
            if in_properties {
                in_properties = false;
            }
            continue;
        }

        if in_properties {
            // Properties 区域：查找 doi = ... 行
            if let Some(doi_part) = trimmed.strip_prefix("doi = ") {
                let doi = doi_part.trim();
                if let Some(norm) = normalize_doi(doi) {
                    result.insert("doi".to_string(), serde_json::Value::String(norm));
                }
            }
            continue;
        }

        // 解析键值对
        if let Some(colon_pos) = trimmed.find(':') {
            let key = trimmed[..colon_pos].trim().to_lowercase();
            let value = trimmed[colon_pos + 1..].trim().to_string();

            match key.as_str() {
                "title" => {
                    if !value.is_empty() && !looks_like_placeholder(&value) {
                        result.insert("title".to_string(), serde_json::Value::String(value));
                    }
                }
                "author" => {
                    if !value.is_empty() {
                        let authors = split_authors(&value);
                        result.insert("authors".to_string(), serde_json::json!(authors));
                    }
                }
                "subject" => {
                    if !value.is_empty() {
                        // Subject 可能包含 DOI
                        if let Some(doi) = scan_doi(&value) {
                            result.insert("doi".to_string(), serde_json::Value::String(doi));
                        }
                        // 如果长度足够，作为 abstract
                        if value.len() > 80 {
                            result.insert("abstract".to_string(), serde_json::Value::String(value));
                        }
                    }
                }
                "keywords" => {
                    if !value.is_empty() {
                        current_keywords.push(value);
                    }
                }
                "creation date" => {
                    if let Some(year) = parse_year_from_date(&value) {
                        result.insert("year".to_string(), serde_json::Value::Number(year.into()));
                    }
                }
                _ => {}
            }
        } else if !trimmed.is_empty()
            && !trimmed.starts_with("pdfcpu")
            && !trimmed.starts_with("./")
        {
            // Keywords 的续行（缩进的非键值对行）
            if !current_keywords.is_empty() && !trimmed.starts_with("━━") {
                current_keywords.push(trimmed.to_string());
            }
        }
    }

    // 保存 keywords
    if !current_keywords.is_empty() {
        result.insert("keywords".to_string(), serde_json::json!(current_keywords));
    }

    if result.is_empty() {
        None
    } else {
        Some(serde_json::Value::Object(result))
    }
}

/// 从 lopdf Document 提取全部文本（用于 DOI 扫描，只取前若干页）
fn extract_pdf_text(doc: &Document) -> Result<String, ()> {
    let pages = doc.get_pages();
    let mut text = String::new();
    let mut page_count = 0;

    for page_id in pages.values() {
        if page_count >= 3 {
            break;
        }
        page_count += 1;

        if let Ok(content) = doc.get_and_decode_page_content(*page_id) {
            for op in content.operations {
                if op.operator == "Tj" || op.operator == "TJ" {
                    for operand in &op.operands {
                        if let lopdf::Object::String(bytes, _) = operand {
                            text.push_str(&String::from_utf8_lossy(bytes));
                        } else if let lopdf::Object::Array(arr) = operand {
                            for item in arr {
                                if let lopdf::Object::String(bytes, _) = item {
                                    text.push_str(&String::from_utf8_lossy(bytes));
                                }
                            }
                        }
                    }
                    text.push(' ');
                }
            }
        }
    }

    if text.is_empty() {
        Err(())
    } else {
        Ok(text)
    }
}

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
    // Single token ASCII slug（全小写字母数字 + 点/横线）
    if !title.contains(' ')
        && title.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_')
    {
        return true;
    }
    false
}

/// 检测是否为占位标题（如 "Entire document"、"Untitled" 等）
pub(crate) fn looks_like_placeholder(title: &str) -> bool {
    let t = title.trim().to_lowercase();
    if t.is_empty() {
        return true;
    }
    let placeholders = [
        "entire document",
        "untitled",
        "document",
        "title",
        "unknown",
        "n/a",
        "no title",
        "not available",
        "confidential",
        "draft",
        "new document",
        "new doc",
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
    // D:YYYYMMDDHHmmSS format
    let re = regex::Regex::new(r"D:(\d{4})(\d{2})(\d{2})").unwrap();
    if let Some(caps) = re.captures(date_str) {
        if let Some(year) = caps.get(1).and_then(|m| m.as_str().parse::<i64>().ok()) {
            if (1900..=2100).contains(&year) {
                return Some(year);
            }
        }
    }
    // Any 19xx/20xx
    let re2 = regex::Regex::new(r"(19|20)\d{2}").unwrap();
    re2.find(date_str)
        .and_then(|m| m.as_str().parse::<i64>().ok())
}

fn scan_doi(text: &str) -> Option<String> {
    let re = regex::Regex::new(r"10\.\d{4,9}/[-._;()/:A-Za-z0-9]+").unwrap();
    re.find(text)
        .and_then(|m| normalize_doi(m.as_str()))
}