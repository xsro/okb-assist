//! BibTeX 解析：把 `.bib` 文件里的条目转成可落库的文档元数据。
//!
//! 只覆盖「批量新建条目」需要的部分：条目类型、引用键、常见字段，
//! 以及把 LaTeX 的转义/重音还原成可读文本（库里不保存 LaTeX 源码）。
//!
//! 解析器手写、无外部依赖，能处理：
//! - `@article{key, field = {value}, ...}` 与 `@article(key, ...)` 两种括号；
//! - 花括号嵌套（`{A {B} C}`）、`"..."` 引号值、数字与 `#` 拼接；
//! - `@string` 宏、`@comment` / `@preamble`（跳过）。

use std::collections::HashMap;

use super::crossref::{normalize_doi, strip_markup};

/// 一条 BibTeX 条目（字段值是**未解码**的原始文本）
#[derive(Debug, Clone, Default)]
pub struct BibEntry {
    /// 条目类型（小写，如 `article`）
    pub entry_type: String,
    /// 引用键（`@article{key, ...}` 里的 key）
    pub key: String,
    /// 字段名（小写）→ 原始值
    pub fields: Vec<(String, String)>,
}

impl BibEntry {
    /// 取字段原始值（字段名大小写不敏感，重复字段取第一个）
    pub fn field(&self, name: &str) -> Option<&str> {
        self.fields
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }

    /// 取字段的**可读**值：解 LaTeX → 去 HTML 标记 → 合并空白
    fn text(&self, name: &str) -> String {
        match self.field(name) {
            Some(v) => decode_latex(v),
            None => String::new(),
        }
    }

    /// 依次尝试多个字段名，返回第一个非空的可读值
    fn text_any(&self, names: &[&str]) -> String {
        for name in names {
            let v = self.text(name);
            if !v.is_empty() {
                return v;
            }
        }
        String::new()
    }

    /// 转成可落库的元数据；既没有标题也没有 DOI 时返回 `None`（这种条目落库没有意义）
    pub fn to_record(&self) -> Option<BibRecord> {
        let mut title = self.text("title");
        let subtitle = self.text("subtitle");
        if !subtitle.is_empty() && !title.is_empty() {
            title = format!("{}: {}", title, subtitle);
        }

        let authors = {
            let raw = self
                .field("author")
                .or_else(|| self.field("editor"))
                .unwrap_or("");
            split_authors(&decode_latex(raw))
        };

        let year = self
            .field("year")
            .and_then(|v| parse_year(&decode_latex(v)))
            .or_else(|| self.field("date").and_then(|v| parse_year(&decode_latex(v))));

        let url = self.text_any(&["url", "howpublished"]);
        let doi = {
            let explicit = self.field("doi").map(|v| decode_latex(v)).unwrap_or_default();
            match normalize_doi(&explicit) {
                Some(d) => d,
                None => doi_from_url(&url).unwrap_or_default(),
            }
        };

        let journal = self.text_any(&["journal", "journaltitle", "booktitle", "container-title"]);
        let publisher = self.text_any(&["publisher", "school", "institution", "organization"]);

        let keywords = split_keywords(
            &self
                .field("keywords")
                .or_else(|| self.field("keyword"))
                .map(|v| decode_latex(v))
                .unwrap_or_default(),
        );

        let abstract_text = self.text("abstract");
        let doc_type = doc_type_of(&self.entry_type).to_string();
        let language = normalize_language(&self.text_any(&["language", "langid"]));

        if title.is_empty() && doi.is_empty() {
            return None;
        }

        Some(BibRecord {
            key: self.key.clone(),
            title,
            authors,
            year,
            doi,
            journal,
            publisher,
            keywords,
            abstract_text,
            doc_type,
            language,
        })
    }
}

/// 由 BibTeX 条目提炼出的文档元数据（对应 `documents` 表的可见字段）
#[derive(Debug, Clone, Default)]
pub struct BibRecord {
    pub key: String,
    pub title: String,
    pub authors: Vec<String>,
    pub year: Option<i64>,
    pub doi: String,
    pub journal: String,
    /// 出版社（`documents.source` 列复用它，供引用条目使用）
    pub publisher: String,
    pub keywords: Vec<String>,
    pub abstract_text: String,
    /// Zotero 标准类型名（`journalArticle` / `book` / ...）
    pub doc_type: String,
    pub language: String,
}

/// 解析结果
#[derive(Debug, Default, Clone)]
pub struct ParseResult {
    pub entries: Vec<BibEntry>,
    /// 解析过程中的问题（带行号），前端会原样展示
    pub warnings: Vec<String>,
    /// 被跳过（或用于收集宏）的条目数：`@comment` / `@preamble` / `@string`
    pub skipped: usize,
}

