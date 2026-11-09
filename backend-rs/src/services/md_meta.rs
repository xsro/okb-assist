//! 从 MinerU 解析出的 Markdown 里**启发式**抽取元数据（不调用大模型）。
//!
//! 只处理「**有权威线索**」的情况，不做版面猜测：
//!
//! 1. **引用格式 / 引用本文 / Cite this article as** 引用块（`作者. 题名[J]. 刊名, 年, 卷(期): 页`）；
//! 2. **DOI**（`10.xxxx/…`，含被空格/换行拆开的写法）。
//!
//! 两者至少有其一才输出；一个都没有就返回空对象（调用方据此跳过并打日志）。
//! 这样得到的题名/作者/刊名/年份都直接来自文献自己写的引用条目，比按行序或字号猜可靠得多，
//! 单篇耗时通常 < 5 ms，适合批量回填。

use serde_json::{Map, Value};
use std::sync::OnceLock;

use super::pdf_meta::{
    char_count, collapse_spaces, contains_han, find_citation, head_before_references,
    normalize_fullwidth, parse_citation_fields, scan_doi,
};

// ── 正则（编译一次复用） ────────────────────────────────

static RE_MD_IMAGE: OnceLock<regex::Regex> = OnceLock::new();
static RE_MD_LINK: OnceLock<regex::Regex> = OnceLock::new();
static RE_MD_HTML: OnceLock<regex::Regex> = OnceLock::new();
static RE_MD_MATH: OnceLock<regex::Regex> = OnceLock::new();
static RE_MD_HEADING: OnceLock<regex::Regex> = OnceLock::new();
static RE_MD_LIST: OnceLock<regex::Regex> = OnceLock::new();
static RE_MD_QUOTE: OnceLock<regex::Regex> = OnceLock::new();
static RE_MD_AUTHOR_FRAGMENT: OnceLock<regex::Regex> = OnceLock::new();

fn re<'a>(cell: &'a OnceLock<regex::Regex>, pattern: &str) -> &'a regex::Regex {
    cell.get_or_init(|| regex::Regex::new(pattern).expect("内置 Markdown 正则无效"))
}

/// 从引用块里认的字段（其余键（volume/issue/pages）也保留，落库时会被忽略）
const KEEP_TEXT_KEYS: &[&str] = &["title", "journal", "journal_en"];
const KEEP_LIST_KEYS: &[&str] = &["authors", "authors_en"];

/// 把 Markdown 摊平成一行为单位的纯文本（去掉图片/公式/HTML/强调/标题井号…）
fn markdown_to_plain(markdown: &str) -> String {
    let mut out: Vec<String> = Vec::new();
    let mut in_fence = false;
    let mut in_math = false;
    let mut in_front = false;

    for (idx, raw) in markdown.lines().enumerate() {
        let trimmed = raw.trim();
        // 开头的 YAML front matter
        if idx == 0 && trimmed == "---" {
            in_front = true;
            continue;
        }
        if in_front {
            if trimmed == "---" {
                in_front = false;
            }
            continue;
        }
        // 代码块整段跳过（里面的 DOI/引用格式不是文献自身的）
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            in_fence = !in_fence;
            continue;
        }
        if in_fence {
            continue;
        }
        // 块级公式 `$$ … $$` 跳过
        if trimmed.starts_with("$$") && !trimmed[2..].contains("$$") {
            in_math = !in_math;
            continue;
        }
        if in_math {
            if trimmed.contains("$$") {
                in_math = false;
            }
            continue;
        }
        if let Some(line) = clean_md_line(trimmed) {
            out.push(line);
        }
    }
    out.join("\n")
}

/// 去掉行内 Markdown 语法；返回 `None` 表示这行不携带文本
fn clean_md_line(raw: &str) -> Option<String> {
    let mut s = raw.trim().to_string();
    if s.is_empty() {
        return None;
    }
    // 标题井号 / 列表符号 / 引用符号
    s = re(&RE_MD_HEADING, r"^#{1,6}\s+").replace(&s, "").to_string();
    s = re(&RE_MD_LIST, r"^(?:[-+]|\d{1,2}[.)])\s+")
        .replace(&s, "")
        .to_string();
    s = re(&RE_MD_QUOTE, r"^>\s*").replace(&s, "").to_string();
    // 图片（连替代文本一起丢）→ 链接保留文字 → HTML 标签 → 公式
    s = re(&RE_MD_IMAGE, r"!\[[^\]]*\]\([^)]*\)")
        .replace_all(&s, "")
        .to_string();
    s = re(&RE_MD_LINK, r"\[([^\]]*)\]\([^)]*\)")
        .replace_all(&s, "$1")
        .to_string();
    s = re(&RE_MD_HTML, r"<[^>]*>").replace_all(&s, "").to_string();
    s = re(&RE_MD_MATH, r"\${1,2}[^$]*\${1,2}")
        .replace_all(&s, "")
        .to_string();
    // 强调 / 行内代码 / 表格竖线 / 转义
    s = s
        .replace("**", "")
        .replace("__", "")
        .replace("~~", "")
        .replace('*', "")
        .replace('`', "")
        .replace('|', " ");
    s = s.replace("\\[", "[").replace("\\]", "]").replace("\\_", "_");
    let s = collapse_spaces(&normalize_fullwidth(&s));
    let s = s.trim().to_string();
    if s.is_empty() {
        return None;
    }
    Some(s)
}

