//! MCP 服务。
//!
//! 提供与 Python 版 `app/mcp_server.py` 一致的 MCP 工具集，
//! 以及 Streamable HTTP JSON-RPC 端点（`/assist/mcp/stream`）。

use std::sync::Arc;

use axum::extract::Extension;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Json, Response};
use serde_json::{json, Value};
use sqlx::Row;

use crate::config::Settings;
use crate::database::Database;
use crate::models::{DocStatus, Document};
use crate::paths;
use crate::services::grep_search::{grep_search as do_grep, parse_doc_ids};

/// Document 全部列的 SELECT 语句（列顺序与 `Document` 结构体字段一致）。
const DOC_COLUMNS: &str = "id, filename, file_hash, title, authors, year, doi, source, journal, \
     keywords, abstract, category, doc_type, language, title_en, authors_en, \
     keywords_en, abstract_en, journal_en, mineru_task_id, status, status_message, \
     progress, qdrant_collection, vector_db_id, created_at, updated_at";

pub struct McpServer {
    db: Arc<Database>,
    settings: Arc<Settings>,
}

impl McpServer {
    pub fn new(db: Arc<Database>, settings: Arc<Settings>) -> Self {
        Self { db, settings }
    }

    pub async fn list_tools(&self) -> Vec<Value> {
        vec![
            json!({
                "name": "grep_search",
                "description": "Full-text search of document content (grep-based, lightweight and fast). No vector database required, supports regex. Supports pagination via page/offset and context truncation.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "query": {"type": "string", "description": "Search keywords (supports regex)"},
                        "limit": {"type": "integer", "default": 10, "description": "Results per page (alias: max_results)"},
                        "max_results": {"type": "integer", "default": 10, "description": "Results per page (alias for limit)"},
                        "page": {"type": "integer", "default": 1, "description": "Page number (1-based)"},
                        "offset": {"type": "integer", "default": 0, "description": "Zero-based result offset (overrides page when provided)"},
                        "context": {"type": "integer", "default": 2, "description": "Number of context lines before/after each match"},
                        "max_context_chars": {"type": "integer", "default": 500, "description": "Maximum characters per result content (0 = no truncation)"},
                        "doc_ids": {"type": "string", "default": "", "description": "Document ID scope, supports commas and ranges (e.g. '1,2,5-100')"},
                        "algorithm": {"type": "string", "default": "full"},
                        "regex": {"type": "boolean", "default": true},
                        "journal": {"type": "string", "default": ""},
                        "year_start": {"type": "integer", "default": 0},
                        "year_end": {"type": "integer", "default": 0}
                    },
                    "required": ["query"]
                }
            }),
            json!({
                "name": "search_info",
                "description": "Search document metadata (title, authors, journal, keywords, abstract, DOI, etc.), supports Chinese and English. Supports pagination, field filtering and year/type filters.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "query": {"type": "string"},
                        "limit": {"type": "integer", "default": 10, "description": "Results per page (alias: max_results)"},
                        "max_results": {"type": "integer", "default": 10, "description": "Results per page (alias for limit)"},
                        "page": {"type": "integer", "default": 1, "description": "Page number (1-based)"},
                        "offset": {"type": "integer", "default": 0, "description": "Zero-based result offset (overrides page when provided)"},
                        "year_from": {"type": "integer", "default": 0, "description": "Filter: minimum publication year"},
                        "year_to": {"type": "integer", "default": 0, "description": "Filter: maximum publication year"},
                        "doc_type": {"type": "string", "default": "", "description": "Filter: document type (comma-separated for multiple)"},
                        "fields": {"type": "array", "items": {"type": "string"}, "description": "Optional list of fields to return (e.g. ['id','title','year'])"}
                    },
                    "required": ["query"]
                }
            }),
            json!({
                "name": "read_markdown",
                "description": "Read document Markdown content (paginated), or extract a specific section by heading.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "id": {"type": "integer"},
                        "page": {"type": "integer", "default": 1},
                        "page_size": {"type": "integer", "default": 5000},
                        "section": {"type": "string", "description": "Extract the section matching this heading (case-insensitive), e.g. 'Introduction'. Takes precedence over pagination."},
                        "sections": {"type": "array", "items": {"type": "string"}, "description": "Extract multiple sections by heading names."}
                    },
                    "required": ["id"]
                }
            }),
            json!({
                "name": "get_document_info",
                "description": "Get detailed document information, including metadata, processing status, PDF and Markdown links.",
                "inputSchema": {
                    "type": "object",
                    "properties": {"id": {"type": "integer"}},
                    "required": ["id"]
                }
            }),
            json!({
                "name": "list_documents",
                "description": "Search or list documents. Supports search by title/author, filter by status and document type, and field filtering.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "query": {"type": "string", "default": ""},
                        "status": {"type": "string", "default": ""},
                        "doc_type": {"type": "string", "default": ""},
                        "page": {"type": "integer", "default": 1},
                        "page_size": {"type": "integer", "default": 20, "description": "Results per page (1-100, alias: limit)"},
                        "limit": {"type": "integer", "default": 20, "description": "Results per page (alias for page_size)"},
                        "fields": {"type": "array", "items": {"type": "string"}, "description": "Optional list of fields to return (e.g. ['id','title','year'])"}
                    }
                }
            }),
            json!({
                "name": "get_document_abstract",
                "description": "Get document abstract information. Multi-language abstracts are both returned when available.",
                "inputSchema": {
                    "type": "object",
                    "properties": {"id": {"type": "integer"}},
                    "required": ["id"]
                }
            }),
            json!({
                "name": "get_stats",
                "description": "Get knowledge base statistics, including total document count, counts by status and type.",
                "inputSchema": {"type": "object", "properties": {}}
            }),
            json!({
                "name": "list_doc_types",
                "description": "List all document types used in the knowledge base (Zotero standard types).",
                "inputSchema": {"type": "object", "properties": {}}
            }),
        ]
    }

    pub async fn call_tool(&self, name: &str, args: &Value) -> Value {
        let text = match name {
            "grep_search" => self.tool_grep_search(args).await,
            "search_info" => self.tool_search_info(args).await,
            "read_markdown" => self.tool_read_markdown(args).await,
            "get_document_info" => self.tool_get_document_info(args).await,
            "list_documents" => self.tool_list_documents(args).await,
            "get_document_abstract" => self.tool_get_document_abstract(args).await,
            "get_stats" => self.tool_get_stats().await,
            "list_doc_types" => self.tool_list_doc_types().await,
            _ => {
                return json!({
                    "isError": true,
                    "content": [{"type": "text", "text": format!("Unknown tool: {}", name)}]
                })
            }
        };
        json!({"content": [{"type": "text", "text": text}]})
    }

    // ── 工具实现 ──

    async fn fetch_doc(&self, doc_id: i64) -> Option<Document> {
        sqlx::query_as::<_, Document>(&format!("SELECT {} FROM documents WHERE id = ?", DOC_COLUMNS))
            .bind(doc_id)
            .fetch_optional(self.db.pool())
            .await
            .unwrap_or(None)
    }

    fn format_doc(&self, doc: &Document) -> Value {
        let year = doc.year;
        let has_markdown = std::path::Path::new(&paths::get_markdown_path(&self.settings, doc.id)).exists();
        json!({
            "id": doc.id,
            "filename": doc.filename,
            "title": doc.title.clone().unwrap_or_else(|| doc.filename.clone()),
            "authors": doc.authors,
            "year": year,
            "doi": doc.doi,
            "journal": doc.journal,
            "keywords": doc.keywords,
            "abstract": doc.abstract_text,
            "category": doc.category,
            "doc_type": doc.doc_type,
            "language": doc.language,
            "status": doc.status,
            "has_markdown": has_markdown,
            "pdf_url": format!("/assist/api/documents/{}/pdf", doc.id),
            "markdown_url": format!("/assist/markdown/{}", doc.id),
            "detail_url": format!("/assist/detail/{}", doc.id),
        })
    }

    fn pretty(v: &Value) -> String {
        serde_json::to_string_pretty(v).unwrap_or_else(|_| v.to_string())
    }

    /// 统一的错误返回格式：`{"error": {"code": ..., "message": ...}}`。
    fn err(code: &str, message: &str) -> String {
        Self::pretty(&json!({"error": {"code": code, "message": message}}))
    }

    /// 解析分页参数，返回 `(offset, limit)`。`offset` 优先于 `page`；`limit` 兼容 `max_results` 别名。
    fn parse_pagination(args: &Value, default_limit: i64, max_limit: i64) -> (i64, i64) {
        let limit = args["limit"]
            .as_i64()
            .or_else(|| args["max_results"].as_i64())
            .unwrap_or(default_limit)
            .clamp(1, max_limit);
        let offset = args["offset"].as_i64().unwrap_or_else(|| {
            (args["page"].as_i64().unwrap_or(1).max(1) - 1).saturating_mul(limit)
        });
        (offset.max(0), limit)
    }

    /// 解析 `fields` 参数为字段白名单；未提供或为空时返回 `None`。
    fn parse_fields(args: &Value) -> Option<Vec<String>> {
        args.get("fields")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|x| x.as_str().map(|s| s.to_string()))
                    .collect::<Vec<_>>()
            })
            .filter(|v| !v.is_empty())
    }

    /// 按字段白名单过滤 JSON 对象；无白名单时原样返回。
    fn filter_fields(value: &mut Value, fields: &Option<Vec<String>>) {
        if let Some(fields) = fields {
            if let Some(obj) = value.as_object_mut() {
                let keep: std::collections::HashSet<&str> = fields.iter().map(|s| s.as_str()).collect();
                obj.retain(|k, _| keep.contains(k.as_str()));
            }
        }
    }

    /// 按字符数截断字符串（超过 `max` 时追加省略号）。`max = 0` 表示不截断。
    fn truncate_chars(s: &str, max: usize) -> String {
        if max == 0 || s.chars().count() <= max {
            s.to_string()
        } else {
            let mut out: String = s.chars().take(max).collect();
            out.push('…');
            out
        }
    }

    async fn tool_grep_search(&self, args: &Value) -> String {
        let query = args["query"].as_str().unwrap_or("").to_string();
        let context = args["context"].as_i64().unwrap_or(2).max(0) as usize;
        let doc_ids = args["doc_ids"].as_str().unwrap_or("").to_string();
        let algorithm = args["algorithm"].as_str().unwrap_or("full").to_string();
        let regex = args["regex"].as_bool().unwrap_or(true);
        let journal = args["journal"].as_str().unwrap_or("").to_string();
        let year_start = args["year_start"].as_i64().unwrap_or(0);
        let year_end = args["year_end"].as_i64().unwrap_or(0);
        let max_context_chars = args["max_context_chars"].as_i64().unwrap_or(500).max(0) as usize;

        if query.trim().is_empty() {
            return Self::err("invalid_argument", "查询不能为空");
        }

        let (offset, limit) = Self::parse_pagination(args, 10, 100);

        let id_list = match parse_doc_ids(&doc_ids) {
            Ok(v) => v,
            Err(e) => return Self::err("invalid_argument", &e),
        };

        let journal_opt = if journal.trim().is_empty() { None } else { Some(journal.trim()) };
        let year_start_opt = if year_start > 0 { Some(year_start) } else { None };
        let year_end_opt = if year_end > 0 { Some(year_end) } else { None };

        // 多取一条，用于判断是否还有下一页
        let fetch_limit = offset.saturating_add(limit).saturating_add(1) as usize;
        let results = do_grep(
            query.trim(),
            context,
            fetch_limit,
            id_list.as_deref(),
            &algorithm,
            regex,
            self.db.pool(),
            &self.settings,
            journal_opt,
            year_start_opt,
            year_end_opt,
        )
        .await;

        let has_more = results.len() as i64 > offset.saturating_add(limit);
        let page_results = results.into_iter().skip(offset as usize).take(limit as usize);

        let mut enriched = Vec::new();
        for hit in page_results {
            let doc_id = hit["id"].as_i64();
            let raw_content = hit["content"].as_str().unwrap_or("").to_string();
            let content = Self::truncate_chars(&raw_content, max_context_chars);
            let mut info = json!({
                "id": doc_id,
                "content": content,
            });
            if let Some(id) = doc_id {
                if let Some(doc) = self.fetch_doc(id).await {
                    info["title"] = json!(doc.title.clone().unwrap_or_default());
                    info["authors"] = json!(doc.authors.clone().unwrap_or_default());
                    info["year"] = json!(doc.year);
                    info["journal"] = json!(doc.journal.clone().unwrap_or_default());
                }
            }
            enriched.push(info);
        }

        Self::pretty(&json!({
            "query": query,
            "offset": offset,
            "limit": limit,
            "returned": enriched.len(),
            "has_more": has_more,
            "results": enriched,
        }))
    }

    async fn tool_search_info(&self, args: &Value) -> String {
        let query = args["query"].as_str().unwrap_or("").to_string();
        let year_from = args["year_from"].as_i64().unwrap_or(0);
        let year_to = args["year_to"].as_i64().unwrap_or(0);
        let doc_type = args["doc_type"].as_str().unwrap_or("").to_string();
        let fields = Self::parse_fields(args);

        if query.trim().is_empty() {
            return Self::err("invalid_argument", "查询不能为空");
        }

        let (offset, limit) = Self::parse_pagination(args, 10, 100);
        let like = format!("%{}%", query.trim());

        // 计数
        let mut cqb = sqlx::QueryBuilder::new("SELECT COUNT(*) FROM documents WHERE ");
        append_search_info_filters(&mut cqb, &like, year_from, year_to, &doc_type);
        let total: i64 = cqb
            .build()
            .fetch_one(self.db.pool())
            .await
            .map(|r| r.get::<i64, _>(0))
            .unwrap_or(0);

        // 查询
        let mut qb = sqlx::QueryBuilder::new(format!("SELECT {} FROM documents WHERE ", DOC_COLUMNS));
        append_search_info_filters(&mut qb, &like, year_from, year_to, &doc_type);
        qb.push(" ORDER BY updated_at DESC LIMIT ").push_bind(limit)
            .push(" OFFSET ").push_bind(offset);
        let docs: Vec<Document> = qb
            .build_query_as::<Document>()
            .fetch_all(self.db.pool())
            .await
            .unwrap_or_default();

        let results: Vec<Value> = docs
            .iter()
            .map(|doc| {
                let mut v = json!({
                    "id": doc.id,
                    "title": doc.title.clone().unwrap_or_default(),
                    "title_en": doc.title_en,
                    "authors": doc.authors.clone().unwrap_or_default(),
                    "authors_en": doc.authors_en,
                    "year": doc.year,
                    "doi": doc.doi.clone().unwrap_or_default(),
                    "journal": doc.journal.clone().unwrap_or_default(),
                    "journal_en": doc.journal_en,
                    "keywords": doc.keywords.clone().unwrap_or_default(),
                    "abstract": doc.abstract_text.as_deref().unwrap_or("").chars().take(300).collect::<String>(),
                    "doc_type": doc.doc_type.clone().unwrap_or_default(),
                    "language": doc.language.clone().unwrap_or_default(),
                    "category": doc.category.clone().unwrap_or_default(),
                    "status": doc.status,
                });
                Self::filter_fields(&mut v, &fields);
                v
            })
            .collect();

        Self::pretty(&json!({
            "query": query,
            "total": total,
            "page": (offset / limit) + 1,
            "limit": limit,
            "total_pages": (total + limit - 1) / limit,
            "results": results,
        }))
    }

    async fn tool_read_markdown(&self, args: &Value) -> String {
        let doc_id = args["id"].as_i64().unwrap_or(0);
        let page = args["page"].as_i64().unwrap_or(1).max(1) as usize;
        let page_size = args["page_size"].as_i64().unwrap_or(5000).max(1) as usize;

        // 解析章节过滤目标（section 与 sections 合并）
        let mut targets: Vec<String> = Vec::new();
        if let Some(s) = args["section"].as_str() {
            let s = s.trim();
            if !s.is_empty() {
                targets.push(s.to_string());
            }
        }
        if let Some(arr) = args["sections"].as_array() {
            for x in arr {
                if let Some(s) = x.as_str() {
                    let s = s.trim();
                    if !s.is_empty() {
                        targets.push(s.to_string());
                    }
                }
            }
        }

        let doc = match self.fetch_doc(doc_id).await {
            Some(d) => d,
            None => return Self::err("not_found", &format!("文档 {} 不存在", doc_id)),
        };

        let md_path = paths::get_markdown_path(&self.settings, doc_id);
        let content = match std::fs::read_to_string(&md_path) {
            Ok(c) => c,
            Err(_) => {
                return Self::err("not_found", "Markdown file not yet generated, please parse the PDF first")
            }
        };

        let title = doc.title.clone().unwrap_or_else(|| doc.filename.clone());

        // 章节提取模式：优先于分页
        if !targets.is_empty() {
            let sections = parse_markdown_sections(&content);
            let matched = extract_matched_sections(&sections, &targets);
            if matched.is_empty() {
                return Self::pretty(&json!({
                    "error": {"code": "not_found", "message": "未找到匹配的章节"},
                    "available_sections": top_headings(&sections, 50),
                }));
            }
            let matched_sections: Vec<&str> = matched.iter().map(|(h, _, _)| h.as_str()).collect();
            let extracted = matched
                .iter()
                .map(|(_, _, c)| c.as_str())
                .collect::<Vec<_>>()
                .join("\n\n");
            return Self::pretty(&json!({
                "id": doc_id,
                "title": title,
                "section": targets.join(", "),
                "matched_sections": matched_sections,
                "content": extracted,
            }));
        }

        // 分页模式
        let total_pages = (content.len() + page_size - 1) / page_size.max(1);
        let total_pages = total_pages.max(1);
        let start = (page - 1) * page_size;
        let page_content = if start < content.len() {
            content.chars().skip(start).take(page_size).collect::<String>()
        } else {
            String::new()
        };

        Self::pretty(&json!({
            "id": doc_id,
            "title": title,
            "page": page,
            "total_pages": total_pages,
            "content": page_content,
        }))
    }

    async fn tool_get_document_info(&self, args: &Value) -> String {
        let doc_id = args["id"].as_i64().unwrap_or(0);
        match self.fetch_doc(doc_id).await {
            Some(doc) => Self::pretty(&self.format_doc(&doc)),
            None => Self::err("not_found", &format!("文档 {} 不存在", doc_id)),
        }
    }

    async fn tool_list_documents(&self, args: &Value) -> String {
        let query = args["query"].as_str().unwrap_or("").to_string();
        let status = args["status"].as_str().unwrap_or("").to_string();
        let doc_type = args["doc_type"].as_str().unwrap_or("").to_string();
        let page = args["page"].as_i64().unwrap_or(1).max(1) as i64;
        // page_size 为主，limit 为兼容别名，范围 1-100
        let page_size = args["page_size"]
            .as_i64()
            .or_else(|| args["limit"].as_i64())
            .unwrap_or(20)
            .clamp(1, 100) as i64;
        let fields = Self::parse_fields(args);

        // 计数
        let mut cqb = sqlx::QueryBuilder::new("SELECT COUNT(*) FROM documents WHERE 1=1");
        append_list_filters(&mut cqb, &query, &status, &doc_type);
        let total: i64 = cqb
            .build()
            .fetch_one(self.db.pool())
            .await
            .map(|r| r.get::<i64, _>(0))
            .unwrap_or(0);

        // 分页查询
        let mut qb = sqlx::QueryBuilder::new(format!("SELECT {} FROM documents WHERE 1=1", DOC_COLUMNS));
        append_list_filters(&mut qb, &query, &status, &doc_type);
        qb.push(" ORDER BY created_at DESC LIMIT ").push_bind(page_size)
            .push(" OFFSET ").push_bind((page - 1).saturating_mul(page_size));

        let docs: Vec<Document> = qb
            .build_query_as::<Document>()
            .fetch_all(self.db.pool())
            .await
            .unwrap_or_default();
        let items: Vec<Value> = docs
            .iter()
            .map(|d| {
                let mut v = self.format_doc(d);
                Self::filter_fields(&mut v, &fields);
                v
            })
            .collect();

        Self::pretty(&json!({
            "total": total,
            "page": page,
            "page_size": page_size,
            "total_pages": if total == 0 { 0 } else { (total + page_size - 1) / page_size },
            "items": items,
        }))
    }

    async fn tool_get_document_abstract(&self, args: &Value) -> String {
        let doc_id = args["id"].as_i64().unwrap_or(0);
        let doc = match self.fetch_doc(doc_id).await {
            Some(d) => d,
            None => return Self::err("not_found", &format!("文档 {} 不存在", doc_id)),
        };
        let mut result = json!({
            "id": doc_id,
            "title": doc.title.clone().unwrap_or_else(|| doc.filename.clone()),
            "abstract": doc.abstract_text,
        });
        if let Some(ae) = &doc.abstract_en {
            result["abstract_en"] = json!(ae);
        }
        if let Some(lang) = &doc.language {
            if lang != "en" {
                result["language"] = json!(lang);
            }
        }
        Self::pretty(&result)
    }

    async fn tool_get_stats(&self) -> String {
        let total: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM documents")
            .fetch_one(self.db.pool())
            .await
            .unwrap_or(0);

        let mut status_counts = serde_json::Map::new();
        for s in [
            DocStatus::Uploaded,
            DocStatus::Parsing,
            DocStatus::MarkdownDone,
            DocStatus::Error,
        ] {
            let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM documents WHERE status = ?")
                .bind(s.as_str())
                .fetch_one(self.db.pool())
                .await
                .unwrap_or(0);
            if count > 0 {
                status_counts.insert(s.as_str().to_string(), json!(count));
            }
        }

        let rows: Vec<(Option<String>,)> =
            sqlx::query_as("SELECT doc_type FROM documents WHERE doc_type IS NOT NULL AND doc_type != ''")
                .fetch_all(self.db.pool())
                .await
                .unwrap_or_default();
        let mut type_counts: serde_json::Map<String, Value> = serde_json::Map::new();
        for (dt,) in rows {
            if let Some(dt) = dt {
                let e = type_counts.entry(dt).or_insert(json!(0));
                *e = json!(e.as_i64().unwrap_or(0) + 1);
            }
        }
        let mut sorted: Vec<(String, Value)> = type_counts.into_iter().collect();
        sorted.sort_by(|a, b| b.1.as_i64().unwrap_or(0).cmp(&a.1.as_i64().unwrap_or(0)));
        let type_counts_sorted: serde_json::Map<String, Value> = sorted.into_iter().collect();

        Self::pretty(&json!({
            "total_documents": total,
            "status_counts": status_counts,
            "type_counts": type_counts_sorted,
            "indexed_count": status_counts.get("indexed").and_then(|v| v.as_i64()).unwrap_or(0),
        }))
    }

    async fn tool_list_doc_types(&self) -> String {
        let rows: Vec<(Option<String>,)> =
            sqlx::query_as("SELECT DISTINCT doc_type FROM documents WHERE doc_type IS NOT NULL AND doc_type != ''")
                .fetch_all(self.db.pool())
                .await
                .unwrap_or_default();
        let mut types: Vec<String> = rows.into_iter().filter_map(|(t,)| t).collect();
        types.sort();
        Self::pretty(&json!({"doc_types": types}))
    }
}

