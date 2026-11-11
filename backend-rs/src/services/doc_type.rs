//! 文档类型（`documents.doc_type`）的唯一词表与归一化入口。
//!
//! **本项目只认 Zotero 标准类型名**（`journalArticle` / `conferencePaper` / …）。
//! 历史上这里被写进过各种同义词（Crossref 的 `journal-article` 被映射成 `journal`、
//! BibTeX 的 `inproceedings`、旧版 Python 实现的 `article`、手填的 `book_section` …），
//! 导致 `type_counts` 被拆成 `article` / `journalArticle` / `journal` / `article` 一堆碎片，
//! 还让 `citation.rs` 的 BibTeX / GB/T 条目类型判断失效（`journalArticle` 匹配不到任何分支）。
//!
//! 因此所有**写入**路径（上传/登记/Crossref 回填/元数据抽取/手工编辑/BibTeX 导入/启动迁移）
//! 都必须经过 [`normalize`]，所有**读取**侧需要其他词表（Crossref / BibTeX / CSL）时
//! 必须经过 [`to_crossref`]，不要在别处再造一套映射。

/// 标准类型（值, 中文标签）。顺序即前端候选列表顺序。
pub const STANDARD: &[(&str, &str)] = &[
    ("journalArticle", "期刊论文"),
    ("conferencePaper", "会议论文"),
    ("thesis", "学位论文"),
    ("book", "专著"),
    ("bookSection", "专著章节"),
    ("preprint", "预印本"),
    ("report", "报告"),
    ("patent", "专利"),
    ("review", "综述"),
    ("webpage", "网页"),
    ("presentation", "演示"),
    ("manuscript", "手稿"),
    ("document", "其他/文档"),
];

/// 未知类型的落点（保证写入的永远是标准值）
pub const FALLBACK: &str = "document";

/// 归一化到 Zotero 标准类型名。
///
/// 幂等；空串返回空串（表示「未设置」），其余未知值一律落到 [`FALLBACK`]。
/// 大小写、`_` / `-` / 空格 / 中文名都容忍，方便旧数据、Crossref、BibTeX、用户手填混用。
pub fn normalize(raw: &str) -> String {
    let raw = raw.trim();
    if raw.is_empty() {
        return String::new();
    }
    // 统一小写 + 把 `_`、`-`、连续空白都折叠成单个空格，下面只需要写一种形式
    let mut key = String::with_capacity(raw.len());
    let mut last_space = false;
    for c in raw.chars().flat_map(|c| c.to_lowercase()) {
        if c == '_' || c == '-' || c.is_whitespace() {
            if !last_space {
                key.push(' ');
                last_space = true;
            }
        } else {
            key.push(c);
            last_space = false;
        }
    }
    let key = key.trim();

    let mapped = match key {
        // 期刊论文
        "journalarticle" | "journal article" | "journal" | "article" | "periodical" | "magazine article"
        | "newspaper article" => "journalArticle",
        // 会议论文
        "conferencepaper" | "conference paper" | "conference" | "conference proceedings" | "proceedings"
        | "proceedings article" | "inproceedings" | "paper conference" | "会议论文" => "conferencePaper",
        // 学位论文
        "thesis" | "dissertation" | "degree" | "mastersthesis" | "master thesis" | "phdthesis"
        | "phd thesis" | "硕士学位论文" | "博士学位论文" | "学位论文" => "thesis",
        // 专著
        "book" | "monograph" | "edited book" | "reference book" | "专著" => "book",
        // 专著章节 / 论文集章节
        "booksection" | "book section" | "bookchapter" | "book chapter" | "chapter" | "incollection"
        | "inbook" | "book part" | "book set" | "章节" => "bookSection",
        // 预印本
        "preprint" | "posted content" | "postedcontent" | "预印本" => "preprint",
        // 报告
        "report" | "techreport" | "technical report" | "报告" => "report",
        // 专利
        "patent" | "专利" => "patent",
        // 综述
        "review" | "peer review" | "peerreview" | "综述" => "review",
        // 网页
        "webpage" | "website" | "web page" | "blogpost" | "blog post" | "网页" => "webpage",
        // 演示 / 手稿
        "presentation" | "conference presentation" | "演示" => "presentation",
        "manuscript" | "手稿" => "manuscript",
        // 明确的「其他」
        "document" | "other" | "misc" | "generic" | "unknown" | "dataset" | "reference entry"
        | "数据集" | "其他" => FALLBACK,
        // 已经就是标准值（上面没列出大小写变体的兜底）
        _ => {
            if let Some((value, _)) = STANDARD
                .iter()
                .find(|(value, _)| value.to_lowercase() == key)
            {
                value
            } else {
                FALLBACK
            }
        }
    };
    mapped.to_string()
}

