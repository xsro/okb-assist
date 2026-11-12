//! 检索式解析器（Web of Science 风格，兼容旧 `field:value` 写法）。
//!
//! 语法（大小写不敏感）：
//!
//! ```text
//! TS=(transformer OR attention) AND PY=2020-2023
//! AU=Vaswani* AND (TI="attention is all you need" OR AB=transformer)
//! DO=10.1000/xyz NOT DT=review
//! ALL=transformer AND PDF=present AND HEALTH=ok
//! ```
//!
//! - **字段标签**：`TAG=value`（WoS 写法）与 `TAG:value`（旧写法）等价，值含空格时用引号：
//!   `SO="IEEE TRANSACTIONS ON …"`。
//! - **布尔运算符**：`AND` / `OR` / `NOT`（也接受 `&&` / `||` / `!`、旧写法的 `-field:value`）；
//!   相邻条件之间不写运算符时按 `AND` 处理（隐式 AND）。
//! - **括号**：`( … )` 任意嵌套；字段后可直接跟括号组，组内裸值套用该字段：
//!   `TS=(transformer OR attention)`、`PY=(2020 OR 2021)`。
//! - **通配符**：`*` 任意字符、`?` 单个字符、`$` 0 或 1 个字符（`$` 按 `%` 近似）；
//!   不含通配符时按「包含」匹配（两侧自动补 `%`），含通配符时按原样匹配（`ATT*` = 前缀）。
//! - **数值**：`PY=2023` / `PY=2020-2023` / `PY>2020` / `PY<=2023`；文件大小支持单位
//!   （`filesize_pdf>1MB`、`filesize_md=10KB-2MB`）。
//! - **排序**：`SORT=PY ORDER=asc`（等价旧写法 `sort:year order:asc`），不参与 WHERE。
//!
//! 字段标签表见 [`resolve_tag`]；未知 `TAG=` 直接报错（400），未知 `tag:` 按旧行为当成自由文本
//! （否则 `https://…` 这类值会被误判）。
//!
//! 解析结果由 [`compile`] 直接编译为 SQL 片段 + 绑定值，供 `/documents/search-info` 使用。
//! **不要把 `%` / `_` 直接拼进 SQL**：文本值统一经 [`like_pattern`] 转义，SQL 里带 `ESCAPE '\'`。

use serde::Serialize;

use crate::services::file_state::{self, FileKind};

// ── 文本列（与 `services::i18n` 的双语语义一致：基础列 + `i18n.en.*`） ──

const C_TITLE: &[&str] = &["title", "json_extract(i18n, '$.en.title')"];
const C_AUTHORS: &[&str] = &["authors", "json_extract(i18n, '$.en.authors')"];
const C_JOURNAL: &[&str] = &["journal", "json_extract(i18n, '$.en.journal')"];
const C_KEYWORDS: &[&str] = &["keywords", "json_extract(i18n, '$.en.keywords')"];
const C_ABSTRACT: &[&str] = &["abstract", "json_extract(i18n, '$.en.abstract')"];
const C_DOI: &[&str] = &["doi"];
const C_SOURCE: &[&str] = &["source"];
const C_FILENAME: &[&str] = &["filename"];
const C_CATEGORY: &[&str] = &["category"];
const C_LANGUAGE: &[&str] = &["language"];

/// `TS=` 主题检索：标题 + 作者关键词 + 摘要（WoS 的 Title / Author Keywords / Abstract 语义）
const C_TOPIC: &[&str] = &[
    "title",
    "json_extract(i18n, '$.en.title')",
    "keywords",
    "json_extract(i18n, '$.en.keywords')",
    "abstract",
    "json_extract(i18n, '$.en.abstract')",
];

/// `ALL=` / 自由文本：全部元数据列（**与旧的自由文本检索范围一致**，别随手增删）
const C_ALL: &[&str] = &[
    "title",
    "json_extract(i18n, '$.en.title')",
    "authors",
    "json_extract(i18n, '$.en.authors')",
    "journal",
    "json_extract(i18n, '$.en.journal')",
    "keywords",
    "json_extract(i18n, '$.en.keywords')",
    "abstract",
    "json_extract(i18n, '$.en.abstract')",
    "doi",
    "source",
    "filename",
    "category",
];

// ── 字段标签 ────────────────────────────────────────────

