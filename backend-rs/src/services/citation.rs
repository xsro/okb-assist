//! 引用条目生成：BibTeX 与 GB/T 7714。
//!
//! 输入既可以是 Crossref 的原始 `message` 记录（字段最全），
//! 也可以退化为数据库里的 `Document` 元数据。

use serde_json::Value;

use crate::models::Document;
use crate::services::crossref::{normalize_doi, strip_markup};

/// 单个作者
#[derive(Debug, Clone, Default)]
pub struct Author {
    /// 名（Crossref 的 `given`；中文姓名一般为空）
    pub given: String,
    /// 姓（Crossref 的 `family`；中文姓名为完整姓名）
    pub family: String,
    /// 无法拆分姓名时（Crossref 的 `name` 字段）整体视作一个姓名
    pub literal: bool,
}

impl Author {
    /// 是否包含中日韩字符
    fn is_cjk(&self) -> bool {
        self.family.chars().any(is_cjk) || self.given.chars().any(is_cjk)
    }

    /// 完整姓名（用于界面展示）
    pub fn full_name(&self) -> String {
        if self.given.is_empty() {
            self.family.clone()
        } else if self.family.is_empty() {
            self.given.clone()
        } else if self.is_cjk() {
            format!("{}{}", self.family, self.given)
        } else {
            format!("{} {}", self.given, self.family)
        }
    }
}

fn is_cjk(c: char) -> bool {
    matches!(
        c as u32,
        0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xF900..=0xFAFF | 0x3040..=0x30FF | 0xAC00..=0xD7AF
    )
}

/// 统一的引用记录
#[derive(Debug, Clone, Default)]
pub struct CitationRecord {
    /// Crossref 类型（journal-article / book / book-chapter / ...）
    pub crossref_type: String,
    pub title: String,
    pub authors: Vec<Author>,
    /// 期刊或丛书/书名（Crossref 的 container-title）
    pub container_title: String,
    pub publisher: String,
    pub year: Option<i64>,
    pub volume: String,
    pub issue: String,
    pub pages: String,
    pub doi: String,
    pub issn: String,
    pub isbn: String,
    pub url: String,
    /// 语言（zh / en / ...）
    pub language: String,
    /// 数据来源：crossref / document
    pub source: String,
}

impl CitationRecord {
    /// 由 Crossref `message` 记录构造
    pub fn from_crossref(message: &Value, source: &str) -> Self {
        let title = match message.get("title") {
            Some(Value::Array(arr)) => strip_markup(arr.first().and_then(|v| v.as_str()).unwrap_or("")),
            Some(Value::String(s)) => strip_markup(s),
            _ => String::new(),
        };

        let mut authors = Vec::new();
        if let Some(list) = message.get("author").and_then(|v| v.as_array()) {
            for a in list {
                if let Some(name) = a.get("name").and_then(|v| v.as_str()) {
                    let name = name.trim();
                    if !name.is_empty() {
                        authors.push(Author {
                            given: String::new(),
                            family: name.to_string(),
                            literal: true,
                        });
                    }
                    continue;
                }
                let given = a.get("given").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
                let family = a.get("family").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
                if !given.is_empty() || !family.is_empty() {
                    authors.push(Author {
                        given,
                        family,
                        literal: false,
                    });
                }
            }
        }

        let container_title = match message.get("container-title") {
            Some(Value::Array(arr)) => arr
                .first()
                .and_then(|v| v.as_str())
                .map(strip_markup)
                .unwrap_or_default(),
            Some(Value::String(s)) => strip_markup(s),
            _ => String::new(),
        };

        let year = crossref_year(message);
        let pages = non_empty(message, "page")
            .or_else(|| non_empty(message, "article-number"))
            .unwrap_or_default();

        Self {
            crossref_type: message.get("type").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            title,
            authors,
            container_title,
            publisher: non_empty(message, "publisher").unwrap_or_default(),
            year,
            volume: non_empty(message, "volume").unwrap_or_default(),
            issue: non_empty(message, "issue").unwrap_or_default(),
            pages,
            doi: normalize_doi(message.get("DOI").and_then(|v| v.as_str()).unwrap_or(""))
                .unwrap_or_default(),
            issn: join_list(message.get("ISSN")),
            isbn: join_list(message.get("ISBN")),
            url: message.get("URL").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            language: message.get("language").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            source: source.to_string(),
        }
    }

