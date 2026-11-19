//! 纯 Rust 全文搜索服务（v2：match 级结果）。
//!
//! 内部使用：
//! - **字面量预筛**：`memchr::memmem::Finder`（ASCII 且不区分大小写、无词边界时）
//! - **正则**：`regex::Regex`（同时负责给出命中区间，便于返回 `matched/column/sentence`）
//! - **目录遍历**：`walkdir`
//!
//! v2 相比 v1 的变化：
//! - 每条结果里给出 `matches[]`：`line / column / matched / sentence / section`，
//!   以及 `match_count`（该文档内的**全部**命中数，不受 `max_matches_per_doc` 截断影响）；
//! - 支持 `count_only`（只数不改写内容）、`strip_references`（跳过参考文献小节）、
//!   `whole_word`、`case_sensitive`、`max_matches_per_doc`；
//! - `content` 仍保留（合并后的上下文块），因此旧客户端不受影响。

use std::collections::HashSet;
use std::io::BufRead;
use std::path::Path;

use sqlx::Row;

/// 从文件路径中提取文档 ID。
///
/// 先按文件 stem 解析（`{id}.md`），失败则看父目录名（`{id}/xxx.md`）。
pub fn extract_doc_id(file_path: &str) -> Option<i64> {
    let path = Path::new(file_path);
    if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
        if let Ok(id) = stem.parse::<i64>() {
            return Some(id);
        }
    }
    if let Some(parent) = path.parent() {
        if let Some(dir_name) = parent.file_name().and_then(|s| s.to_str()) {
            if let Ok(id) = dir_name.parse::<i64>() {
                return Some(id);
            }
        }
    }
    None
}

/// 获取 markdown 文件所在父目录（从 system.json 模板推导）。
fn markdown_parent_dir(settings: &crate::config::Settings) -> Option<String> {
    let sample = crate::paths::get_markdown_path(settings, 0);
    let dir = Path::new(&sample).parent()?;
    if dir.is_dir() {
        Some(dir.to_string_lossy().to_string())
    } else {
        None
    }
}

/// 解析 '1,2,5-100,4' 形式的文档 ID 列表
pub fn parse_doc_ids(s: &str) -> Result<Option<Vec<i64>>, String> {
    let s = s.trim();
    if s.is_empty() {
        return Ok(None);
    }
    let mut ids = Vec::new();
    for part in s.split(',') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        if part.contains('-') {
            let mut iter = part.split('-');
            let lo_s = iter.next().ok_or("Invalid range")?.trim();
            let hi_s = iter.next().ok_or("Invalid range")?.trim();
            let lo: i64 = lo_s.parse().map_err(|_| format!("Invalid number: {}", lo_s))?;
            let hi: i64 = hi_s.parse().map_err(|_| format!("Invalid number: {}", hi_s))?;
            let (lo, hi) = if lo > hi { (hi, lo) } else { (lo, hi) };
            ids.extend(lo..=hi);
        } else {
            let n: i64 = part.parse().map_err(|_| format!("Invalid number: {}", part))?;
            ids.push(n);
        }
    }
    Ok(Some(ids))
}

// ── 查询参数 ──

/// 结果模式：`Doc` 沿用旧的「每文档一条」，`Match` 强调逐条命中（两者都返回 `matches[]`，
/// 区别只在于 `content` 的聚合方式与默认的 `max_matches_per_doc`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MatchMode {
    Doc,
    Match,
}

impl MatchMode {
    pub fn parse(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "match" | "matches" => MatchMode::Match,
            _ => MatchMode::Doc,
        }
    }
}

#[derive(Debug, Clone)]
pub struct GrepQuery {
    pub query: String,
    /// 上下文行数（写入 `content`）
    pub context: usize,
    /// 最多返回多少个文档（旧的 `limit`）
    pub limit: usize,
    /// 每个文档最多返回多少条命中（`match_count` 仍是全量计数）
    pub max_matches_per_doc: usize,
    pub doc_ids: Option<Vec<i64>>,
    pub algorithm: String,
    pub regex: bool,
    pub case_sensitive: bool,
    pub whole_word: bool,
    pub include_sentence: bool,
    pub strip_references: bool,
    pub count_only: bool,
    pub mode: MatchMode,
    pub journal: Option<String>,
    pub year_start: Option<i64>,
    pub year_end: Option<i64>,
    pub start_after_id: Option<i64>,
}

