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
                "description": "Full-text search of document content (grep-based, lightweight and fast). No vector database required, supports regex.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "query": {"type": "string", "description": "Search keywords (supports regex)"},
                        "limit": {"type": "integer", "default": 10},
                        "context": {"type": "integer", "default": 2},
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
                "description": "Search document metadata (title, authors, journal, keywords, abstract, DOI, etc.)",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "query": {"type": "string"},
                        "limit": {"type": "integer", "default": 10}
                    },
                    "required": ["query"]
                }
            }),
            json!({
                "name": "read_markdown",
                "description": "Read document Markdown content (paginated). Markdown is the text format parsed from PDF.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "doc_id": {"type": "integer"},
                        "page": {"type": "integer", "default": 1},
                        "page_size": {"type": "integer", "default": 5000}
                    },
                    "required": ["doc_id"]
                }
            }),
            json!({
                "name": "get_document_info",
                "description": "Get detailed document information, including metadata, processing status, PDF and Markdown links.",
                "inputSchema": {
                    "type": "object",
                    "properties": {"doc_id": {"type": "integer"}},
                    "required": ["doc_id"]
                }
            }),
            json!({
                "name": "list_documents",
                "description": "Search or list documents. Supports search by title/author, filter by status and document type.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "query": {"type": "string", "default": ""},
                        "status": {"type": "string", "default": ""},
                        "doc_type": {"type": "string", "default": ""},
                        "page": {"type": "integer", "default": 1},
                        "page_size": {"type": "integer", "default": 20}
                    }
                }
            }),
            json!({
                "name": "get_pdf_url",
                "description": "Get the PDF download/preview link for a document.",
                "inputSchema": {
                    "type": "object",
                    "properties": {"doc_id": {"type": "integer"}},
                    "required": ["doc_id"]
                }
            }),
            json!({
                "name": "get_document_abstract",
                "description": "Get document abstract information. Multi-language abstracts are both returned when available.",
                "inputSchema": {
                    "type": "object",
                    "properties": {"doc_id": {"type": "integer"}},
                    "required": ["doc_id"]
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
            "get_pdf_url" => self.tool_get_pdf_url(args).await,
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

    async fn tool_grep_search(&self, args: &Value) -> String {
        let query = args["query"].as_str().unwrap_or("").to_string();
        let limit = args["limit"].as_i64().unwrap_or(10).max(1) as usize;
        let context = args["context"].as_i64().unwrap_or(2).max(0) as usize;
        let doc_ids = args["doc_ids"].as_str().unwrap_or("").to_string();
        let algorithm = args["algorithm"].as_str().unwrap_or("full").to_string();
        let regex = args["regex"].as_bool().unwrap_or(true);
        let journal = args["journal"].as_str().unwrap_or("").to_string();
        let year_start = args["year_start"].as_i64().unwrap_or(0);
        let year_end = args["year_end"].as_i64().unwrap_or(0);

        if query.trim().is_empty() {
            return Self::pretty(&json!({"error": "查询不能为空"}));
        }

        let id_list = match parse_doc_ids(&doc_ids) {
            Ok(v) => v,
            Err(e) => return Self::pretty(&json!({"error": e})),
        };

        let journal_opt = if journal.trim().is_empty() { None } else { Some(journal.trim()) };
        let year_start_opt = if year_start > 0 { Some(year_start) } else { None };
        let year_end_opt = if year_end > 0 { Some(year_end) } else { None };

        let results = do_grep(
            query.trim(),
            context,
            limit,
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

        let mut enriched = Vec::new();
        for hit in &results {
            let doc_id = hit["document_id"].as_i64();
            let mut info = json!({
                "document_id": doc_id,
                "content": hit["content"],
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

        Self::pretty(&json!({"query": query, "total": enriched.len(), "results": enriched}))
    }

    async fn tool_search_info(&self, args: &Value) -> String {
        let query = args["query"].as_str().unwrap_or("").to_string();
        let limit = args["limit"].as_i64().unwrap_or(10).max(1) as i64;

        if query.trim().is_empty() {
            return Self::pretty(&json!({"error": "查询不能为空"}));
        }

        let like = format!("%{}%", query.trim());
        let sql = format!(
            "SELECT {} FROM documents WHERE title LIKE ?1 OR title_en LIKE ?1 OR authors LIKE ?1 \
             OR authors_en LIKE ?1 OR journal LIKE ?1 OR journal_en LIKE ?1 OR keywords LIKE ?1 \
             OR keywords_en LIKE ?1 OR abstract LIKE ?1 OR abstract_en LIKE ?1 OR doi LIKE ?1 \
             OR category LIKE ?1 OR source LIKE ?1 ORDER BY updated_at DESC LIMIT ?2",
            DOC_COLUMNS
        );
        let docs = sqlx::query_as::<_, Document>(&sql)
            .bind(&like)
            .bind(limit)
            .fetch_all(self.db.pool())
            .await
            .unwrap_or_default();

        let results: Vec<Value> = docs
            .iter()
            .map(|doc| {
                json!({
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
                })
            })
            .collect();

        Self::pretty(&json!({"query": query, "total": results.len(), "results": results}))
    }

    async fn tool_read_markdown(&self, args: &Value) -> String {
        let doc_id = args["doc_id"].as_i64().unwrap_or(0);
        let page = args["page"].as_i64().unwrap_or(1).max(1) as usize;
        let page_size = args["page_size"].as_i64().unwrap_or(5000).max(1) as usize;

        let doc = match self.fetch_doc(doc_id).await {
            Some(d) => d,
            None => return Self::pretty(&json!({"error": format!("文档 {} 不存在", doc_id)})),
        };

        let md_path = paths::get_markdown_path(&self.settings, doc_id);
        let content = match std::fs::read_to_string(&md_path) {
            Ok(c) => c,
            Err(_) => {
                return Self::pretty(&json!({
                    "error": "Markdown file not yet generated, please parse the PDF first"
                }))
            }
        };

        let total_pages = (content.len() + page_size - 1) / page_size.max(1);
        let total_pages = total_pages.max(1);
        let start = (page - 1) * page_size;
        let page_content = if start < content.len() {
            content.chars().skip(start).take(page_size).collect::<String>()
        } else {
            String::new()
        };

        Self::pretty(&json!({
            "doc_id": doc_id,
            "title": doc.title.clone().unwrap_or_else(|| doc.filename.clone()),
            "page": page,
            "total_pages": total_pages,
            "content": page_content,
        }))
    }

    async fn tool_get_document_info(&self, args: &Value) -> String {
        let doc_id = args["doc_id"].as_i64().unwrap_or(0);
        match self.fetch_doc(doc_id).await {
            Some(doc) => Self::pretty(&self.format_doc(&doc)),
            None => Self::pretty(&json!({"error": format!("文档 {} 不存在", doc_id)})),
        }
    }

    async fn tool_list_documents(&self, args: &Value) -> String {
        let query = args["query"].as_str().unwrap_or("").to_string();
        let status = args["status"].as_str().unwrap_or("").to_string();
        let doc_type = args["doc_type"].as_str().unwrap_or("").to_string();
        let page = args["page"].as_i64().unwrap_or(1).max(1) as i64;
        let page_size = args["page_size"].as_i64().unwrap_or(20).max(1) as i64;

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
            .push(" OFFSET ").push_bind((page - 1) * page_size);

        let docs: Vec<Document> = qb
            .build_query_as::<Document>()
            .fetch_all(self.db.pool())
            .await
            .unwrap_or_default();
        let items: Vec<Value> = docs.iter().map(|d| self.format_doc(d)).collect();

        Self::pretty(&json!({
            "total": total,
            "page": page,
            "page_size": page_size,
            "total_pages": (total + page_size - 1) / page_size,
            "items": items,
        }))
    }

    async fn tool_get_pdf_url(&self, args: &Value) -> String {
        let doc_id = args["doc_id"].as_i64().unwrap_or(0);
        let doc = match self.fetch_doc(doc_id).await {
            Some(d) => d,
            None => return Self::pretty(&json!({"error": format!("文档 {} 不存在", doc_id)})),
        };
        let pdf_path = paths::get_pdf_path(&self.settings, doc_id);
        if !std::path::Path::new(&pdf_path).exists() {
            return Self::pretty(&json!({"error": "PDF file does not exist"}));
        }
        Self::pretty(&json!({
            "doc_id": doc_id,
            "filename": doc.filename,
            "pdf_url": format!("/assist/api/documents/{}/pdf", doc_id),
            "title": doc.title,
        }))
    }

    async fn tool_get_document_abstract(&self, args: &Value) -> String {
        let doc_id = args["doc_id"].as_i64().unwrap_or(0);
        let doc = match self.fetch_doc(doc_id).await {
            Some(d) => d,
            None => return Self::pretty(&json!({"error": format!("文档 {} 不存在", doc_id)})),
        };
        let mut result = json!({
            "doc_id": doc_id,
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
            DocStatus::Extracting,
            DocStatus::MetaDone,
            DocStatus::Indexing,
            DocStatus::Indexed,
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