    /// 由数据库文档元数据构造（无 Crossref 记录时兜底）
    pub fn from_document(doc: &Document, source: &str) -> Self {
        let title = doc.title.clone().unwrap_or_default();
        let raw_authors = doc.authors.clone().unwrap_or_default();
        let authors = raw_authors
            .split(|c| c == ';' || c == '；' || c == ',')
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .map(parse_author_name)
            .collect::<Vec<_>>();

        let doc_type = doc.doc_type.clone().unwrap_or_default().to_lowercase();
        let crossref_type = match doc_type.as_str() {
            "journal" | "article" | "journal-article" => "journal-article",
            "book" | "monograph" => "book",
            "book-chapter" | "chapter" | "incollection" => "book-chapter",
            "conference" | "proceedings" | "conference-paper" => "proceedings-article",
            "thesis" | "dissertation" => "thesis",
            "report" => "report",
            "preprint" => "posted-content",
            "dataset" => "dataset",
            _ => "",
        }
        .to_string();

        let language = doc.language.clone().unwrap_or_else(|| {
            if title.chars().any(is_cjk) {
                "zh".to_string()
            } else {
                "en".to_string()
            }
        });

        Self {
            crossref_type,
            title,
            authors,
            container_title: doc.journal.clone().unwrap_or_default(),
            publisher: doc.source.clone().unwrap_or_default(),
            year: doc.year,
            volume: String::new(),
            issue: String::new(),
            pages: String::new(),
            doi: normalize_doi(doc.doi.as_deref().unwrap_or("")).unwrap_or_default(),
            issn: String::new(),
            isbn: String::new(),
            url: String::new(),
            language,
            source: source.to_string(),
        }
    }

    /// 是否中文记录（用于“等”/“et al”等本地化差异）
    fn is_chinese(&self) -> bool {
        self.language.to_lowercase().starts_with("zh")
            || self.authors.iter().any(|a| a.is_cjk())
            || self.title.chars().any(is_cjk)
    }
}

