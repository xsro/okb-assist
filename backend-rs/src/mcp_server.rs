//! MCP 服务。
//!
//! 提供 MCP 工具列表与工具调用逻辑。Streamable HTTP 端点在后续版本接入。

use std::sync::Arc;

use serde_json::{json, Value};

use crate::config::Settings;
use crate::database::Database;

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
                "name": "search_documents",
                "description": "Semantic search across documents",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "query": {"type": "string"},
                        "limit": {"type": "integer", "default": 5}
                    }
                }
            }),
            json!({
                "name": "grep_search",
                "description": "Full-text grep search",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "query": {"type": "string"},
                        "limit": {"type": "integer", "default": 10}
                    }
                }
            }),
            json!({
                "name": "get_document",
                "description": "Get document metadata by ID",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "document_id": {"type": "integer"}
                    },
                    "required": ["document_id"]
                }
            }),
            json!({
                "name": "list_documents",
                "description": "List documents",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "page": {"type": "integer", "default": 1},
                        "page_size": {"type": "integer", "default": 50}
                    }
                }
            }),
            json!({
                "name": "get_markdown",
                "description": "Get document markdown content",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "document_id": {"type": "integer"}
                    },
                    "required": ["document_id"]
                }
            }),
        ]
    }

    pub async fn call_tool(&self, name: &str, args: &Value) -> Value {
        match name {
            "search_documents" => {
                let query = args["query"].as_str().unwrap_or("");
                let limit = args["limit"].as_i64().unwrap_or(5) as usize;
                json!({
                    "content": [{
                        "type": "text",
                        "text": format!("Search results for '{}' (limit: {})", query, limit)
                    }]
                })
            }
            "grep_search" => {
                let query = args["query"].as_str().unwrap_or("");
                let limit = args["limit"].as_i64().unwrap_or(10) as usize;
                json!({
                    "content": [{
                        "type": "text",
                        "text": format!("Grep results for '{}' (limit: {})", query, limit)
                    }]
                })
            }
            "get_document" => {
                let doc_id = args["document_id"].as_i64().unwrap_or(0);
                json!({
                    "content": [{
                        "type": "text",
                        "text": format!("Document {}", doc_id)
                    }]
                })
            }
            "list_documents" => {
                let page = args["page"].as_i64().unwrap_or(1);
                let page_size = args["page_size"].as_i64().unwrap_or(50);
                json!({
                    "content": [{
                        "type": "text",
                        "text": format!("Page {} size {}", page, page_size)
                    }]
                })
            }
            "get_markdown" => {
                let doc_id = args["document_id"].as_i64().unwrap_or(0);
                json!({
                    "content": [{
                        "type": "text",
                        "text": format!("Markdown for document {}", doc_id)
                    }]
                })
            }
            _ => json!({
                "error": "Unknown tool",
                "content": [{
                    "type": "text",
                    "text": format!("Unknown tool: {}", name)
                }]
            })
        }
    }
}