/// 解析整个 `.bib` 文件
pub fn parse_bibtex(input: &str) -> ParseResult {
    let mut sc = Scanner::new(input.trim_start_matches('\u{feff}'));
    let mut result = ParseResult::default();
    let mut macros: HashMap<String, String> = HashMap::new();

    while let Some(c) = sc.peek() {
        if c != '@' {
            sc.pos += 1;
            continue;
        }
        let at = sc.pos;
        sc.pos += 1;
        let entry_type = sc.read_ident().to_lowercase();
        if entry_type.is_empty() {
            result.warnings.push(format!("第 {} 行：@ 后面缺少条目类型", sc.line_of(at)));
            continue;
        }
        sc.skip_ws();
        let open = match sc.peek() {
            Some(o @ ('{' | '(')) => o,
            _ => {
                result
                    .warnings
                    .push(format!("第 {} 行：@{} 缺少开括号", sc.line_of(at), entry_type));
                continue;
            }
        };
        sc.pos += 1;
        let body = match read_entry_body(&mut sc, open) {
            Some(b) => b,
            None => {
                result
                    .warnings
                    .push(format!("第 {} 行：@{} 条目未闭合，后续内容已忽略", sc.line_of(at), entry_type));
                break;
            }
        };

        match entry_type.as_str() {
            "comment" | "preamble" => {
                result.skipped += 1;
            }
            "string" => {
                // @string{name = "value"} 可能与普通条目同段（多个定义用逗号分隔）
                for (name, value) in parse_fields(&body, &macros, false) {
                    macros.insert(name, value);
                }
                result.skipped += 1;
            }
            _ => {
                let mut bs = Scanner::new(&body);
                bs.skip_ws();
                let mut key = bs.read_token();
                // 键后面紧跟 `,`（或没有字段时直接结束）
                bs.skip_ws();
                if bs.peek() == Some(',') {
                    bs.pos += 1;
                }
                if key.is_empty() {
                    // 没有键（少数导出工具的写法）：把第一个逗号前的内容当键
                    key = body.split(',').next().unwrap_or("").trim().to_string();
                }
                let fields = parse_fields(&body, &macros, true);
                result.entries.push(BibEntry {
                    entry_type,
                    key,
                    fields,
                });
            }
        }
    }

    result
}

// ── 扫描器 ──────────────────────────────────────────────

struct Scanner {
    chars: Vec<char>,
    pos: usize,
}

impl Scanner {
    fn new(text: &str) -> Self {
        Self {
            chars: text.chars().collect(),
            pos: 0,
        }
    }

    fn peek(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
    }

    fn bump(&mut self) -> Option<char> {
        let c = self.peek();
        if c.is_some() {
            self.pos += 1;
        }
        c
    }

    fn skip_ws(&mut self) {
        while matches!(self.peek(), Some(c) if c.is_whitespace()) {
            self.pos += 1;
        }
    }

    /// 当前位置的行号（1 开始，仅用于报错）
    fn line_of(&self, at: usize) -> usize {
        self.chars[..at.min(self.chars.len())]
            .iter()
            .filter(|c| **c == '\n')
            .count()
            + 1
    }

    /// 读一段标识符（字母/数字/`-`/`_`）
    fn read_ident(&mut self) -> String {
        let mut out = String::new();
        while let Some(c) = self.peek() {
            if c.is_alphanumeric() || c == '-' || c == '_' {
                out.push(c);
                self.pos += 1;
            } else {
                break;
            }
        }
        out
    }

    /// 读到空白 / 结构符号为止（键、数字、宏名）
    fn read_token(&mut self) -> String {
        let mut out = String::new();
        while let Some(c) = self.peek() {
            if c.is_whitespace() || matches!(c, ',' | '}' | ')' | '=' | '#') {
                break;
            }
            out.push(c);
            self.pos += 1;
        }
        out
    }

    /// 读取 `{...}`（去掉最外层花括号），`\{` / `\}` 不参与配对
    fn read_braced(&mut self) -> Option<String> {
        if self.peek() != Some('{') {
            return None;
        }
        self.pos += 1;
        let mut depth = 1usize;
        let mut out = String::new();
        while let Some(c) = self.bump() {
            match c {
                '\\' => {
                    out.push(c);
                    if let Some(n) = self.bump() {
                        out.push(n);
                    }
                }
                '{' => {
                    depth += 1;
                    out.push(c);
                }
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        return Some(out);
                    }
                    out.push(c);
                }
                _ => out.push(c),
            }
        }
        None
    }

    /// 读取 `"..."`（去掉引号），花括号里的引号不算结束
    fn read_quoted(&mut self) -> Option<String> {
        if self.peek() != Some('"') {
            return None;
        }
        self.pos += 1;
        let mut depth = 0usize;
        let mut out = String::new();
        while let Some(c) = self.bump() {
            match c {
                '\\' => {
                    out.push(c);
                    if let Some(n) = self.bump() {
                        out.push(n);
                    }
                }
                '{' => {
                    depth += 1;
                    out.push(c);
                }
                '}' => {
                    if depth > 0 {
                        depth -= 1;
                    }
                    out.push(c);
                }
                '"' if depth == 0 => return Some(out),
                _ => out.push(c),
            }
        }
        None
    }
}