fn non_empty(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

fn join_list(value: Option<&Value>) -> String {
    match value {
        Some(Value::Array(arr)) => arr
            .iter()
            .filter_map(|v| v.as_str())
            .collect::<Vec<_>>()
            .join(", "),
        Some(Value::String(s)) => s.trim().to_string(),
        _ => String::new(),
    }
}

/// Crossref 年份：优先 issued，其次 published-print / published-online / created
fn crossref_year(message: &Value) -> Option<i64> {
    for key in ["issued", "published-print", "published-online", "published", "created"] {
        if let Some(year) = message
            .get(key)
            .and_then(|v| v.get("date-parts"))
            .and_then(|v| v.as_array())
            .and_then(|parts| parts.first())
            .and_then(|first| first.as_array())
            .and_then(|arr| arr.first())
            .and_then(|v| v.as_i64())
        {
            return Some(year);
        }
    }
    None
}

/// 从“Given Family”字符串猜测姓名（西文取最后一个词作姓，中文整体作姓名）
fn parse_author_name(raw: &str) -> Author {
    let name = raw.trim();
    if name.chars().any(is_cjk) {
        return Author {
            given: String::new(),
            family: name.to_string(),
            literal: true,
        };
    }
    let mut parts: Vec<&str> = name.split_whitespace().collect();
    if parts.len() >= 2 {
        let family = parts.pop().unwrap().to_string();
        Author {
            given: parts.join(" "),
            family,
            literal: false,
        }
    } else {
        Author {
            given: String::new(),
            family: name.to_string(),
            literal: true,
        }
    }
}

// ── BibTeX ──────────────────────────────────────────────

/// 生成 BibTeX 条目
pub fn to_bibtex(rec: &CitationRecord) -> String {
    let entry = bib_entry_type(&rec.crossref_type);
    let mut fields: Vec<(&str, String)> = Vec::new();

    if let Some(authors) = bib_authors(rec) {
        fields.push(("author", authors));
    }
    if !rec.title.is_empty() {
        fields.push(("title", latex_escape(&rec.title)));
    }

    match entry {
        "article" => {
            if !rec.container_title.is_empty() {
                fields.push(("journal", latex_escape(&rec.container_title)));
            }
        }
        "incollection" | "inproceedings" => {
            if !rec.container_title.is_empty() {
                fields.push(("booktitle", latex_escape(&rec.container_title)));
            }
        }
        _ => {}
    }

    if !rec.publisher.is_empty() && entry != "article" {
        fields.push(("publisher", latex_escape(&rec.publisher)));
    }
    if let Some(year) = rec.year {
        fields.push(("year", year.to_string()));
    }
    if !rec.volume.is_empty() {
        fields.push(("volume", latex_escape(&rec.volume)));
    }
    if !rec.issue.is_empty() {
        fields.push(("number", latex_escape(&rec.issue)));
    }
    if !rec.pages.is_empty() {
        fields.push(("pages", normalize_pages(&rec.pages, "--")));
    }
    if !rec.doi.is_empty() {
        fields.push(("doi", rec.doi.clone()));
    }
    if !rec.issn.is_empty() {
        fields.push(("issn", rec.issn.clone()));
    }
    if !rec.isbn.is_empty() {
        fields.push(("isbn", rec.isbn.clone()));
    }
    if !rec.url.is_empty() {
        fields.push(("url", rec.url.clone()));
    }

    let mut out = format!("@{}{{{},\n", entry, citation_key(rec));
    let width = fields.iter().map(|(k, _)| k.len()).max().unwrap_or(0);
    for (name, value) in &fields {
        out.push_str(&format!("  {:width$} = {{{}}},\n", name, value, width = width));
    }
    out.push_str("}\n");
    out
}

/// 生成引用键（`作者姓+年份+标题首词`，形如 `li2012robot`）
pub fn citation_key(rec: &CitationRecord) -> String {
    let mut key = String::new();
    if let Some(first) = rec.authors.first() {
        key.push_str(&slug(&first.family, true));
    }
    if key.is_empty() {
        key.push_str("anon");
    }
    if let Some(year) = rec.year {
        key.push_str(&year.to_string());
    }
    let title_word = rec
        .title
        .split_whitespace()
        .filter(|w| w.chars().any(|c| c.is_alphanumeric()))
        .find(|w| !is_stop_word(w));
    if let Some(word) = title_word {
        key.push_str(&slug(word, true));
    }
    if key.is_empty() {
        "reference".to_string()
    } else {
        key
    }
}

fn is_stop_word(word: &str) -> bool {
    matches!(
        word.to_lowercase().trim_matches(|c: char| !c.is_alphanumeric()),
        "a" | "an" | "the" | "on" | "of" | "in" | "for" | "and" | "to" | "with" | "from"
    )
}

/// 只保留字母数字（可选保留中文字符），用于生成引用键
fn slug(text: &str, keep_cjk: bool) -> String {
    text.chars()
        .filter(|c| c.is_alphanumeric() || (keep_cjk && is_cjk(*c)))
        .collect::<String>()
        .to_lowercase()
}

fn bib_entry_type(crossref_type: &str) -> &'static str {
    match crossref_type.to_lowercase().as_str() {
        "journal-article" => "article",
        "book" | "monograph" | "edited-book" | "reference-book" => "book",
        "book-chapter" | "book-section" | "book-part" | "book-set" => "incollection",
        "proceedings-article" => "inproceedings",
        "report" => "techreport",
        "thesis" | "dissertation" | "degree" => "phdthesis",
        "dataset" => "misc",
        "posted-content" => "unpublished",
        _ => "misc",
    }
}