/// 向 QueryBuilder 追加 list_documents 的筛选条件。
fn append_list_filters<'a>(
    qb: &mut sqlx::QueryBuilder<'a, sqlx::Sqlite>,
    query: &str,
    status: &str,
    doc_type: &str,
) {
    if !query.trim().is_empty() {
        let like = format!("%{}%", query.trim());
        qb.push(" AND (title LIKE ").push_bind(like.clone())
            .push(" OR authors LIKE ").push_bind(like.clone())
            .push(" OR filename LIKE ").push_bind(like).push(")");
    }
    if !status.trim().is_empty() {
        let statuses: Vec<String> = status.split(',').map(|s| s.trim().to_string()).collect();
        qb.push(" AND status IN (");
        for (i, s) in statuses.iter().enumerate() {
            if i > 0 {
                qb.push(", ");
            }
            qb.push_bind(s.clone());
        }
        qb.push(")");
    }
    if !doc_type.trim().is_empty() {
        let types: Vec<String> = doc_type.split(',').map(|s| s.trim().to_string()).collect();
        qb.push(" AND doc_type IN (");
        for (i, t) in types.iter().enumerate() {
            if i > 0 {
                qb.push(", ");
            }
            qb.push_bind(t.clone());
        }
        qb.push(")");
    }
}