/// `Smith, J.` / `Doe, A` 这类「姓, 名缩写」片段（英文引用里的作者单元）
fn is_author_fragment(text: &str) -> bool {
    let t = text.trim();
    if t.is_empty() || char_count(t) > 40 {
        return false;
    }
    let pattern = "^[A-Z][A-Za-z'\\-]+(?:\\s+[A-Z][a-z]+)?,\\s*(?:[A-Z]\\.?\\s*)+(?:[A-Z][A-Za-z'\\-]+)?$";
    re(&RE_MD_AUTHOR_FRAGMENT, pattern).is_match(t)
}

/// 英文引用块的题名切分修正。
///
/// `parse_citation_fields` 按「作者．题名[J]」切分（中文全角点），但英文引用是
/// `Smith, J., Doe, A. Learning to rank[J]. …`，第一个句点在作者内部，于是切出
/// `title = "Doe, A. Learning to rank"`。这里把题名开头「像作者片段」的部分挪回 `authors`。
fn fix_english_title_split(map: &mut Map<String, Value>) {
    let Some(title) = map.get("title").and_then(|v| v.as_str()).map(|s| s.to_string()) else {
        return;
    };
    let mut rest = title;
    let mut moved: Vec<String> = Vec::new();
    loop {
        let Some((head, tail)) = rest.split_once('.') else {
            break;
        };
        let head = head.trim();
        let tail = tail.trim();
        if tail.is_empty() || char_count(tail) < 4 || !is_author_fragment(head) {
            break;
        }
        moved.push(head.to_string());
        rest = tail.to_string();
    }
    if moved.is_empty() {
        return;
    }
    map.insert("title".to_string(), Value::String(rest));
    let mut names: Vec<Value> = moved.into_iter().map(Value::String).collect();
    if let Some(existing) = map.get("authors").and_then(|v| v.as_array()) {
        names.extend(existing.iter().cloned());
    }
    map.insert("authors".to_string(), Value::Array(names));
}