impl GrepQuery {
    pub fn new(query: impl Into<String>) -> Self {
        Self {
            query: query.into(),
            context: 2,
            limit: 10,
            max_matches_per_doc: 5,
            doc_ids: None,
            algorithm: "full".to_string(),
            regex: true,
            case_sensitive: false,
            whole_word: false,
            include_sentence: true,
            strip_references: false,
            count_only: false,
            mode: MatchMode::Doc,
            journal: None,
            year_start: None,
            year_end: None,
            start_after_id: None,
        }
    }
}

// ── 内部结构 ──

/// 一条命中
#[derive(Debug, Clone)]
struct MatchItem {
    /// 1-indexed 行号
    line: usize,
    /// 该行内 1-indexed 的字符列（按字节偏移换算到字符数）
    column: usize,
    /// 实际命中的文本
    matched: String,
    /// 命中所在的完整句子（可选）
    sentence: Option<String>,
    /// 最近的 Markdown 标题（小节名）
    section: Option<String>,
}

/// 一个文件的扫描结果
#[derive(Debug, Default)]
struct FileScan {
    matches: Vec<MatchItem>,
    /// 全部命中数（可能 > matches.len()）
    total: usize,
    /// 合并后的上下文块：(起始行 1-indexed, 行内容)
    blocks: Vec<(usize, String)>,
}

/// 标题：(行索引 0-indexed, 标题文本)
type Headings = Vec<(usize, String)>;

/// 读取文件所有行（保持 1:1 行号）。
fn read_lines(path: &str) -> Option<Vec<String>> {
    let file = std::fs::File::open(path).ok()?;
    let reader = std::io::BufReader::new(file);
    Some(reader.lines().map(|l| l.unwrap_or_default()).collect())
}

/// 收集 Markdown 标题行。
fn collect_headings(lines: &[String]) -> Headings {
    let mut out = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        let t = line.trim_start();
        if !t.starts_with('#') {
            continue;
        }
        let level = t.chars().take_while(|c| *c == '#').count();
        if !(1..=6).contains(&level) {
            continue;
        }
        let rest = t[level..].trim();
        if rest.is_empty() {
            continue;
        }
        out.push((i, rest.to_string()));
    }
    out
}

/// 判断标题是否属于「参考文献 / Bibliography」小节。
fn is_reference_heading(title: &str) -> bool {
    // 去掉编号前缀：`VI. References` / `6 References` / `8: Bibliography`
    let lower = title.trim().to_ascii_lowercase();
    let last = lower.rsplit(['.', ':']).find(|s| !s.trim().is_empty()).unwrap_or(&lower);
    let t = last
        .trim()
        .trim_start_matches(|c: char| c.is_ascii_digit() || c.is_whitespace() || c == '-');
    // 只认「就是参考文献」的标题，避免把 `Reference Frames and Notation` 这类正文小节误判
    matches!(
        t,
        "references" | "reference" | "bibliography" | "literature cited" | "参考文献" | "引用文献"
    ) || t.starts_with("references and")
        || t.starts_with("bibliography and")
        || t.starts_with("references (")
        || t.starts_with("references[")
}

/// 找到 `line_idx` 之前（含）最近的标题。
fn section_for(headings: &Headings, line_idx: usize) -> Option<(&str, bool)> {
    // headings 已按行号升序；用二分找最后一个 <= line_idx
    let pos = headings.partition_point(|(i, _)| *i <= line_idx);
    if pos == 0 {
        return None;
    }
    let (_, title) = &headings[pos - 1];
    Some((title.as_str(), is_reference_heading(title)))
}