/// 读取条目主体（`{` / `(` 之后到配对闭括号之间的文本）
fn read_entry_body(sc: &mut Scanner, open: char) -> Option<String> {
    let close = if open == '{' { '}' } else { ')' };
    let mut depth = 0usize;
    let mut out = String::new();
    while let Some(c) = sc.bump() {
        match c {
            '\\' => {
                out.push(c);
                if let Some(n) = sc.bump() {
                    out.push(n);
                }
            }
            '{' => {
                depth += 1;
                out.push(c);
            }
            '}' if depth > 0 => {
                depth -= 1;
                out.push(c);
            }
            '}' if open == '{' => return Some(out),
            '}' => {} // 括号条目里孤立的 `}`：忽略
            c if c == close && depth == 0 => return Some(out),
            c => out.push(c),
        }
    }
    None
}

/// 解析 `name = value, name = value, ...`
fn parse_fields(body: &str, macros: &HashMap<String, String>, skip_key: bool) -> Vec<(String, String)> {
    let mut sc = Scanner::new(body);
    let mut fields: Vec<(String, String)> = Vec::new();
    // 普通条目：先跳过引用键（第一个逗号之前的内容）；`@string` 没有键
    if skip_key {
        loop {
            match sc.peek() {
                Some(',') => {
                    sc.pos += 1;
                    break;
                }
                Some(_) => sc.pos += 1,
                None => return fields,
            }
        }
    }

    loop {
        sc.skip_ws();
        while sc.peek() == Some(',') {
            sc.pos += 1;
            sc.skip_ws();
        }
        if sc.peek().is_none() {
            break;
        }
        let name = sc.read_ident().to_lowercase();
        if name.is_empty() {
            // 结构字符（没有字段名）：跳过，避免死循环
            sc.pos += 1;
            continue;
        }
        sc.skip_ws();
        if sc.peek() != Some('=') {
            // 字段格式不对：跳到下一个逗号
            while let Some(c) = sc.peek() {
                sc.pos += 1;
                if c == ',' {
                    break;
                }
            }
            continue;
        }
        sc.pos += 1;
        let value = read_value(&mut sc, macros);
        if !fields.iter().any(|(k, _)| *k == name) {
            fields.push((name, value));
        }
    }

    fields
}

/// 读取字段值：`{...}` / `"..."` / 裸词（数字或宏名），支持 `#` 拼接
fn read_value(sc: &mut Scanner, macros: &HashMap<String, String>) -> String {
    let mut out = String::new();
    loop {
        sc.skip_ws();
        match sc.peek() {
            Some('{') => match sc.read_braced() {
                Some(v) => out.push_str(&v),
                None => return out,
            },
            Some('"') => match sc.read_quoted() {
                Some(v) => out.push_str(&v),
                None => return out,
            },
            Some(_) => {
                let bare = sc.read_token();
                if bare.is_empty() {
                    return out;
                }
                if let Some(v) = macros.get(&bare.to_lowercase()) {
                    out.push_str(v);
                } else {
                    out.push_str(&bare);
                }
            }
            None => return out,
        }
        sc.skip_ws();
        if sc.peek() == Some('#') {
            sc.pos += 1;
            continue;
        }
        return out;
    }
}

// ── 字段规范化 ──────────────────────────────────────────

/// 拆分 `author` 字段：BibTeX 用 ` and ` 分隔姓名
fn split_authors(raw: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut current = String::new();
    for token in raw.split_whitespace() {
        if token.eq_ignore_ascii_case("and") {
            push_author(&mut out, &current);
            current.clear();
        } else {
            if !current.is_empty() {
                current.push(' ');
            }
            current.push_str(token);
        }
    }
    push_author(&mut out, &current);
    out
}

fn push_author(out: &mut Vec<String>, raw: &str) {
    let name = normalize_author_name(raw);
    if !name.is_empty() {
        out.push(name);
    }
}

