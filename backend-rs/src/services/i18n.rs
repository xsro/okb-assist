//! 双语元数据（`documents.i18n`）的唯一转换入口。
//!
//! 语义：
//! - 基础列（`title` / `authors` / `journal` / `keywords` / `abstract`）存 **PDF 主要语言**；
//! - `i18n` 只存**翻译**：`{"en":{"title":…,"authors":…,"journal":…,"keywords":…,"abstract":…}}`；
//! - 基础语言为 `en`（译名即本体）或无法判定时不拆，`i18n = NULL`；
//! - 列表字段沿用 `; ` 分隔纯文本（与基础列、`list_to_db` / `parse_str_list` 完全一致）。
//!
//! **所有**「扁平元数据（模型输出 / Crossref / PDF 启发式）↔ `i18n`」的转换都必须走本模块，
//! 不要在别处另写映射（历史教训同 `services::doc_type`）。

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::models::Document;
use crate::services::pdf_meta::{contains_han, normalize_doi};

/// 单个语言下的元数据字段（键名与基础列同名）。
///
/// 列表字段（`authors` / `keywords`）存 `; ` 分隔纯文本，与基础列一致。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct I18nFields {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub authors: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub journal: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keywords: Option<String>,
    #[serde(
        default,
        rename = "abstract",
        alias = "abstract_text",
        skip_serializing_if = "Option::is_none"
    )]
    pub abstract_text: Option<String>,
}

impl I18nFields {
    pub fn is_empty(&self) -> bool {
        self.title.is_none()
            && self.authors.is_none()
            && self.journal.is_none()
            && self.keywords.is_none()
            && self.abstract_text.is_none()
    }
}

/// 语言码 → 该语言的元数据。`BTreeMap` 保证序列化顺序稳定（便于比对与日志）。
pub type I18nMap = BTreeMap<String, I18nFields>;

// ── 基础工具：语言码 ────────────────────────────────────

/// 归一化语言码：去空白、转小写、去区域后缀（`en-US` / `zh_CN` → `en` / `zh`）。
/// 只接受 2–3 个 ASCII 字母（ISO 639-1 / 常见 639-2）。
pub fn normalize_lang_key(raw: &str) -> Option<String> {
    let key = raw.trim().to_ascii_lowercase();
    if key.is_empty() {
        return None;
    }
    let primary = key.split(['-', '_']).next().unwrap_or("").trim();
    if primary.len() < 2
        || primary.len() > 3
        || !primary.chars().all(|c| c.is_ascii_alphabetic())
    {
        return None;
    }
    Some(primary.to_string())
}

/// 记录的基础语言：优先用 `language` 列，缺失时按标题是否含汉字推断（zh / en）；
/// 都无法判定时返回 `None`（此时**不拆**翻译，全部写基础列）。
pub fn base_language(declared: Option<&str>, title: Option<&str>) -> Option<String> {
    if let Some(lang) = declared.and_then(normalize_lang_key) {
        return Some(lang);
    }
    let title = title.unwrap_or("").trim();
    if title.is_empty() {
        return None;
    }
    if contains_han(title) {
        return Some("zh".to_string());
    }
    if title.chars().any(|c| c.is_ascii_alphabetic()) {
        return Some("en".to_string());
    }
    None
}

/// 单语言权威源（Crossref 等）该写到哪里：`Some(lang)` = 写 `i18n[lang]`，`None` = 写基础列。
///
/// 规则：基础语言与来源语言不同（且基础语言已知）→ 整段写翻译；
/// 相同或基础语言未知 → 写基础列（沿用旧行为）。
pub fn route_target_lang(base_lang: Option<&str>, source_lang: &str) -> Option<String> {
    let base = base_lang?;
    if base == source_lang {
        return None;
    }
    Some(source_lang.to_string())
}

/// 扁平元数据里声明的语言（Crossref 的 `language` 字段等）。
pub fn source_language(flat: &Value) -> Option<String> {
    flat.get("language")
        .and_then(|v| v.as_str())
        .and_then(normalize_lang_key)
}

// ── 解析 / 序列化 ───────────────────────────────────────

/// 规整：语言码归一、字段去空白、去空字段、去空语言。
pub fn prune(map: &I18nMap) -> I18nMap {
    let mut out = I18nMap::new();
    for (lang, fields) in map {
        let Some(lang) = normalize_lang_key(lang) else {
            continue;
        };
        let clean = I18nFields {
            title: clean_text(&fields.title),
            authors: clean_text(&fields.authors),
            journal: clean_text(&fields.journal),
            keywords: clean_text(&fields.keywords),
            abstract_text: clean_text(&fields.abstract_text),
        };
        if clean.is_empty() {
            continue;
        }
        // 同一语言码出现多次（如 `en` 与 `en-US` 归一后相同）时只补空，不丢数据
        match out.get_mut(&lang) {
            Some(existing) => merge_fields_missing(existing, &clean),
            None => {
                out.insert(lang, clean);
            }
        }
    }
    out
}

