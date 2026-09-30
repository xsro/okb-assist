//! 高级搜索查询字符串解析器。
//!
//! 解析类似 `transformer title:attention year:>2020` 的查询语法，
//! 生成结构化的搜索条件，用于构建 SQL 查询。
//!
//! 支持的语法（与前端 searchParser.ts 一致）：
//! - `keyword`                         自由文本搜索
//! - `field:value`                     字段精确/模糊匹配
//! - `field:>value`                    数值大于
//! - `field:>=value`                   数值大于等于
//! - `field:<value`                    数值小于（-1）
//! - `field:<=value`                   数值小于等于
//! - `field:min-max`                   数值范围
//! - `"exact phrase"`                  精确短语
//! - `-field:value`                    排除条件
//! - `expr1 OR expr2`                  逻辑或

use serde::Serialize;

// ── 字段定义 ────────────────────────────────────────────

/// 可搜索的文本字段（用于 LIKE 查询）
const TEXT_FIELDS: &[&str] = &[
    "title", "title_en",
    "authors", "authors_en",
    "journal", "journal_en",
    "keywords", "keywords_en",
    "abstract", "abstract_en",
    "doi", "source", "filename", "category",
];

/// 可作为 lang/language 搜索的字段（映射到 language 列）
/// 搜索字段前缀 → 映射信息
fn resolve_field(prefix: &str) -> Option<FieldInfo> {
    match prefix {
        "title" => Some(FieldInfo::text("title")),
        "author" | "authors" => Some(FieldInfo::text("authors")),
        "keyword" | "keywords" => Some(FieldInfo::text("keywords")),
        "abstract" => Some(FieldInfo::text("abstract")),
        "journal" => Some(FieldInfo::text("journal")),
        "doi" => Some(FieldInfo::text("doi")),
        "source" => Some(FieldInfo::text("source")),
        "filename" => Some(FieldInfo::text("filename")),
        "category" => Some(FieldInfo::text("category")),
        "lang" | "language" => Some(FieldInfo::text("language")),
        "type" | "doc_type" => Some(FieldInfo::text("doc_type")),
        "status" => Some(FieldInfo::enum_field("status")),
        "year" => Some(FieldInfo::number("year")),
        "sort" => Some(FieldInfo::sort("__sort__")),
        "order" => Some(FieldInfo::sort("__order__")),
        _ => None,
    }
}

