//! 基于系统 grep 的轻量全文搜索服务。

use std::collections::HashSet;
use std::path::Path;
use std::process::Command;
use std::sync::OnceLock;

use sqlx::Row;

// ── 编译一次、全局复用的正则 ──
static GREP_LINE_RE: OnceLock<regex::Regex> = OnceLock::new();
static DOC_ID_RE_1: OnceLock<regex::Regex> = OnceLock::new();
static DOC_ID_RE_2: OnceLock<regex::Regex> = OnceLock::new();
static DOC_ID_RE_3: OnceLock<regex::Regex> = OnceLock::new();

fn grep_line_re() -> &'static regex::Regex {
    GREP_LINE_RE.get_or_init(|| regex::Regex::new(r"^(.+?)[\:\-](\d+)[\:\-](.*)$").unwrap())
}

fn doc_id_re_1() -> &'static regex::Regex {
    DOC_ID_RE_1.get_or_init(|| regex::Regex::new(r"/(\d+)\.md$").unwrap())
}

fn doc_id_re_2() -> &'static regex::Regex {
    DOC_ID_RE_2.get_or_init(|| regex::Regex::new(r"/(\d+)/\1\.md$").unwrap())
}

fn doc_id_re_3() -> &'static regex::Regex {
    DOC_ID_RE_3.get_or_init(|| regex::Regex::new(r"/(\d+)/[^/]+\.md$").unwrap())
}

/// grep 单批文件，返回解析结果（受 limit 限制）。
fn run_grep(
    query: &str,
    context_lines: usize,
    regex: bool,
    paths: &[String],
    limit: usize,
) -> Vec<serde_json::Value> {
    let mut cmd = Command::new("grep");
    cmd.arg("-rn").arg("-i").arg(format!("-C{}", context_lines));
    if !regex {
        cmd.arg("-F");
    }
    cmd.arg(query);
    for p in paths {
        cmd.arg(p);
    }

    match cmd.output() {
        Ok(output) => {
            if output.status.code().unwrap_or(1) > 1 {
                return Vec::new();
            }
            let stdout = String::from_utf8_lossy(&output.stdout);
            parse_grep_output(&stdout, limit)
        }
        Err(_) => Vec::new(),
    }
}