/// 取命中所在句子：在行内以 `. ! ? 。 ！ ？` + 空白/结尾 为界，最多 ±160 字符。
fn sentence_around(line: &str, start: usize, end: usize) -> String {
    if line.is_empty() {
        return String::new();
    }
    let start = start.min(line.len());
    let end = end.min(line.len()).max(start);
    // 默认从句首开始；只有找到句末标点或超过窗口上限时才前移
    let mut s = 0usize;
    for (idx, ch) in line[..start].char_indices().rev() {
        if matches!(ch, '.' | '!' | '?' | '。' | '！' | '？' | ';' | '；') {
            s = idx + ch.len_utf8();
            break;
        }
        if start - idx > 400 {
            s = idx;
            break;
        }
    }
    let mut e = line.len();
    let mut it = line[end..].char_indices().peekable();
    while let Some((idx, ch)) = it.next() {
        if matches!(ch, '.' | '!' | '?' | '。' | '！' | '？' | ';' | '；') {
            // 句号后面必须是空白或结尾，避免把 ``3.5`` / ``Fig. 3`` 切断得太碎
            let abs = end + idx;
            let after_space = it
                .peek()
                .map(|(_, c)| c.is_whitespace())
                .unwrap_or(true);
            if after_space {
                e = abs + ch.len_utf8();
                break;
            }
        }
        if (end + idx) - end > 400 {
            e = end + idx;
            break;
        }
    }
    let mut out = line[s..e].trim().to_string();
    // 单句过长时做窗口裁剪（保留命中）
    const MAX: usize = 320;
    if out.chars().count() > MAX {
        let rel_start = line[s..start].chars().count();
        let rel_end = line[s..end].chars().count();
        let pad = 120usize;
        let lo = rel_start.saturating_sub(pad);
        let hi = (rel_end + pad).min(out.chars().count());
        out = out.chars().skip(lo).take(hi - lo).collect::<String>().trim().to_string();
    }
    out
}

/// 行内字节偏移 → 1-indexed 字符列号。
fn char_column(line: &str, byte_off: usize) -> usize {
    line[..byte_off.min(line.len())].chars().count() + 1
}

/// 编译「用于取命中区间」的正则。
fn compile_span_regex(q: &GrepQuery) -> Result<regex::Regex, String> {
    let base = if q.regex {
        q.query.clone()
    } else {
        regex::escape(&q.query)
    };
    let base = if q.whole_word {
        format!(r"\b(?:{})\b", base)
    } else {
        base
    };
    let pattern = if q.case_sensitive {
        base
    } else {
        format!("(?i){}", base)
    };
    regex::Regex::new(&pattern).map_err(|e| format!("正则无效：{}", e))
}

/// 扫描单个文件。
fn scan_file(
    lines: &[String],
    span_re: &regex::Regex,
    prefilter: Option<&memchr::memmem::Finder<'_>>,
    q: &GrepQuery,
) -> FileScan {
    let total = lines.len();
    let mut scan = FileScan::default();
    if total == 0 {
        return scan;
    }
    let headings = collect_headings(lines);

    let mut matched_lines: Vec<usize> = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        let hit = match prefilter {
            Some(f) => f.find(line.to_ascii_lowercase().as_bytes()).is_some(),
            None => span_re.is_match(line),
        };
        if !hit {
            continue;
        }
        let sec = section_for(&headings, i);
        if q.strip_references {
            if let Some((_, true)) = sec {
                continue;
            }
        }
        let mut found_here = false;
        for m in span_re.find_iter(line) {
            scan.total += 1;
            found_here = true;
            if scan.matches.len() >= q.max_matches_per_doc {
                continue;
            }
            let matched = m.as_str().to_string();
            scan.matches.push(MatchItem {
                line: i + 1,
                column: char_column(line, m.start()),
                matched,
                sentence: if q.include_sentence {
                    Some(sentence_around(line, m.start(), m.end()))
                } else {
                    None
                },
                section: sec.map(|(t, _)| t.to_string()),
            });
        }
        if found_here {
            matched_lines.push(i);
        }
    }

    if q.count_only || matched_lines.is_empty() {
        return scan;
    }

    // 合并上下文窗口
    let ctx = q.context;
    let mut windows: Vec<(usize, usize)> = Vec::new(); // [start, end) 0-indexed
    for &idx in &matched_lines {
        let s = idx.saturating_sub(ctx);
        let e = (idx + ctx + 1).min(total);
        match windows.last_mut() {
            Some((_, last_end)) if s <= *last_end => *last_end = (*last_end).max(e),
            _ => windows.push((s, e)),
        }
    }
    for (s, e) in windows {
        scan.blocks.push((s + 1, lines[s..e].join("\n")));
    }
    scan
}

