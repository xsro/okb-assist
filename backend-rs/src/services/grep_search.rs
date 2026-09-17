//! 基于系统 grep 的轻量全文搜索服务。

use std::process::Command;

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

/// 执行 grep 搜索
pub async fn grep_search(
    query: &str,
    context_lines: usize,
    limit: usize,
    doc_ids: Option<&[i64]>,
    algorithm: &str,
    regex: bool,
    search_paths: &[String],
) -> Vec<serde_json::Value> {
    if search_paths.is_empty() {
        return Vec::new();
    }

    let mut cmd = Command::new("grep");
    cmd.arg("-rn").arg("-i").arg(format!("-C{}", context_lines));
    if !regex {
        cmd.arg("-F");
    }
    cmd.arg(query);
    for path in search_paths {
        cmd.arg(path);
    }

    let output = cmd.output();
    match output {
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
    let re = regex::Regex::new(r"^(.+?)[\:\-](\d+)[\:\-](.*)$").ok()?;
    let caps = re.captures(line)?;
    Some((
        caps.get(1)?.as_str().to_string(),
        caps.get(2)?.as_str().to_string(),
        caps.get(3)?.as_str().to_string(),
    ))
}

fn extract_doc_id(file_path: &str) -> Option<i64> {
    let re = regex::Regex::new(r"/(\d+)\.md$").ok()?;
    if let Some(caps) = re.captures(file_path) {
        return caps.get(1)?.as_str().parse().ok();
    }
    let re2 = regex::Regex::new(r"/(\d+)/\1\.md$").ok()?;
    if let Some(caps) = re2.captures(file_path) {
        return caps.get(1)?.as_str().parse().ok();
    }
    let re3 = regex::Regex::new(r"/(\d+)/[^/]+\.md$").ok()?;
    if let Some(caps) = re3.captures(file_path) {
        return caps.get(1)?.as_str().parse().ok();
    }
    None
}

/// 列出所有 markdown 文件路径
pub fn list_all_markdown_paths(settings: &crate::config::Settings) -> Vec<String> {
    use std::path::Path;
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

/// 期刊/年份过滤
pub async fn meta_filter_ids(
    db: &sqlx::SqlitePool,
    journal: Option<&str>,
    year_start: Option<i64>,
    year_end: Option<i64>,
) -> Vec<i64> {
    let mut query_str = String::from("SELECT id FROM documents WHERE 1=1");
    let mut binds: Vec<serde_json::Value> = Vec::new();

    if let Some(j) = journal {
        if !j.trim().is_empty() {
            query_str.push_str(" AND journal LIKE ?");
            binds.push(serde_json::Value::String(format!("%{}%", j.trim())));
        }
    }
    if let Some(ys) = year_start {
        query_str.push_str(" AND year >= ?");
        binds.push(serde_json::Value::Number(ys.into()));
    }
    if let Some(ye) = year_end {
        query_str.push_str(" AND year <= ?");
        binds.push(serde_json::Value::Number(ye.into()));
    }

    // For simplicity, use a simpler approach with individual queries
    if journal.is_some() && year_start.is_some() && year_end.is_some() {
        let j = journal.unwrap();
        let like = format!("%{}%", j.trim());
        match sqlx::query_as::<_, (i64,)>(
            "SELECT id FROM documents WHERE journal LIKE ? AND year >= ? AND year <= ?"
        )
            .bind(&like)
            .bind(year_start.unwrap())
            .bind(year_end.unwrap())
            .fetch_all(db)
            .await
        {
            Ok(rows) => return rows.into_iter().map(|(id,)| id).collect(),
            Err(_) => return Vec::new(),
        }
    }

    // Fallback: just return all IDs
    match sqlx::query_as::<_, (i64,)>("SELECT id FROM documents")
        .fetch_all(db)
        .await
    {
        Ok(rows) => rows.into_iter().map(|(id,)| id).collect(),
        Err(_) => Vec::new(),
    }
}