/// `Last, First` → `First Last`（中文姓名保持原样拼在一起）
fn normalize_author_name(raw: &str) -> String {
    let name = collapse_ws(raw).trim_matches(',').trim().to_string();
    if !name.contains(',') {
        return name;
    }
    let parts: Vec<&str> = name
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect();
    match parts.len() {
        0 => String::new(),
        1 => parts[0].to_string(),
        _ => {
            let family = parts[0];
            // `von Last, Jr, First`：最后一段是名，中间的 Jr 之类后缀丢弃
            let given = parts[parts.len() - 1];
            if has_cjk(family) {
                format!("{}{}", family, given)
            } else {
                format!("{} {}", given, family).trim().to_string()
            }
        }
    }
}

/// 年份：取前 4 位数字并限定在 1000–2200（`2020-05-01` / `{2020}` / `2020` 都适用）
fn parse_year(raw: &str) -> Option<i64> {
    let digits: String = raw.chars().filter(|c| c.is_ascii_digit()).take(4).collect();
    digits.parse::<i64>().ok().filter(|y| (1000..=2200).contains(y))
}

/// DOI：优先 `doi` 字段（已在上游处理），这里补一条从 URL 里认 DOI 的路
fn doi_from_url(url: &str) -> Option<String> {
    let lower = url.to_lowercase();
    let idx = lower.find("doi.org/")?;
    normalize_doi(&url[idx + "doi.org/".len()..])
}

/// 关键词：按 `;` `,` 换行切分
fn split_keywords(raw: &str) -> Vec<String> {
    raw.split([';', ',', '；', '，', '\n'])
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

/// BibTeX 条目类型 → Zotero 标准类型名（与 `pipeline::normalize_doc_type` 的输出一致）
fn doc_type_of(entry_type: &str) -> &'static str {
    match entry_type {
        "article" | "periodical" => "journalArticle",
        "book" | "booklet" | "collection" | "manual" | "mvbook" | "proceedings" | "reference" => {
            "book"
        }
        "inbook" | "incollection" | "bookinbook" | "suppbook" | "suppcollection" => "bookSection",
        "inproceedings" | "conference" => "conferencePaper",
        "phdthesis" | "mastersthesis" | "thesis" => "thesis",
        "techreport" | "report" => "report",
        "unpublished" => "manuscript",
        "online" | "electronic" | "www" | "webpage" => "webpage",
        "patent" => "patent",
        "preprint" => "preprint",
        _ => "document",
    }
}

/// 语言字段规范化：只认中/英，其它原样保留（小写）
fn normalize_language(raw: &str) -> String {
    let l = raw.trim().to_lowercase();
    if l.is_empty() {
        return String::new();
    }
    if l.starts_with("zh") || l == "chi" || l.contains("chinese") || l.contains("中文") {
        "zh".to_string()
    } else if l.starts_with("en") || l == "eng" || l.contains("english") {
        "en".to_string()
    } else {
        l
    }
}

fn has_cjk(text: &str) -> bool {
    text.chars().any(has_cjk_char)
}

fn has_cjk_char(c: char) -> bool {
    matches!(
        c as u32,
        0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xF900..=0xFAFF | 0x3040..=0x30FF | 0xAC00..=0xD7AF
    )
}

/// 合并连续空白并去首尾空白
fn collapse_ws(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut prev_space = false;
    for c in text.chars() {
        if c.is_whitespace() {
            if !prev_space && !out.is_empty() {
                out.push(' ');
            }
            prev_space = true;
        } else {
            out.push(c);
            prev_space = false;
        }
    }
    out.trim_end().to_string()
}

// ── LaTeX → 可读文本 ────────────────────────────────────