/// 一个字段标签的语义。
#[derive(Debug, Clone, Copy)]
enum FieldSpec {
    /// 文本列集合，任一列 LIKE 命中即算命中
    Text(&'static [&'static str]),
    /// 数值列（支持 `=` / `>` / `>=` / `<` / `<=` / `a-b` 范围）
    Number(&'static str),
    /// 枚举列（逗号多选 → `IN (…)`，值先过 `doc_type::normalize`）
    Enum(&'static str),
    /// 文件事实三态（`present` / `absent` / `unknown`）
    State(FileKind),
    /// PDF 健康（`ok` / `warning` / `error` / `missing` / `unchecked`）
    Health,
    /// 排序字段（不参与 WHERE）
    SortBy,
    /// 排序方向（不参与 WHERE）
    SortOrder,
}

/// 字段标签 / 旧前缀 → 语义。**新增字段只在这里加一行**，不要在别处另造映射表。
fn resolve_tag(raw: &str) -> Option<FieldSpec> {
    let name = raw.trim().to_ascii_lowercase();
    Some(match name.as_str() {
        // 文本（WoS 标签 + 旧前缀，二者等价）
        "ti" | "title" => FieldSpec::Text(C_TITLE),
        "au" | "author" | "authors" => FieldSpec::Text(C_AUTHORS),
        "so" | "journal" | "publication" => FieldSpec::Text(C_JOURNAL),
        "ab" | "abstract" => FieldSpec::Text(C_ABSTRACT),
        "ak" | "kw" | "keyword" | "keywords" => FieldSpec::Text(C_KEYWORDS),
        "do" | "doi" => FieldSpec::Text(C_DOI),
        "src" | "source" => FieldSpec::Text(C_SOURCE),
        "fn" | "filename" => FieldSpec::Text(C_FILENAME),
        "cat" | "category" => FieldSpec::Text(C_CATEGORY),
        "la" | "lang" | "language" => FieldSpec::Text(C_LANGUAGE),
        // 多列
        "ts" | "topic" => FieldSpec::Text(C_TOPIC),
        "all" | "ft" | "any" => FieldSpec::Text(C_ALL),
        // 数值
        "py" | "year" => FieldSpec::Number("year"),
        "id" | "an" => FieldSpec::Number("id"),
        "filesize_pdf" | "pdf_size" => FieldSpec::Number("pdf_size"),
        "filesize_md" | "md_size" => FieldSpec::Number("md_size"),
        "filesize_zip" | "zip_size" => FieldSpec::Number("zip_size"),
        // 枚举
        "dt" | "type" | "doc_type" => FieldSpec::Enum("doc_type"),
        // 文件事实
        "pdf" | "has_pdf" => FieldSpec::State(FileKind::Pdf),
        "md" | "has_md" => FieldSpec::State(FileKind::Md),
        "zip" | "has_zip" => FieldSpec::State(FileKind::Zip),
        "health" | "pdf_health" => FieldSpec::Health,
        // 排序
        "sort" | "sort_by" => FieldSpec::SortBy,
        "order" | "sort_order" => FieldSpec::SortOrder,
        _ => return None,
    })
}

/// 排序字段归一化：`SORT=PY` / `sort:year` 都映射到列名（其余原样小写交给调用方兜底）。
fn normalize_sort(raw: &str) -> String {
    let v = raw.trim().to_ascii_lowercase();
    match v.as_str() {
        "py" | "y" => "year".to_string(),
        "ti" => "title".to_string(),
        "au" => "authors".to_string(),
        "so" => "journal".to_string(),
        "dt" => "doc_type".to_string(),
        "an" => "id".to_string(),
        "filesize_pdf" => "pdf_size".to_string(),
        "filesize_md" => "md_size".to_string(),
        "filesize_zip" => "zip_size".to_string(),
        _ => v,
    }
}

// ── 错误 ────────────────────────────────────────────────

/// 检索式语法错误（端点应回 400，`position` 是 0-based 字符下标，便于前端高亮）。
#[derive(Debug, Clone, Serialize)]
pub struct QueryError {
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub position: Option<usize>,
}

impl QueryError {
    fn at(message: impl Into<String>, position: usize) -> Self {
        Self { message: message.into(), position: Some(position) }
    }
    fn plain(message: impl Into<String>) -> Self {
        Self { message: message.into(), position: None }
    }
}

// ── 分词 ────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    LParen,
    RParen,
    And,
    Or,
    Not,
    /// 字段标签：`(名字, 分隔符)`，分隔符是 `'='` / `':'` / `'>'` / `'>='` / `'<'` / `'<='`
    Field(String, String),
    Word(String),
    Phrase(String),
}

#[derive(Debug, Clone)]
struct Spanned {
    tok: Tok,
    pos: usize,
}

/// 分词：括号、引号短语、布尔运算符、字段标签、普通值。
fn lex(input: &str) -> Result<Vec<Spanned>, QueryError> {
    let chars: Vec<char> = input.chars().collect();
    let mut out: Vec<Spanned> = Vec::new();
    let mut i = 0usize;

    while i < chars.len() {
        let c = chars[i];
        if c.is_whitespace() {
            i += 1;
            continue;
        }
        let start = i;
        match c {
            '(' => {
                out.push(Spanned { tok: Tok::LParen, pos: start });
                i += 1;
            }
            ')' => {
                out.push(Spanned { tok: Tok::RParen, pos: start });
                i += 1;
            }
            '"' | '\'' => {
                let quote = c;
                i += 1;
                let mut buf = String::new();
                let mut closed = false;
                while i < chars.len() {
                    if chars[i] == '\\' && i + 1 < chars.len() {
                        // 支持 `\"` / `\'` / `\\`
                        let next = chars[i + 1];
                        if next == quote || next == '\\' {
                            buf.push(next);
                            i += 2;
                            continue;
                        }
                    }
                    if chars[i] == quote {
                        closed = true;
                        i += 1;
                        break;
                    }
                    buf.push(chars[i]);
                    i += 1;
                }
                if !closed {
                    return Err(QueryError::at("引号未闭合（缺少结尾的引号）", start));
                }
                out.push(Spanned { tok: Tok::Phrase(buf), pos: start });
            }
            '-' => {
                // 旧写法的否定前缀：`-year:2020` / `-transformer`
                if i + 1 < chars.len() && !chars[i + 1].is_whitespace() {
                    out.push(Spanned { tok: Tok::Not, pos: start });
                    i += 1;
                } else {
                    return Err(QueryError::at("`-` 后面缺少条件", start));
                }
            }
            _ => {
                // 读到分隔符为止（空白 / 括号 / 引号）
                let mut buf = String::new();
                while i < chars.len() {
                    let ch = chars[i];
                    if ch.is_whitespace() || ch == '(' || ch == ')' || ch == '"' || ch == '\'' {
                        break;
                    }
                    buf.push(ch);
                    i += 1;
                }
                push_word(&mut out, buf, start)?;
            }
        }
    }

    Ok(out)
}

/// 把一个「词」变成 token：布尔运算符 / 字段标签 / 普通值。
fn push_word(out: &mut Vec<Spanned>, raw: String, pos: usize) -> Result<(), QueryError> {
    let lower = raw.to_ascii_lowercase();

    // 1) 布尔运算符（`NEAR` / `SAME` 退化为 AND，见模块注释）
    match lower.as_str() {
        "and" | "&&" | "near" | "same" => {
            out.push(Spanned { tok: Tok::And, pos });
            return Ok(());
        }
        "or" | "||" => {
            out.push(Spanned { tok: Tok::Or, pos });
            return Ok(());
        }
        "not" | "!" => {
            out.push(Spanned { tok: Tok::Not, pos });
            return Ok(());
        }
        _ => {}
    }
    if lower.starts_with("near/") || lower.starts_with("same/") {
        out.push(Spanned { tok: Tok::And, pos });
        return Ok(());
    }

    // 2) 字段标签：`TAG=` / `TAG:` / `TAG>=` …
    if let Some((name, sep, rest)) = split_tag(&raw) {
        out.push(Spanned { tok: Tok::Field(name.to_string(), sep.to_string()), pos });
        if !rest.is_empty() {
            let value = if sep == ">" || sep == ">=" || sep == "<" || sep == "<=" {
                // `PY>=2020` → 值带上比较符，交给数值解析
                format!("{}{}", sep, rest)
            } else {
                rest.to_string()
            };
            out.push(Spanned { tok: Tok::Word(value), pos });
        }
        return Ok(());
    }

    // 3) 普通值
    out.push(Spanned { tok: Tok::Word(raw), pos });
    Ok(())
}