fn bib_authors(rec: &CitationRecord) -> Option<String> {
    if rec.authors.is_empty() {
        return None;
    }
    let names: Vec<String> = rec
        .authors
        .iter()
        .map(|a| {
            if a.literal || a.given.is_empty() {
                // 整体姓名（或中文姓名）用花括号保护，避免被 BibTeX 拆分
                format!("{{{}}}", latex_escape(&a.full_name()))
            } else {
                format!("{}, {}", latex_escape(&a.family), latex_escape(&a.given))
            }
        })
        .collect();
    Some(names.join(" and "))
}

/// BibTeX 特殊字符转义
fn latex_escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '\\' => out.push_str(r"\textbackslash{}"),
            '&' => out.push_str(r"\&"),
            '%' => out.push_str(r"\%"),
            '$' => out.push_str(r"\$"),
            '#' => out.push_str(r"\#"),
            '_' => out.push_str(r"\_"),
            '{' => out.push_str(r"\{"),
            '}' => out.push_str(r"\}"),
            '~' => out.push_str(r"\textasciitilde{}"),
            '^' => out.push_str(r"\textasciicircum{}"),
            // 连字符变体（Crossref 常见 U+2010/U+2011）统一为普通连字符
            '\u{2010}' | '\u{2011}' => out.push('-'),
            _ => out.push(c),
        }
    }
    out
}

// ── GB/T 7714 ───────────────────────────────────────────

/// 文献类型归类（决定 GB/T 出版项拼接方式）
#[derive(Debug, Clone, Copy, PartialEq)]
enum GbtKind {
    /// 期刊论文：刊名, 年, 卷(期): 页码
    Journal,
    /// 图书章节/会议论文：// 书名. 出版社, 年: 页码
    InBook,
    /// 专著/报告/学位论文：出版社, 年
    Book,
}

fn gbt_kind(crossref_type: &str) -> GbtKind {
    match crossref_type.to_lowercase().as_str() {
        "journal-article" => GbtKind::Journal,
        "book-chapter" | "book-section" | "book-part" | "proceedings-article" => GbtKind::InBook,
        _ => GbtKind::Book,
    }
}

/// 生成 GB/T 7714（著者-题名-文献类型标识）格式条目
pub fn to_gbt7714(rec: &CitationRecord) -> String {
    let mut out = String::new();

    let authors = gbt_authors(rec);
    if !authors.is_empty() {
        out.push_str(&authors);
        out.push_str(". ");
    }
    out.push_str(&plain_text(rec.title.trim()));

    let type_mark = gbt_type_mark(&rec.crossref_type);
    if !type_mark.is_empty() {
        out.push_str(&format!("[{}]", type_mark));
    }

    let year = rec.year.map(|y| y.to_string());
    let mut volume = rec.volume.clone();
    if !rec.issue.is_empty() {
        volume = format!("{}({})", volume, rec.issue);
    }
    let pages = if rec.pages.is_empty() {
        String::new()
    } else {
        normalize_pages(&rec.pages, "-")
    };

    match gbt_kind(&rec.crossref_type) {
        GbtKind::Journal => {
            let mut segments: Vec<String> = Vec::new();
            if !rec.container_title.is_empty() {
                segments.push(plain_text(&rec.container_title));
            }
            if let Some(y) = &year {
                segments.push(y.clone());
            }
            if !volume.is_empty() {
                segments.push(volume.clone());
            }
            if !segments.is_empty() {
                out.push_str(". ");
                out.push_str(&segments.join(", "));
                if !pages.is_empty() {
                    out.push_str(&format!(": {}", pages));
                }
            }
        }
        GbtKind::InBook => {
            if !rec.container_title.is_empty() {
                out.push_str("//");
                out.push_str(&plain_text(&rec.container_title));
            }
            let mut segments: Vec<String> = Vec::new();
            if !rec.publisher.is_empty() {
                segments.push(rec.publisher.clone());
            }
            if let Some(y) = &year {
                segments.push(y.clone());
            }
            if !segments.is_empty() {
                out.push_str(". ");
                out.push_str(&segments.join(", "));
                if !pages.is_empty() {
                    out.push_str(&format!(": {}", pages));
                }
            }
        }
        GbtKind::Book => {
            let mut segments: Vec<String> = Vec::new();
            if !rec.publisher.is_empty() {
                segments.push(rec.publisher.clone());
            }
            if !rec.container_title.is_empty() && !rec.publisher.is_empty() {
                segments.insert(0, rec.container_title.clone());
            } else if !rec.container_title.is_empty() {
                segments.push(rec.container_title.clone());
            }
            if let Some(y) = &year {
                segments.push(y.clone());
            }
            if !segments.is_empty() {
                out.push_str(". ");
                out.push_str(&segments.join(", "));
            }
        }
    }

    out.push('.');
    out
}