// ── 主入口 ──

/// 执行 grep 搜索。
///
/// 返回值按文档聚合：
/// ```json
/// {"id":96,"match_count":3,"content":"...","first_line":410,
///  "matches":[{"line":412,"column":31,"matched":"Panda","sentence":"...","section":"VI. Experiments"}]}
/// ```
pub async fn grep_search(
    q: &GrepQuery,
    db: &sqlx::SqlitePool,
    settings: &crate::config::Settings,
) -> Result<Vec<serde_json::Value>, String> {
    if q.query.trim().is_empty() {
        return Ok(Vec::new());
    }
    let span_re = compile_span_regex(q)?;

    // 1. 期刊/年份元数据预筛
    let mut doc_ids_owned: Option<Vec<i64>> = q.doc_ids.clone();
    if q.journal.is_some() || q.year_start.is_some() || q.year_end.is_some() {
        // 期刊过滤支持别名（TRO / TAC …），与检索式 `SO=`、列表 `?journal=` 同一套解析
        let aliases = settings.journal_aliases();
        let meta = meta_filter_ids(db, q.journal.as_deref(), &aliases, q.year_start, q.year_end).await;
        match doc_ids_owned.as_mut() {
            Some(ids) => {
                let set: HashSet<i64> = meta.into_iter().collect();
                ids.retain(|id| set.contains(id));
            }
            None => doc_ids_owned = Some(meta),
        }
        if doc_ids_owned.as_ref().map(|v| v.is_empty()).unwrap_or(false) {
            return Ok(Vec::new());
        }
    }

    // 2. 确定候选文件
    let fast_enabled = q.algorithm == "fast" && doc_ids_owned.is_none();
    let mut entries: Vec<(i64, String)> = if fast_enabled {
        let candidates = metadata_candidate_ids(db, q.query.trim()).await;
        let mut v: Vec<(i64, String)> = candidates
            .into_iter()
            .map(|id| (id, crate::paths::get_markdown_path(settings, id)))
            .filter(|(_, p)| Path::new(p).exists())
            .collect();
        if v.is_empty() {
            // 无候选 → 回退全量目录扫描
            v = collect_dir_entries(settings);
        }
        v.sort_by_key(|(id, _)| *id);
        v
    } else {
        match &doc_ids_owned {
            Some(ids) if !ids.is_empty() => {
                let mut v: Vec<(i64, String)> = ids
                    .iter()
                    .map(|id| (*id, crate::paths::get_markdown_path(settings, *id)))
                    .filter(|(_, p)| Path::new(p).exists())
                    .collect();
                v.sort_by_key(|(id, _)| *id);
                v
            }
            Some(_) => Vec::new(),
            None => collect_dir_entries(settings),
        }
    };

    // 3. 游标过滤
    if let Some(start_id) = q.start_after_id {
        entries.retain(|(id, _)| *id > start_id);
    }

    // 4. 预筛器（仅 ASCII 字面量、不区分大小写、无词边界时启用）
    let use_memmem = !q.regex
        && !q.whole_word
        && !q.case_sensitive
        && q.query.is_ascii()
        && !q.query.is_empty();
    let lowered = q.query.to_ascii_lowercase();

    let qq = q.clone();
    let span_re_owned = span_re.clone();
    let result = tokio::task::spawn_blocking(move || {
        let prefilter = if use_memmem {
            Some(memchr::memmem::Finder::new(lowered.as_bytes()))
        } else {
            None
        };
        let mut out: Vec<serde_json::Value> = Vec::new();
        for (doc_id, path) in &entries {
            if out.len() >= qq.limit {
                break;
            }
            let Some(lines) = read_lines(path) else { continue };
            let scan = scan_file(&lines, &span_re_owned, prefilter.as_ref(), &qq);
            if scan.total == 0 {
                continue;
            }
            let matches: Vec<serde_json::Value> = scan
                .matches
                .iter()
                .map(|m| {
                    let mut v = serde_json::json!({
                        "line": m.line,
                        "column": m.column,
                        "matched": m.matched,
                    });
                    if let Some(s) = &m.sentence {
                        v["sentence"] = serde_json::json!(s);
                    }
                    if let Some(s) = &m.section {
                        v["section"] = serde_json::json!(s);
                    }
                    v
                })
                .collect();

            let content = scan
                .blocks
                .iter()
                .map(|(_, text)| text.as_str())
                .collect::<Vec<_>>()
                .join("\n…\n");
            let first_line = scan.blocks.first().map(|(l, _)| *l).unwrap_or(0);

            if qq.count_only {
                out.push(serde_json::json!({
                    "id": doc_id,
                    "match_count": scan.total,
                }));
            } else {
                out.push(serde_json::json!({
                    "id": doc_id,
                    "match_count": scan.total,
                    "content": content,
                    "first_line": first_line,
                    "file_path": path,
                    "matches": matches,
                }));
            }
        }
        out
    })
    .await
    .unwrap_or_default();

    Ok(result)
}