/// 还原 LaTeX 文本里的常见写法：
/// 重音（`\"o` / `{\"o}` / `\'{e}`）、转义（`\&` / `\%` / `\_`）、
/// 只保留参数的格式命令（`\emph{}` / `\textit{}` / `\url{}` / `\href{}{}`）、
/// `--` / `---` 与 `~`，以及 `$..$` 数学定界符。
pub fn decode_latex(raw: &str) -> String {
    let chars: Vec<char> = raw.chars().collect();
    let mut out = String::with_capacity(raw.len());
    let mut i = 0usize;
    while i < chars.len() {
        let c = chars[i];
        match c {
            '\\' => {
                i += 1;
                let Some(n) = chars.get(i).copied() else { break };
                if let Some(mark) = accent_mark(&chars, i) {
                    let (base, next) = read_accent_operand(&chars, i + 1);
                    if let Some(base) = base {
                        match compose_accent(mark, base) {
                            Some(c) => out.push(c),
                            // 没有预组合字符时保留分解形式（仍是合法 Unicode）
                            None => {
                                out.push(base);
                                out.push(mark);
                            }
                        }
                    }
                    i = next;
                    continue;
                }
                match n {
                    '&' | '%' | '$' | '#' | '_' | '{' | '}' => {
                        out.push(n);
                        i += 1;
                    }
                    // 换行 / 间距命令
                    '\\' => {
                        out.push(' ');
                        i += 1;
                    }
                    ',' | ';' | ':' | '!' | ' ' | '\n' | '\t' | '/' | '-' => i += 1,
                    _ if n.is_alphabetic() => {
                        let start = i;
                        let mut j = i;
                        while j < chars.len() && chars[j].is_alphabetic() {
                            j += 1;
                        }
                        let name: String = chars[start..j].iter().collect::<String>().to_lowercase();
                        i = j;
                        match name.as_str() {
                            "ss" => out.push('ß'),
                            "ae" => out.push('æ'),
                            "oe" => out.push('œ'),
                            "aa" => out.push('å'),
                            "o" => out.push('ø'),
                            "l" => out.push('ł'),
                            "i" => out.push('i'),
                            "j" => out.push('j'),
                            "dag" => out.push('†'),
                            "ddag" => out.push('‡'),
                            "pounds" => out.push('£'),
                            "textbackslash" => out.push('\\'),
                            "ldots" | "dots" | "textellipsis" => out.push('…'),
                            "textendash" => out.push('–'),
                            "textemdash" => out.push('—'),
                            "textasciitilde" => out.push('~'),
                            "tex" => out.push_str("TeX"),
                            "latex" => out.push_str("LaTeX"),
                            "xelatex" => out.push_str("XeLaTeX"),
                            "bibtex" => out.push_str("BibTeX"),
                            "textregistered" => out.push('®'),
                            "texttrademark" => out.push('™'),
                            "copyright" | "textcopyright" => out.push('©'),
                            "times" => out.push('×'),
                            "pm" => out.push('±'),
                            "degree" => out.push('°'),
                            // 只保留参数的格式命令（内容再交给外层/递归处理）
                            "emph" | "textit" | "textbf" | "texttt" | "textsc" | "textsf"
                            | "textrm" | "textup" | "textsl" | "textnormal" | "mbox" | "hbox"
                            | "text" | "mathrm" | "mathit" | "mathbf" | "mathsf" | "mathtt"
                            | "mathnormal" | "textsuperscript" | "textsubscript" | "uppercase"
                            | "lowercase" | "url" | "nolinkurl" | "bibinfo" => {
                                while matches!(chars.get(i), Some(c) if c.is_whitespace()) {
                                    i += 1;
                                }
                                if let Some((inner, next)) = take_group(&chars, i) {
                                    out.push_str(&decode_latex(&inner));
                                    i = next;
                                }
                            }
                            // `\href{url}{text}`：丢掉 URL，只留可见文字
                            "href" => {
                                while matches!(chars.get(i), Some(c) if c.is_whitespace()) {
                                    i += 1;
                                }
                                if let Some((_, next)) = take_group(&chars, i) {
                                    i = next;
                                }
                                while matches!(chars.get(i), Some(c) if c.is_whitespace()) {
                                    i += 1;
                                }
                                if let Some((inner, next)) = take_group(&chars, i) {
                                    out.push_str(&decode_latex(&inner));
                                    i = next;
                                }
                            }
                            // 其它命令：丢掉命令名本身
                            _ => {}
                        }
                    }
                    _ => i += 1,
                }
            }
            // 分组花括号与数学定界符只是排版标记
            '{' | '}' | '$' => i += 1,
            '~' => {
                out.push(' ');
                i += 1;
            }
            '-' => {
                if chars.get(i + 1) == Some(&'-') {
                    if chars.get(i + 2) == Some(&'-') {
                        out.push('—');
                        i += 3;
                    } else {
                        out.push('–');
                        i += 2;
                    }
                } else {
                    out.push('-');
                    i += 1;
                }
            }
            '`' => {
                if chars.get(i + 1) == Some(&'`') {
                    out.push('“');
                    i += 2;
                } else {
                    out.push('‘');
                    i += 1;
                }
            }
            '\'' if chars.get(i + 1) == Some(&'\'') => {
                out.push('”');
                i += 2;
            }
            _ => {
                out.push(c);
                i += 1;
            }
        }
    }
    strip_markup(&collapse_ws(&out))
}