/// 向 QueryBuilder 追加 search_info 的筛选条件（不含 WHERE 关键字）。
fn append_search_info_filters<'a>(
    qb: &mut sqlx::QueryBuilder<'a, sqlx::Sqlite>,
    like: &str,
    year_from: i64,
    year_to: i64,
    doc_type: &str,
) {
    qb.push("(");
    let fields = [
        "title", "title_en", "authors", "authors_en", "journal", "journal_en",
        "keywords", "keywords_en", "abstract", "abstract_en", "doi", "category",
        "source", "filename",
    ];
    for (i, f) in fields.iter().enumerate() {
        if i > 0 {
            qb.push(" OR ");
        }
        qb.push(f).push(" LIKE ").push_bind(like.to_string());
    }
    qb.push(")");
    if year_from > 0 {
        qb.push(" AND year >= ").push_bind(year_from);
    }
    if year_to > 0 {
        qb.push(" AND year <= ").push_bind(year_to);
    }
    if !doc_type.trim().is_empty() {
        let types: Vec<String> = doc_type.split(',').map(|s| s.trim().to_string()).collect();
        qb.push(" AND doc_type IN (");
        for (i, t) in types.iter().enumerate() {
            if i > 0 {
                qb.push(", ");
            }
            qb.push_bind(t.clone());
        }
        qb.push(")");
    }
}