/// 从 Markdown 正文启发式抽取元数据（仅在存在引用格式 / 引用本文 / DOI 时）。
///
/// 返回与「大模型输出」同形的 JSON：引用块给出 `title` / `authors` / `journal` / `journal_en` /
/// `year` / `volume` / `issue` / `pages`，DOI 单独给出 `doi`。
/// 两者都没有时返回**空对象**，调用方应视为「无可用线索」并跳过。
pub fn extract_metadata_from_markdown(markdown: &str) -> Value {
    let plain = markdown_to_plain(markdown);
    if plain.trim().is_empty() {
        return Value::Object(Map::new());
    }
    // 参考文献之后的内容不参与，避免抓到别人文献的 DOI / 引用
    let head = head_before_references(&plain);

    let mut map = Map::new();

    // 1) DOI（可能是 `DOI: 10.xxxx`，也可能是被空格拆开的写法）
    if let Some(doi) = scan_doi(head) {
        map.insert("doi".to_string(), Value::String(doi));
    }

    // 2) 引用格式 / 引用本文 / Cite this article as（结构化程度最高）
    if let Some(citation) = find_citation(head) {
        for (key, value) in parse_citation_fields(&citation) {
            map.entry(key).or_insert(value);
        }
    }

    // 2.1) 英文引用 `Smith, J., Doe, A. 题名[J]` 会在第一个句点处切错，修正题名
    fix_english_title_split(&mut map);

    // 3) 只保留能落库、且来自上面两条线索的字段
    let has_text = KEEP_TEXT_KEYS
        .iter()
        .any(|k| map.get(*k).and_then(|v| v.as_str()).map(|s| !s.trim().is_empty()).unwrap_or(false));
    let has_list = KEEP_LIST_KEYS
        .iter()
        .any(|k| map.get(*k).and_then(|v| v.as_array()).map(|a| !a.is_empty()).unwrap_or(false));
    let has_doi = map
        .get("doi")
        .and_then(|v| v.as_str())
        .map(|s| !s.is_empty())
        .unwrap_or(false);

    if !(has_text || has_list || has_doi) {
        // 既没引用块也没 DOI：不猜
        return Value::Object(Map::new());
    }

    // 4) 语言：有题名时按汉字判断（供 `MetaRecord::apply` 处理 `*_en` 字段的“无意义”逻辑）
    if let Some(title) = map.get("title").and_then(|v| v.as_str()) {
        if char_count(title) >= 4 {
            let language = if contains_han(title) { "zh" } else { "en" };
            map.insert("language".to_string(), Value::String(language.to_string()));
        }
    }

    Value::Object(map)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &Value, key: &str) -> String {
        v.get(key)
            .and_then(|x| x.as_str())
            .unwrap_or_default()
            .to_string()
    }

    fn list(v: &Value, key: &str) -> Vec<String> {
        v.get(key)
            .and_then(|x| x.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|x| x.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default()
    }

    #[test]
    fn chinese_citation_block_is_parsed() {
        let md = "# 一些正文标题\n\n引用本文: 张三, 李四, 王五. 基于深度学习的遥感图像识别研究[J]. 计算机学报, 2023, 46(5): 100-110.\n\nDOI: 10.11897/SP.J.1016.2023.00100\n\n## 1 引言\n\n正文……\n";
        let meta = extract_metadata_from_markdown(md);
        assert_eq!(s(&meta, "title"), "基于深度学习的遥感图像识别研究");
        assert_eq!(list(&meta, "authors"), vec!["张三", "李四", "王五"]);
        assert_eq!(s(&meta, "journal"), "计算机学报");
        assert_eq!(meta.get("year").and_then(|v| v.as_i64()), Some(2023));
        assert_eq!(s(&meta, "doi"), "10.11897/SP.J.1016.2023.00100");
        assert_eq!(s(&meta, "language"), "zh");
    }

    #[test]
    fn english_cite_this_article_block_is_parsed() {
        let md = "Cite this article as: Smith, J., Doe, A. Learning to rank[J]. Journal of Testing, 2021, 12(3): 45-67.\n";
        let meta = extract_metadata_from_markdown(md);
        assert_eq!(s(&meta, "title"), "Learning to rank");
        assert!(!list(&meta, "authors").is_empty());
        assert_eq!(meta.get("year").and_then(|v| v.as_i64()), Some(2021));
    }

    #[test]
    fn doi_only_markdown_yields_doi_only() {
        let md = "# 编队飞行\n\n些正文。\n\nDOI:10.3969/j.issn.1001-506X.2013.01.23\n";
        let meta = extract_metadata_from_markdown(md);
        assert_eq!(s(&meta, "doi"), "10.3969/j.issn.1001-506X.2013.01.23");
        // 其余字段不猜
        assert_eq!(s(&meta, "title"), "");
        assert!(list(&meta, "authors").is_empty());
    }

    #[test]
    fn no_citation_and_no_doi_returns_empty() {
        let md = "# 题目\n\n作者甲, 作者乙\n\n摘要: ……\n\n关键词: a; b\n";
        let meta = extract_metadata_from_markdown(md);
        assert!(
            meta.as_object().map(|o| o.is_empty()).unwrap_or(false),
            "没有引用格式/DOI 时不应输出任何字段: {meta}"
        );
    }

    #[test]
    fn doi_inside_code_fence_is_ignored() {
        let md = "```\nDOI: 10.9999/fake.2020.1\n```\n\n正文没有 DOI。\n";
        let meta = extract_metadata_from_markdown(md);
        assert!(s(&meta, "doi").is_empty());
    }

    #[test]
    fn reference_section_doi_is_ignored() {
        let md = "正文。\n\n参考文献\n[1] 某某. 别的论文[J]. 某刊, 2019, 1(1): 1-9. DOI: 10.9999/other.2019.1\n";
        let meta = extract_metadata_from_markdown(md);
        assert!(s(&meta, "doi").is_empty(), "参考文献里的 DOI 不能算: {meta}");
    }

    #[test]
    fn markdown_markup_does_not_break_citation_detection() {
        let md = "> **引用格式：** 张三, 李四. 某研究方法的改进与应用[J]. 某学报, 2020, 3(2): 10-20.\n";
        let meta = extract_metadata_from_markdown(md);
        assert_eq!(s(&meta, "title"), "某研究方法的改进与应用");
    }
}