/// 判断 `chars[i]`（反斜杠之后）是不是重音符，返回对应的组合符号
fn accent_mark(chars: &[char], i: usize) -> Option<char> {
    let c = chars.get(i).copied()?;
    let mark = match c {
        '\'' | '’' => '\u{0301}',
        '`' => '\u{0300}',
        '^' => '\u{0302}',
        '"' => '\u{0308}',
        '~' => '\u{0303}',
        '=' => '\u{0304}',
        '.' => '\u{0307}',
        'u' => '\u{0306}',
        'v' => '\u{030C}',
        'H' => '\u{030B}',
        'c' => '\u{0327}',
        'k' => '\u{0328}',
        'r' => '\u{030A}',
        'b' => '\u{0331}',
        'd' => '\u{0323}',
        _ => return None,
    };
    // 字母型重音符（\c / \u / \v ...）后面必须紧跟 `{` 或空白，
    // 否则是普通命令（\url / \cite / \ldots ...），不能当重音吃掉。
    if c.is_alphabetic() {
        match chars.get(i + 1) {
            Some('{') => {}
            Some(w) if w.is_whitespace() => {
                let ok = match chars.get(i + 2) {
                    Some('{') => true,
                    Some(n) => n.is_alphabetic(),
                    None => false,
                };
                if !ok {
                    return None;
                }
            }
            _ => return None,
        }
    }
    Some(mark)
}

/// 把「基础字符 + 组合重音符号」还原成最常见的预组合字符。
///
/// 库里不保存分解形式的 Unicode（`u` 加组合分音符看起来和 `ü` 一样，但比较/检索时不同），
/// 只有找不到预组合字符时才退回分解形式。
fn compose_accent(mark: char, base: char) -> Option<char> {
    Some(match mark {
        // 锐音符
        '\u{0301}' => match base {
            'a' => 'á', 'e' => 'é', 'i' => 'í', 'o' => 'ó', 'u' => 'ú', 'y' => 'ý',
            'c' => 'ć', 'n' => 'ń', 's' => 'ś', 'z' => 'ź', 'l' => 'ĺ', 'r' => 'ŕ', 'g' => 'ǵ',
            'A' => 'Á', 'E' => 'É', 'I' => 'Í', 'O' => 'Ó', 'U' => 'Ú', 'Y' => 'Ý',
            'C' => 'Ć', 'N' => 'Ń', 'S' => 'Ś', 'Z' => 'Ź', 'L' => 'Ĺ', 'R' => 'Ŕ', 'G' => 'Ǵ',
            _ => return None,
        },
        // 抑音符
        '\u{0300}' => match base {
            'a' => 'à', 'e' => 'è', 'i' => 'ì', 'o' => 'ò', 'u' => 'ù', 'y' => 'ỳ',
            'n' => 'ǹ', 'w' => 'ẁ',
            'A' => 'À', 'E' => 'È', 'I' => 'Ì', 'O' => 'Ò', 'U' => 'Ù', 'Y' => 'Ỳ',
            'N' => 'Ǹ', 'W' => 'Ẁ',
            _ => return None,
        },
        // 抑扬符
        '\u{0302}' => match base {
            'a' => 'â', 'e' => 'ê', 'i' => 'î', 'o' => 'ô', 'u' => 'û', 'y' => 'ŷ',
            'c' => 'ĉ', 'g' => 'ĝ', 'h' => 'ĥ', 'j' => 'ĵ', 's' => 'ŝ', 'w' => 'ŵ',
            'A' => 'Â', 'E' => 'Ê', 'I' => 'Î', 'O' => 'Ô', 'U' => 'Û', 'Y' => 'Ŷ',
            'C' => 'Ĉ', 'G' => 'Ĝ', 'H' => 'Ĥ', 'J' => 'Ĵ', 'S' => 'Ŝ', 'W' => 'Ŵ',
            _ => return None,
        },
        // 分音符
        '\u{0308}' => match base {
            'a' => 'ä', 'e' => 'ë', 'i' => 'ï', 'o' => 'ö', 'u' => 'ü', 'y' => 'ÿ',
            'A' => 'Ä', 'E' => 'Ë', 'I' => 'Ï', 'O' => 'Ö', 'U' => 'Ü', 'Y' => 'Ÿ',
            _ => return None,
        },
        // 波浪号
        '\u{0303}' => match base {
            'a' => 'ã', 'n' => 'ñ', 'o' => 'õ', 'i' => 'ĩ', 'u' => 'ũ', 'y' => 'ỹ',
            'A' => 'Ã', 'N' => 'Ñ', 'O' => 'Õ', 'I' => 'Ĩ', 'U' => 'Ũ', 'Y' => 'Ỹ',
            _ => return None,
        },
        // 上横线
        '\u{0304}' => match base {
            'a' => 'ā', 'e' => 'ē', 'i' => 'ī', 'o' => 'ō', 'u' => 'ū',
            'A' => 'Ā', 'E' => 'Ē', 'I' => 'Ī', 'O' => 'Ō', 'U' => 'Ū',
            _ => return None,
        },
        // 短音符
        '\u{0306}' => match base {
            'a' => 'ă', 'e' => 'ĕ', 'g' => 'ğ', 'i' => 'ĭ', 'o' => 'ŏ', 'u' => 'ŭ',
            'A' => 'Ă', 'E' => 'Ĕ', 'G' => 'Ğ', 'I' => 'Ĭ', 'O' => 'Ŏ', 'U' => 'Ŭ',
            _ => return None,
        },
        // 上点
        '\u{0307}' => match base {
            'z' => 'ż', 'c' => 'ċ', 'e' => 'ė', 'g' => 'ġ',
            'Z' => 'Ż', 'C' => 'Ċ', 'E' => 'Ė', 'G' => 'Ġ',
            _ => return None,
        },
        // 上圆圈（BibTeX 的 \r 与 \aa）
        '\u{030A}' | '\u{0325}' => match base {
            'a' => 'å', 'u' => 'ů', 'A' => 'Å', 'U' => 'Ů',
            _ => return None,
        },
        // 倒折音符
        '\u{030C}' => match base {
            'c' => 'č', 'd' => 'ď', 'e' => 'ě', 'g' => 'ǧ', 'h' => 'ȟ', 'k' => 'ǩ',
            'n' => 'ň', 'r' => 'ř', 's' => 'š', 't' => 'ť', 'z' => 'ž',
            'C' => 'Č', 'D' => 'Ď', 'E' => 'Ě', 'G' => 'Ǧ', 'H' => 'Ȟ', 'K' => 'Ǩ',
            'N' => 'Ň', 'R' => 'Ř', 'S' => 'Š', 'T' => 'Ť', 'Z' => 'Ž',
            _ => return None,
        },
        // 软音符
        '\u{0327}' => match base {
            'c' => 'ç', 'g' => 'ģ', 'k' => 'ķ', 'l' => 'ļ', 'n' => 'ņ', 'r' => 'ŗ', 's' => 'ş',
            't' => 'ţ',
            'C' => 'Ç', 'G' => 'Ģ', 'K' => 'Ķ', 'L' => 'Ļ', 'N' => 'Ņ', 'R' => 'Ŗ', 'S' => 'Ş',
            'T' => 'Ţ',
            _ => return None,
        },
        // 反尾形符
        '\u{0328}' => match base {
            'a' => 'ą', 'e' => 'ę', 'i' => 'į', 'u' => 'ų',
            'A' => 'Ą', 'E' => 'Ę', 'I' => 'Į', 'U' => 'Ų',
            _ => return None,
        },
        // 双锐音符
        '\u{030B}' => match base {
            'o' => 'ő', 'u' => 'ű', 'O' => 'Ő', 'U' => 'Ű',
            _ => return None,
        },
        // 下点
        '\u{0323}' => match base {
            'a' => 'ạ', 'e' => 'ẹ', 'i' => 'ị', 'o' => 'ọ', 'u' => 'ụ', 'y' => 'ỵ',
            'A' => 'Ạ', 'E' => 'Ẹ', 'I' => 'Ị', 'O' => 'Ọ', 'U' => 'Ụ', 'Y' => 'Ỵ',
            _ => return None,
        },
        _ => return None,
    })
}