/// Markdown 章节（标题 + 级别 + 标题后正文）。
struct MdSection {
    heading: String,
    level: usize,
    body: String,
}

/// 解析 Markdown ATX 标题行，返回 `(标题文本, 级别)`。仅识别 `# `、`## ` 等标准标题。
fn parse_heading_line(line: &str) -> Option<(String, usize)> {
    let trimmed = line.trim_start();
    let level = trimmed.bytes().take_while(|b| *b == b'#').count();
    if level == 0 || level > 6 {
        return None;
    }
    let after = &trimmed[level..];
    if !after.is_empty() && !after.starts_with(' ') && !after.starts_with('\t') {
        return None;
    }
    let text = after.trim();
    if text.is_empty() {
        return None;
    }
    Some((text.to_string(), level))
}

/// 解析全文为章节列表；每个标题的正文归属其下方，直到下一个标题。
fn parse_markdown_sections(content: &str) -> Vec<MdSection> {
    let mut sections: Vec<MdSection> = Vec::new();
    let mut current: Option<MdSection> = None;

    for line in content.lines() {
        if let Some((heading, level)) = parse_heading_line(line) {
            if let Some(sec) = current.take() {
                sections.push(sec);
            }
            current = Some(MdSection { heading, level, body: String::new() });
        } else if let Some(sec) = current.as_mut() {
            if !sec.body.is_empty() {
                sec.body.push('\n');
            }
            sec.body.push_str(line);
        }
        // 首个标题之前的内容（通常是论文标题）忽略
    }
    if let Some(sec) = current.take() {
        sections.push(sec);
    }
    sections
}