#[derive(Debug, Clone)]
pub struct FieldInfo {
    /// 映射到的数据库列名
    pub column: &'static str,
    /// 字段类型
    pub field_type: FieldType,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub enum FieldType {
    /// 文本模糊匹配（LIKE）
    Text,
    /// 数值范围
    Number,
    /// 枚举精确匹配（=）
    Enum,
    /// 排序控制（不参与 WHERE）
    Sort,
}

impl FieldInfo {
    const fn text(col: &'static str) -> Self {
        Self { column: col, field_type: FieldType::Text }
    }
    const fn number(col: &'static str) -> Self {
        Self { column: col, field_type: FieldType::Number }
    }
    const fn enum_field(col: &'static str) -> Self {
        Self { column: col, field_type: FieldType::Enum }
    }
    const fn sort(col: &'static str) -> Self {
        Self { column: col, field_type: FieldType::Sort }
    }
}

// ── 状态值别名 ──────────────────────────────────────────

fn resolve_status_alias(value: &str) -> &str {
    match value.to_ascii_lowercase().as_str() {
        "uploaded" => "uploaded",
        "parsing" => "parsing",
        "done" | "markdown_done" => "markdown_done",
        "extracting" => "extracting",
        "indexing" | "indexed" => "indexed",
        "error" => "error",
        _ => value,
    }
}

// ── 解析结果类型 ────────────────────────────────────────

#[derive(Debug, Clone, Serialize)]
pub struct ParsedQuery {
    /// 自由文本 token
    pub text_tokens: Vec<String>,
    /// 字段过滤条件 (column → values, LIKE 匹配)
    pub field_filters: Vec<(String, Vec<String>)>,
    /// 数值范围条件 (column → (min, max))
    pub range_filters: Vec<(String, RangeBound)>,
    /// 排除条件
    pub negations: Vec<Negation>,
    /// OR 分组（仅有 OR 时填充）
    pub or_groups: Vec<ParsedQuery>,
    /// 排序字段
    pub sort_by: Option<String>,
    /// 排序方向
    pub sort_order: Option<String>,
    /// 原始查询
    pub raw: String,
}

#[derive(Debug, Clone, Copy, Serialize)]
pub struct RangeBound {
    pub min: Option<i64>,
    pub max: Option<i64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Negation {
    pub column: String,
    pub value: String,
    pub field_type: FieldType,
}

impl ParsedQuery {
    fn new(raw: &str) -> Self {
        Self {
            text_tokens: Vec::new(),
            field_filters: Vec::new(),
            range_filters: Vec::new(),
            negations: Vec::new(),
            or_groups: Vec::new(),
            sort_by: None,
            sort_order: None,
            raw: raw.to_string(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.raw.trim().is_empty()
    }
}

// ── 分词器 ──────────────────────────────────────────────

/// 生成查询字符串的 token 序列，支持引号分组
fn tokenize(input: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut in_quote = false;
    let mut chars = input.chars().peekable();

    while let Some(ch) = chars.next() {
        match ch {
            '"' => {
                in_quote = !in_quote;
                current.push(ch);
            }
            ' ' if !in_quote => {
                if !current.is_empty() {
                    tokens.push(current.clone());
                    current.clear();
                }
            }
            _ => {
                current.push(ch);
            }
        }
    }
    if !current.is_empty() {
        tokens.push(current);
    }
    tokens
}

// ── 主解析入口 ─────────────────────────────────────────

/// 解析查询字符串，返回 ParsedQuery
pub fn parse_query(input: &str) -> ParsedQuery {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return ParsedQuery::new("");
    }

    // 先按 OR 分割
    let or_parts = split_or(trimmed);

    if or_parts.len() > 1 {
        let groups: Vec<ParsedQuery> = or_parts
            .iter()
            .map(|p| parse_single(p.trim()))
            .collect();

        let mut result = ParsedQuery::new(trimmed);
        result.or_groups = groups;
        result
    } else {
        parse_single(trimmed)
    }
}

/// 按 OR 分割（忽略引号内的内容）
fn split_or(input: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut in_quote = false;
    let mut i = 0;
    let chars: Vec<char> = input.chars().collect();

    while i < chars.len() {
        if chars[i] == '"' {
            in_quote = !in_quote;
        }
        if !in_quote {
            // 检查 OR（前后空格包围）
            if i + 3 < chars.len()
                && (chars[i] == ' ' || chars[i] == '　')
                && (chars[i + 1] == 'O' || chars[i + 1] == 'Ｏ')
                && (chars[i + 2] == 'R' || chars[i + 2] == 'Ｒ')
                && (chars[i + 3] == ' ' || chars[i + 3] == '　')
            {
                parts.push(current.trim().to_string());
                current.clear();
                i += 4; // 跳过 " OR "
                continue;
            }
        }
        current.push(chars[i]);
        i += 1;
    }
    if !current.trim().is_empty() {
        parts.push(current.trim().to_string());
    }
    parts
}

/// 解析单个表达式（不含 OR）
fn parse_single(input: &str) -> ParsedQuery {
    let mut q = ParsedQuery::new(input);

    for token in tokenize(input) {
        // 精确短语（带引号）
        if token.starts_with('"') && token.ends_with('"') && token.len() >= 2 {
            let inner = &token[1..token.len() - 1];
            if !inner.is_empty() {
                q.text_tokens.push(inner.to_string());
            }
            continue;
        }

        // 否定前缀: -field:value
        if let Some(rest) = token.strip_prefix('-') {
            if let Some((prefix, value)) = rest.split_once(':') {
                if let Some(info) = resolve_field(prefix) {
                    if info.field_type != FieldType::Sort {
                        q.negations.push(Negation {
                            column: info.column.to_string(),
                            value: value.trim().to_string(),
                            field_type: info.field_type.clone(),
                        });
                    }
                    continue;
                }
            }
            q.text_tokens.push(token);
            continue;
        }

        // field:value pattern
        if let Some((prefix, value)) = token.split_once(':') {
            if let Some(info) = resolve_field(prefix) {
                let value = value.trim().to_string();
                match info.field_type {
                    FieldType::Text | FieldType::Enum => {
                        // 状态值别名转换
                        let val = if info.column == "status" {
                            resolve_status_alias(&value).to_string()
                        } else {
                            value
                        };
                        add_or_create_filter(&mut q.field_filters, info.column.to_string(), val);
                    }
                    FieldType::Number => {
                        if let Some(range) = parse_range(&value) {
                            q.range_filters.push((info.column.to_string(), range));
                        } else {
                            // 不能解析为数值范围，当文本搜
                            add_or_create_filter(&mut q.field_filters, info.column.to_string(), value);
                        }
                    }
                    FieldType::Sort => {
                        if info.column == "__sort__" {
                            q.sort_by = Some(value.to_ascii_lowercase());
                        } else if info.column == "__order__" {
                            let v = value.to_ascii_lowercase();
                            if v == "asc" || v == "desc" {
                                q.sort_order = Some(v);
                            }
                        }
                    }
                }
                continue;
            }
        }

        // 普通文本 token
        q.text_tokens.push(token);
    }

    q
}

/// 解析数值范围表达式
fn parse_range(value: &str) -> Option<RangeBound> {
    // >2020, >=2020, <2020, <=2020, 2020-2023, 2020
    let v = value.trim();

    if let Some(rest) = v.strip_prefix(">=") {
        if let Ok(n) = rest.trim().parse::<i64>() {
            return Some(RangeBound { min: Some(n), max: None });
        }
    }
    if let Some(rest) = v.strip_prefix("<=") {
        if let Ok(n) = rest.trim().parse::<i64>() {
            return Some(RangeBound { min: None, max: Some(n) });
        }
    }
    if let Some(rest) = v.strip_prefix('>') {
        if let Ok(n) = rest.trim().parse::<i64>() {
            return Some(RangeBound { min: Some(n + 1), max: None });
        }
    }
    if let Some(rest) = v.strip_prefix('<') {
        if let Ok(n) = rest.trim().parse::<i64>() {
            return Some(RangeBound { min: None, max: Some(n - 1) });
        }
    }

    // 范围 2020-2023
    if let Some((a, b)) = v.split_once('-') {
        if let (Ok(min), Ok(max)) = (a.trim().parse::<i64>(), b.trim().parse::<i64>()) {
            return Some(RangeBound { min: Some(min), max: Some(max) });
        }
    }

    // 精确值 2020
    if let Ok(n) = v.parse::<i64>() {
        return Some(RangeBound { min: Some(n), max: Some(n) });
    }

    None
}

fn add_or_create_filter(filters: &mut Vec<(String, Vec<String>)>, column: String, value: String) {
    if let Some((_, values)) = filters.iter_mut().find(|(c, _)| *c == column) {
        values.push(value);
    } else {
        filters.push((column, vec![value]));
    }
}

// ── SQL 构建 ────────────────────────────────────────────

use sqlx::QueryBuilder;

/// 将 ParsedQuery 构建为 SQL WHERE 子句和参数
pub struct SqlQueryParts {
    pub where_clause: String,
    pub has_conditions: bool,
}

/// 构建单个查询组的 SQL 条件（不含 WHERE 关键字）
/// 返回 (where_sql_fragment, bind_values)
pub fn build_where_clause(
    q: &ParsedQuery,
    with_fts: bool,  // 是否包含自由文本搜索
) -> (String, Vec<String>) {
    let mut conditions: Vec<String> = Vec::new();
    let mut binds: Vec<String> = Vec::new();

    // 自由文本：跨多字段 LIKE
    if with_fts && !q.text_tokens.is_empty() {
        let text = q.text_tokens.join(" ");
        let like = format!("%{}%", text);
        let ors: Vec<String> = TEXT_FIELDS
            .iter()
            .map(|f| format!("{} LIKE ?", f))
            .collect();
        conditions.push(format!("({})", ors.join(" OR ")));
        for _ in TEXT_FIELDS {
            binds.push(like.clone());
        }
    }

    // 文本字段过滤
    for (column, values) in &q.field_filters {
        if column == "status" {
            // 枚举精确匹配
            let placeholders: Vec<String> = values.iter().map(|_| "?".to_string()).collect();
            conditions.push(format!("{} IN ({})", column, placeholders.join(",")));
            binds.extend(values.iter().cloned());
        } else if column == "doc_type" {
            let placeholders: Vec<String> = values.iter().map(|_| "?".to_string()).collect();
            conditions.push(format!("{} IN ({})", column, placeholders.join(",")));
            binds.extend(values.iter().cloned());
        } else if column == "language" {
            let like = format!("%{}%", values.join(" "));
            conditions.push(format!("{} LIKE ?", column));
            binds.push(like);
        } else {
            // 文本字段 LIKE
            let like = format!("%{}%", values.join(" "));
            conditions.push(format!("{} LIKE ?", column));
            binds.push(like);
        }
    }

    // 数值范围过滤
    for (column, range) in &q.range_filters {
        if column == "year" {
            if let Some(min) = range.min {
                if let Some(max) = range.max {
                    if min == max {
                        conditions.push("year = ?".to_string());
                        binds.push(min.to_string());
                    } else {
                        conditions.push("year >= ? AND year <= ?".to_string());
                        binds.push(min.to_string());
                        binds.push(max.to_string());
                    }
                } else {
                    conditions.push("year >= ?".to_string());
                    binds.push(min.to_string());
                }
            } else if let Some(max) = range.max {
                conditions.push("year <= ?".to_string());
                binds.push(max.to_string());
            }
        }
    }

    // 否定条件
    for neg in &q.negations {
        match neg.field_type {
            FieldType::Text => {
                let like = format!("%{}%", neg.value);
                conditions.push(format!("{} NOT LIKE ?", neg.column));
                binds.push(like);
            }
            FieldType::Number | FieldType::Enum => {
                conditions.push(format!("{} != ?", neg.column));
                binds.push(neg.value.clone());
            }
            FieldType::Sort => { /* 不参与 WHERE */ }
        }
    }

    if conditions.is_empty() {
        ("1=1".to_string(), binds)
    } else {
        (conditions.join(" AND "), binds)
    }
}

/// 使用 QueryBuilder 构建完整的搜索 SQL（含 OR 分组支持）
pub fn build_search_sql<'a>(
    qb: &mut QueryBuilder<'a, sqlx::Sqlite>,
    pq: &ParsedQuery,
    columns: &str,
    with_fts: bool,
) {
    if pq.or_groups.is_empty() {
        // 无 OR：简单 AND
        let (where_sql, binds) = build_where_clause(pq, with_fts);
        qb.push("SELECT ").push(columns).push(" FROM documents WHERE ").push(&where_sql);
        for b in &binds {
            qb.push_bind(b.clone());
        }
    } else {
        // 有 OR：用括号包裹每个组，然后用 OR 连接
        qb.push("SELECT ").push(columns).push(" FROM documents WHERE ");
        for (i, group) in pq.or_groups.iter().enumerate() {
            if i > 0 {
                qb.push(" OR ");
            }
            let (where_sql, binds) = build_where_clause(group, with_fts);
            qb.push("(").push(&where_sql).push(")");
            for b in &binds {
                qb.push_bind(b.clone());
            }
        }
    }
}

/// 构建 COUNT 查询
pub fn build_count_sql<'a>(
    qb: &mut QueryBuilder<'a, sqlx::Sqlite>,
    pq: &ParsedQuery,
    with_fts: bool,
) {
    if pq.or_groups.is_empty() {
        let (where_sql, binds) = build_where_clause(pq, with_fts);
        qb.push("SELECT COUNT(*) FROM documents WHERE ").push(&where_sql);
        for b in &binds {
            qb.push_bind(b.clone());
        }
    } else {
        qb.push("SELECT COUNT(*) FROM documents WHERE ");
        for (i, group) in pq.or_groups.iter().enumerate() {
            if i > 0 {
                qb.push(" OR ");
            }
            let (where_sql, binds) = build_where_clause(group, with_fts);
            qb.push("(").push(&where_sql).push(")");
            for b in &binds {
                qb.push_bind(b.clone());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_simple_keyword() {
        let pq = parse_query("transformer");
        assert_eq!(pq.text_tokens, vec!["transformer"]);
        assert!(pq.or_groups.is_empty());
    }

    #[test]
    fn test_field_filter() {
        let pq = parse_query("title:attention");
        assert!(pq.text_tokens.is_empty());
        assert_eq!(pq.field_filters.len(), 1);
        assert_eq!(pq.field_filters[0].0, "title");
        assert_eq!(pq.field_filters[0].1, vec!["attention"]);
    }

    #[test]
    fn test_combined() {
        let pq = parse_query("transformer title:attention year:>2020");
        assert_eq!(pq.text_tokens, vec!["transformer"]);
        assert_eq!(pq.field_filters.len(), 1);
        assert_eq!(pq.field_filters[0].0, "title");
        assert_eq!(pq.range_filters.len(), 1);
        assert_eq!(pq.range_filters[0].0, "year");
        assert_eq!(pq.range_filters[0].1.min, Some(2021));
    }

    #[test]
    fn test_year_range() {
        let pq = parse_query("year:2020-2023");
        let (_, bound) = &pq.range_filters[0];
        assert_eq!(bound.min, Some(2020));
        assert_eq!(bound.max, Some(2023));
    }

    #[test]
    fn test_or() {
        let pq = parse_query("author:smith OR year:2023");
        assert_eq!(pq.or_groups.len(), 2);
        assert_eq!(pq.or_groups[0].field_filters[0].1, vec!["smith"]);
        assert_eq!(pq.or_groups[1].range_filters[0].1.min, Some(2023));
    }

    #[test]
    fn test_negation() {
        let pq = parse_query("-year:2020 transformer");
        assert_eq!(pq.negations.len(), 1);
        assert_eq!(pq.negations[0].column, "year");
        assert_eq!(pq.text_tokens, vec!["transformer"]);
    }

    #[test]
    fn test_exact_phrase() {
        let pq = parse_query("\"deep learning\" transformer");
        assert_eq!(pq.text_tokens, vec!["deep learning", "transformer"]);
    }

    #[test]
    fn test_status_alias() {
        let pq = parse_query("status:done");
        assert_eq!(pq.field_filters[0].1, vec!["markdown_done"]);
    }

    #[test]
    fn test_sort() {
        let pq = parse_query("transformer sort:year order:asc");
        assert_eq!(pq.sort_by, Some("year".to_string()));
        assert_eq!(pq.sort_order, Some("asc".to_string()));
    }

    #[test]
    fn test_tokenize() {
        let tokens = tokenize(r#"transformer title:"attention is all""#);
        assert_eq!(tokens.len(), 2);
        assert!(tokens[1].starts_with("title:"));
    }

    #[test]
    fn test_build_where_simple() {
        let pq = parse_query("transformer");
        let (sql, binds) = build_where_clause(&pq, true);
        assert!(sql.contains("LIKE"));
        assert_eq!(binds.len(), TEXT_FIELDS.len());
    }



    #[test]
    fn test_build_where_field_filter() {
        let pq = parse_query("author:smith");
        let (sql, binds) = build_where_clause(&pq, true);
        assert!(sql.contains("authors LIKE ?"));
        assert_eq!(binds.len(), 1); // 只有 field filter，无 FTS
    }

    #[test]
    fn test_empty() {
        let pq = parse_query("");
        assert!(pq.is_empty());
    }
}