/// 递归收集 markdown 目录下的 `(doc_id, path)`，按 ID 升序。
fn collect_dir_entries(settings: &crate::config::Settings) -> Vec<(i64, String)> {
    let Some(dir) = markdown_parent_dir(settings) else {
        return Vec::new();
    };
    let mut entries: Vec<(i64, String)> = walkdir::WalkDir::new(dir)
        .follow_links(false)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .filter(|e| e.path().extension().map(|ext| ext == "md").unwrap_or(false))
        .filter_map(|entry| {
            let path_str = entry.path().to_string_lossy().to_string();
            extract_doc_id(&path_str).map(|id| (id, path_str))
        })
        .collect();
    entries.sort_by_key(|(id, _)| *id);
    entries
}

/// 元数据预筛候选文档 ID
pub async fn metadata_candidate_ids(db: &sqlx::SqlitePool, query: &str) -> Vec<i64> {
    let like = format!("%{}%", query);
    match sqlx::query_as::<_, (i64,)>(
        "SELECT id FROM documents WHERE 
         title LIKE ?1 OR abstract LIKE ?1 
         OR json_extract(i18n, '$.en.abstract') LIKE ?1
         OR keywords LIKE ?1 OR json_extract(i18n, '$.en.keywords') LIKE ?1
         OR authors LIKE ?1 OR json_extract(i18n, '$.en.authors') LIKE ?1",
    )
    .bind(&like)
    .fetch_all(db)
    .await
    {
        Ok(rows) => rows.into_iter().map(|(id,)| id).collect(),
        Err(_) => Vec::new(),
    }
}