/// 纯文本规范化：连字符变体统一为普通连字符（GB/T 为纯文本输出）
fn plain_text(text: &str) -> String {
    text.replace(['\u{2010}', '\u{2011}'], "-")
}

/// 规范化页码范围：把各种连字符统一为目标分隔符
fn normalize_pages(pages: &str, dash: &str) -> String {
    let re = regex::Regex::new(r"\s*[-\u{2010}\u{2011}\u{2012}\u{2013}\u{2014}]+\s*").unwrap();
    re.replace_all(pages.trim(), dash).to_string()
}

fn gbt_type_mark(crossref_type: &str) -> &'static str {
    match crossref_type.to_lowercase().as_str() {
        "journal-article" => "J",
        "book" | "monograph" | "edited-book" | "reference-book" => "M",
        "book-chapter" | "book-section" | "book-part" => "M",
        "proceedings-article" => "C",
        "report" => "R",
        "thesis" | "dissertation" | "degree" => "D",
        "dataset" | "posted-content" | "reference-entry" => "EB/OL",
        _ => "Z",
    }
}

fn gbt_authors(rec: &CitationRecord) -> String {
    if rec.authors.is_empty() {
        return String::new();
    }
    let names: Vec<String> = rec.authors.iter().map(gbt_author).collect();
    if names.len() > 3 {
        let more = if rec.is_chinese() { "等" } else { "et al" };
        format!("{}, {}", names[..3].join(", "), more)
    } else {
        names.join(", ")
    }
}

fn gbt_author(author: &Author) -> String {
    if author.is_cjk() || author.literal {
        return author.full_name().trim().to_string();
    }
    // 西文：姓全大写 + 名首字母
    let family = author.family.to_uppercase();
    let initials: Vec<String> = author
        .given
        .split(|c: char| c == ' ' || c == '.' || c == '-')
        .filter(|s| !s.is_empty())
        .filter_map(|word| word.chars().next())
        .map(|c| c.to_uppercase().to_string())
        .collect();
    if initials.is_empty() {
        family
    } else {
        format!("{} {}", family, initials.join(" "))
    }
}

// ── 入口 ────────────────────────────────────────────────

/// 支持的引用格式
pub const FORMAT_BIBTEX: &str = "bibtex";
pub const FORMAT_GBT7714: &str = "gbt7714";

/// 规范化格式参数，返回 None 表示不支持
pub fn normalize_format(raw: &str) -> Option<&'static str> {
    match raw.trim().to_lowercase().as_str() {
        "" | "bib" | "bibtex" => Some(FORMAT_BIBTEX),
        "gbt" | "gbt7714" | "gbt7714-2015" | "gb" => Some(FORMAT_GBT7714),
        _ => None,
    }
}

/// 按格式生成条目内容
pub fn render(rec: &CitationRecord, format: &str) -> String {
    match format {
        FORMAT_GBT7714 => to_gbt7714(rec),
        _ => to_bibtex(rec),
    }
}