/// grep 递归目录模式（仅扫 *.md），不受 ARG_MAX 限制。
fn run_grep_dir(
    query: &str,
    context_lines: usize,
    regex: bool,
    parent_dir: &str,
    limit: usize,
) -> Vec<serde_json::Value> {
    let mut cmd = Command::new("grep");
    cmd.arg("-r")
        .arg("--include=*.md")
        .arg("-rn")
        .arg("-i")
        .arg(format!("-C{}", context_lines));
    if !regex {
        cmd.arg("-F");
    }
    cmd.arg(query).arg(parent_dir);

    match cmd.output() {
        Ok(output) => {
            if output.status.code().unwrap_or(1) > 1 {
                return Vec::new();
            }
            let stdout = String::from_utf8_lossy(&output.stdout);
            parse_grep_output(&stdout, limit)
        }
        Err(_) => Vec::new(),
    }
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

/// 执行 grep 搜索（完整对齐 Python 版逻辑）。
///
/// 内部完成：期刊/年份元数据预筛 → 与 doc_ids 取交集 → fast 算法候选预筛 →
/// 构建搜索路径 → 调用系统 grep → 解析输出。
pub async fn grep_search(
    query: &str,
    context_lines: usize,
    limit: usize,
    doc_ids: Option<&[i64]>,
    algorithm: &str,
    regex: bool,
    db: &sqlx::SqlitePool,
    settings: &crate::config::Settings,
    journal: Option<&str>,
    year_start: Option<i64>,
    year_end: Option<i64>,
) -> Vec<serde_json::Value> {
    let mut doc_ids_owned: Option<Vec<i64>> = doc_ids.map(|v| v.to_vec());

    // 1. 期刊/年份元数据预筛
    if journal.is_some() || year_start.is_some() || year_end.is_some() {
        let meta = meta_filter_ids(db, journal, year_start, year_end).await;
        if meta.is_empty() {
            return Vec::new();
        }
        match doc_ids_owned.as_mut() {
            Some(ids) => {
                let set: HashSet<i64> = meta.iter().copied().collect();
                ids.retain(|id| set.contains(id));
                if ids.is_empty() {
                    return Vec::new();
                }
            }
            None => doc_ids_owned = Some(meta),
        }
    }

    // 2. 确定搜索模式：目录递归 or 指定文件列表
    let fast_enabled = algorithm == "fast" && doc_ids_owned.is_none();

    if fast_enabled {
        let candidates = metadata_candidate_ids(db, query).await;
        let paths: Vec<String> = candidates
            .iter()
            .map(|did| crate::paths::get_markdown_path(settings, *did))
            .filter(|p| Path::new(p).exists())
            .collect();
        if paths.is_empty() {
            // 无候选 → 回退全量目录扫描
            match markdown_parent_dir(settings) {
                Some(dir) => return run_grep_dir(query, context_lines, regex, &dir, limit),
                None => return Vec::new(),
            }
        }
        // 有候选 → 分批精确搜索
        return run_grep_batched(query, context_lines, regex, &paths, limit);
    }

    // 非 fast 模式
    match &doc_ids_owned {
        Some(ids) if !ids.is_empty() => {
            let paths: Vec<String> = ids
                .iter()
                .map(|did| crate::paths::get_markdown_path(settings, *did))
                .filter(|p| Path::new(p).exists())
                .collect();
            if paths.is_empty() {
                return Vec::new();
            }
            run_grep_batched(query, context_lines, regex, &paths, limit)
        }
        _ => {
            // 全量扫描 → 目录递归
            match markdown_parent_dir(settings) {
                Some(dir) => run_grep_dir(query, context_lines, regex, &dir, limit),
                None => Vec::new(),
            }
        }
    }
}

fn parse_grep_output(output: &str, limit: usize) -> Vec<serde_json::Value> {
    let mut results = Vec::new();
    let mut current_file = String::new();
    let mut current_lines: Vec<String> = Vec::new();
    let mut seen_files: std::collections::HashSet<i64> = std::collections::HashSet::new();

    for line in output.lines() {
        if line == "--" {
            if !current_file.is_empty() && !current_lines.is_empty() {
                if let Some(doc_id) = extract_doc_id(&current_file) {
                    if !seen_files.contains(&doc_id) {
                        seen_files.insert(doc_id);
                        results.push(serde_json::json!({
                            "document_id": doc_id,
                            "content": current_lines.join("\n"),
                            "file_path": current_file,
                        }));
                        if results.len() >= limit {
                            return results;
                        }
                    }
                }
            }
            current_lines.clear();
            continue;
        }

        if let Some(caps) = parse_grep_line(line) {
            if !current_file.is_empty() && caps.0 != current_file && !current_lines.is_empty() {
                if let Some(doc_id) = extract_doc_id(&current_file) {
                    if !seen_files.contains(&doc_id) {
                        seen_files.insert(doc_id);
                        results.push(serde_json::json!({
                            "document_id": doc_id,
                            "content": current_lines.join("\n"),
                            "file_path": current_file,
                        }));
                        if results.len() >= limit {
                            return results;
                        }
                    }
                }
                current_lines.clear();
            }
            current_file = caps.0;
            current_lines.push(caps.2);
        } else if !line.trim().is_empty() {
            current_lines.push(line.to_string());
        }
    }

    // 处理最后一块
    if !current_file.is_empty() && !current_lines.is_empty() {
        if let Some(doc_id) = extract_doc_id(&current_file) {
            if !seen_files.contains(&doc_id) {
                results.push(serde_json::json!({
                    "document_id": doc_id,
                    "content": current_lines.join("\n"),
                    "file_path": current_file,
                }));
            }
        }
    }

    results.truncate(limit);
    results
}

fn parse_grep_line(line: &str) -> Option<(String, String, String)> {
    let re = grep_line_re();
    let caps = re.captures(line)?;
    Some((
        caps.get(1)?.as_str().to_string(),
        caps.get(2)?.as_str().to_string(),
        caps.get(3)?.as_str().to_string(),
    ))
}

pub fn extract_doc_id(file_path: &str) -> Option<i64> {
    if let Some(caps) = doc_id_re_1().captures(file_path) {
        return caps.get(1)?.as_str().parse().ok();
    }
    if let Some(caps) = doc_id_re_2().captures(file_path) {
        return caps.get(1)?.as_str().parse().ok();
    }
    if let Some(caps) = doc_id_re_3().captures(file_path) {
        return caps.get(1)?.as_str().parse().ok();
    }
    None
}

/// 分批执行 grep（避免 ARG_MAX 溢出），每批最多 500 个文件。
fn run_grep_batched(
    query: &str,
    context_lines: usize,
    regex: bool,
    paths: &[String],
    limit: usize,
) -> Vec<serde_json::Value> {
    const BATCH: usize = 500;
    let mut results = Vec::new();
    for chunk in paths.chunks(BATCH) {
        if results.len() >= limit {
            break;
        }
        let mut batch = run_grep(query, context_lines, regex, chunk, limit - results.len());
        results.append(&mut batch);
    }
    results
}

/// 列出所有 markdown 文件路径（用于调试 / 兼容旧调用）
pub fn list_all_markdown_paths(settings: &crate::config::Settings) -> Vec<String> {
    let sample = crate::paths::get_markdown_path(settings, 0);
    let dir = match Path::new(&sample).parent() {
        Some(d) => d,
        None => return Vec::new(),
    };
    if !dir.is_dir() {
        return Vec::new();
    }
    match std::fs::read_dir(dir) {
        Ok(entries) => entries
            .filter_map(|e| e.ok())
            .map(|e| e.path().to_string_lossy().to_string())
            .filter(|p| p.ends_with(".md"))
            .collect(),
        Err(_) => Vec::new(),
    }
}

/// 元数据预筛候选文档 ID
pub async fn metadata_candidate_ids(
    db: &sqlx::SqlitePool,
    query: &str,
) -> Vec<i64> {
    let like = format!("%{}%", query);
    match sqlx::query_as::<_, (i64,)>(
        "SELECT id FROM documents WHERE 
         title LIKE ?1 OR abstract LIKE ?1 OR abstract_en LIKE ?1 
         OR keywords LIKE ?1 OR keywords_en LIKE ?1 OR authors LIKE ?1"
    )
        .bind(&like)
        .fetch_all(db)
        .await
    {
        Ok(rows) => rows.into_iter().map(|(id,)| id).collect(),
        Err(_) => Vec::new(),
    }
}

/// 期刊/年份过滤（AND 关系，journal 忽略大小写 LIKE 模糊匹配）
pub async fn meta_filter_ids(
    db: &sqlx::SqlitePool,
    journal: Option<&str>,
    year_start: Option<i64>,
    year_end: Option<i64>,
) -> Vec<i64> {
    let mut qb = sqlx::QueryBuilder::new("SELECT id FROM documents WHERE 1=1");
    if let Some(j) = journal {
        if !j.trim().is_empty() {
            qb.push(" AND journal LIKE ").push_bind(format!("%{}%", j.trim()));
        }
    }
    if let Some(ys) = year_start {
        qb.push(" AND year >= ").push_bind(ys);
    }
    if let Some(ye) = year_end {
        qb.push(" AND year <= ").push_bind(ye);
    }
    let rows = qb.build().fetch_all(db).await.unwrap_or_default();
    rows.iter().map(|r| r.get::<i64, _>("id")).collect()
}