/// 读取重音的操作数：`{o}` / `{\i}` / ` o` / `o`，返回（基础字符，下一个位置）
fn read_accent_operand(chars: &[char], mut i: usize) -> (Option<char>, usize) {
    while matches!(chars.get(i), Some(c) if c.is_whitespace()) {
        i += 1;
    }
    if i >= chars.len() {
        return (None, i);
    }
    if chars[i] != '{' {
        return (Some(chars[i]), i + 1);
    }
    let mut j = i + 1;
    while matches!(chars.get(j), Some(c) if c.is_whitespace()) {
        j += 1;
    }
    let base = match chars.get(j).copied() {
        Some('\\') => {
            // `\'{\i}`：无点 i / j
            j += 1;
            let start = j;
            while matches!(chars.get(j), Some(c) if c.is_alphabetic()) {
                j += 1;
            }
            let name: String = chars[start..j].iter().collect();
            match name.as_str() {
                "i" => Some('i'),
                "j" => Some('j'),
                _ => None,
            }
        }
        Some(c) => {
            j += 1;
            Some(c)
        }
        None => None,
    };
    while matches!(chars.get(j), Some(c) if *c != '}') {
        j += 1;
    }
    if j < chars.len() {
        j += 1;
    }
    (base, j)
}

/// 取出 `chars[i]` 开头的 `{...}` 内容（`chars[i]` 必须是 `{`），返回（内容，下一个位置）
fn take_group(chars: &[char], i: usize) -> Option<(String, usize)> {
    if chars.get(i) != Some(&'{') {
        return None;
    }
    let mut depth = 1usize;
    let mut out = String::new();
    let mut j = i + 1;
    while j < chars.len() {
        let c = chars[j];
        if c == '\\' {
            out.push(c);
            if let Some(n) = chars.get(j + 1) {
                out.push(*n);
                j += 2;
                continue;
            }
            j += 1;
            continue;
        }
        if c == '{' {
            depth += 1;
            out.push(c);
            j += 1;
            continue;
        }
        if c == '}' {
            depth -= 1;
            if depth == 0 {
                return Some((out, j + 1));
            }
            out.push(c);
            j += 1;
            continue;
        }
        out.push(c);
        j += 1;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_typical_zotero_export() {
        let raw = r#"
@article{li2020control,
	title = {Predefined-time control of a {Space} Robot},
	author = {Li, Wei and Zhang, San and M{\"u}ller, Hans},
	journal = {IEEE Transactions on Automatic Control},
	year = {2020},
	volume = {65},
	doi = {10.1109/TAC.2019.1234567},
	keywords = {space robot; predefined-time; control},
	abstract = {We study the problem.},
}
"#;
        let parsed = parse_bibtex(raw);
        assert_eq!(parsed.entries.len(), 1);
        let rec = parsed.entries[0].to_record().expect("record");
        assert_eq!(rec.key, "li2020control");
        assert_eq!(rec.title, "Predefined-time control of a Space Robot");
        assert_eq!(rec.authors, vec!["Wei Li", "San Zhang", "Hans Müller"]);
        assert_eq!(rec.year, Some(2020));
        assert_eq!(rec.doi, "10.1109/TAC.2019.1234567");
        assert_eq!(rec.journal, "IEEE Transactions on Automatic Control");
        assert_eq!(rec.keywords, vec!["space robot", "predefined-time", "control"]);
        assert_eq!(rec.abstract_text, "We study the problem.");
        assert_eq!(rec.doc_type, "journalArticle");
    }

    #[test]
    fn handles_paren_delimiters_quotes_and_string_macros() {
        let raw = r#"
@string{ieee = "IEEE Transactions"}
@inproceedings(key1, author = "Doe, John and Roe, Jane", title = "A " # "quoted" # " title", booktitle = ieee # " on Control", year = 1999)
@comment{ignored}
"#;
        let parsed = parse_bibtex(raw);
        assert_eq!(parsed.entries.len(), 1);
        assert_eq!(parsed.skipped, 2);
        let rec = parsed.entries[0].to_record().expect("record");
        assert_eq!(parsed.entries[0].entry_type, "inproceedings");
        assert_eq!(rec.title, "A quoted title");
        assert_eq!(rec.journal, "IEEE Transactions on Control");
        assert_eq!(rec.year, Some(1999));
        assert_eq!(rec.doc_type, "conferencePaper");
        assert!(rec.doi.is_empty());
    }

    #[test]
    fn decodes_latex_and_dashes() {
        assert_eq!(decode_latex(r"{\'E}cole des Mines -- 1920"), "École des Mines – 1920");
        assert_eq!(decode_latex(r"\emph{Deep} \textit{learning} 50\% of \& more"), "Deep learning 50% of & more");
        assert_eq!(
            decode_latex(r"$\alpha$ -- \url{https://example.com/a_b}"),
            "– https://example.com/a_b"
        );
        assert_eq!(decode_latex(r"{\c{c}}a va"), "ça va");
        assert_eq!(decode_latex(r"The {\TeX}book"), "The TeXbook");
        assert_eq!(decode_latex(r"\href{https://doi.org/x}{see \emph{paper}}"), "see paper");
    }

    #[test]
    fn skips_entries_without_title_and_doi() {
        let parsed = parse_bibtex("@misc{onlynote, note = {hello}}");
        assert_eq!(parsed.entries.len(), 1);
        assert!(parsed.entries[0].to_record().is_none());
    }

    #[test]
    fn reports_unclosed_entry() {
        let parsed = parse_bibtex("@article{broken, title = {Never closed");
        assert!(parsed.entries.is_empty());
        assert_eq!(parsed.warnings.len(), 1);
    }

    #[test]
    fn extracts_doi_from_url() {
        let parsed = parse_bibtex("@article{a, title={T}, url={https://doi.org/10.1000/xyz}}");
        let rec = parsed.entries[0].to_record().expect("record");
        assert_eq!(rec.doi, "10.1000/xyz");
    }
}
