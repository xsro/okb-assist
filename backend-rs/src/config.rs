//! 配置桥接层 — 从 ConfigManager 动态读取配置属性。
//!
//! 使用 axum Extension 注入，支持运行时配置热更新。

use std::sync::Arc;

use crate::config_manager::ConfigManager;

/// MinerU 配置项（V1 API）
///
/// - `tier`: 解析档位，可选 "flash" / "basic" / "standard" / "advanced"
/// - 本地和官方云均使用相同的 V1 API 端点
/// - `enabled`: 是否启用此配置（false 表示跳过，默认 true）
#[derive(Debug, Clone)]
pub struct MinerUConfig {
    pub url: String,
    pub key: String,
    pub mineru_type: String,
    pub tier: String,
    pub task_timeout: u64,
    pub enabled: bool,
}

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

    /// 获取所有 MinerU 配置（数组格式）
    /// 获取所有启用（enabled=true）的 MinerU 配置
    /// 若所有配置均未设置 enabled，则全部使用（向后兼容）
    pub fn mineru_configs(&self) -> Vec<MinerUConfig> {
        let arr = self.get_config()["mineru"]
            .as_array()
            .cloned()
            .unwrap_or_default();

        let has_explicit_enabled = arr.iter().any(|v| v.get("enabled").is_some());

        arr.iter()
            .filter(|v| {
                // 如有显式 enabled 标记，只取 enabled=true 的配置
                // 如无 enabled 标记，向后兼容：全部使用
                if has_explicit_enabled {
                    v.get("enabled").and_then(|e| e.as_bool()).unwrap_or(false)
                } else {
                    true
                }
            })
            .map(|v| MinerUConfig {
                url: v["url"]
                    .as_str()
                    .unwrap_or("http://127.0.0.1:8002")
                    .trim_end_matches('/')
                    .to_string(),
                key: v["key"].as_str().unwrap_or("key").to_string(),
                mineru_type: v["type"]
                    .as_str()
                    .unwrap_or("local")
                    .to_string(),
                tier: v["tier"].as_str().or_else(|| v["model_version"].as_str()).unwrap_or("standard").to_string(),
                task_timeout: v["task_timeout"].as_u64().unwrap_or(300),
                enabled: v.get("enabled").and_then(|e| e.as_bool()).unwrap_or(true),
            })
            .collect()
    }

    /// 返回 active_mineru_key 的值（空字符串表示使用第一个启用的配置）
    pub fn active_mineru_key(&self) -> String {
        self.get_config()["active_mineru_key"]
            .as_str()
            .unwrap_or("")
            .to_string()
    }

    /// 返回当前选中的 MinerU 配置（匹配 active_mineru_key），
    /// 若未设置 active_mineru_key 则返回第一个启用的配置。
    pub fn active_mineru_config(&self) -> Option<MinerUConfig> {
        let configs = self.mineru_configs();
        let active_key = self.active_mineru_key();

        if !active_key.is_empty() {
            // 有 active_key，找匹配的配置
            if let Some(cfg) = configs.iter().find(|c| c.key == active_key) {
                return Some(cfg.clone());
            }
        }

        // 无 active_key 或未匹配到，返回第一个启用的配置
        configs.into_iter().next()
    }

    /// 兼容旧接口：返回第一个 MinerU 配置的 URL
    pub fn mineru_url(&self) -> String {
        self.mineru_configs()
            .first()
            .map(|c| c.url.clone())
            .unwrap_or_else(|| "http://127.0.0.1:8002".to_string())
    }

    /// 兼容旧接口：返回第一个 MinerU 配置的 key
    pub fn mineru_key(&self) -> String {
        self.mineru_configs()
            .first()
            .map(|c| c.key.clone())
            .unwrap_or_else(|| "key".to_string())
    }

    /// 兼容旧接口：返回第一个 MinerU 配置的 type
    pub fn mineru_type(&self) -> String {
        self.mineru_configs()
            .first()
            .map(|c| c.mineru_type.clone())
            .unwrap_or_else(|| "local".to_string())
    }

    pub fn mineru_tasks(&self) -> usize {
        self.get_config()["mineru"]
            .as_array()
            .and_then(|arr| arr.first())
            .and_then(|v| v.get("max_tasks"))
            .and_then(|v| v.as_u64())
            .unwrap_or(3) as usize
    }

    pub fn mineru_task_timeout(&self) -> u64 {
        self.get_config()["mineru"]
            .as_array()
            .and_then(|arr| arr.first())
            .and_then(|v| v.get("task_timeout"))
            .and_then(|v| v.as_u64())
            .unwrap_or(300)
    }

    /// 返回第一个 MinerU 配置的 tier
    pub fn mineru_tier(&self) -> String {
        self.mineru_configs()
            .first()
            .map(|c| c.tier.clone())
            .unwrap_or_else(|| "standard".to_string())
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
            .unwrap_or("sqlite:///data/okb_assist.db")
            .to_string()
    }

    pub fn uploads_folder(&self) -> String {
        self.get_system_config()["uploads_folder"]
            .as_str()
            .unwrap_or("data/_uploads")
            .to_string()
    }

    /// 获取 config.json 中的 base_url（部署基础地址）。
    /// 用于拼接完整的文档链接（pdf_url / markdown_url / detail_url）。
    /// 未设置或为空时返回空字符串，调用方需自行拼接相对路径。
    pub fn base_url(&self) -> String {
        self.get_config()["base_url"]
            .as_str()
            .unwrap_or("")
            .trim_end_matches('/')
            .to_string()
    }

    /// 获取 config.json 中的 alias_expiration_hours（别名链接有效期，单位：小时）。
    /// 默认 1 小时。
    pub fn alias_expiration_hours(&self) -> i64 {
        self.get_config()["alias_expiration_hours"]
            .as_i64()
            .unwrap_or(1)
            .max(1)
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

    pub fn mutool_path(&self) -> String {
        self.get_system_config()["mutool_path"]
            .as_str()
            .unwrap_or("mutool")
            .to_string()
    }

    pub fn ui_path(&self) -> String {
        self.get_system_config()["ui_path"]
            .as_str()
            .unwrap_or("frontend/dist")
            .to_string()
    }

    // ── system.json 路径信息（用于路径变量替换） ──
    /// 返回解析后的工作目录（绝对路径）。
    pub fn cwd(&self) -> String {
        self.manager.cwd()
    }

    /// 返回 system.json 所在目录的路径（绝对路径）。
    pub fn system_dir(&self) -> String {
        self.manager.system_dir()
    }

    /// 返回 system.json 的完整路径（绝对路径）。
    pub fn system_path(&self) -> String {
        self.manager.system_path()
    }

    /// 解析路径模板中的变量（仅 {id} 和 {env:VAR}）。
    pub fn substitute_path(&self, template: &str, doc_id: i64) -> String {
        crate::config_manager::ConfigManager::substitute_path_variables(template, doc_id)
    }
}