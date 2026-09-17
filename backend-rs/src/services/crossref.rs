//! Crossref 元数据获取服务。

use serde_json::Value;

pub struct CrossrefClient {
    base_url: String,
    mailto: String,
    http: reqwest::Client,
}

impl CrossrefClient {
    pub fn new(base_url: Option<&str>, mailto: Option<&str>) -> Self {
        Self {
            base_url: base_url.unwrap_or("https://api.crossref.org/works").to_string(),
            mailto: mailto.unwrap_or("okb-assist@example.com").to_string(),
            http: reqwest::Client::new(),
        }
    }

    fn user_agent(&self) -> String {
        format!("okb-assist/1.0 (mailto:{})", self.mailto)
    }

    /// 按 DOI 查询
    pub async fn lookup_by_doi(&self, doi: &str) -> Option<Value> {
        let normalized = normalize_doi(doi)?;
        let url = format!("{}/{}", self.base_url, normalized);
        let resp = self.http.get(&url)
            .query(&[("mailto", &self.mailto)])
            .header("User-Agent", self.user_agent())
            .send().await
            .ok()?;

        if resp.status() == 404 {
            return None;
        }
        if !resp.status().is_success() {
            return None;
        }

        let data: Value = resp.json().await.ok()?;
        let parsed = parse_crossref_item(&data["message"])?;
        Some(serde_json::json!({
            "raw": data,
            "parsed": parsed,
        }))
    }

    /// 按题名查询
    pub async fn lookup_by_title(&self, title: &str) -> Option<Value> {
        let resp = self.http.get(&self.base_url)
            .query(&[
                ("query.bibliographic", title),
                ("rows", "1"),
                ("mailto", &self.mailto),
            ])
            .header("User-Agent", self.user_agent())
            .send().await
            .ok()?;

        if !resp.status().is_success() {
            return None;
        }

        let data: Value = resp.json().await.ok()?;
        let items = data["message"]["items"].as_array()?;
        let item = items.first()?;
        let parsed = parse_crossref_item(item)?;
        Some(serde_json::json!({
            "raw": data,
            "parsed": parsed,
        }))
    }
}

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

/// 去除 HTML 标记
fn strip_markup(text: &str) -> String {
    use regex::Regex;
    let t = Regex::new(r"<tex-math[^>]*>(.*?)</tex-math>")
        .unwrap().replace_all(text, " $1 ");
    let t = Regex::new(r"<mml:math[^>]*>(.*?)</mml:math>")
        .unwrap().replace_all(&t, " $1 ");
    let t = Regex::new(r"<[^>]+>").unwrap().replace_all(&t, "");
    let t = t.replace("&amp;", "&").replace("&lt;", "<").replace("&gt;", ">");
    t.replace("&quot;", "\"").replace("&#39;", "'").replace("&nbsp;", " ")
        .trim().to_string()
}

/// 从 Crossref item 解析作者
fn crossref_authors(item: &Value) -> Vec<String> {
    let mut out = Vec::new();
    if let Some(authors) = item["author"].as_array() {
        for a in authors {
            if let Some(name) = a["name"].as_str() {
                out.push(name.to_string());
            } else {
                let given = a["given"].as_str().unwrap_or("").trim();
                let family = a["family"].as_str().unwrap_or("").trim();
                if !given.is_empty() && !family.is_empty() {
                    out.push(format!("{} {}", given, family));
                } else if !family.is_empty() {
                    out.push(family.to_string());
                } else if !given.is_empty() {
                    out.push(given.to_string());
                }
            }
        }
    }
    out
}

/// 映射 Crossref 类型到文档类型
fn doc_type_map(crossref_type: &str) -> &'static str {
    match crossref_type.to_lowercase().as_str() {
        "journal-article" => "journal",
        "book" => "book",
        "book-chapter" => "book",
        "proceedings-article" => "conference",
        "conference-paper" => "conference",
        "dataset" => "dataset",
        "report" => "report",
        "thesis" => "thesis",
        "posted-content" => "preprint",
        "reference-entry" => "reference",
        "peer-review" => "other",
        _ => "other",
    }
}

/// 解析 Crossref 条目
fn parse_crossref_item(message: &Value) -> Option<Value> {
    let raw_title = message.get("title");
    let title = if let Some(arr) = raw_title.and_then(|v| v.as_array()) {
        strip_markup(arr.first().and_then(|v| v.as_str()).unwrap_or(""))
    } else {
        strip_markup(raw_title.and_then(|v| v.as_str()).unwrap_or(""))
    };

    let authors = crossref_authors(message);
    let doi = normalize_doi(message["DOI"].as_str().unwrap_or(""));

    let year = message["issued"]["date-parts"].as_array()
        .and_then(|parts| parts.first())
        .and_then(|first_part| first_part.as_array())
        .and_then(|arr| arr.first())
        .and_then(|v| v.as_i64());

    let source = message["publisher"].as_str().unwrap_or("Crossref").to_string();

    let journal = match message.get("container-title") {
        Some(v) if v.is_array() => v.as_array().unwrap()
            .first().and_then(|v| v.as_str()).unwrap_or("").to_string(),
        Some(v) => v.as_str().unwrap_or("").to_string(),
        None => String::new(),
    };

    let keywords = message.get("subject").and_then(|v| v.as_array())
        .map(|arr| arr.iter().filter_map(|v| v.as_str().map(String::from)).collect::<Vec<_>>());

    let abstract_text = strip_markup(message["abstract"].as_str().unwrap_or(""));
    let doc_type = doc_type_map(message["type"].as_str().unwrap_or(""));
    let language = message["language"].as_str().map(String::from);

    Some(serde_json::json!({
        "title": title,
        "authors": authors,
        "doi": doi,
        "year": year,
        "source": source,
        "journal": journal,
        "keywords": keywords,
        "abstract": abstract_text,
        "doc_type": doc_type,
        "language": language,
        "title_en": if !title.is_empty() { Some(title) } else { None },
        "authors_en": if !authors.is_empty() { Some(authors) } else { None },
        "journal_en": if !journal.is_empty() { Some(journal) } else { None },
        "keywords_en": keywords,
        "abstract_en": if !abstract_text.is_empty() { Some(abstract_text) } else { None },
    }))
}