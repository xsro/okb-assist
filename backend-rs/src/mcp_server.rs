//! MCP 服务。
//!
//! 提供文献库检索的 MCP 工具集，以及 Streamable HTTP JSON-RPC 端点（`/assist/mcp/stream`）。

use std::sync::Arc;

use axum::extract::Extension;
use axum::extract::Request;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Json, Response};
use serde_json::{json, Value};
use sqlx::Row;
use tracing::{debug, info, warn};

use crate::auth::Role;
use crate::config::Settings;
use crate::database::Database;
use crate::models::{DocStatus, Document};
use crate::paths;
use crate::routers::documents::{get_or_create_alias, sync_doc_file_sizes, write_atomic};
use crate::services::grep_search::{grep_search as do_grep, parse_doc_ids};

/// Document 全部列的 SELECT 语句（列顺序与 `Document` 结构体字段一致）。
// `year` 必须 CAST：老库里有写成 `''`（TEXT）的年份，按 `Option<i64>` 解码 TEXT 会报
// `mismatched types`，整条查询取不到任何行（与 documents.rs 的 DOC_COLUMNS 保持一致）。
const DOC_COLUMNS: &str = "id, filename, file_hash, title, authors, CAST(NULLIF(TRIM(year), '') AS INTEGER) AS year, doi, source, journal, \
     keywords, abstract, category, doc_type, language, i18n, mineru_task_id, fs_synced_at, \
     qdrant_collection, vector_db_id, created_at, updated_at, \
     pdf_size, md_size, zip_size, pdf_health, pdf_health_message";

/// 分页 offset 上限（防止恶意大 offset 导致全表扫描）。
const MAX_OFFSET: i64 = 10000;

/// `find_replace_markdown` 单次 `all=true` 允许替换的最大命中数。
const MAX_FIND_REPLACE: usize = 200;

pub struct McpServer {
    db: Arc<Database>,
    settings: Arc<Settings>,
    /// 调用方角色。写类工具（Markdown 编辑）仅 `admin` 可见且可用，
    /// 非 admin 连 `tools/list` 里都看不到它们。
    role: Role,
}

impl McpServer {
    pub fn new(db: Arc<Database>, settings: Arc<Settings>, role: Role) -> Self {
        Self { db, settings, role }
    }

    /// 是否允许使用写类工具（当前仅 `admin`）。
    pub fn can_write(&self) -> bool {
        matches!(self.role, Role::Admin)
    }