/// 标题文本归一化（去除首尾空白、压缩内部空白）。
fn normalize_heading(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// 判断标题是否匹配目标（不区分大小写；先精确匹配，再子串匹配）。
fn heading_matches(heading: &str, target: &str) -> bool {
    let h = normalize_heading(heading);
    let t = normalize_heading(target);
    if t.is_empty() {
        return false;
    }
    let h_l = h.to_lowercase();
    let t_l = t.to_lowercase();
    h_l == t_l || h_l.contains(&t_l)
}

/// 重建标题行。
fn format_heading(heading: &str, level: usize) -> String {
    format!("{} {}", "#".repeat(level), heading)
}

/// 提取匹配的章节（含其子章节），返回 `(标题, 级别, 内容)`。
fn extract_matched_sections(sections: &[MdSection], targets: &[String]) -> Vec<(String, usize, String)> {
    let mut result = Vec::new();
    for target in targets {
        let Some(idx) = sections.iter().position(|s| heading_matches(&s.heading, target)) else {
            continue;
        };
        let level = sections[idx].level;
        let mut content = format_heading(&sections[idx].heading, level);
        if !sections[idx].body.is_empty() {
            content.push('\n');
            content.push_str(&sections[idx].body);
        }
        let mut i = idx + 1;
        while i < sections.len() && sections[i].level > level {
            content.push('\n');
            content.push_str(&format_heading(&sections[i].heading, sections[i].level));
            if !sections[i].body.is_empty() {
                content.push('\n');
                content.push_str(&sections[i].body);
            }
            i += 1;
        }
        result.push((sections[idx].heading.clone(), level, content));
    }
    result
}

/// 顶层标题列表（优先 1 级，缺省时取 2 级）。
fn top_headings(sections: &[MdSection], limit: usize) -> Vec<String> {
    let level1: Vec<&str> = sections
        .iter()
        .filter(|s| s.level == 1)
        .map(|s| s.heading.as_str())
        .collect();
    let pool: Vec<&str> = if level1.is_empty() {
        sections
            .iter()
            .filter(|s| s.level == 2)
            .map(|s| s.heading.as_str())
            .collect()
    } else {
        level1
    };
    pool.into_iter().take(limit).map(|s| s.to_string()).collect()
}

/// Streamable HTTP JSON-RPC 端点。
pub async fn mcp_stream_handler(
    Extension(db): Extension<Arc<Database>>,
    Extension(settings): Extension<Arc<Settings>>,
    headers: HeaderMap,
    body: String,
) -> Response {
    // Bearer token 校验（mcp_token）
    let mcp_token = settings.mcp_token();
    let auth_enabled = !mcp_token.is_empty() && mcp_token != "change-me";
    if auth_enabled {
        let authorized = headers
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .map(|s| s == format!("Bearer {}", mcp_token))
            .unwrap_or(false);
        let lan = headers
            .get("x-forwarded-for")
            .and_then(|v| v.to_str().ok())
            .map(|s| s.split(',').next().unwrap_or("").trim().starts_with("192.168.1."))
            .unwrap_or(false);
        if !authorized && !lan {
            return (
                StatusCode::UNAUTHORIZED,
                Json(json!({"jsonrpc": "2.0", "error": {"code": -32001, "message": "Unauthorized"}})),
            )
                .into_response();
        }
    }

    let server = McpServer::new(db, settings);
    let req: Value = match serde_json::from_str(&body) {
        Ok(v) => v,
        Err(_) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({"jsonrpc": "2.0", "error": {"code": -32700, "message": "Parse error"}})),
            )
                .into_response();
        }
    };

    let method = req.get("method").and_then(|m| m.as_str()).unwrap_or("").to_string();
    let id = req.get("id").cloned().unwrap_or(Value::Null);
    let params = req.get("params").cloned().unwrap_or(json!({}));

    // 通知类消息不返回响应
    if method.starts_with("notifications/") {
        return (StatusCode::ACCEPTED, "").into_response();
    }

    let response = match method.as_str() {
        "initialize" => json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": {
                "protocolVersion": params.get("protocolVersion").cloned().unwrap_or_else(|| json!("2024-11-05")),
                "capabilities": {"tools": {}},
                "serverInfo": {"name": "OKB-Assist", "version": "0.1.0-rust"}
            }
        }),
        "ping" => json!({"jsonrpc": "2.0", "id": id, "result": {}}),
        "tools/list" => json!({"jsonrpc": "2.0", "id": id, "result": {"tools": server.list_tools().await}}),
        "tools/call" => {
            let name = params.get("name").and_then(|n| n.as_str()).unwrap_or("");
            let args = params.get("arguments").cloned().unwrap_or(json!({}));
            let result = server.call_tool(name, &args).await;
            json!({"jsonrpc": "2.0", "id": id, "result": result})
        }
        "resources/list" => json!({"jsonrpc": "2.0", "id": id, "result": {"resources": []}}),
        "resources/read" => json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": {"code": -32601, "message": "Resources not supported"}
        }),
        _ => json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": {"code": -32601, "message": format!("Method not found: {}", method)}
        }),
    };

    Json(response).into_response()
}
