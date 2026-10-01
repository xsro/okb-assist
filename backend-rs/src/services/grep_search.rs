//! 纯 Rust 全文搜索服务。
//!
//! 替代了旧版外部 `grep` 进程方案。内部使用：
//! - **字面量搜索**：`memchr::memmem::Finder`（SIMD 加速，比 grep 的 Boyer-Moore 更快）
//! - **正则搜索**：`regex::Regex`
//! - **目录遍历**：`walkdir`（已在依赖树中）
//! - **所有依赖均已在 Cargo.lock 中**，无需新增任何 crate。

use std::collections::HashSet;
use std::io::BufRead;
use std::path::Path;
use std::sync::OnceLock;

use sqlx::Row;

// ── 编译一次的正则：用于提取 doc_id（供旧版兼容调用） ──
static DOC_ID_RE_1: OnceLock<regex::Regex> = OnceLock::new();
static DOC_ID_RE_2: OnceLock<regex::Regex> = OnceLock::new();
static DOC_ID_RE_3: OnceLock<regex::Regex> = OnceLock::new();

fn doc_id_re_1() -> &'static regex::Regex {
    DOC_ID_RE_1.get_or_init(|| regex::Regex::new(r"/(\d+)\.md$").unwrap())
}
fn doc_id_re_2() -> &'static regex::Regex {
    DOC_ID_RE_2.get_or_init(|| regex::Regex::new(r"/(\d+)/(\d+)\.md$").unwrap())
}
fn doc_id_re_3() -> &'static regex::Regex {
    DOC_ID_RE_3.get_or_init(|| regex::Regex::new(r"/(\d+)/[^/]+\.md$").unwrap())
}