/// 是否为标准值（空串不算）
pub fn is_standard(raw: &str) -> bool {
    let raw = raw.trim();
    !raw.is_empty() && STANDARD.iter().any(|(value, _)| *value == raw)
}

/// 中文标签（用于前端展示；未知值原样返回）
pub fn label(raw: &str) -> &str {
    STANDARD
        .iter()
        .find(|(value, _)| *value == raw)
        .map(|(_, label)| *label)
        .unwrap_or(raw)
}

/// 把逗号分隔的类型列表（筛选参数、`type:a,b` 搜索语法）归一化成标准值并去重。
/// 空值/未知值被丢掉，不会变成 `document` 误命中。
pub fn normalize_many(raw: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for part in raw.split(',') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        let value = normalize(part);
        if value.is_empty() || !out.contains(&value) {
            if !value.is_empty() {
                out.push(value);
            }
        }
    }
    out
}

/// 标准类型 → Crossref 类型（`citation.rs` 生成 BibTeX / GB/T 7714 时用）。
///
/// 同时容忍历史同义词，保证老数据也能出正确的条目类型。
pub fn to_crossref(raw: &str) -> &'static str {
    match normalize(raw).as_str() {
        "journalArticle" => "journal-article",
        "conferencePaper" => "proceedings-article",
        "thesis" => "thesis",
        "book" => "book",
        "bookSection" => "book-chapter",
        "preprint" => "posted-content",
        "report" => "report",
        "review" => "peer-review",
        "webpage" => "reference-entry",
        // patent / presentation / manuscript / document / 未设置：Crossref 没有对应类型
        _ => "",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_maps_legacy_synonyms() {
        // 历史脏值 / 各家词表都要落到同一处
        for raw in ["article", "journalArticle", "journal", "journal-article", "Journal Article"] {
            assert_eq!(normalize(raw), "journalArticle", "raw = {raw}");
        }
        for raw in ["conference", "conferencePaper", "inproceedings", "proceedings-article"] {
            assert_eq!(normalize(raw), "conferencePaper", "raw = {raw}");
        }
        assert_eq!(normalize("book_section"), "bookSection");
        assert_eq!(normalize("book-chapter"), "bookSection");
        assert_eq!(normalize("misc"), "document");
        assert_eq!(normalize("posted-content"), "preprint");
        assert_eq!(normalize("dissertation"), "thesis");
        assert_eq!(normalize("   "), "");
        assert_eq!(normalize("完全没见过的类型"), "document");
    }

    #[test]
    fn normalize_is_idempotent_for_standard_values() {
        for (value, _) in STANDARD {
            assert_eq!(normalize(value), *value);
            assert!(is_standard(value));
        }
    }

    #[test]
    fn to_crossref_covers_standard_values() {
        assert_eq!(to_crossref("journalArticle"), "journal-article");
        assert_eq!(to_crossref("journal"), "journal-article");
        assert_eq!(to_crossref("conferencePaper"), "proceedings-article");
        assert_eq!(to_crossref("bookSection"), "book-chapter");
        assert_eq!(to_crossref(""), "");
    }

    #[test]
    fn normalize_many_handles_filters() {
        // 前端 / API / MCP 传逗号分隔的同义词，去重后落成标准值
        assert_eq!(
            normalize_many("article, journal, journalArticle"),
            vec!["journalArticle".to_string()]
        );
        assert_eq!(
            normalize_many("conference,inproceedings,book_section"),
            vec!["conferencePaper".to_string(), "bookSection".to_string()]
        );
        assert!(normalize_many("  ,, ").is_empty());
    }
}