fn clean_text(raw: &Option<String>) -> Option<String> {
    let trimmed = raw.as_deref().unwrap_or("").trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

/// 读数据库列：`NULL` / 非法 JSON → 空表（打 `warn`，绝不 panic、不 500）。
pub fn parse(raw: Option<&str>) -> I18nMap {
    let raw = raw.unwrap_or("").trim();
    if raw.is_empty() {
        return I18nMap::new();
    }
    match from_value(&serde_json::from_str::<Value>(raw).unwrap_or(Value::Null)) {
        Ok(map) => map,
        Err(e) => {
            let preview: String = raw.chars().take(200).collect();
            tracing::warn!("i18n 字段解析失败（按空处理）：{}；原文前 200 字={}", e, preview);
            I18nMap::new()
        }
    }
}

/// 从 JSON 值构造（手工编辑入口用，非法结构返回错误）。
pub fn from_value(value: &Value) -> Result<I18nMap, String> {
    match value {
        Value::Null => Ok(I18nMap::new()),
        Value::Object(obj) => {
            let mut map = I18nMap::new();
            for (lang, fields) in obj {
                let lang = normalize_lang_key(lang)
                    .ok_or_else(|| format!("非法语言码：{}", lang))?;
                if !fields.is_object() {
                    return Err(format!("语言 {} 的值必须是对象", lang));
                }
                let parsed: I18nFields = serde_json::from_value(fields.clone())
                    .map_err(|e| format!("语言 {} 的结构非法：{}", lang, e))?;
                map.insert(lang, parsed);
            }
            Ok(prune(&map))
        }
        _ => Err("i18n 必须是对象（语言码 → 字段）或 null".to_string()),
    }
}

/// 写数据库列：规整后为空 → `NULL`（与其它可空列语义一致）。
pub fn to_db(map: &I18nMap) -> Option<String> {
    let clean = prune(map);
    if clean.is_empty() {
        return None;
    }
    serde_json::to_string(&clean).ok()
}

/// 下发给前端 / MCP：恒定对象（空表 → `{}`），非法内容已被规整。
pub fn to_value(map: &I18nMap) -> Value {
    serde_json::to_value(prune(map)).unwrap_or_else(|_| json!({}))
}

/// 解析数据库列并直接转成下发用的 JSON 对象。
pub fn value_from_db(raw: Option<&str>) -> Value {
    to_value(&parse(raw))
}

// ── 扁平元数据 ↔ i18n ──────────────────────────────────

/// 从扁平元数据的 `*_en` 键提取英文翻译（基础语言为 `en` / 未知时返回空）。
pub fn translations_from_flat(flat: &Value, base_lang: Option<&str>) -> I18nMap {
    let mut map = I18nMap::new();
    let Some(base) = base_lang else {
        return map;
    };
    if base == "en" {
        return map;
    }
    let fields = I18nFields {
        title: flat_text(flat, "title_en"),
        authors: flat_list_text(flat, "authors_en"),
        journal: flat_text(flat, "journal_en"),
        keywords: flat_list_text(flat, "keywords_en"),
        abstract_text: flat_text(flat, "abstract_en"),
    };
    if !fields.is_empty() {
        map.insert("en".to_string(), fields);
    }
    map
}

/// 从扁平元数据的**基础键**提取单语言字段（Crossref 等权威源路由用）。
pub fn fields_from_flat(flat: &Value) -> I18nFields {
    I18nFields {
        title: flat_text(flat, "title"),
        authors: flat_list_text(flat, "authors"),
        journal: flat_text(flat, "journal"),
        keywords: flat_list_text(flat, "keywords"),
        abstract_text: flat_text(flat, "abstract"),
    }
}

/// 用五个字段拼出一个语言的条目（`MetaRecord` 落库、测试等用）。
pub fn single_lang_map(lang: &str, fields: I18nFields) -> I18nMap {
    let mut map = I18nMap::new();
    if fields.is_empty() {
        return map;
    }
    if let Some(lang) = normalize_lang_key(lang) {
        map.insert(lang, fields);
    }
    map
}

/// 从模型/PDF/Crossref 的扁平输出补空写入文档：基础列只补空，翻译只补空。
///
/// 与旧的 `fill_meta_fields` 行为一致，只是 `*_en` 改为落 `i18n`。
pub fn fill_doc_missing(doc: &mut Document, meta: &Value) {
    // 基础列：只补空
    if doc.title.as_deref().map_or(true, |s| s.trim().is_empty()) {
        if let Some(t) = flat_text(meta, "title") {
            doc.title = Some(t);
        }
    }
    if doc.authors.as_deref().map_or(true, |s| s.trim().is_empty()) {
        if let Some(a) = flat_list_text(meta, "authors") {
            doc.authors = Some(a);
        }
    }
    if doc.year.is_none() {
        if let Some(y) = meta.get("year").and_then(|v| v.as_i64()) {
            doc.year = Some(y);
        }
    }
    if doc.doi.as_deref().map_or(true, |s| s.trim().is_empty()) {
        if let Some(d) = meta.get("doi").and_then(|v| v.as_str()) {
            doc.doi = normalize_doi(d);
        }
    }
    if doc
        .keywords
        .as_deref()
        .map_or(true, |s| s.trim().is_empty())
    {
        if let Some(k) = flat_list_text(meta, "keywords") {
            doc.keywords = Some(k);
        }
    }
    if doc
        .abstract_text
        .as_deref()
        .map_or(true, |s| s.trim().is_empty())
    {
        if let Some(a) = flat_text(meta, "abstract") {
            doc.abstract_text = Some(a);
        }
    }
    if doc.journal.as_deref().map_or(true, |s| s.trim().is_empty()) {
        if let Some(j) = flat_text(meta, "journal") {
            doc.journal = Some(j);
        }
    }

    // 翻译：语言未知 → 不拆（全部留在基础列）；英文文献 → 不留 en 镜像
    let Some(base) = base_language(doc.language.as_deref(), doc.title.as_deref()) else {
        return;
    };
    let mut map = parse(doc.i18n.as_deref());
    if base == "en" {
        if map.remove("en").is_some() {
            doc.i18n = to_db(&map);
        }
        return;
    }
    let incoming = translations_from_flat(meta, Some(&base));
    let before = map.clone();
    merge_missing(&mut map, &incoming);
    drop_translations_equal_to_base(&mut map, doc);
    if map != before {
        doc.i18n = to_db(&map);
    }
}

/// 丢掉与基础列完全相同的翻译值（英文文献的镜像、同一段文字重复填写）。
pub fn drop_translations_equal_to_base(map: &mut I18nMap, doc: &Document) {
    let base_lang = base_language(doc.language.as_deref(), doc.title.as_deref());
    let base = [
        doc.title.as_deref().unwrap_or("").trim(),
        doc.authors.as_deref().unwrap_or("").trim(),
        doc.journal.as_deref().unwrap_or("").trim(),
        doc.keywords.as_deref().unwrap_or("").trim(),
        doc.abstract_text.as_deref().unwrap_or("").trim(),
    ];
    for (lang, fields) in map.iter_mut() {
        if base_lang.as_deref() == Some(lang.as_str()) {
            // 基础语言不允许出现翻译条目
            *fields = I18nFields::default();
            continue;
        }
        let values = [
            &mut fields.title,
            &mut fields.authors,
            &mut fields.journal,
            &mut fields.keywords,
            &mut fields.abstract_text,
        ];
        for (slot, base_value) in values.into_iter().zip(base.iter()) {
            if !base_value.is_empty() && slot.as_deref() == Some(*base_value) {
                *slot = None;
            }
        }
    }
    map.retain(|_, f| !f.is_empty());
}

// ── 合并 ────────────────────────────────────────────────

/// 只补空（目标已有的值一律不动）。
pub fn merge_missing(target: &mut I18nMap, source: &I18nMap) {
    for (lang, fields) in source {
        let Some(lang) = normalize_lang_key(lang) else {
            continue;
        };
        match target.get_mut(&lang) {
            Some(existing) => merge_fields_missing(existing, fields),
            None => {
                let clean = I18nFields {
                    title: clean_text(&fields.title),
                    authors: clean_text(&fields.authors),
                    journal: clean_text(&fields.journal),
                    keywords: clean_text(&fields.keywords),
                    abstract_text: clean_text(&fields.abstract_text),
                };
                if !clean.is_empty() {
                    target.insert(lang, clean);
                }
            }
        }
    }
}

fn merge_fields_missing(target: &mut I18nFields, source: &I18nFields) {
    macro_rules! fill {
        ($field:ident) => {
            if target.$field.is_none() {
                target.$field = clean_text(&source.$field);
            }
        };
    }
    fill!(title);
    fill!(authors);
    fill!(journal);
    fill!(keywords);
    fill!(abstract_text);
}

/// 覆盖（来源非空值覆盖目标，空值保留目标）——`run_crossref_override` 用。
pub fn merge_prefer(target: &mut I18nMap, source: &I18nMap) {
    for (lang, fields) in source {
        let Some(lang) = normalize_lang_key(lang) else {
            continue;
        };
        let entry = target.entry(lang).or_default();
        macro_rules! prefer {
            ($field:ident) => {
                if let Some(v) = clean_text(&fields.$field) {
                    entry.$field = Some(v);
                }
            };
        }
        prefer!(title);
        prefer!(authors);
        prefer!(journal);
        prefer!(keywords);
        prefer!(abstract_text);
    }
}

// ── 列表字段（`; ` 分隔纯文本）────────────────────────────

/// 数据库列 → 列表：先试 JSON 数组字符串（历史格式），再按 `; , 、 ，` 拆分。
pub fn parse_str_list(raw: Option<&str>) -> Vec<String> {
    let raw = raw.unwrap_or("").trim();
    if raw.is_empty() {
        return Vec::new();
    }
    if let Ok(Value::Array(items)) = serde_json::from_str::<Value>(raw) {
        return items
            .iter()
            .filter_map(|v| v.as_str().map(|s| s.trim().to_string()))
            .filter(|s| !s.is_empty())
            .collect();
    }
    raw.split([';', ',', '、', '，'])
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

/// 列表 → 数据库列 / i18n 字段：`; ` 分隔纯文本（空列表 → 空串）。
pub fn list_to_db(items: &[String]) -> String {
    items
        .iter()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("; ")
}

// ── 扁平 JSON 取值 ──────────────────────────────────────

fn flat_text(flat: &Value, key: &str) -> Option<String> {
    match flat.get(key)? {
        Value::String(s) => clean_text(&Some(s.clone())),
        Value::Array(items) => {
            let text = list_to_db(
                &items
                    .iter()
                    .filter_map(|v| v.as_str())
                    .map(String::from)
                    .collect::<Vec<_>>(),
            );
            clean_text(&Some(text))
        }
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

fn flat_list_text(flat: &Value, key: &str) -> Option<String> {
    let raw = flat.get(key)?;
    let text = match raw {
        Value::Array(items) => list_to_db(
            &items
                .iter()
                .filter_map(|v| v.as_str())
                .map(String::from)
                .collect::<Vec<_>>(),
        ),
        Value::String(s) => list_to_db(&parse_str_list(Some(s))),
        _ => String::new(),
    };
    clean_text(&Some(text))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_language_keys() {
        assert_eq!(normalize_lang_key("EN-us").as_deref(), Some("en"));
        assert_eq!(normalize_lang_key("zh_CN").as_deref(), Some("zh"));
        assert_eq!(normalize_lang_key(""), None);
        assert_eq!(normalize_lang_key("english"), None);
    }

    #[test]
    fn prunes_empty_values() {
        let mut map = I18nMap::new();
        map.insert(
            "en".into(),
            I18nFields {
                title: Some("  ".into()),
                ..Default::default()
            },
        );
        assert!(to_db(&map).is_none());
    }

    #[test]
    fn roundtrip_and_merge() {
        let mut map = I18nMap::new();
        map.insert(
            "en".into(),
            I18nFields {
                title: Some("Title".into()),
                authors: Some("A; B".into()),
                ..Default::default()
            },
        );
        let db = to_db(&map).unwrap();
        assert_eq!(parse(Some(&db)), map);

        let incoming = I18nMap::from([(
            "en".to_string(),
            I18nFields {
                title: Some("New".into()),
                journal: Some("J".into()),
                ..Default::default()
            },
        )]);
        let mut missing = map.clone();
        merge_missing(&mut missing, &incoming);
        assert_eq!(missing["en"].title.as_deref(), Some("Title"));
        assert_eq!(missing["en"].journal.as_deref(), Some("J"));

        let mut preferred = map.clone();
        merge_prefer(&mut preferred, &incoming);
        assert_eq!(preferred["en"].title.as_deref(), Some("New"));
    }

    #[test]
    fn routes_english_source_into_translation_for_chinese_docs() {
        assert_eq!(
            route_target_lang(Some("zh"), "en").as_deref(),
            Some("en")
        );
        assert_eq!(route_target_lang(Some("en"), "en"), None);
        assert_eq!(route_target_lang(None, "en"), None);
    }
}