/// 从文件路径中提取文档 ID。
///
/// 优先用路径解析（零分配），回退到正则。
pub fn extract_doc_id(file_path: &str) -> Option<i64> {
    let path = Path::new(file_path);
    // 从文件 stem 解析
    if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
        if let Ok(id) = stem.parse::<i64>() {
            return Some(id);
        }
    }
    // 回退：可能文件名不直接是数字，检查父目录名
    if let Some(parent) = path.parent() {
        if let Some(dir_name) = parent.file_name().and_then(|s| s.to_str()) {
            if let Ok(id) = dir_name.parse::<i64>() {
                return Some(id);
            }
        }
    }
    // 最后用正则（兼容旧路径格式）
    if let Some(caps) = doc_id_re_1().captures(file_path) {
        return caps.get(1)?.as_str().parse().ok();
    }
    if let Some(caps) = doc_id_re_2().captures(file_path) {
        let dir_id = caps.get(1)?.as_str();
        let file_id = caps.get(2)?.as_str();
        if dir_id == file_id {
            return dir_id.parse().ok();
        }
    }
    if let Some(caps) = doc_id_re_3().captures(file_path) {
        return caps.get(1)?.as_str().parse().ok();
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

// ── 内部搜索核心 ──

/// 在文件中的搜索结果：一个匹配块（含上下文）
#[derive(Debug)]
struct MatchBlock {
    file_path: String,
    /// 块内第一行在文件中的行号（1-indexed）
    first_line: usize,
    /// 块内连续行内容
    lines: Vec<String>,
}

/// 读取文件所有行到 Vec，可选返回文件总行数。
fn read_lines(path: &str) -> Option<Vec<String>> {
    let file = std::fs::File::open(path).ok()?;
    let reader = std::io::BufReader::new(file);
    reader.lines().filter_map(|l| l.ok()).collect::<Vec<_>>().into()
}

/// 在单个文件的各行中搜索匹配。
///
/// 返回匹配块列表（已合并重叠上下文），每块包含连续行。
fn search_file_lines(
    lines: &[String],
    _query: &str,
    context: usize,
    is_regex: bool,
    compiled_regex: Option<&regex::Regex>,
    case_folded_query: &str,
) -> Vec<MatchBlock> {
    let total = lines.len();
    if total == 0 {
        return Vec::new();
    }

    // --- 第 1 步：找出所有匹配行的索引 ---
    let match_indices: Vec<usize> = if is_regex {
        let re = match compiled_regex {
            Some(r) => r,
            None => return Vec::new(),
        };
        lines.iter().enumerate().filter_map(|(i, line)| {
            if re.is_match(line) { Some(i) } else { None }
        }).collect()
    } else {
        let finder = memchr::memmem::Finder::new(case_folded_query.as_bytes());
        lines.iter().enumerate().filter_map(|(i, line)| {
            if finder.find(line.to_ascii_lowercase().as_bytes()).is_some() {
                Some(i)
            } else {
                None
            }
        }).collect()
    };

    if match_indices.is_empty() {
        return Vec::new();
    }

    // --- 第 2 步：合并重叠的上下文窗口 ---
    // 每个匹配会覆盖 [max(0, idx-context), min(total, idx+context+1)] 范围
    // 如果窗口间隔 <= 1（相邻或重叠），合并为一块
    let mut blocks: Vec<(usize, usize)> = Vec::new(); // (start, end) inclusive-exclusive

    for &idx in &match_indices {
        let block_start = idx.saturating_sub(context);
        let block_end = (idx + context + 1).min(total);

        if let Some(&mut (_, ref mut last_end)) = blocks.last_mut() {
            if block_start <= *last_end {
                // 重叠或相邻，合并
                *last_end = (*last_end).max(block_end);
            } else {
                blocks.push((block_start, block_end));
            }
        } else {
            blocks.push((block_start, block_end));
        }
    }

    // --- 第 3 步：构建 MatchBlock ---
    blocks
        .into_iter()
        .map(|(start, end)| {
            let file_path = String::new(); // 由调用者填充
            let first_line = start + 1; // 1-indexed
            let lines_slice = &lines[start..end];
            MatchBlock {
                file_path,
                first_line,
                lines: lines_slice.to_vec(),
            }
        })
        .collect()
}

/// 搜索单个文件，返回搜索结果。
fn search_one_file(
    path: &str,
    query: &str,
    context: usize,
    is_regex: bool,
    compiled_regex: Option<&regex::Regex>,
    case_folded_query: &str,
) -> Vec<MatchBlock> {
    let lines = match read_lines(path) {
        Some(l) => l,
        None => return Vec::new(),
    };
    let mut blocks = search_file_lines(&lines, query, context, is_regex, compiled_regex, case_folded_query);
    // 填入文件路径
    for block in &mut blocks {
        block.file_path = path.to_string();
    }
    blocks
}

/// 搜索多个文件，返回所有搜索结果（受 limit 限制）。
fn search_files(
    paths: &[String],
    query: &str,
    context: usize,
    limit: usize,
    is_regex: bool,
    compiled_regex: Option<&regex::Regex>,
    case_folded_query: &str,
) -> Vec<serde_json::Value> {
    let mut results = Vec::new();
    let mut seen_docs: HashSet<i64> = HashSet::new();

    for path in paths {
        if results.len() >= limit {
            break;
        }
        let doc_id = match extract_doc_id(path) {
            Some(id) => id,
            None => continue,
        };
        if seen_docs.contains(&doc_id) {
            continue;
        }
        let blocks = search_one_file(path, query, context, is_regex, compiled_regex, case_folded_query);
        for block in blocks {
            if results.len() >= limit {
                break;
            }
            if seen_docs.contains(&doc_id) {
                // 同一个文档的多个匹配块合并（只保留 doc_id 去重逻辑不变）
                continue;
            }
            seen_docs.insert(doc_id);
            results.push(serde_json::json!({
                "id": doc_id,
                "content": block.lines.join("\n"),
                "file_path": block.file_path,
                "first_line": block.first_line,
            }));
        }
    }
    results
}

/// 递归搜索目录（仅扫 *.md 文件）。
fn search_dir(
    dir: &str,
    query: &str,
    context: usize,
    limit: usize,
    is_regex: bool,
    compiled_regex: Option<&regex::Regex>,
    case_folded_query: &str,
) -> Vec<serde_json::Value> {
    let mut results = Vec::new();
    let mut seen_docs: HashSet<i64> = HashSet::new();

    let walker = walkdir::WalkDir::new(dir)
        .follow_links(false)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .filter(|e| {
            e.path().extension().map(|ext| ext == "md").unwrap_or(false)
        });

    for entry in walker {
        if results.len() >= limit {
            break;
        }
        let path_str = entry.path().to_string_lossy().to_string();
        let doc_id = match extract_doc_id(&path_str) {
            Some(id) => id,
            None => continue,
        };
        if seen_docs.contains(&doc_id) {
            continue;
        }
        let blocks = search_one_file(&path_str, query, context, is_regex, compiled_regex, case_folded_query);
        for block in blocks {
            if results.len() >= limit {
                break;
            }
            if seen_docs.contains(&doc_id) {
                continue;
            }
            seen_docs.insert(doc_id);
            results.push(serde_json::json!({
                "id": doc_id,
                "content": block.lines.join("\n"),
                "file_path": block.file_path,
                "first_line": block.first_line,
            }));
        }
    }
    results
}

/// 执行 grep 搜索（完整对齐旧版 API）。
///
/// 内部完成：期刊/年份元数据预筛 → 与 doc_ids 取交集 → fast 算法候选预筛 →
/// 构建搜索路径 → 纯 Rust 内部搜索（无外部进程）。
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

    // 2. 预备搜索参数（编译正则 / 折叠大小写）
    let is_regex = regex;
    let q_lower = query.to_ascii_lowercase();
    let compiled_regex: Option<regex::Regex> = if is_regex {
        regex::Regex::new(&format!("(?i){}", query)).ok()
    } else {
        None
    };

    // 3. 确定搜索模式并执行
    let fast_enabled = algorithm == "fast" && doc_ids_owned.is_none();

    // 将搜索结果构建移到 spawn_blocking 中
    let q_out = q_lower.clone();
    let q_raw = query.to_string();

    let result: Vec<serde_json::Value> = if fast_enabled {
        let candidates = metadata_candidate_ids(db, &q_raw).await;
        let paths: Vec<String> = candidates
            .iter()
            .map(|did| crate::paths::get_markdown_path(settings, *did))
            .filter(|p| Path::new(p).exists())
            .collect();
        if paths.is_empty() {
            // 无候选 → 回退全量目录扫描
            match markdown_parent_dir(settings) {
                Some(dir) => tokio::task::spawn_blocking(move || {
                    search_dir(&dir, &q_raw, context_lines, limit, is_regex, compiled_regex.as_ref(), &q_out)
                }).await.unwrap_or_default(),
                None => Vec::new(),
            }
        } else {
            tokio::task::spawn_blocking(move || {
                search_files(&paths, &q_raw, context_lines, limit, is_regex, compiled_regex.as_ref(), &q_out)
            }).await.unwrap_or_default()
        }
    } else {
        match &doc_ids_owned {
            Some(ids) if !ids.is_empty() => {
                let paths: Vec<String> = ids
                    .iter()
                    .map(|did| crate::paths::get_markdown_path(settings, *did))
                    .filter(|p| Path::new(p).exists())
                    .collect();
                if paths.is_empty() {
                    Vec::new()
                } else {
                    tokio::task::spawn_blocking(move || {
                        search_files(&paths, &q_raw, context_lines, limit, is_regex, compiled_regex.as_ref(), &q_out)
                    }).await.unwrap_or_default()
                }
            }
            _ => {
                // 全量扫描 → 目录递归
                match markdown_parent_dir(settings) {
                    Some(dir) => tokio::task::spawn_blocking(move || {
                        search_dir(&dir, &q_raw, context_lines, limit, is_regex, compiled_regex.as_ref(), &q_out)
                    }).await.unwrap_or_default(),
                    None => Vec::new(),
                }
            }
        }
    };

    result
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