/// 识别开头的字段标签：`标识符` + (`=` / `:` / `>` / `>=` / `<` / `<=`)。
///
/// 返回 `(标签名, 分隔符, 剩余部分)`；不是标签时返回 `None`（如 `2020-2023`、`10.1000/x`）。
fn split_tag(raw: &str) -> Option<(&str, &str, &str)> {
    let bytes = raw.as_bytes();
    if bytes.is_empty() || !(bytes[0].is_ascii_alphabetic() || bytes[0] == b'_') {
        return None;
    }
    let mut i = 0usize;
    while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
        i += 1;
    }
    let name = &raw[..i];
    let rest = &raw[i..];
    for sep in [">=", "<=", "=", ":", ">", "<"] {
        if let Some(tail) = rest.strip_prefix(sep) {
            // `a>=b` 里 `>` 优先匹配 `>=`（上面的顺序保证）
            return Some((name, sep, tail));
        }
    }
    None
}

// ── AST ─────────────────────────────────────────────────

#[derive(Debug, Clone)]
enum Node {
    /// 恒真（空的排序条件等）
    True,
    And(Vec<Node>),
    Or(Vec<Node>),
    Not(Box<Node>),
    /// 文本：任一列 LIKE 命中
    Text { columns: &'static [&'static str], pattern: String },
    /// 数值范围（min == max 时按等值）
    Range { column: &'static str, min: Option<i64>, max: Option<i64> },
    /// 枚举多选
    Enum { column: &'static str, values: Vec<String> },
    /// 字面量 SQL（文件事实三态，不占绑定参数）
    Raw(String),
    /// PDF 健康（可能带绑定值）
    Health { values: Vec<String>, include_null: bool },
}

fn combine_and(mut terms: Vec<Node>) -> Option<Node> {
    terms.retain(|n| !matches!(n, Node::True));
    match terms.len() {
        0 => None,
        1 => terms.pop(),
        _ => Some(Node::And(terms)),
    }
}

fn combine_or(mut terms: Vec<Node>) -> Option<Node> {
    terms.retain(|n| !matches!(n, Node::True));
    match terms.len() {
        0 => None,
        1 => terms.pop(),
        _ => Some(Node::Or(terms)),
    }
}

// ── 语法分析（递归下降） ────────────────────────────────

struct Parser {
    toks: Vec<Spanned>,
    i: usize,
    /// 字段上下文：字段后跟括号组时，组内的裸值套用该字段（`TS=(a OR b)`）
    ctx: Option<FieldSpec>,
    sort_by: Option<String>,
    sort_order: Option<String>,
}

impl Parser {
    fn new(toks: Vec<Spanned>) -> Self {
        Self { toks, i: 0, ctx: None, sort_by: None, sort_order: None }
    }

    fn peek(&self) -> Option<&Tok> {
        self.toks.get(self.i).map(|s| &s.tok)
    }

    fn next(&mut self) -> Option<Spanned> {
        let sp = self.toks.get(self.i).cloned();
        if sp.is_some() {
            self.i += 1;
        }
        sp
    }

    fn parse(&mut self) -> Result<Node, QueryError> {
        let node = self.parse_or()?.unwrap_or(Node::True);
        if let Some(sp) = self.toks.get(self.i) {
            return Err(QueryError::at(
                format!("多余的内容：`{}`", tok_text(&sp.tok)),
                sp.pos,
            ));
        }
        Ok(node)
    }

    /// `or_expr := and_expr (OR and_expr)*`
    fn parse_or(&mut self) -> Result<Option<Node>, QueryError> {
        let mut terms: Vec<Node> = Vec::new();
        if let Some(n) = self.parse_and()? {
            terms.push(n);
        }
        while matches!(self.peek(), Some(Tok::Or)) {
            let sp = self.next().expect("peek 已确认存在");
            self.ensure_operand("OR", sp.pos)?;
            terms.push(self.parse_and()?.expect("ensure_operand 已确认不是空条件"));
        }
        Ok(combine_or(terms))
    }

    /// `and_expr := unary ((AND)? unary)*`（相邻条件按隐式 AND）
    fn parse_and(&mut self) -> Result<Option<Node>, QueryError> {
        let mut terms: Vec<Node> = Vec::new();
        loop {
            match self.peek() {
                None | Some(Tok::RParen) | Some(Tok::Or) => break,
                Some(Tok::And) => {
                    let sp = self.next().expect("peek 已确认存在");
                    self.ensure_operand("AND", sp.pos)?;
                    terms.push(self.parse_not()?);
                }
                _ => terms.push(self.parse_not()?),
            }
        }
        Ok(combine_and(terms))
    }

    /// 运算符后面必须紧跟一个条件，否则报带位置的语法错误
    fn ensure_operand(&self, op: &str, pos: usize) -> Result<(), QueryError> {
        match self.peek() {
            None | Some(Tok::RParen) | Some(Tok::And) | Some(Tok::Or) => Err(QueryError::at(
                format!("`{}` 后面缺少条件", op),
                pos,
            )),
            _ => Ok(()),
        }
    }

    /// `unary := NOT unary | primary`
    fn parse_not(&mut self) -> Result<Node, QueryError> {
        if matches!(self.peek(), Some(Tok::Not)) {
            let sp = self.next().expect("peek 已确认存在");
            self.ensure_operand("NOT", sp.pos)?;
            let inner = self.parse_not()?;
            return Ok(Node::Not(Box::new(inner)));
        }
        self.parse_primary()
    }

    fn parse_primary(&mut self) -> Result<Node, QueryError> {
        let sp = self
            .next()
            .ok_or_else(|| QueryError::at("查询意外结束（这里应该还有一个条件）", 0))?;
        match sp.tok {
            Tok::LParen => {
                let saved = self.ctx;
                let inner = self.parse_or()?;
                if !matches!(self.peek(), Some(Tok::RParen)) {
                    self.ctx = saved;
                    return Err(QueryError::at("括号未闭合（缺少 `)`）", sp.pos));
                }
                self.next();
                self.ctx = saved;
                inner.ok_or_else(|| QueryError::at("括号里没有条件", sp.pos))
            }
            Tok::Field(name, sep) => self.parse_field(&name, &sep, sp.pos),
            Tok::Phrase(p) => self.leaf_from_value(p, sp.pos),
            Tok::Word(w) => self.leaf_from_value(w, sp.pos),
            Tok::RParen => Err(QueryError::at("多余的 `)`", sp.pos)),
            Tok::And | Tok::Or => Err(QueryError::at(
                format!("运算符 `{}` 前面缺少条件", tok_text(&sp.tok)),
                sp.pos,
            )),
            Tok::Not => Err(QueryError::at("`NOT` 后面缺少条件", sp.pos)),
        }
    }

    /// 字段 + 值。文本字段支持后跟括号组（`TS=(a OR b)`）。
    fn parse_field(&mut self, name: &str, sep: &str, pos: usize) -> Result<Node, QueryError> {
        let spec = match resolve_tag(name) {
            Some(s) => s,
            None => {
                if sep == "=" {
                    return Err(QueryError::at(
                        format!("未知字段标签 `{}`（大写标签见语法提示，如 TS / TI / AU / SO / PY / AB / AK / DO / DT / ALL）", name.to_uppercase()),
                        pos,
                    ));
                }
                // 旧写法里的未知前缀：整体当自由文本（`https://x`、`10.1/x:y` 之类）
                let value = match self.next() {
                    Some(Spanned { tok: Tok::Word(w), .. }) => w,
                    Some(Spanned { tok: Tok::Phrase(p), .. }) => p,
                    _ => return Ok(Node::True),
                };
                return self.leaf_from_value(format!("{}:{}", name, value), pos);
            }
        };

        match spec {
            FieldSpec::SortBy => {
                let raw = self.take_value_text(name, sep, pos)?;
                self.sort_by = Some(normalize_sort(&raw));
                Ok(Node::True)
            }
            FieldSpec::SortOrder => {
                let raw = self.take_value_text(name, sep, pos)?;
                let v = raw.trim().to_ascii_lowercase();
                if v != "asc" && v != "desc" {
                    return Err(QueryError::at(
                        format!("排序方向只能是 asc 或 desc，实际是 `{}`", raw),
                        pos,
                    ));
                }
                self.sort_order = Some(v);
                Ok(Node::True)
            }
            FieldSpec::Number(column) => {
                let raw = self.take_value_text(name, sep, pos)?;
                let range = parse_range(&raw).ok_or_else(|| {
                    QueryError::at(
                        format!(
                            "`{}` 需要数值或范围（如 2023 / 2020-2023 / >2020），实际是 `{}`",
                            name.to_uppercase(),
                            raw
                        ),
                        pos,
                    )
                })?;
                Ok(Node::Range { column, min: range.min, max: range.max })
            }
            FieldSpec::Enum(column) => {
                let raw = self.take_value_text(name, sep, pos)?;
                let values = super::doc_type::normalize_many(&raw);
                if values.is_empty() {
                    return Err(QueryError::at(
                        format!("`{}` 没有可识别的文献类型：`{}`", name.to_uppercase(), raw),
                        pos,
                    ));
                }
                Ok(Node::Enum { column, values })
            }
            FieldSpec::State(kind) => {
                let raw = self.take_value_text(name, sep, pos)?;
                self.state_leaf(kind, &raw, name, pos)
            }
            FieldSpec::Health => {
                let raw = self.take_value_text(name, sep, pos)?;
                self.health_leaf(&raw, name, pos)
            }
            FieldSpec::Text(columns) => {
                // `TS=(a OR b)`：把字段压进组内的裸值
                if matches!(self.peek(), Some(Tok::LParen)) {
                    let saved = self.ctx;
                    self.ctx = Some(FieldSpec::Text(columns));
                    let node = self.parse_primary();
                    self.ctx = saved;
                    return node;
                }
                let raw = self.take_value_text(name, sep, pos)?;
                Ok(Node::Text { columns, pattern: like_pattern(&raw) })
            }
        }
    }

    /// 取字段后面的值（`Word` 或引号短语），没有则报错。
    fn take_value_text(&mut self, name: &str, sep: &str, pos: usize) -> Result<String, QueryError> {
        match self.next() {
            Some(Spanned { tok: Tok::Word(w), .. }) => Ok(w),
            Some(Spanned { tok: Tok::Phrase(p), .. }) => Ok(p),
            Some(_) => Err(QueryError::at(
                format!(
                    "`{}{}` 后面需要一个值（含空格请用引号）",
                    name.to_uppercase(),
                    sep
                ),
                pos,
            )),
            None => Err(QueryError::at(
                format!("`{}{}` 后面缺少值", name.to_uppercase(), sep),
                pos,
            )),
        }
    }

    /// 裸值（自由文本或括号组内的值）→ 叶子节点；有字段上下文时套用该字段。
    fn leaf_from_value(&mut self, raw: String, pos: usize) -> Result<Node, QueryError> {
        match self.ctx {
            None => Ok(Node::Text { columns: C_ALL, pattern: like_pattern(&raw) }),
            Some(FieldSpec::Text(columns)) => {
                Ok(Node::Text { columns, pattern: like_pattern(&raw) })
            }
            Some(FieldSpec::Number(column)) => match parse_range(&raw) {
                Some(range) => Ok(Node::Range { column, min: range.min, max: range.max }),
                // 不是数值就按文本兜底（老写法把不能解析的值当文本搜）
                None => Ok(Node::Text { columns: single_column(column), pattern: like_pattern(&raw) }),
            },
            Some(FieldSpec::Enum(column)) => {
                let values = super::doc_type::normalize_many(&raw);
                if values.is_empty() {
                    return Err(QueryError::at(format!("无法识别的文献类型：`{}`", raw), pos));
                }
                Ok(Node::Enum { column, values })
            }
            Some(FieldSpec::State(kind)) => self.state_leaf(kind, &raw, "PDF/MD/ZIP", pos),
            Some(FieldSpec::Health) => self.health_leaf(&raw, "HEALTH", pos),
            Some(FieldSpec::SortBy) | Some(FieldSpec::SortOrder) => Ok(Node::True),
        }
    }

    /// 文件事实三态：逗号多选，条件之间 OR。
    fn state_leaf(
        &mut self,
        kind: FileKind,
        raw: &str,
        name: &str,
        pos: usize,
    ) -> Result<Node, QueryError> {
        let mut ors: Vec<String> = Vec::new();
        for item in raw.split(',').map(str::trim).filter(|s| !s.is_empty()) {
            match file_state::state_condition(kind, item) {
                Some(cond) => ors.push(cond),
                None => {
                    return Err(QueryError::at(
                        format!(
                            "`{}` 的取值 `{}` 非法（仅支持 present / absent / unknown）",
                            name.to_uppercase(),
                            item
                        ),
                        pos,
                    ))
                }
            }
        }
        match ors.len() {
            0 => Err(QueryError::at(
                format!("`{}` 后面缺少状态（present / absent / unknown）", name.to_uppercase()),
                pos,
            )),
            1 => Ok(Node::Raw(ors.pop().expect("len == 1"))),
            _ => Ok(Node::Raw(format!("({})", ors.join(" OR ")))),
        }
    }

    fn health_leaf(&mut self, raw: &str, name: &str, pos: usize) -> Result<Node, QueryError> {
        match file_state::health_condition(raw) {
            Err(detail) => Err(QueryError::at(format!("`{}` {}", name, detail), pos)),
            Ok(None) => Err(QueryError::at(
                format!(
                    "`{}` 后面缺少状态（ok / warning / error / missing / unchecked）",
                    name
                ),
                pos,
            )),
            Ok(Some(cond)) => match cond {
                file_state::HealthCondition::Sql(literal) => Ok(Node::Raw(literal)),
                file_state::HealthCondition::Values { values, include_null } => {
                    Ok(Node::Health { values, include_null })
                }
            },
        }
    }
}

// 数值字段收到非数值时的单列文本兜底（老写法把不能解析的值当文本搜）
const COL_YEAR: &[&str] = &["year"];
const COL_ID: &[&str] = &["id"];
const COL_PDF_SIZE: &[&str] = &["pdf_size"];
const COL_MD_SIZE: &[&str] = &["md_size"];
const COL_ZIP_SIZE: &[&str] = &["zip_size"];

fn single_column(column: &'static str) -> &'static [&'static str] {
    match column {
        "id" => COL_ID,
        "pdf_size" => COL_PDF_SIZE,
        "md_size" => COL_MD_SIZE,
        "zip_size" => COL_ZIP_SIZE,
        _ => COL_YEAR,
    }
}

fn tok_text(tok: &Tok) -> String {
    match tok {
        Tok::LParen => "(".to_string(),
        Tok::RParen => ")".to_string(),
        Tok::And => "AND".to_string(),
        Tok::Or => "OR".to_string(),
        Tok::Not => "NOT".to_string(),
        Tok::Field(n, s) => format!("{}{}", n, s),
        Tok::Word(w) => w.clone(),
        Tok::Phrase(p) => format!("\"{}\"", p),
    }
}

// ── 值处理 ──────────────────────────────────────────────

/// 文本值 → LIKE 模式。
///
/// - `*` → `%`、`?` → `_`、`$` → `%`（WoS 通配符；`$` 按 `%` 近似）；
/// - 不含通配符时两侧补 `%`（包含匹配），含通配符时按原样匹配（`ATT*` = 前缀匹配）；
/// - `%` / `_` / `\` 一律转义，SQL 侧配 `ESCAPE '\'`，所以普通含 `%` 的值也能精确命中。
fn like_pattern(raw: &str) -> String {
    let mut out = String::new();
    let mut has_wildcard = false;
    for ch in raw.chars() {
        match ch {
            '*' => {
                out.push('%');
                has_wildcard = true;
            }
            '?' => {
                out.push('_');
                has_wildcard = true;
            }
            '$' => {
                out.push('%');
                has_wildcard = true;
            }
            '%' => out.push_str("\\%"),
            '_' => out.push_str("\\_"),
            '\\' => out.push_str("\\\\"),
            c => out.push(c),
        }
    }
    if has_wildcard {
        out
    } else {
        format!("%{}%", out)
    }
}

/// 解析带单位的文件大小值（如 "10KB", "1.5MB", "512"）为字节数
fn parse_file_size_value(s: &str) -> Option<i64> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    let num_end = s.find(|c: char| !c.is_ascii_digit() && c != '.').unwrap_or(s.len());
    if num_end == 0 {
        return None;
    }
    let num_str = &s[..num_end];
    let unit_str = s[num_end..].trim().to_ascii_uppercase();
    let num: f64 = num_str.parse().ok()?;
    let bytes = match unit_str.as_str() {
        "GB" | "GIB" => num * 1024.0 * 1024.0 * 1024.0,
        "MB" | "MIB" => num * 1024.0 * 1024.0,
        "KB" | "KIB" => num * 1024.0,
        "B" | "" => num,
        _ => return None,
    };
    Some(bytes as i64)
}

#[derive(Debug, Clone, Copy)]
struct RangeBound {
    min: Option<i64>,
    max: Option<i64>,
}

/// 解析数值表达式：`2023`、`2020-2023`、`>2020`、`>=2020`、`<2020`、`<=2020`、`10KB-2MB`。
fn parse_range(value: &str) -> Option<RangeBound> {
    let v = value.trim();
    if v.is_empty() {
        return None;
    }

    let (op, rest) = if let Some(r) = v.strip_prefix(">=") {
        (">=", r.trim())
    } else if let Some(r) = v.strip_prefix("<=") {
        ("<=", r.trim())
    } else if let Some(r) = v.strip_prefix('>') {
        (">", r.trim())
    } else if let Some(r) = v.strip_prefix('<') {
        ("<", r.trim())
    } else if let Some(r) = v.strip_prefix('=') {
        ("=", r.trim())
    } else {
        ("", v)
    };

    if let Some(bytes) = parse_file_size_value(rest) {
        return match op {
            ">=" => Some(RangeBound { min: Some(bytes), max: None }),
            "<=" => Some(RangeBound { min: None, max: Some(bytes) }),
            ">" => Some(RangeBound { min: Some(bytes + 1), max: None }),
            "<" => Some(RangeBound { min: None, max: Some(bytes - 1) }),
            _ => Some(RangeBound { min: Some(bytes), max: Some(bytes) }),
        };
    }

    if op.is_empty() {
        if let Some((a, b)) = v.split_once('-') {
            if let (Ok(min), Ok(max)) = (a.trim().parse::<i64>(), b.trim().parse::<i64>()) {
                return Some(RangeBound { min: Some(min), max: Some(max) });
            }
            if let (Some(va), Some(vb)) = (
                parse_file_size_value(a.trim()),
                parse_file_size_value(b.trim()),
            ) {
                return Some(RangeBound { min: Some(va), max: Some(vb) });
            }
        }
        if let Ok(n) = v.parse::<i64>() {
            return Some(RangeBound { min: Some(n), max: Some(n) });
        }
    } else if let Ok(n) = rest.parse::<i64>() {
        return match op {
            ">=" => Some(RangeBound { min: Some(n), max: None }),
            "<=" => Some(RangeBound { min: None, max: Some(n) }),
            ">" => Some(RangeBound { min: Some(n + 1), max: None }),
            "<" => Some(RangeBound { min: None, max: Some(n - 1) }),
            _ => Some(RangeBound { min: Some(n), max: Some(n) }),
        };
    }

    None
}

// ── SQL 编译 ────────────────────────────────────────────

/// 编译结果：WHERE 片段 + 绑定值（按出现顺序）+ 排序。
#[derive(Debug, Clone, Default)]
pub struct CompiledQuery {
    /// 不含 `WHERE` 关键字；无条件时为 `1=1`
    pub where_clause: String,
    /// 与 `where_clause` 里的 `?` 一一对应
    pub binds: Vec<String>,
    /// 排序字段（已归一化，可能是列名或任意输入，调用方需兜底白名单）
    pub sort_by: Option<String>,
    /// `asc` / `desc`
    pub sort_order: Option<String>,
}

impl CompiledQuery {
    /// 是否没有任何检索条件（排序指令不算）
    pub fn is_trivial(&self) -> bool {
        self.where_clause == "1=1"
    }
}

/// 解析并编译检索式。
pub fn compile(input: &str) -> Result<CompiledQuery, QueryError> {
    let toks = lex(input)?;
    let mut parser = Parser::new(toks);
    let node = parser.parse()?;
    let mut sql = String::new();
    let mut binds: Vec<String> = Vec::new();
    emit(&node, &mut sql, &mut binds);
    Ok(CompiledQuery {
        where_clause: if sql.is_empty() { "1=1".to_string() } else { sql },
        binds,
        sort_by: parser.sort_by,
        sort_order: parser.sort_order,
    })
}

fn emit_child(node: &Node, sql: &mut String, binds: &mut Vec<String>) {
    match node {
        Node::And(_) | Node::Or(_) => {
            sql.push('(');
            emit(node, sql, binds);
            sql.push(')');
        }
        _ => emit(node, sql, binds),
    }
}

fn emit(node: &Node, sql: &mut String, binds: &mut Vec<String>) {
    match node {
        Node::True => sql.push_str("1=1"),
        Node::And(terms) | Node::Or(terms) => {
            let joiner = if matches!(node, Node::And(_)) { " AND " } else { " OR " };
            if terms.is_empty() {
                sql.push_str("1=1");
                return;
            }
            for (i, t) in terms.iter().enumerate() {
                if i > 0 {
                    sql.push_str(joiner);
                }
                emit_child(t, sql, binds);
            }
        }
        Node::Not(inner) => {
            sql.push_str("NOT (");
            emit(inner, sql, binds);
            sql.push(')');
        }
        Node::Text { columns, pattern } => {
            let ors: Vec<String> = columns
                .iter()
                .map(|c| format!("{} LIKE ? ESCAPE '\\'", c))
                .collect();
            sql.push('(');
            sql.push_str(&ors.join(" OR "));
            sql.push(')');
            for _ in 0..columns.len() {
                binds.push(pattern.clone());
            }
        }
        Node::Range { column, min, max } => match (*min, *max) {
            (Some(min), Some(max)) if min == max => {
                sql.push_str(&format!("{} = ?", column));
                binds.push(min.to_string());
            }
            (Some(min), Some(max)) => {
                sql.push_str(&format!("({0} >= ? AND {0} <= ?)", column));
                binds.push(min.to_string());
                binds.push(max.to_string());
            }
            (Some(min), None) => {
                sql.push_str(&format!("{} >= ?", column));
                binds.push(min.to_string());
            }
            (None, Some(max)) => {
                sql.push_str(&format!("{} <= ?", column));
                binds.push(max.to_string());
            }
            (None, None) => sql.push_str("1=1"),
        },
        Node::Enum { column, values } => {
            let placeholders = vec!["?"; values.len()].join(",");
            sql.push_str(&format!("{} IN ({})", column, placeholders));
            binds.extend(values.iter().cloned());
        }
        Node::Raw(literal) => sql.push_str(literal),
        Node::Health { values, include_null } => {
            if values.is_empty() {
                sql.push_str("pdf_health IS NULL");
            } else {
                let placeholders = vec!["?"; values.len()].join(",");
                if *include_null {
                    sql.push_str(&format!("(pdf_health IN ({}) OR pdf_health IS NULL)", placeholders));
                } else {
                    sql.push_str(&format!("pdf_health IN ({})", placeholders));
                }
                binds.extend(values.iter().cloned());
            }
        }
    }
}

// ── 测试 ────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn where_sql(q: &str) -> (String, Vec<String>) {
        let c = compile(q).expect("语法应通过");
        (c.where_clause, c.binds)
    }

    fn err(q: &str) -> String {
        compile(q).expect_err("语法应报错").message
    }

    #[test]
    fn test_free_text() {
        let (sql, binds) = where_sql("transformer");
        assert_eq!(sql.matches("LIKE").count(), C_ALL.len());
        assert_eq!(binds.len(), C_ALL.len());
        assert!(binds.iter().all(|b| b == "%transformer%"));
    }

    #[test]
    fn test_multiple_free_text_is_and() {
        let (sql, _) = where_sql("transformer attention");
        assert!(sql.contains(" AND "));
    }

    #[test]
    fn test_wos_tag_equals_legacy_colon() {
        let (a, ab) = where_sql("TI=attention");
        let (b, bb) = where_sql("title:attention");
        assert_eq!(a, b);
        assert_eq!(ab, bb);
        assert!(a.contains("title LIKE ?"));
        assert_eq!(ab, vec!["%attention%".to_string(); 2]);
    }

    #[test]
    fn test_quoted_value_with_space() {
        let (sql, binds) = where_sql("SO=\"IEEE TRANSACTIONS ON AUTOMATIC CONTROL\"");
        // journal 列 = 基础列 + i18n.en.journal，两列各一个绑定值
        assert!(sql.contains("journal LIKE ?"));
        assert_eq!(binds, vec!["%IEEE TRANSACTIONS ON AUTOMATIC CONTROL%".to_string(); 2]);
    }

    #[test]
    fn test_topic_group() {
        let (sql, binds) = where_sql("TS=(transformer OR attention)");
        // 组内的两个裸值都套用 TS 的列集合
        assert_eq!(sql.matches("LIKE").count(), C_TOPIC.len() * 2);
        assert!(sql.contains(" OR "));
        assert_eq!(binds[0], "%transformer%");
        assert_eq!(binds[binds.len() - 1], "%attention%");
    }

    #[test]
    fn test_boolean_precedence() {
        // AND 优先级高于 OR：右侧的 AND 组必须被括号包起来
        let (sql, _) = where_sql("TI=a OR TI=b AND TI=c");
        assert!(sql.starts_with("(title LIKE"), "实际 SQL：{}", sql);
        assert!(sql.contains(" OR (("), "实际 SQL：{}", sql);
    }

    #[test]
    fn test_parentheses() {
        let (sql, _) = where_sql("(TI=a OR TI=b) AND PY=2023");
        assert!(sql.starts_with("((title LIKE"), "实际 SQL：{}", sql);
        assert!(sql.contains("year = ?"));
    }

    #[test]
    fn test_not_and_legacy_minus() {
        let (a, _) = where_sql("NOT TI=a");
        let (b, _) = where_sql("-TI=a");
        assert!(a.starts_with("NOT ((title LIKE"), "实际 SQL：{}", a);
        assert_eq!(a, b);
    }

    #[test]
    fn test_wildcards() {
        // 含通配符时按原样匹配（不补 %）
        let (_, binds) = where_sql("AU=Vaswani*");
        assert_eq!(binds, vec!["Vaswani%".to_string(); 2]);
        let (_, binds) = where_sql("TI=trans?ormer");
        assert_eq!(binds, vec!["trans_ormer".to_string(); 2]);
        // 不含通配符时两侧补 %
        let (_, binds) = where_sql("TI=attention");
        assert_eq!(binds, vec!["%attention%".to_string(); 2]);
    }

    #[test]
    fn test_like_escape() {
        let (sql, binds) = where_sql("TI=50%_rate");
        assert!(sql.contains("ESCAPE '\\'"));
        assert_eq!(binds, vec!["%50\\%\\_rate%".to_string(); 2]);
    }

    #[test]
    fn test_year_range_and_ops() {
        let (_, binds) = where_sql("PY=2020-2023");
        assert_eq!(binds, vec!["2020".to_string(), "2023".to_string()]);
        let (_, binds) = where_sql("PY>2020");
        assert_eq!(binds, vec!["2021".to_string()]);
        let (_, binds) = where_sql("PY>=2020");
        assert_eq!(binds, vec!["2020".to_string()]);
        let (_, binds) = where_sql("PY=2023");
        assert_eq!(binds, vec!["2023".to_string()]);
        let (_, binds) = where_sql("PY<2020");
        assert_eq!(binds, vec!["2019".to_string()]);
    }

    #[test]
    fn test_id_range() {
        let (_, binds) = where_sql("ID=1-30");
        assert_eq!(binds, vec!["1".to_string(), "30".to_string()]);
    }

    #[test]
    fn test_file_size_units() {
        let (sql, binds) = where_sql("filesize_pdf>1MB");
        assert!(sql.contains("pdf_size >= ?"));
        assert_eq!(binds, vec![(1024 * 1024 + 1).to_string()]);
        let (_, binds) = where_sql("filesize_md=10KB-2MB");
        assert_eq!(binds, vec![(10 * 1024).to_string(), (2 * 1024 * 1024).to_string()]);
    }

    #[test]
    fn test_doc_type_normalized() {
        let (sql, binds) = where_sql("DT=article");
        assert!(sql.contains("doc_type IN (?)"));
        assert_eq!(binds, vec!["journalArticle".to_string()]);
        let (_, binds) = where_sql("type:article,book_section");
        assert_eq!(binds, vec!["journalArticle".to_string(), "bookSection".to_string()]);
    }

    #[test]
    fn test_file_state_and_health() {
        let (sql, _) = where_sql("PDF=present AND MD=absent");
        assert!(sql.contains("COALESCE(pdf_size, 0) > 0"));
        assert!(sql.contains("fs_synced_at IS NOT NULL"));
        let (sql, _) = where_sql("ZIP=unknown");
        assert!(sql.contains("fs_synced_at IS NULL"));

        let (sql, binds) = where_sql("HEALTH=warning,error");
        assert!(sql.contains("pdf_health IN (?,?)"));
        assert_eq!(binds, vec!["warning".to_string(), "error".to_string()]);
        let (sql, binds) = where_sql("HEALTH=unchecked");
        assert!(sql.contains("pdf_health IS NULL"));
        assert!(binds.is_empty());
        let (sql, binds) = where_sql("HEALTH=error,unchecked");
        assert!(sql.contains("OR pdf_health IS NULL"));
        assert_eq!(binds, vec!["error".to_string()]);
    }

    #[test]
    fn test_sort_directives() {
        let c = compile("TI=x SORT=PY ORDER=asc").expect("语法应通过");
        assert_eq!(c.sort_by.as_deref(), Some("year"));
        assert_eq!(c.sort_order.as_deref(), Some("asc"));
        // 旧写法同样生效
        let c = compile("TI=x sort:year order:desc").expect("语法应通过");
        assert_eq!(c.sort_by.as_deref(), Some("year"));
        assert_eq!(c.sort_order.as_deref(), Some("desc"));
        // 只有排序时不产生 WHERE 条件
        let c = compile("SORT=year ORDER=desc").expect("语法应通过");
        assert!(c.is_trivial());
    }

    #[test]
    fn test_empty_query() {
        let c = compile("   ").expect("空查询应通过");
        assert!(c.is_trivial());
        assert!(c.binds.is_empty());
    }

    #[test]
    fn test_full_wos_query() {
        let c = compile(
            "TS=(transformer OR \"deep learning\") AND PY=2020-2023 AND DT=journalArticle AND PDF=present SORT=PY ORDER=desc",
        )
        .expect("语法应通过");
        assert_eq!(c.sort_by.as_deref(), Some("year"));
        assert_eq!(c.sort_order.as_deref(), Some("desc"));
        assert!(c.where_clause.contains("COALESCE(pdf_size, 0) > 0"));
        assert!(c.where_clause.contains("doc_type IN (?)"));
        assert!(c.binds.contains(&"%deep learning%".to_string()));
        // 每个 ? 都要有对应绑定
        assert_eq!(c.where_clause.matches('?').count(), c.binds.len());
    }

    #[test]
    fn test_unknown_tag_errors_on_equals_only() {
        assert!(err("ZZ=1").contains("未知字段标签"));
        // 旧写法的未知前缀仍按自由文本处理（`https://…` 之类不能被误判成错误）
        let c = compile("https://example.com").expect("旧写法应通过");
        assert!(c.binds.iter().any(|b| b.contains("https://example.com")));
    }

    #[test]
    fn test_error_cases() {
        assert!(err("(TI=a").contains("括号未闭合"));
        assert!(err("TI=a)").contains("多余的"));
        assert!(err("TI=").contains("缺少值"));
        assert!(err("TI=a AND").contains("AND` 后面缺少条件"));
        assert!(err("TI=a OR").contains("OR` 后面缺少条件"));
        assert!(err("PY=abc").contains("需要数值"));
        assert!(err("PDF=maybe").contains("非法"));
        assert!(err("ORDER=down").contains("排序方向"));
        assert!(err("\"unterminated").contains("引号未闭合"));
    }

    #[test]
    fn test_near_degrades_to_and() {
        let (sql, _) = where_sql("TI=attention NEAR/3 transformer");
        assert!(sql.contains(" AND "));
    }

    /// 真跑一遍 SQL：验证 `ESCAPE`、`json_extract`、括号/绑定数量在 SQLite 上真的能执行。
    ///
    /// 这是**唯一**能证明「生成的 SQL 语法正确」的手段（纯字符串断言漏得掉拼写错误）。
    #[test]
    fn test_sql_runs_on_sqlite() {
        let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
        rt.block_on(async {
            let pool = sqlx::sqlite::SqlitePoolOptions::new()
                .max_connections(1)
                .connect("sqlite::memory:")
                .await
                .expect("内存库");
            sqlx::query(
                "CREATE TABLE documents (id INTEGER PRIMARY KEY, title TEXT, authors TEXT,                  journal TEXT, keywords TEXT, abstract TEXT, doi TEXT, source TEXT, filename TEXT,                  category TEXT, language TEXT, doc_type TEXT, year INTEGER, i18n TEXT,                  pdf_size INTEGER, md_size INTEGER, zip_size INTEGER, pdf_health TEXT, fs_synced_at TEXT)",
            )
            .execute(&pool)
            .await
            .expect("建表");
            sqlx::query(
                r#"INSERT INTO documents (id, title, authors, journal, year, doc_type, i18n, pdf_size, md_size, fs_synced_at, pdf_health) VALUES
                   (1, 'Attention Is All You Need', 'Vaswani Ashish', 'NeurIPS', 2017, 'conferencePaper',
                       '{"en":{"title":"Attention Is All You Need"}}', 100, 20, '2026-01-01', 'ok'),
                   (2, 'Deep Learning', 'LeCun Yann', 'Nature', 2015, 'journalArticle', NULL, 0, 0, '2026-01-01', NULL)"#,
            )
            .execute(&pool)
            .await
            .expect("插入");

            let ids = |q: &'static str| {
                let pool = pool.clone();
                async move {
                    let c = compile(q).expect("语法应通过");
                    let sql = format!("SELECT id FROM documents WHERE {} ORDER BY id", c.where_clause);
                    let mut query = sqlx::query_scalar::<_, i64>(&sql);
                    for b in &c.binds {
                        query = query.bind(b.clone());
                    }
                    query
                        .fetch_all(&pool)
                        .await
                        .unwrap_or_else(|e| panic!("SQL 执行失败：{}
{}
{}", q, sql, e))
                }
            };

            assert_eq!(ids("TI=attention").await, vec![1]);
            assert_eq!(ids("title:attention").await, vec![1]); // 旧写法等价
            assert_eq!(ids("TS=deep").await, vec![2]); // 主题含题名/关键词/摘要
            assert_eq!(ids("PY=2015-2017").await, vec![1, 2]);
            assert_eq!(ids("PY>2016").await, vec![1]);
            assert_eq!(ids("NOT DT=journalArticle").await, vec![1]);
            assert_eq!(ids("DOC_TYPE=no-such-type").await.is_empty(), true);
            assert_eq!(ids("PDF=present").await, vec![1]); // 0 字节不算存在
            assert_eq!(ids("HEALTH=unchecked").await, vec![2]);
            assert_eq!(ids("HEALTH=ok,unchecked").await, vec![1, 2]);
            assert_eq!(ids("TI=\"all you need\"").await, vec![1]);
            assert_eq!(ids("AU=Vaswani*").await, vec![1]);
            assert_eq!(ids("(TI=attention OR TI=deep) AND PY>2016").await, vec![1]);
            // 通配符转义：`50%` 不能当成通配符（结果为空但必须能执行）
            assert!(ids("TI=50%_rate").await.is_empty());
        });
    }

    #[test]
    fn test_bind_count_matches_placeholders() {
        for q in [
            "ALL=transformer",
            "TS=(a OR b) AND PY=2020",
            "NOT (TI=a OR AU=b*)",
            "DT=thesis AND HEALTH=ok,unchecked AND MD=present",
        ] {
            let c = compile(q).expect("语法应通过");
            assert_eq!(
                c.where_clause.matches('?').count(),
                c.binds.len(),
                "绑定值数量不匹配：{}",
                q
            );
        }
    }
}