    pub async fn list_tools(&self) -> Vec<Value> {
        let mut tools = vec![
            json!({
                "name": "grep_search",
                "description": "Full-text search of document content (grep-based, lightweight and fast). No vector database required, supports regex. Results are sorted by document ID ascending; use start_after_id for cursor-based pagination.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "query": {"type": "string", "description": "Search keywords (supports regex)"},
                        "limit": {"type": "integer", "default": 10, "description": "Results per page (alias: max_results)"},
                        "max_results": {"type": "integer", "default": 10, "description": "Results per page (alias for limit)"},
                        "start_after_id": {"type": "integer", "default": 0, "description": "Cursor: start scanning from document IDs greater than this value. Use last_id from previous response for next page."},
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
                "description": "Search document metadata (title, authors, journal, keywords, abstract, DOI, etc.), supports Chinese and English. Supports pagination via page/offset (alias: max_results for limit), field filtering and year/type filters.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "query": {"type": "string"},
                        "limit": {"type": "integer", "default": 10, "description": "Results per page (alias: max_results)"},
                        "max_results": {"type": "integer", "default": 10, "description": "Results per page (alias for limit)"},
                        "page": {"type": "integer", "default": 1, "description": "Page number (1-based). Use offset for precise control."},
                        "offset": {"type": "integer", "default": 0, "description": "Zero-based result offset (overrides page when provided)"},
                        "year_from": {"type": "integer", "default": 0, "description": "Filter: minimum publication year"},
                        "year_to": {"type": "integer", "default": 0, "description": "Filter: maximum publication year"},
                        "doc_type": {"type": "string", "default": "", "description": "Filter: document type (comma-separated for multiple)"},
                        "fields": {"type": "array", "items": {"type": "string"}, "description": "Optional list of fields to return (e.g. ['id','title','year'])"},
                        "sort_by": {"type": "string", "default": "updated_at", "description": "Sort field: title, authors, year, id, journal, doc_type, created_at, updated_at, pdf_size, md_size, zip_size"},
                        "sort_order": {"type": "string", "default": "desc", "description": "Sort direction: asc or desc"}
                    },
                    "required": ["query"]
                }
            }),
            json!({
                "name": "read_markdown",
                "description": "Read document Markdown content (line-based pagination, won't break document structure), or extract a specific section by heading. The response includes `hash` (file sha256), which is the `base_hash` expected by the write tools.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "id": {"type": "integer"},
                        "line_start": {"type": "integer", "default": 0, "description": "Starting line number (0-indexed)."},
                        "line_count": {"type": "integer", "default": 5000, "description": "Number of lines to return."},
                        "section": {"type": "string", "description": "Extract the section matching this heading (case-insensitive), e.g. 'Introduction'. Takes precedence over pagination."},
                        "sections": {"type": "array", "items": {"type": "string"}, "description": "Extract multiple sections by heading names."}
                    },
                    "required": ["id"]
                }
            }),
            json!({
                "name": "get_document_info",
                "description": "Get detailed document information, including metadata, processing status, PDF and Markdown links. NOTE: 'pdf_need_auth_url' requires an auth token and will fail for unauthenticated clients; use 'temp_downloadable_pdf_url' instead, which is directly fetchable without auth (valid until 'temp_downloadable_pdf_expiration').",
                "inputSchema": {
                    "type": "object",
                    "properties": {"id": {"type": "integer"}},
                    "required": ["id"]
                }
            }),
            json!({
                "name": "list_documents",
                "description": "Search or list documents. Supports search by title/author, filtering by file facts and document type, plus field filtering. NOTE: the old `status` parameter was removed — use md_state / pdf_state / zip_state (present|absent|unknown), synced (0|1) or pdf_health instead.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "query": {"type": "string", "default": ""},
                        "doc_type": {"type": "string", "default": ""},
                        "md_state": {"type": "string", "default": "", "description": "Markdown file facts: present / absent / unknown (comma-separated for multiple)"},
                        "pdf_state": {"type": "string", "default": "", "description": "PDF file facts: present / absent / unknown"},
                        "zip_state": {"type": "string", "default": "", "description": "Image asset zip file facts: present / absent / unknown"},
                        "synced": {"type": "string", "default": "", "description": "1 = file facts already synced from disk, 0 = not synced yet (status unknown)"},
                        "has_md": {"type": "string", "default": "", "description": "1 = has Markdown, 0 = no Markdown (includes unsynced)"},
                        "has_pdf": {"type": "string", "default": "", "description": "1 = has PDF, 0 = no PDF"},
                        "has_zip": {"type": "string", "default": "", "description": "1 = has image asset zip, 0 = none"},
                        "pdf_health": {"type": "string", "default": "", "description": "ok / warning / error / missing / unchecked (comma-separated)"},
                        "page": {"type": "integer", "default": 1},
                        "page_size": {"type": "integer", "default": 20, "description": "Results per page (1-100, alias: limit)"},
                        "limit": {"type": "integer", "default": 20, "description": "Results per page (alias for page_size)"},
                        "fields": {"type": "array", "items": {"type": "string"}, "description": "Optional list of fields to return (e.g. ['id','title','year'])"},
                        "sort_by": {"type": "string", "default": "created_at", "description": "Sort field: title, authors, year, id, journal, doc_type, created_at, updated_at, pdf_size, md_size, zip_size"},
                        "sort_order": {"type": "string", "default": "desc", "description": "Sort direction: asc or desc"}
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
            json!({
                "name": "get_toc",
                "description": "Get document table of contents (headings with line numbers). Extracted from the full Markdown file, independent of pagination.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "id": {"type": "integer"}
                    },
                    "required": ["id"]
                }
            }),
        ];

        // 写类工具：仅 admin 可见（非 admin 的 tools/list 里不会出现）
        if self.can_write() {
            tools.extend(Self::write_tools());
        }
        tools
    }

    /// 写类工具定义（Markdown 编辑），仅 `admin` 可见/可用。
    ///
    /// 三者语义与 Web 端编辑接口一致：直接读写 Markdown 源文件（不做图片路径
    /// 重写/还原），写后立即同步文件事实（`md_size`），因此派生状态仍由文件推导。
    fn write_tools() -> Vec<Value> {
        vec![
            json!({
                "name": "edit_markdown",
                "description": "Edit a document's Markdown source by replacing / inserting / deleting a line range (0-indexed, same semantics as the Web source editor). line_count=0 inserts; delete=true removes the range; the number of new lines is the line count of `content` (an empty string is ONE blank line). Reads/writes the raw file (image paths untouched). Safety: pass `expected_text` (the exact current text of the target range) and/or `base_hash` (sha256 returned by read_markdown / a previous edit); `dry_run=true` previews without writing. Requires admin.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "id": {"type": "integer"},
                        "line_start": {"type": "integer", "description": "Starting line (0-indexed, inclusive). Clamped to total_lines."},
                        "line_count": {"type": "integer", "default": 0, "description": "Number of existing lines to replace (0 = pure insertion)."},
                        "content": {"type": "string", "default": "", "description": "Replacement text; its line count is `content.split('\\n').count()`. Empty string means one blank line."},
                        "delete": {"type": "boolean", "default": false, "description": "true = delete the [line_start, line_start+line_count) range and ignore content."},
                        "expected_text": {"type": "string", "description": "Exact current text of the target range. On mismatch nothing is written and the actual content plus a suggested line_start are returned."},
                        "base_hash": {"type": "string", "description": "Optimistic concurrency: sha256 of the file read earlier. Mismatch returns error code `conflict`."},
                        "dry_run": {"type": "boolean", "default": false, "description": "Preview only, do not write."}
                    },
                    "required": ["id", "line_start"]
                }
            }),
            json!({
                "name": "replace_markdown",
                "description": "Replace the WHOLE Markdown source of a document (must be non-empty). Use only when the change rewrites most of the file; prefer edit_markdown / find_replace_markdown otherwise. Supports `base_hash` and `dry_run`. Requires admin.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "id": {"type": "integer"},
                        "content": {"type": "string", "description": "New full Markdown content (required, non-empty)."},
                        "base_hash": {"type": "string", "description": "Optimistic concurrency: sha256 of the current file. Mismatch returns error code `conflict`."},
                        "dry_run": {"type": "boolean", "default": false, "description": "Preview only, do not write."}
                    },
                    "required": ["id", "content"]
                }
            }),
            json!({
                "name": "find_replace_markdown",
                "description": "Exact (case-sensitive, literal) find & replace on a document's Markdown source. By default the `find` text must occur exactly once; if it occurs more often nothing is written and the hit count plus line previews are returned — pass all=true to replace every occurrence (max 200). Supports `base_hash` and `dry_run`. Requires admin.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "id": {"type": "integer"},
                        "find": {"type": "string", "description": "Literal text to find (not a regex), must be non-empty."},
                        "replace": {"type": "string", "default": "", "description": "Replacement text; empty string deletes the matched text."},
                        "all": {"type": "boolean", "default": false, "description": "Replace every occurrence (max 200). Default requires a unique match."},
                        "base_hash": {"type": "string", "description": "Optimistic concurrency: sha256 of the current file. Mismatch returns error code `conflict`."},
                        "dry_run": {"type": "boolean", "default": false, "description": "Preview only, do not write."}
                    },
                    "required": ["id", "find"]
                }
            }),
        ]
    }

    /// 写类工具的权限拒绝错误（供 `call_tool` 使用）。
    fn permission_denied(&self, tool: &str) -> Value {
        json!({
            "isError": true,
            "content": [{"type": "text", "text": Self::err(
                "forbidden",
                &format!(
                    "工具 {} 需要 admin 权限（当前角色 {}）。Markdown 编辑属于写操作，仅管理员可用。",
                    tool,
                    self.role.as_str()
                )
            )}]
        })
    }

    /// 前置校验：验证必需的字符串参数不为空，返回 `Ok(value)` 或 `Err(error_response)`。
    fn validate_required_string(args: &Value, key: &str) -> Result<String, Value> {
        args.get(key)
            .and_then(|v| v.as_str())
            .filter(|s| !s.trim().is_empty())
            .map(|s| s.to_string())
            .ok_or_else(|| {
                json!({
                    "isError": true,
                    "content": [{"type": "text", "text": Self::err("invalid_argument", &format!("缺少必需参数: {}", key))}]
                })
            })
    }

    /// 前置校验：验证必需的整数参数 > 0，返回 `Ok(value)` 或 `Err(error_response)`。
    fn validate_required_i64(args: &Value, key: &str) -> Result<i64, Value> {
        args.get(key)
            .and_then(|v| v.as_i64())
            .filter(|v| *v > 0)
            .ok_or_else(|| {
                json!({
                    "isError": true,
                    "content": [{"type": "text", "text": Self::err("invalid_argument", &format!("缺少必需的参数或值无效: {}", key))}]
                })
            })
    }

    /// 严格参数校验：拒绝 `inputSchema` 未声明的参数。
    ///
    /// 用于避免旧客户端（如传旧 `status`）**静默丢失筛选**：直接 `isError` 并给出
    /// 可用参数清单 + 替代建议（错误文本会回给工具调用方，包括问答里的模型）。
    ///
    /// 宽容处：`args` 不是对象、或某键值为 `null` 时跳过（部分客户端会把所有字段都填 `null`）。
    fn reject_unknown_args(tools: &[Value], name: &str, args: &Value) -> Result<(), Value> {
        let Some(obj) = args.as_object() else { return Ok(()) };
        let Some(tool) = tools.iter().find(|t| t["name"].as_str() == Some(name)) else {
            return Ok(());
        };
        let Some(props) = tool["inputSchema"]["properties"].as_object() else {
            return Ok(());
        };

        let unknown: Vec<String> = obj
            .iter()
            .filter(|(k, v)| !v.is_null() && !props.contains_key(k.as_str()))
            .map(|(k, _)| k.clone())
            .collect();
        if unknown.is_empty() {
            return Ok(());
        }

        let mut allowed: Vec<&str> = props.keys().map(|s| s.as_str()).collect();
        allowed.sort_unstable();
        let hints: Vec<String> = unknown.iter().map(|k| Self::removed_arg_hint(k)).collect();
        let msg = format!(
            "未知参数 {}。可用参数：{}。{}",
            unknown
                .iter()
                .map(|k| format!("`{}`", k))
                .collect::<Vec<_>>()
                .join(", "),
            allowed.join(", "),
            hints.join(" ")
        );
        Err(json!({
            "isError": true,
            "content": [{"type": "text", "text": Self::err("invalid_argument", &msg)}]
        }))
    }

    /// 已删除参数的替代提示（只用于报错文案，**不是**兼容层，不会让旧参数生效）
    fn removed_arg_hint(name: &str) -> String {
        match name {
            "status" => "`status` 已移除：请改用 md_state / pdf_state / zip_state（present|absent|unknown）、synced（0|1）或 pdf_health（ok|warning|error|missing|unchecked）。".to_string(),
            "reset_status" => "`reset_status` 已移除：重新解析请调 POST /assist/api/pipeline/{id}/reset（清空解析产物）。".to_string(),
            _ => String::new(),
        }
    }

    pub async fn call_tool(&self, name: &str, args: &Value) -> Value {
        // 严格参数校验：schema 未声明的参数直接报错（不发默忽略）
        let tools = self.list_tools().await;
        if let Err(e) = Self::reject_unknown_args(&tools, name, args) {
            return e;
        }

        // 前置校验：必填参数在进入工具前检查
        macro_rules! check_required_str {
            ($key:expr) => {
                match Self::validate_required_string(args, $key) {
                    Err(e) => return e,
                    Ok(v) => v,
                }
            };
        }
        macro_rules! check_required_id {
            () => {
                match Self::validate_required_i64(args, "id") {
                    Err(e) => return e,
                    Ok(v) => v,
                }
            };
        }

        let text = match name {
            "grep_search" => {
                let _query = check_required_str!("query");
                self.tool_grep_search(args).await
            }
            "search_info" => {
                let _query = check_required_str!("query");
                self.tool_search_info(args).await
            }
            "read_markdown" => {
                let _id = check_required_id!();
                self.tool_read_markdown(args).await
            }
            "get_document_info" => {
                let _id = check_required_id!();
                self.tool_get_document_info(args).await
            }
            "list_documents" => self.tool_list_documents(args).await,
            "get_document_abstract" => {
                let _id = check_required_id!();
                self.tool_get_document_abstract(args).await
            }
            "get_stats" => self.tool_get_stats().await,
            "list_doc_types" => self.tool_list_doc_types().await,
            "get_toc" => {
                let _id = check_required_id!();
                self.tool_get_toc(args).await
            }
            // ── 写类工具（仅 admin）──
            "edit_markdown" => {
                if !self.can_write() {
                    return self.permission_denied("edit_markdown");
                }
                let _id = check_required_id!();
                self.tool_edit_markdown(args).await
            }
            "replace_markdown" => {
                if !self.can_write() {
                    return self.permission_denied("replace_markdown");
                }
                let _id = check_required_id!();
                self.tool_replace_markdown(args).await
            }
            "find_replace_markdown" => {
                if !self.can_write() {
                    return self.permission_denied("find_replace_markdown");
                }
                let _id = check_required_id!();
                let _find = check_required_str!("find");
                self.tool_find_replace_markdown(args).await
            }
            _ => {
                warn!(tool = %name, "Unknown tool called");
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
        // 状态由文件事实（同步列）+ 运行时推导，不再读状态列/不 stat
        let state = crate::services::file_state::derive_state(
            doc.fs_synced_at.as_deref(),
            doc.pdf_size,
            doc.md_size,
            crate::routers::pipeline::runtime_task(doc.id).as_ref(),
            crate::services::file_state::last_error(doc.id).as_deref(),
            doc.mineru_task_id.as_deref(),
        );
        let base = self.settings.base_url();
        let prefix = if base.is_empty() { String::new() } else { base };
        let (alias, alias_expires) = get_or_create_alias(
            doc.id,
            doc.year,
            doc.title.as_deref(),
            self.settings.alias_expiration_hours(),
        );
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
            "i18n": crate::services::i18n::value_from_db(doc.i18n.as_deref()),
            "status": state.status.as_str(),
            "status_message": state.message,
            "synced": state.synced,
            "has_markdown": doc.has_md(),
            "has_pdf": doc.has_pdf(),
            "has_asset": doc.has_zip(),
            "pdf_health": doc.pdf_health,
            "pdf_size": doc.pdf_size,
            "md_size": doc.md_size,
            "pdf_need_auth_url": format!("{}/assist/api/documents/{}/pdf", prefix, doc.id),
            "temp_downloadable_pdf_url": format!("{}/assist/file/{}", prefix, alias),
            "temp_downloadable_pdf_expiration": alias_expires,
            "markdown_url": format!("{}/assist/markdown/{}", prefix, doc.id),
            "detail_url": format!("{}/assist/detail/{}", prefix, doc.id),
        })
    }

    fn pretty(v: &Value) -> String {
        serde_json::to_string_pretty(v).unwrap_or_else(|_| v.to_string())
    }

    /// 统一的错误返回格式：`{"error": {"code": ..., "message": ...}}`。
    fn err(code: &str, message: &str) -> String {
        Self::pretty(&json!({"error": {"code": code, "message": message}}))
    }

    /// 解析分页参数，返回 `(offset, limit)`。
    /// `offset` 优先于 `page`；`limit` 兼容 `max_results` 别名。
    /// `offset` 上限为 `MAX_OFFSET`，超出时返回 `(MAX_OFFSET, limit)` 并记录警告。
    fn parse_pagination(args: &Value, default_limit: i64, max_limit: i64) -> (i64, i64) {
        let limit = args["limit"]
            .as_i64()
            .or_else(|| args["max_results"].as_i64())
            .unwrap_or(default_limit)
            .clamp(1, max_limit);
        let offset = args["offset"].as_i64().unwrap_or_else(|| {
            (args["page"].as_i64().unwrap_or(1).max(1) - 1).saturating_mul(limit)
        });
        let offset = offset.max(0);
        if offset > MAX_OFFSET {
            warn!(offset, max = %MAX_OFFSET, "Pagination offset exceeds maximum, clamping");
            (MAX_OFFSET, limit)
        } else {
            (offset, limit)
        }
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

    /// 排序字段白名单
    const SORTABLE_COLUMNS: &[(&str, &str)] = &[
        ("title", "title"),
        ("authors", "authors"),
        ("year", "year"),
        ("id", "id"),
        ("journal", "journal"),
        ("doc_type", "doc_type"),
        ("created_at", "created_at"),
        ("updated_at", "updated_at"),
        ("pdf_size", "pdf_size"),
        ("md_size", "md_size"),
        ("zip_size", "zip_size"),
    ];

    /// 拼接 ORDER BY 子句（防 SQL 注入：仅允许白名单字段）。
    fn append_order_clause<'a>(
        qb: &mut sqlx::QueryBuilder<'a, sqlx::Sqlite>,
        sort_by: &str,
        sort_order: &str,
    ) {
        let col = Self::SORTABLE_COLUMNS
            .iter()
            .find(|(k, _)| *k == sort_by)
            .map(|(_, v)| *v)
            .unwrap_or("updated_at");
        let dir = match sort_order {
            "asc" => "ASC",
            _ => "DESC",
        };
        qb.push(" ORDER BY ").push(col).push(" ").push(dir);
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
        let start_after_id = args["start_after_id"].as_i64().filter(|v| *v > 0);

        // query 为空由前置校验拦截，这里保留冗余检查以防绕开
        if query.trim().is_empty() {
            return Self::err("invalid_argument", "查询不能为空");
        }

        let limit = args["limit"]
            .as_i64()
            .or_else(|| args["max_results"].as_i64())
            .unwrap_or(10)
            .clamp(1, 100) as usize;

        let id_list = match parse_doc_ids(&doc_ids) {
            Ok(v) => v,
            Err(e) => return Self::err("invalid_argument", &e),
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
            start_after_id,
        )
        .await;

        // 批量查询文档元数据（避免 N+1）
        let all_results: Vec<Value> = results;
        let ids: Vec<i64> = all_results.iter().filter_map(|hit| hit["id"].as_i64()).collect();
        let mut doc_map: std::collections::HashMap<i64, Document> = std::collections::HashMap::new();
        if !ids.is_empty() {
            let ids_str: Vec<String> = ids.iter().map(|id| id.to_string()).collect();
            let sql = format!("SELECT {} FROM documents WHERE id IN ({})", DOC_COLUMNS, ids_str.join(","));
            if let Ok(rows) = sqlx::query_as::<_, Document>(&sql).fetch_all(self.db.pool()).await {
                for doc in rows {
                    doc_map.insert(doc.id, doc);
                }
            }
        }
        let mut enriched = Vec::new();
        for hit in all_results {
            let doc_id = hit["id"].as_i64();
            let raw_content = hit["content"].as_str().unwrap_or("").to_string();
            let content = Self::truncate_chars(&raw_content, max_context_chars);
            let truncated = max_context_chars > 0 && raw_content.chars().count() > max_context_chars;
            let mut info = json!({
                "id": doc_id,
                "content": content,
                "truncated": truncated,
                "original_length": raw_content.chars().count(),
            });
            if let Some(id) = doc_id {
                if let Some(doc) = doc_map.get(&id) {
                    info["title"] = json!(doc.title.clone().unwrap_or_default());
                    info["authors"] = json!(doc.authors.clone().unwrap_or_default());
                    info["year"] = json!(doc.year);
                    info["journal"] = json!(doc.journal.clone().unwrap_or_default());
                }
            }
            enriched.push(info);
        }

        // 计算 last_id（结果中最大的 doc_id）
        let last_id = enriched.iter()
            .filter_map(|r| r["id"].as_i64())
            .max();

        Self::pretty(&json!({
            "query": query,
            "limit": limit,
            "returned": enriched.len(),
            "last_id": last_id,
            "results": enriched,
        }))
    }

    async fn tool_search_info(&self, args: &Value) -> String {
        let query = args["query"].as_str().unwrap_or("").to_string();
        let year_from = args["year_from"].as_i64().unwrap_or(0);
        let year_to = args["year_to"].as_i64().unwrap_or(0);
        let doc_type = args["doc_type"].as_str().unwrap_or("").to_string();
        let fields = Self::parse_fields(args);
        let sort_by = args["sort_by"].as_str().unwrap_or("updated_at").to_string();
        let sort_order = args["sort_order"].as_str().unwrap_or("desc").to_string();

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
        Self::append_order_clause(&mut qb, &sort_by, &sort_order);
        qb.push(" LIMIT ").push_bind(limit)
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
                    "authors": doc.authors.clone().unwrap_or_default(),
                    "year": doc.year,
                    "doi": doc.doi.clone().unwrap_or_default(),
                    "journal": doc.journal.clone().unwrap_or_default(),
                    "keywords": doc.keywords.clone().unwrap_or_default(),
                    "i18n": crate::services::i18n::value_from_db(doc.i18n.as_deref()),
                    "abstract": doc.abstract_text.as_deref().unwrap_or("").chars().take(300).collect::<String>(),
                    "doc_type": doc.doc_type.clone().unwrap_or_default(),
                    "language": doc.language.clone().unwrap_or_default(),
                    "category": doc.category.clone().unwrap_or_default(),
                    "status": crate::services::file_state::derive_state(
                        doc.fs_synced_at.as_deref(),
                        doc.pdf_size,
                        doc.md_size,
                        crate::routers::pipeline::runtime_task(doc.id).as_ref(),
                        crate::services::file_state::last_error(doc.id).as_deref(),
                        doc.mineru_task_id.as_deref(),
                    ).status.as_str(),
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

        // 解析行范围参数
        let line_start = args["line_start"].as_i64().map(|v| v.max(0)).unwrap_or(0) as usize;
        let line_count = args["line_count"].as_i64().map(|v| v.max(1)).unwrap_or(5000) as usize;

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
        // 文件 sha256：可直接作为写工具（edit_markdown / replace_markdown / find_replace_markdown）
        // 的 `base_hash`，用于乐观并发校验。
        let hash = crate::utils::sha256_hex_async(content.clone().into_bytes()).await;

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
                "hash": hash,
                "content": extracted,
            }));
        }

        // 分页模式（行切片）
        let total_lines = content.lines().count();
        let lines: Vec<&str> = content.lines().collect();
        let actual_start = line_start.min(lines.len());
        let actual_end = (actual_start + line_count).min(lines.len());
        let lines_returned = actual_end - actual_start;
        let page_content = if lines_returned > 0 {
            lines[actual_start..actual_end].join("\n")
        } else {
            String::new()
        };

        Self::pretty(&json!({
            "id": doc_id,
            "title": title,
            "line_start": actual_start,
            "line_count": line_count,
            "lines_returned": lines_returned,
            "total_lines": total_lines,
            "hash": hash,
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
        let doc_type = args["doc_type"].as_str().unwrap_or("").to_string();
        let page = args["page"].as_i64().unwrap_or(1).max(1) as i64;
        // page_size 为主，limit 为兼容别名，范围 1-100
        let page_size = args["page_size"]
            .as_i64()
            .or_else(|| args["limit"].as_i64())
            .unwrap_or(20)
            .clamp(1, 100) as i64;
        let fields = Self::parse_fields(args);
        let sort_by = args["sort_by"].as_str().unwrap_or("created_at").to_string();
        let sort_order = args["sort_order"].as_str().unwrap_or("desc").to_string();

        // 计数
        let mut cqb = sqlx::QueryBuilder::new("SELECT COUNT(*) FROM documents WHERE 1=1");
        if let Err(e) = append_list_filters(&mut cqb, &query, &doc_type, args) {
            return Self::err("invalid_argument", &e);
        }
        let total: i64 = cqb
            .build()
            .fetch_one(self.db.pool())
            .await
            .map(|r| r.get::<i64, _>(0))
            .unwrap_or(0);

        // 分页查询
        let mut qb = sqlx::QueryBuilder::new(format!("SELECT {} FROM documents WHERE 1=1", DOC_COLUMNS));
        if let Err(e) = append_list_filters(&mut qb, &query, &doc_type, args) {
            return Self::err("invalid_argument", &e);
        }
        Self::append_order_clause(&mut qb, &sort_by, &sort_order);
        qb.push(" LIMIT ").push_bind(page_size)
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
            "i18n": crate::services::i18n::value_from_db(doc.i18n.as_deref()),
        });
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
        let (uploaded, markdown_done, unsynced): (i64, i64, i64) = sqlx::query_as(
            "SELECT \
               SUM(CASE WHEN COALESCE(md_size,0) = 0 THEN 1 ELSE 0 END), \
               SUM(CASE WHEN COALESCE(md_size,0) > 0 THEN 1 ELSE 0 END), \
               SUM(CASE WHEN fs_synced_at IS NULL THEN 1 ELSE 0 END) \
             FROM documents",
        )
        .fetch_one(self.db.pool())
        .await
        .unwrap_or((0, 0, 0));
        status_counts.insert(DocStatus::Uploaded.as_str().to_string(), json!(uploaded));
        status_counts.insert(DocStatus::MarkdownDone.as_str().to_string(), json!(markdown_done));
        let runtime_errors = crate::services::file_state::error_snapshot().len() as i64;
        if runtime_errors > 0 {
            status_counts.insert(DocStatus::Error.as_str().to_string(), json!(runtime_errors));
        }
        let runtime_parsing = crate::routers::pipeline::runtime_task_count();
        if runtime_parsing > 0 {
            status_counts.insert(DocStatus::Parsing.as_str().to_string(), json!(runtime_parsing));
        }
        status_counts.insert("unsynced".to_string(), json!(unsynced));

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

        // 索引状态记在 document_vector_index 上（documents 不再有解析状态列）
        let indexed_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(DISTINCT document_id) FROM document_vector_index WHERE status = 'indexed'",
        )
        .fetch_one(self.db.pool())
        .await
        .unwrap_or(0);

        Self::pretty(&json!({
            "total_documents": total,
            "status_counts": status_counts,
            "type_counts": type_counts_sorted,
            "indexed_count": indexed_count,
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
        // `standard` = 唯一认可的候选词表（模型筛选时应该用它，而不是库里遗留的碎片值）
        let standard: Vec<Value> = crate::services::doc_type::STANDARD
            .iter()
            .map(|(value, label)| json!({"value": value, "label": label}))
            .collect();
        Self::pretty(&json!({"doc_types": types, "standard": standard}))
    }

    async fn tool_get_toc(&self, args: &Value) -> String {
        let doc_id = args["id"].as_i64().unwrap_or(0);

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
        let mut toc: Vec<Value> = Vec::new();
        let re = regex::Regex::new(r"^(#{1,6})\s+(.+)$").unwrap();

        for (line_idx, line) in content.lines().enumerate() {
            if let Some(caps) = re.captures(line) {
                let level = caps.get(1).unwrap().as_str().len();
                let heading = caps.get(2).unwrap().as_str().trim().to_string();
                if !heading.is_empty() {
                    toc.push(json!({
                        "level": level,
                        "title": heading,
                        "line": line_idx,
                    }));
                }
            }
        }

        Self::pretty(&json!({
            "id": doc_id,
            "title": title,
            "total_headings": toc.len(),
            "toc": toc,
        }))
    }

    // ── 写类工具实现：Markdown 编辑（仅 admin）──

    /// 读取待编辑的 Markdown：返回 `(文档, 文件路径, 原文)`。
    /// 错误时直接返回已格式化好的错误文本（供工具原样回传）。
    async fn load_markdown(&self, doc_id: i64) -> Result<(Document, String, String), String> {
        let doc = match self.fetch_doc(doc_id).await {
            Some(d) => d,
            None => return Err(Self::err("not_found", &format!("文档 {} 不存在", doc_id))),
        };
        let md_path = paths::get_markdown_path(&self.settings, doc_id);
        let raw = match std::fs::read_to_string(&md_path) {
            Ok(c) => c,
            Err(_) => {
                return Err(Self::err(
                    "not_found",
                    "Markdown 文件尚未生成，请先解析 PDF（或调用 upload_parse_result 接口）",
                ))
            }
        };
        Ok((doc, md_path, raw))
    }

    /// 乐观并发校验：`base_hash` 非空且与当前文件 hash 不一致时返回错误文本。
    fn check_base_hash_arg(args: &Value, current_hash: &str) -> Result<(), String> {
        match args["base_hash"].as_str() {
            Some(expected) if !expected.is_empty() && expected != current_hash => Err(Self::pretty(&json!({
                "error": {
                    "code": "conflict",
                    "message": "文件已被其它操作修改（base_hash 不匹配），未写入。请重新 read_markdown 后再编辑",
                },
                "hash": current_hash,
            }))),
            _ => Ok(()),
        }
    }

    /// 写入 Markdown 并同步文件事实（`md_size` 等），返回 `(新 hash, md_size)`。
    async fn persist_markdown(
        &self,
        md_path: &str,
        content: &str,
        doc_id: i64,
    ) -> Result<(String, Option<i64>), String> {
        if let Err(e) = write_atomic(md_path, content) {
            return Err(Self::err("write_failed", &format!("写入 Markdown 失败: {}", e)));
        }
        // 同步文件事实：列表页 md_size 与派生状态都不能过期
        sync_doc_file_sizes(&self.db, &self.settings, doc_id).await;
        let hash = crate::utils::sha256_hex_async(content.as_bytes().to_vec()).await;
        let md_size = std::fs::metadata(md_path).ok().map(|m| m.len() as i64);
        Ok((hash, md_size))
    }

    /// `edit_markdown` —— 按行范围替换 / 插入 / 删行。
    async fn tool_edit_markdown(&self, args: &Value) -> String {
        let doc_id = args["id"].as_i64().unwrap_or(0);
        let line_start = match args["line_start"].as_i64() {
            Some(v) if v >= 0 => v as usize,
            _ => return Self::err("invalid_argument", "line_start 必须是非负整数（0-based）"),
        };
        let line_count = match args["line_count"].as_i64().unwrap_or(0) {
            v if v >= 0 => v as usize,
            _ => return Self::err("invalid_argument", "line_count 不能为负数"),
        };
        let delete = args["delete"].as_bool().unwrap_or(false);
        let dry_run = args["dry_run"].as_bool().unwrap_or(false);
        let content = args["content"].as_str().unwrap_or("");

        let (doc, md_path, raw) = match self.load_markdown(doc_id).await {
            Ok(v) => v,
            Err(e) => return e,
        };
        let current_hash = crate::utils::sha256_hex_async(raw.clone().into_bytes()).await;
        if let Err(e) = Self::check_base_hash_arg(args, &current_hash) {
            return e;
        }

        let ends_with_newline = raw.ends_with('\n');
        let lines: Vec<&str> = raw.lines().collect();
        let total_lines = lines.len();
        let start = line_start.min(total_lines);
        let end = (start + line_count).min(total_lines);
        let old_content = lines[start..end].join("\n");

        // 安全阀：expected_text 必须与目标行范围的实际内容完全一致，否则不写
        if let Some(expected) = args["expected_text"].as_str().filter(|s| !s.is_empty()) {
            if expected != old_content {
                return Self::pretty(&json!({
                    "error": {
                        "code": "mismatch",
                        "message": "expected_text 与目标行范围的实际内容不一致，未写入。请重新 read_markdown 确认行号后再编辑",
                    },
                    "id": doc_id,
                    "line_start": start,
                    "line_count": line_count,
                    "actual_content": clip(&old_content, 2000),
                    "suggested_line_start": locate_text(&lines, expected),
                }));
            }
        }

        let new_lines: Vec<&str> = if delete {
            Vec::new()
        } else {
            content.split('\n').collect()
        };
        let mut merged: Vec<&str> = Vec::with_capacity(total_lines + new_lines.len());
        merged.extend_from_slice(&lines[..start]);
        merged.extend_from_slice(&new_lines);
        merged.extend_from_slice(&lines[end..]);
        let mut result = merged.join("\n");
        if ends_with_newline && !result.is_empty() {
            result.push('\n');
        }

        let action = if delete {
            "delete"
        } else if line_count == 0 {
            "insert"
        } else {
            "replace"
        };
        let title = doc.title.clone().unwrap_or_else(|| doc.filename.clone());
        let changed = result != raw;

        if dry_run {
            return Self::pretty(&json!({
                "dry_run": true,
                "id": doc_id,
                "title": title,
                "action": action,
                "changed": changed,
                "line_start": start,
                "line_count": line_count,
                "lines_written": new_lines.len(),
                "total_lines": merged.len(),
                "hash": current_hash,
                "old_content": clip(&old_content, 2000),
                "new_content": clip(if delete { "" } else { content }, 2000),
            }));
        }

        if !changed {
            return Self::pretty(&json!({
                "id": doc_id,
                "title": title,
                "detail": "内容与原文件一致，未写入",
                "action": action,
                "changed": false,
                "line_start": start,
                "line_count": line_count,
                "lines_written": new_lines.len(),
                "total_lines": total_lines,
                "hash": current_hash,
            }));
        }

        let (hash, md_size) = match self.persist_markdown(&md_path, &result, doc_id).await {
            Ok(v) => v,
            Err(e) => return e,
        };
        info!(
            doc_id,
            action,
            line_start = start,
            line_count,
            lines_written = new_lines.len(),
            "MCP edit_markdown 已写入"
        );

        Self::pretty(&json!({
            "id": doc_id,
            "title": title,
            "detail": "Markdown 已更新",
            "action": action,
            "changed": true,
            "line_start": start,
            "line_count": line_count,
            "lines_written": new_lines.len(),
            "total_lines": merged.len(),
            "md_size": md_size,
            "hash": hash,
            "previous_hash": current_hash,
        }))
    }

    /// `replace_markdown` —— 整篇覆盖。
    async fn tool_replace_markdown(&self, args: &Value) -> String {
        let doc_id = args["id"].as_i64().unwrap_or(0);
        let content = match args["content"].as_str() {
            Some(c) if !c.trim().is_empty() => c,
            _ => {
                return Self::err(
                    "invalid_argument",
                    "content 不能为空：整篇覆盖必须显式提供非空内容（清空请用 edit_markdown + delete）",
                )
            }
        };
        let dry_run = args["dry_run"].as_bool().unwrap_or(false);

        let (doc, md_path, raw) = match self.load_markdown(doc_id).await {
            Ok(v) => v,
            Err(e) => return e,
        };
        let current_hash = crate::utils::sha256_hex_async(raw.clone().into_bytes()).await;
        if let Err(e) = Self::check_base_hash_arg(args, &current_hash) {
            return e;
        }

        let title = doc.title.clone().unwrap_or_else(|| doc.filename.clone());
        let changed = raw != content;
        let total_lines = content.lines().count();

        if dry_run {
            return Self::pretty(&json!({
                "dry_run": true,
                "id": doc_id,
                "title": title,
                "action": "replace_all",
                "changed": changed,
                "old_total_lines": raw.lines().count(),
                "total_lines": total_lines,
                "hash": current_hash,
                "preview": clip(content, 2000),
            }));
        }

        if !changed {
            return Self::pretty(&json!({
                "id": doc_id,
                "title": title,
                "detail": "内容与原文件一致，未写入",
                "action": "replace_all",
                "changed": false,
                "total_lines": total_lines,
                "hash": current_hash,
            }));
        }

        let (hash, md_size) = match self.persist_markdown(&md_path, content, doc_id).await {
            Ok(v) => v,
            Err(e) => return e,
        };
        info!(doc_id, total_lines, "MCP replace_markdown 已整篇覆盖");

        Self::pretty(&json!({
            "id": doc_id,
            "title": title,
            "detail": "Markdown 已整篇覆盖",
            "action": "replace_all",
            "changed": true,
            "total_lines": total_lines,
            "md_size": md_size,
            "hash": hash,
            "previous_hash": current_hash,
        }))
    }

    /// `find_replace_markdown` —— 精确字面量查找替换（默认要求唯一命中）。
    async fn tool_find_replace_markdown(&self, args: &Value) -> String {
        let doc_id = args["id"].as_i64().unwrap_or(0);
        let find = match args["find"].as_str() {
            Some(s) if !s.is_empty() => s,
            _ => return Self::err("invalid_argument", "find 不能为空"),
        };
        let replace = args["replace"].as_str().unwrap_or("");
        let all = args["all"].as_bool().unwrap_or(false);
        let dry_run = args["dry_run"].as_bool().unwrap_or(false);

        let (doc, md_path, raw) = match self.load_markdown(doc_id).await {
            Ok(v) => v,
            Err(e) => return e,
        };
        let current_hash = crate::utils::sha256_hex_async(raw.clone().into_bytes()).await;
        if let Err(e) = Self::check_base_hash_arg(args, &current_hash) {
            return e;
        }

        let occurrences = raw.matches(find).count();
        let matches = match_previews(&raw, find, 5);
        if occurrences == 0 {
            return Self::pretty(&json!({
                "error": {
                    "code": "not_found",
                    "message": "未找到目标文本（find 为区分大小写的字面量匹配），未写入",
                },
                "id": doc_id,
                "find": clip(find, 200),
                "total_lines": raw.lines().count(),
                "hash": current_hash,
            }));
        }
        if occurrences > 1 && !all {
            return Self::pretty(&json!({
                "error": {
                    "code": "ambiguous",
                    "message": format!(
                        "find 命中 {} 处，未写入。请给出更长的唯一文本，或传 all=true 替换全部（上限 {} 处）",
                        occurrences, MAX_FIND_REPLACE
                    ),
                },
                "id": doc_id,
                "occurrences": occurrences,
                "matches": matches,
                "hash": current_hash,
            }));
        }
        if all && occurrences > MAX_FIND_REPLACE {
            return Self::pretty(&json!({
                "error": {
                    "code": "too_many",
                    "message": format!("find 命中 {} 处，超过单次上限 {}；请缩小 find 的文本范围", occurrences, MAX_FIND_REPLACE),
                },
                "id": doc_id,
                "occurrences": occurrences,
            }));
        }

        let result = if all {
            raw.replace(find, replace)
        } else {
            raw.replacen(find, replace, 1)
        };
        let replaced = if all { occurrences } else { 1 };
        let title = doc.title.clone().unwrap_or_else(|| doc.filename.clone());

        if dry_run {
            return Self::pretty(&json!({
                "dry_run": true,
                "id": doc_id,
                "title": title,
                "action": "find_replace",
                "occurrences": occurrences,
                "replaced": replaced,
                "changed": result != raw,
                "matches": matches,
                "hash": current_hash,
                "new_content": clip(&result, 2000),
            }));
        }

        if result == raw {
            return Self::pretty(&json!({
                "id": doc_id,
                "title": title,
                "detail": "替换内容与原文一致，未写入",
                "action": "find_replace",
                "occurrences": occurrences,
                "replaced": 0,
                "changed": false,
                "hash": current_hash,
            }));
        }

        let (hash, md_size) = match self.persist_markdown(&md_path, &result, doc_id).await {
            Ok(v) => v,
            Err(e) => return e,
        };
        info!(
            doc_id,
            occurrences,
            replaced,
            "MCP find_replace_markdown 已写入"
        );

        Self::pretty(&json!({
            "id": doc_id,
            "title": title,
            "detail": "Markdown 已更新",
            "action": "find_replace",
            "changed": true,
            "occurrences": occurrences,
            "replaced": replaced,
            "matches": matches,
            "total_lines": result.lines().count(),
            "md_size": md_size,
            "hash": hash,
            "previous_hash": current_hash,
        }))
    }
}

// ── 辅助函数 ──

/// 截断到 `max` 个字符（按字符而非字节，避免 UTF-8 边界 panic）。
fn clip(s: &str, max: usize) -> String {
    let total = s.chars().count();
    if total <= max {
        return s.to_string();
    }
    format!("{}…（已截断，共 {} 字符）", s.chars().take(max).collect::<String>(), total)
}

/// 在 `lines` 里查找 `text` 首次出现的起始行（0-based）。
///
/// 用于 `edit_markdown` 的 `expected_text` 不匹配时给出建议行号。
fn locate_text(lines: &[&str], text: &str) -> Option<usize> {
    let target: Vec<&str> = text.lines().collect();
    if target.is_empty() || target.len() > lines.len() {
        return None;
    }
    (0..=lines.len() - target.len()).find(|&i| lines[i..i + target.len()] == target[..])
}

/// 列出 `find` 在全文中的前 `max` 个命中（`line` 为 0-based 行号，`text` 为整行文本）。
fn match_previews(raw: &str, find: &str, max: usize) -> Vec<Value> {
    let mut out = Vec::new();
    if find.is_empty() {
        return out;
    }
    let mut from = 0usize;
    while out.len() < max {
        let Some(pos) = raw[from..].find(find) else { break };
        let abs = from + pos;
        let line = raw[..abs].matches('\n').count();
        let line_start = raw[..abs].rfind('\n').map(|i| i + 1).unwrap_or(0);
        let line_end = raw[abs..].find('\n').map(|i| abs + i).unwrap_or(raw.len());
        out.push(json!({
            "line": line,
            "text": clip(&raw[line_start..line_end], 300),
        }));
        from = abs + find.len();
    }
    out
}

/// 向 QueryBuilder 追加 list_documents 的筛选条件。
///
/// 状态参数（`status`）已删除 —— 改用文件事实参数（见 `services::file_state`）：
/// `md_state` / `pdf_state` / `zip_state`（present/absent/unknown）、`synced`、
/// `has_md` / `has_pdf` / `has_zip`、`pdf_health`。非法取值返回 `Err`（→ `isError`）。
fn append_list_filters<'a>(
    qb: &mut sqlx::QueryBuilder<'a, sqlx::Sqlite>,
    query: &str,
    doc_type: &str,
    args: &Value,
) -> Result<(), String> {
    use crate::services::file_state::{self, FileKind};

    if !query.trim().is_empty() {
        let like = format!("%{}%", query.trim());
        qb.push(" AND (title LIKE ").push_bind(like.clone())
            .push(" OR authors LIKE ").push_bind(like.clone())
            .push(" OR filename LIKE ").push_bind(like).push(")");
    }
    // 类型先归一到标准词表：模型（或用户）传 `article` / `journal` 也能命中
    let types = crate::services::doc_type::normalize_many(doc_type);
    if !types.is_empty() {
        qb.push(" AND doc_type IN (");
        for (i, t) in types.iter().enumerate() {
            if i > 0 {
                qb.push(", ");
            }
            qb.push_bind(t.clone());
        }
        qb.push(")");
    }

    // 三态筛选（逗号多选 = OR）
    for (kind, key) in [
        (FileKind::Pdf, "pdf_state"),
        (FileKind::Md, "md_state"),
        (FileKind::Zip, "zip_state"),
    ] {
        let Some(raw) = args[key].as_str().filter(|s| !s.trim().is_empty()) else { continue };
        let mut conds: Vec<String> = Vec::new();
        for st in raw.split(',').map(|s| s.trim()).filter(|s| !s.is_empty()) {
            let cond = file_state::state_condition(kind, st)
                .ok_or_else(|| format!("{} 取值非法：{}（仅 present / absent / unknown）", key, st))?;
            conds.push(cond);
        }
        if conds.len() == 1 {
            qb.push(" AND ").push(conds.remove(0));
        } else if !conds.is_empty() {
            qb.push(" AND (").push(conds.join(" OR ")).push(")");
        }
    }

    // 布尔：是否已同步 / 是否有某文件
    for (key, expr_yes, expr_no) in [
        ("synced", "fs_synced_at IS NOT NULL", "fs_synced_at IS NULL"),
        ("has_pdf", "COALESCE(pdf_size,0) > 0", "NOT (COALESCE(pdf_size,0) > 0)"),
        ("has_md", "COALESCE(md_size,0) > 0", "NOT (COALESCE(md_size,0) > 0)"),
        ("has_zip", "COALESCE(zip_size,0) > 0", "NOT (COALESCE(zip_size,0) > 0)"),
    ] {
        let Some(raw) = args[key].as_str().filter(|s| !s.trim().is_empty()) else { continue };
        let expr = match raw.trim().to_ascii_lowercase().as_str() {
            "1" | "true" | "yes" | "y" | "on" => expr_yes,
            "0" | "false" | "no" | "n" | "off" => expr_no,
            _ => return Err(format!("{} 只接受 1/0（或 true/false）", key)),
        };
        qb.push(" AND ").push(expr);
    }

    // PDF 健康（逗号多选；unchecked = IS NULL）
    if let Some(raw) = args["pdf_health"].as_str().filter(|s| !s.trim().is_empty()) {
        let mut values: Vec<String> = Vec::new();
        let mut unchecked = false;
        for item in raw.split(',').map(|s| s.trim()).filter(|s| !s.is_empty()) {
            match item.to_ascii_lowercase().as_str() {
                "ok" | "warning" | "error" | "missing" => values.push(item.to_ascii_lowercase()),
                "unchecked" | "none" | "null" => unchecked = true,
                _ => return Err(format!("pdf_health 取值非法：{}（ok/warning/error/missing/unchecked）", item)),
            }
        }
        if !values.is_empty() {
            qb.push(" AND pdf_health IN (");
            for (i, v) in values.iter().enumerate() {
                if i > 0 {
                    qb.push(", ");
                }
                qb.push_bind(v.clone());
            }
            if unchecked {
                qb.push(") OR pdf_health IS NULL");
            } else {
                qb.push(")");
            }
        } else if unchecked {
            qb.push(" AND pdf_health IS NULL");
        }
    }

    Ok(())
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
        "category",
        "source",
        "filename",
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
    // 类型先归一到标准词表：模型（或用户）传 `article` / `journal` 也能命中
    let types = crate::services::doc_type::normalize_many(doc_type);
    if !types.is_empty() {
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

// ── Markdown 章节解析 ──

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

// ── 全局日志级别（由 MCP logging/setLevel 控制） ──

use std::sync::atomic::{AtomicU8, Ordering};

/// 当前 MCP 日志级别。0=trace, 1=debug, 2=info, 3=warn, 4=error。
static MCP_LOG_LEVEL: AtomicU8 = AtomicU8::new(2); // 默认 info

fn set_mcp_log_level(level: &str) {
    let v = match level {
        "trace" => 0,
        "debug" => 1,
        "info" => 2,
        "warn" => 3,
        "error" => 4,
        _ => 2,
    };
    MCP_LOG_LEVEL.store(v, Ordering::Relaxed);
    info!(level, "MCP log level updated");
}

/// 检查当前 MCP 日志级别是否允许输出指定级别。
#[allow(dead_code)]
fn mcp_log_enabled(target: u8) -> bool {
    MCP_LOG_LEVEL.load(Ordering::Relaxed) <= target
}

// ── HTTP 端点 ──

/// Streamable HTTP JSON-RPC 端点。
pub async fn mcp_stream_handler(
    Extension(db): Extension<Arc<Database>>,
    Extension(settings): Extension<Arc<Settings>>,
    Extension(user_store): Extension<Arc<crate::users::UserStore>>,
    Extension(sessions): Extension<Arc<crate::session::SessionStore>>,
    req: Request,
) -> Response {
    // 安全提取客户端 IP：优先 X-Forwarded-For（反代环境），回退 TCP 对端地址。
    // 不直接使用 axum 的 `ConnectInfo` 硬提取器，避免服务端未配置
    // into_make_service_with_connect_info 时 MCP 握手直接返回 500。
    let client_ip = crate::extract_client_ip(&req);

    // 拆分请求，取出 headers 与 body
    let (parts, body) = req.into_parts();
    let headers = parts.headers;

    // axum 0.7 的 Request body 为 `axum::body::Body`，需显式收集为字节后再转 String
    let body_bytes = match axum::body::to_bytes(body, 10 * 1024 * 1024).await {
        Ok(b) => b,
        Err(_) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"jsonrpc": "2.0", "error": {"code": -32603, "message": "Failed to read request body"}})),
            )
                .into_response();
        }
    };

    let body = match String::from_utf8(body_bytes.to_vec()) {
        Ok(b) => b,
        Err(_) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({"jsonrpc": "2.0", "error": {"code": -32700, "message": "Parse error: invalid utf-8"}})),
            )
                .into_response();
        }
    };

    // MCP 鉴权。
    //
    // 允许两类调用方：
    // 1. 已完成认证且 `Principal.mcp == true` 的用户凭证：
    //    - 密码登录签发的会话 token（`sess_...`）；
    //    - 用户个人 API token（`okb_...`，要求该 token 与用户都开启 mcp）；
    // 2. 受信任子网（**仅当** `system.json.trusted_subnet_enabled = true`）。
    //
    // 未启用用户体系（库里没有任何用户）时视为单机模式，等同 admin。
    let auth_enabled = user_store.user_count() > 0;
    let mut caller_role = crate::auth::Role::Admin;
    if auth_enabled {
        let token = crate::auth::extract_token_from_headers(&headers);

        let principal = match token.as_deref() {
            // 用户会话 / 个人 token
            Some(t) => match crate::auth::resolve_principal(t, &user_store, &sessions) {
                Some(p) => p,
                None => {
                    warn!(client_ip, "MCP authentication failed: invalid credential");
                    return (
                        StatusCode::UNAUTHORIZED,
                        Json(json!({"jsonrpc": "2.0", "error": {"code": -32001, "message": "Unauthorized"}})),
                    )
                        .into_response();
                }
            },
            None => {
                // 无凭证：仅当启用受信任子网且 IP 命中时放行
                let subnets = settings.trusted_subnets();
                let lan = settings.trusted_subnet_enabled()
                    && !subnets.is_empty()
                    && crate::utils::ip_matches_subnets(&client_ip, &subnets);
                if lan {
                    // 角色与 API 中间件保持一致：取 `trusted_subnet_role`（默认 admin）。
                    // 不要再硬编码 Admin —— MCP 已有写类工具（Markdown 编辑），
                    // 子网角色配成 view-only 时必须同样只读。
                    let role = settings.trusted_subnet_role();
                    crate::auth::Principal::local(role, true)
                } else {
                    warn!(client_ip, "MCP authentication failed: missing credential");
                    return (
                        StatusCode::UNAUTHORIZED,
                        Json(json!({"jsonrpc": "2.0", "error": {"code": -32001, "message": "Unauthorized"}})),
                    )
                        .into_response();
                }
            }
        };

        if !principal.mcp {
            warn!(
                client_ip,
                user = %principal.username,
                "MCP access denied: user/token has mcp = false"
            );
            return (
                StatusCode::UNAUTHORIZED,
                Json(json!({"jsonrpc": "2.0", "error": {"code": -32001, "message": "Unauthorized: MCP access not granted"}})),
            )
                .into_response();
        }

        // 把访问者记入当前请求 span，后续所有 MCP 日志（含 JSONL 的 span 字段）都会带上 user
        crate::record_span_identity(&principal);
        info!(
            client_ip,
            user = %principal.username,
            role = %principal.role.as_str(),
            via = %principal.via.as_str(),
            "MCP request authorized"
        );

        // 写类工具（Markdown 编辑）按调用方角色放行，仅 admin 可用
        caller_role = principal.role.clone();
    }

    let server = McpServer::new(db, settings, caller_role);

    // JSON-RPC 解析
    let req: Value = match serde_json::from_str(&body) {
        Ok(v) => v,
        Err(e) => {
            warn!(error = %e, body_len = %body.len(), "MCP JSON-RPC parse error");
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({"jsonrpc": "2.0", "error": {"code": -32700, "message": "Parse error"}})),
            )
                .into_response();
        }
    };

    // 校验 jsonrpc 版本
    if req.get("jsonrpc").and_then(|v| v.as_str()) != Some("2.0") {
        warn!(body = %body.chars().take(200).collect::<String>(), "MCP request missing jsonrpc 2.0");
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"jsonrpc": "2.0", "error": {"code": -32600, "message": "Invalid Request: must use jsonrpc 2.0"}})),
        )
            .into_response();
    }

    let method = req.get("method").and_then(|m| m.as_str()).unwrap_or("").to_string();
    let id = req.get("id").cloned().unwrap_or(Value::Null);
    let params = req.get("params").cloned().unwrap_or(json!({}));

    // 非通知请求必须有 id
    if id.is_null() && !method.starts_with("notifications/") {
        warn!(method, "MCP request missing id");
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"jsonrpc": "2.0", "error": {"code": -32600, "message": "Invalid Request: missing id"}})),
        )
            .into_response();
    }

    // 通知类消息不返回响应
    if method.starts_with("notifications/") {
        match method.as_str() {
            "notifications/initialized" => {
                debug!("MCP client initialized");
            }
            "notifications/cancelled" => {
                debug!("MCP request cancelled by client");
            }
            "notifications/progress" => {
                // 客户端报告的进度，暂不处理
            }
            "logging/setLevel" => {
                if let Some(level) = params.get("level").and_then(|v| v.as_str()) {
                    set_mcp_log_level(level);
                }
            }
            _ => {
                debug!(method, "MCP unhandled notification");
            }
        }
        return (StatusCode::ACCEPTED, "").into_response();
    }

    // 每个 JSON-RPC 调用开始时打一条 info 日志（HTTP 层的「请求开始」由
    // `main.rs::request_start_log_middleware` 记录，这里补上具体方法名）。
    info!(method = %method, client_ip = %client_ip, "MCP 调用开始");

    let response = match method.as_str() {
        "initialize" => {
            let protocol_version = params
                .get("protocolVersion")
                .cloned()
                .unwrap_or_else(|| json!("2024-11-05"));
            let client_info = params.get("clientInfo");
            info!(
                protocol = %protocol_version,
                client = %client_info.map(|c| c.to_string()).unwrap_or_default(),
                "MCP client initialize"
            );
            json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {
                    "protocolVersion": protocol_version,
                    "capabilities": {
                        "tools": {},
                        "resources": {},
                        "logging": {}
                    },
                    "serverInfo": {"name": "OKB-Assist", "version": "0.1.0-rust"}
                }
            })
        }
        "ping" => json!({"jsonrpc": "2.0", "id": id, "result": {}}),
        "tools/list" => {
            let tools = server.list_tools().await;
            debug!(count = %tools.len(), "MCP tools/list");
            json!({"jsonrpc": "2.0", "id": id, "result": {"tools": tools}})
        }
        "tools/call" => {
            let name = params.get("name").and_then(|n| n.as_str()).unwrap_or("");
            let args = params.get("arguments").cloned().unwrap_or(json!({}));
            info!(tool = %name, args = %args.to_string().chars().take(200).collect::<String>(), "MCP tool call");
            let result = server.call_tool(name, &args).await;
            debug!(tool = %name, result_len = %result.to_string().len(), "MCP tool call completed");
            json!({"jsonrpc": "2.0", "id": id, "result": result})
        }
        "resources/list" => {
            debug!("MCP resources/list (empty)");
            json!({"jsonrpc": "2.0", "id": id, "result": {"resources": []}})
        }
        "resources/read" => {
            // 标准做法：返回空 contents 表示没有可读资源
            debug!("MCP resources/read (not supported)");
            json!({"jsonrpc": "2.0", "id": id, "result": {"contents": []}})
        }
        _ => {
            warn!(method, "MCP unknown method");
            json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": {"code": -32601, "message": format!("Method not found: {}", method)}
            })
        }
    };

    Json(response).into_response()
}

/// MCP Streamable HTTP 端点的 OPTIONS 预检请求处理。
pub async fn mcp_options_handler() -> Response {
    (
        StatusCode::OK,
        [
            (axum::http::header::ALLOW, "POST, OPTIONS"),
        ],
    )
        .into_response()
}