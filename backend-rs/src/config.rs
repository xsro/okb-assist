//! 配置桥接层 — 从 ConfigManager 动态读取配置属性。
//!
//! 使用 axum Extension 注入，支持运行时配置热更新。

use std::sync::Arc;

use crate::config_manager::ConfigManager;

#[derive(Clone)]
pub struct Settings {
    manager: Arc<ConfigManager>,
}

impl Settings {
    pub fn new(manager: Arc<ConfigManager>) -> Self {
        Self { manager }
    }

    fn get_config(&self) -> serde_json::Value {
        self.manager.get_config()
    }

    fn get_system_config(&self) -> serde_json::Value {
        self.manager.get_system_config()
    }

    // ── MinerU ──
    pub fn mineru_url(&self) -> String {
        self.get_config()["mineru"]["url"]
            .as_str()
            .unwrap_or("http://127.0.0.1:8002")
            .trim_end_matches('/')
            .to_string()
    }

    pub fn mineru_key(&self) -> String {
        self.get_config()["mineru"]["key"]
            .as_str()
            .unwrap_or("key")
            .to_string()
    }

    pub fn mineru_type(&self) -> String {
        self.get_config()["mineru"]["type"]
            .as_str()
            .unwrap_or("local")
            .to_string()
    }

    pub fn mineru_tasks(&self) -> usize {
        self.get_config()["mineru"]["max_tasks"]
            .as_u64()
            .unwrap_or(3) as usize
    }

    pub fn mineru_task_timeout(&self) -> u64 {
        self.get_config()["mineru"]["task_timeout"]
            .as_u64()
            .unwrap_or(300)
    }

    pub fn mineru_model_version(&self) -> String {
        self.get_config()["mineru"]["model_version"]
            .as_str()
            .unwrap_or("vlm")
            .to_string()
    }

    // ── Ollama ──
    pub fn ollama_url(&self) -> String {
        self.get_config()["ollama"]["url"]
            .as_str()
            .unwrap_or("http://127.0.0.1:11434")
            .trim_end_matches('/')
            .to_string()
    }

    pub fn ollama_key(&self) -> String {
        self.get_config()["ollama"]["key"]
            .as_str()
            .unwrap_or("")
            .to_string()
    }

    pub fn ollama_model(&self) -> String {
        self.get_config()["ollama"]["model"]
            .as_str()
            .unwrap_or("qwen3.5:9b")
            .to_string()
    }

    // ── FastEmbed ──
    pub fn fastembed_url(&self) -> String {
        self.get_config()
            .get("fastembed")
            .and_then(|v| v.get("url"))
            .and_then(|v| v.as_str())
            .unwrap_or("http://127.0.0.1:8003")
            .trim_end_matches('/')
            .to_string()
    }

    // ── Embedding ──
    pub fn embedding_source(&self) -> String {
        self.manager
            .get_active_vector_db()
            .and_then(|db| {
                db.get("embedding")
                    .and_then(|e| e.get("source"))
                    .and_then(|v| v.as_str())
                    .map(String::from)
            })
            .unwrap_or_else(|| "ollama".to_string())
    }

    pub fn embedding_model(&self) -> String {
        self.manager
            .get_active_vector_db()
            .and_then(|db| {
                db.get("embedding")
                    .and_then(|e| e.get("model"))
                    .and_then(|v| v.as_str())
                    .map(String::from)
            })
            .unwrap_or_else(|| "nomic-embed-text".to_string())
    }

    // ── Qdrant (兼容旧代码) ──
    pub fn qdrant_url(&self) -> String {
        if let Some(db) = self.manager.get_active_vector_db() {
            if db["type"].as_str() == Some("qdrant") {
                if let Some(url) = db.get("url").and_then(|v| v.as_str()) {
                    return url.trim_end_matches('/').to_string();
                }
            }
        }
        "http://127.0.0.1:6333".to_string()
    }

    pub fn qdrant_collection(&self) -> String {
        if let Some(db) = self.manager.get_active_vector_db() {
            if db["type"].as_str() == Some("qdrant") {
                if let Some(col) = db.get("collection").and_then(|v| v.as_str()) {
                    return col.to_string();
                }
            }
        }
        "documents".to_string()
    }

    // ── 系统配置 ──
    pub fn token(&self) -> String {
        self.get_system_config()["token"]
            .as_str()
            .unwrap_or("change-me")
            .to_string()
    }

    pub fn mcp_token(&self) -> String {
        self.get_system_config()["mcp_token"]
            .as_str()
            .unwrap_or("change-me")
            .to_string()
    }

    pub fn max_concurrent_tasks(&self) -> usize {
        self.get_system_config()["max_concurrent_tasks"]
            .as_u64()
            .unwrap_or(3) as usize
    }

    pub fn database_url(&self) -> String {
        self.get_system_config()["database_url"]
            .as_str()
            .unwrap_or("sqlite:///./okb_assist.db")
            .to_string()
    }

    pub fn uploads_folder(&self) -> String {
        self.get_system_config()["uploads_folder"]
            .as_str()
            .unwrap_or("data/_uploads")
            .to_string()
    }

    pub fn public_url(&self) -> String {
        self.get_system_config()["public_url"]
            .as_str()
            .unwrap_or("http://localhost:5001")
            .to_string()
    }

    pub fn subnet_url(&self) -> String {
        self.get_system_config()["subnet_url"]
            .as_str()
            .unwrap_or("http://192.168.1.100:5001")
            .to_string()
    }

    // ── 路径模板 ──
    pub fn markdown_path_template(&self) -> String {
        self.get_system_config()["markdown_path"]
            .as_str()
            .unwrap_or("data/uploads/{id}/{id}.md")
            .to_string()
    }

    pub fn info_path_template(&self) -> String {
        self.get_system_config()["info_path"]
            .as_str()
            .unwrap_or("data/uploads/{id}/{id}.json")
            .to_string()
    }

    pub fn crossref_path_template(&self) -> String {
        self.get_system_config()["crossref_path"]
            .as_str()
            .unwrap_or("data/uploads/{id}/{id}_crossref.json")
            .to_string()
    }

    pub fn markdown_asset_path_template(&self) -> String {
        self.get_system_config()["markdown_asset_path"]
            .as_str()
            .unwrap_or("data/uploads/{id}/{id}.zip")
            .to_string()
    }

    pub fn pdf_path_template(&self) -> String {
        self.get_system_config()["pdf_path"]
            .as_str()
            .unwrap_or("data/uploads/{id}/{id}.pdf")
            .to_string()
    }

    pub fn grep_path(&self) -> String {
        self.get_system_config()["grep_path"]
            .as_str()
            .unwrap_or("grep")
            .to_string()
    }

    pub fn pdfcpu_path(&self) -> String {
        self.get_system_config()["pdfcpu_path"]
            .as_str()
            .unwrap_or("pdfcpu")
            .to_string()
    }
}