/// 期刊/年份过滤（AND 关系）。
///
/// 期刊值先查 `journal_aliases`：命中别名 → `journal` / `i18n.en.journal` 精确匹配（大小写不敏感，
/// 可一对多）；未命中 → 原来的忽略大小写 LIKE 模糊匹配。
pub async fn meta_filter_ids(
    db: &sqlx::SqlitePool,
    journal: Option<&str>,
    aliases: &[crate::services::journal_alias::JournalAlias],
    year_start: Option<i64>,
    year_end: Option<i64>,
) -> Vec<i64> {
    use crate::services::journal_alias;
    let mut qb = sqlx::QueryBuilder::new("SELECT id FROM documents WHERE 1=1");
    if let Some(j) = journal {
        let j = j.trim();
        if !j.is_empty() {
            match journal_alias::resolve(aliases, j) {
                Some(names) => {
                    // 这里显式 push + push_bind 拼 IN 列表（不用 `in_clause`：它返回的文本里已带 `?`，
                    // 再交给 push_bind 会多插一遍占位符，见 mcp_server::push_search_where 的注释）
                    qb.push(" AND (");
                    for (ci, col) in journal_alias::JOURNAL_COLUMNS.iter().enumerate() {
                        if ci > 0 {
                            qb.push(" OR ");
                        }
                        qb.push(*col).push(" COLLATE NOCASE IN (");
                        for (i, name) in names.iter().enumerate() {
                            if i > 0 {
                                qb.push(", ");
                            }
                            qb.push_bind(name.clone());
                        }
                        qb.push(")");
                    }
                    qb.push(")");
                }
                None => {
                    qb.push(" AND journal LIKE ").push_bind(format!("%{}%", j));
                }
            }
        }
    }
    if let Some(ys) = year_start {
        qb.push(" AND CAST(NULLIF(TRIM(year), '') AS INTEGER) >= ").push_bind(ys);
    }
    if let Some(ye) = year_end {
        qb.push(" AND CAST(NULLIF(TRIM(year), '') AS INTEGER) <= ").push_bind(ye);
    }
    let rows = match qb.build().fetch_all(db).await {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(error = %e, "grep_search 元数据预筛失败，按无预筛处理");
            return Vec::new();
        }
    };
    rows.iter().map(|r| r.get::<i64, _>("id")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn q(pattern: &str) -> GrepQuery {
        let mut q = GrepQuery::new(pattern);
        q.include_sentence = true;
        q
    }

    #[test]
    fn sentence_keeps_match() {
        let line = "We use two Panda arms. The KUKA arm is a baseline. Results follow.";
        let idx = line.find("Panda").unwrap();
        let s = sentence_around(line, idx, idx + 5);
        assert!(s.starts_with("We use two Panda arms."), "got: {}", s);
        assert!(s.ends_with("arms."));
    }

    #[test]
    fn reference_heading_variants() {
        for h in ["References", "REFERENCES", "VI. References", "6 References", "8: Bibliography", "参考文献"] {
            assert!(is_reference_heading(h), "should be refs: {}", h);
        }
        for h in ["VI. Experiments", "Reference Frames and Notation", "Results"] {
            assert!(!is_reference_heading(h), "should not be refs: {}", h);
        }
    }

    #[test]
    fn headings_and_references() {
        let lines = vec![
            "# Title".to_string(),
            "## VI. Experiments".to_string(),
            "we used Franka Panda".to_string(),
            "## References".to_string(),
            "[1] Panda, KUKA".to_string(),
        ];
        let hs = collect_headings(&lines);
        assert_eq!(section_for(&hs, 2).unwrap().0, "VI. Experiments");
        assert!(section_for(&hs, 4).unwrap().1);
    }

    #[test]
    fn scan_strips_references_and_counts() {
        let lines = vec![
            "# Title".to_string(),
            "## Experiments".to_string(),
            "Panda and Panda.".to_string(),
            "## References".to_string(),
            "Panda again".to_string(),
        ];
        let mut query = q("Panda");
        query.strip_references = true;
        query.max_matches_per_doc = 5;
        let re = compile_span_regex(&query).unwrap();
        let scan = scan_file(&lines, &re, None, &query);
        assert_eq!(scan.total, 2, "references 小节应被跳过");
        assert_eq!(scan.matches.len(), 2);
        assert_eq!(scan.matches[0].line, 3);
        assert_eq!(scan.matches[0].section.as_deref(), Some("Experiments"));
    }

    #[test]
    fn whole_word_matches_are_bounded() {
        let mut query = q("Digit");
        query.regex = false;
        query.whole_word = true;
        let re = compile_span_regex(&query).unwrap();
        assert!(!re.is_match("Digital twin"));
        assert!(re.is_match("the Digit robot"));
    }

    #[test]
    fn max_matches_caps_but_count_is_full() {
        let lines = vec!["Panda Panda Panda".to_string()];
        let mut query = q("Panda");
        query.regex = false;
        query.max_matches_per_doc = 1;
        let re = compile_span_regex(&query).unwrap();
        let scan = scan_file(&lines, &re, None, &query);
        assert_eq!(scan.total, 3);
        assert_eq!(scan.matches.len(), 1);
    }
}

