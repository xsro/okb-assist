//! 配置桥接层 — 从 ConfigManager 动态读取配置属性。
//!
//! 使用 axum Extension 注入，支持运行时配置热更新。

use std::collections::HashMap;
use std::sync::Arc;

use serde_json::json;

use crate::config_manager::ConfigManager;
use crate::services::llm::{LlmClient, Protocol};

/// MinerU 配置项（V1 API）
///
/// - `tier`: 解析档位，可选 "flash" / "basic" / "standard" / "advanced"
/// - 本地和官方云均使用相同的 V1 API 端点
/// - `enabled`: 是否启用此配置（false 表示跳过，默认 true）
#[derive(Debug, Clone)]
pub struct MinerUConfig {
    pub url: String,
    pub token: String,
    pub name: String,
    pub mineru_type: String,
    pub tier: String,
    pub task_timeout: u64,
    pub max_tasks: usize,
}

/// 单个模型端点配置（协议 + 地址 + 凭据 + 模型名）。
///
/// 同时用于三类场景：
/// - `llm.models`（LLM 模型池，用于「问答」tab 与抽取的回退来源）
/// - `llm_extract`（信息补全 / 元数据抽取专用模型）
/// - 各向量库的 `embedding` 段（向量化，与 LLM 完全独立）
#[derive(Debug, Clone)]
pub struct ModelEndpoint {
    /// 展示名（仅 `llm.models` 中的项使用，其余场景为空）
    pub name: String,
    /// "ollama"（原生）或 "openai"
    pub protocol: String,
    pub url: String,
    pub key: String,
    pub model: String,
    /// 工具调用方式："" / "auto" / "native" / "prompt"（仅 `llm.models` 中的项使用）
    pub tool_mode: String,
}

impl Default for ModelEndpoint {
    fn default() -> Self {
        Self {
            name: String::new(),
            protocol: "ollama".to_string(),
            url: "http://127.0.0.1:11434".to_string(),
            key: String::new(),
            model: "qwen3.5:9b".to_string(),
            tool_mode: String::new(),
        }
    }
}

impl ModelEndpoint {
    /// 按 JSON 字段构造（非字符串/空白字段使用 fallback 中的同名属性）
    fn from_value(value: &serde_json::Value, fallback: &ModelEndpoint) -> Self {
        Self {
            name: non_empty_str(&value["name"]).unwrap_or_else(|| fallback.name.clone()),
            protocol: non_empty_str(&value["protocol"])
                .unwrap_or_else(|| fallback.protocol.clone()),
            url: non_empty_str(&value["url"])
                .map(|u| u.trim_end_matches('/').to_string())
                .unwrap_or_else(|| fallback.url.clone()),
            key: non_empty_str(&value["key"]).unwrap_or_else(|| fallback.key.clone()),
            model: non_empty_str(&value["model"]).unwrap_or_else(|| fallback.model.clone()),
            tool_mode: non_empty_str(&value["tool_mode"])
                .unwrap_or_else(|| fallback.tool_mode.clone()),
        }
    }

    /// 构造客户端（LLM 生成 / embedding 均使用同一实现）
    ///
    /// `key` 支持 `!cat <path>` 形式（由 [`crate::utils::resolve_token`] 在构造时解析），
    /// 因此配置里可以只存文件引用，不把密钥写进 `config.json`。
    pub fn client(&self) -> LlmClient {
        let key = crate::utils::resolve_token(&self.key);
        LlmClient::new(
            Protocol::parse(&self.protocol),
            &self.url,
            if key.is_empty() { None } else { Some(&key) },
            &self.model,
        )
    }
}

/// 取出非空字符串字段（去首尾空白）
fn non_empty_str(value: &serde_json::Value) -> Option<String> {
    value
        .as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(String::from)
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

    /// 获取所有 MinerU 配置
    pub fn mineru_configs(&self) -> Vec<MinerUConfig> {
        let arr = self.get_config()["mineru"]
            .as_array()
            .cloned()
            .unwrap_or_default();

        arr.iter()
            .map(|v| MinerUConfig {
                url: v["url"]
                    .as_str()
                    .unwrap_or("http://127.0.0.1:8002")
                    .trim_end_matches('/')
                    .to_string(),
                token: crate::utils::resolve_token(v["token"].as_str().unwrap_or("")),
                name: v["name"].as_str().unwrap_or("").to_string(),
                mineru_type: v["type"]
                    .as_str()
                    .unwrap_or("local")
                    .to_string(),
                tier: v["tier"].as_str().or_else(|| v["model_version"].as_str()).unwrap_or("standard").to_string(),
                task_timeout: v["task_timeout"].as_u64().unwrap_or(300),
                max_tasks: v["max_tasks"].as_u64().unwrap_or(3) as usize,
            })
            .collect()
    }

    /// 返回 active_mineru 的值（空字符串表示使用第一个配置）
    pub fn active_mineru(&self) -> String {
        self.get_config()["active_mineru"]
            .as_str()
            .unwrap_or("")
            .to_string()
    }

    /// 返回当前选中的 MinerU 配置（匹配 active_mineru 中指定的 name），
    /// 若未设置 active_mineru 则返回第一个配置。
    pub fn active_mineru_config(&self) -> Option<MinerUConfig> {
        let configs = self.mineru_configs();
        let active_name = self.active_mineru();

        if !active_name.is_empty() {
            // 有 active_name，按 name 匹配
            if let Some(cfg) = configs.iter().find(|c| c.name == active_name) {
                return Some(cfg.clone());
            }
        }

        // 无 active_name 或未匹配到，返回第一个配置
        configs.into_iter().next()
    }

    /// 兼容旧接口：返回第一个 MinerU 配置的 URL
    pub fn mineru_url(&self) -> String {
        self.mineru_configs()
            .first()
            .map(|c| c.url.clone())
            .unwrap_or_else(|| "http://127.0.0.1:8002".to_string())
    }

    /// 兼容旧接口：返回第一个 MinerU 配置的 token
    pub fn mineru_key(&self) -> String {
        self.mineru_configs()
            .first()
            .map(|c| c.token.clone())
            .unwrap_or_else(|| String::new())
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

    // ── LLM ──
    //
    // `llm.models` 是 LLM 模型池（被「问答」tab 用于选模型，也是抽取/embedding 的回退来源）；
    // `llm_extract` 是信息补全（元数据抽取）实际使用的模型配置，字段留空
    // 即从模型池中 `from` 指定的项（为空取第一项）继承。

    /// `llm.models` 原始数组（缺失时回退到旧的单对象 `llm` / `ollama`）
    fn llm_models_value(&self) -> Vec<serde_json::Value> {
        let config = self.get_config();
        if let Some(models) = config["llm"]["models"].as_array() {
            return models.clone();
        }
        // 兼容未经迁移的内存配置：单对象 llm / 旧的 ollama 键
        match config.get("llm") {
            Some(llm) if llm.is_object() => vec![llm.clone()],
            _ => match config.get("ollama") {
                Some(old) if old.is_object() => vec![old.clone()],
                _ => Vec::new(),
            },
        }
    }

    /// LLM 模型池（「问答」tab 的可选模型）
    pub fn llm_models(&self) -> Vec<ModelEndpoint> {
        self.llm_models_value()
            .iter()
            .enumerate()
            .map(|(i, v)| {
                let fallback = ModelEndpoint {
                    name: format!("model-{}", i + 1),
                    ..Default::default()
                };
                ModelEndpoint::from_value(v, &fallback)
            })
            .collect()
    }

    /// 模型池的第一项（无任何配置时返回内置默认值）
    fn first_llm_model(&self) -> ModelEndpoint {
        self.llm_models()
            .into_iter()
            .next()
            .unwrap_or_default()
    }

    /// 按名称从模型池取一项（`name` 为空或未命中时取第一项）
    ///
    /// 供对话等交互场景选择模型；与 `llm_extract_config` 的字段继承逻辑不同，
    /// 这里直接返回池中的那一项。
    pub fn llm_model_config(&self, name: Option<&str>) -> ModelEndpoint {
        let models = self.llm_models();
        name.map(str::trim)
            .filter(|n| !n.is_empty())
            .and_then(|n| models.iter().find(|m| m.name == n).cloned())
            .or_else(|| models.into_iter().next())
            .unwrap_or_default()
    }

    /// 信息补全（元数据抽取）使用的模型配置
    pub fn llm_extract_config(&self) -> ModelEndpoint {
        let config = self.get_config();
        let section = config.get("llm_extract").cloned().unwrap_or_else(|| json!({}));

        // 选择继承来源：按 from 指定的名字匹配，未命中则取第一项
        let models = self.llm_models();
        let from = non_empty_str(&section["from"]) ;
        let base = from
            .and_then(|name| models.iter().find(|m| m.name == name).cloned())
            .or_else(|| models.into_iter().next())
            .unwrap_or_default();

        ModelEndpoint::from_value(&section, &base)
    }

    /// 构造信息补全（抽取）客户端
    pub fn extract_client(&self) -> LlmClient {
        self.llm_extract_config().client()
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
    //
    // embedding 配置完全独立于 LLM：位于各向量库的 `embedding` 段。
    // `vdb` 为 None 时使用当前启用中的向量库；字段缺失时回退到模型池第一项
    // （兼容旧版本 embedding 复用 Ollama 配置的行为）。

    /// 解析指定向量库的 embedding 配置
    pub fn embedding_config_for(&self, vdb: Option<&serde_json::Value>) -> ModelEndpoint {
        let db = vdb.cloned().or_else(|| self.manager.get_active_vector_db());
        let emb = db.as_ref().and_then(|d| d.get("embedding")).cloned().unwrap_or_else(|| json!({}));
        let base = self.first_llm_model();

        // 旧字段 source（"ollama"/"openai"）作为 protocol 的回退
        let protocol_field = non_empty_str(&emb["protocol"]).or_else(|| non_empty_str(&emb["source"]));
        let mut resolved = ModelEndpoint::from_value(
            &json!({
                "protocol": protocol_field,
                "url": emb.get("url"),
                "key": emb.get("key"),
                "model": emb.get("model"),
            }),
            &base,
        );
        resolved.name = String::new();
        // embedding 模型语义与 LLM 不同，缺失时用固有默认值而非 LLM 模型名
        if non_empty_str(&emb["model"]).is_none() {
            resolved.model = "nomic-embed-text".to_string();
        }
        resolved
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

    // ── 权限 ──

    /// 从 config.json 读取权限配置（新格式）。
    /// 格式：{ token: { access: role, mcp: bool } }
    pub fn permission_tokens(&self) -> HashMap<String, serde_json::Value> {
        let mut result = HashMap::new();
        if let Some(perms) = self.get_config()["permissions"].as_object() {
            for (token, entry) in perms {
                // entry 可以是旧版字符串（兼容）或新版对象
                if entry.is_string() {
                    // 旧格式 "token": "role" → 转为新格式
                    let role = entry.as_str().unwrap_or("");
                    result.insert(token.clone(), json!({"access": role, "mcp": false}));
                } else if entry.is_object() {
                    result.insert(token.clone(), entry.clone());
                }
            }
        }
        result
    }

    // ── 受信任子网白名单 ──

    /// 返回受信任子网白名单（CIDR 格式字符串列表）。
    ///
    /// 来自匹配子网的请求免 Token/Bearer 鉴权。
    /// 配置缺失或为空数组时，不信任任何子网（全部需要鉴权）。
    pub fn trusted_subnets(&self) -> Vec<String> {
        self.get_system_config()["trusted_subnets"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default()
    }

    // ── 系统配置 ──
    pub fn token(&self) -> String {
        let cfg = self.get_system_config();
        let raw = cfg["token"].as_str().unwrap_or("change-me");
        crate::utils::resolve_token(raw)
    }

    pub fn mcp_token(&self) -> String {
        let cfg = self.get_system_config();
        let raw = cfg["mcp_token"].as_str().unwrap_or("change-me");
        crate::utils::resolve_token(raw)
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
    /// 用于拼接完整的文档链接（MCP 的 pdf_need_auth_url / markdown_url / detail_url）。
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

    /// 问答会话文件路径模板（`{id}` 为会话 ID，字符串）
    pub fn chat_session_path_template(&self) -> String {
        self.get_system_config()["chat_session_path"]
            .as_str()
            .unwrap_or("data/chat_sessions/{id}.json")
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

    pub fn log_path(&self) -> String {
        self.get_system_config()["log_path"]
            .as_str()
            .unwrap_or("okb-log.jsonl")
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
#[cfg(test)]
mod tests {
    use super::*;
    use crate::config_manager::ConfigManager;

    /// 用临时目录构造一个 Settings，写入指定 config.json 内容
    fn settings_with(config_body: serde_json::Value) -> Settings {
        let dir = std::env::temp_dir().join(format!(
            "okb-cfg-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("config.json"),
            serde_json::to_string(&config_body).unwrap(),
        )
        .unwrap();
        std::fs::write(dir.join("system.json"), "{}").unwrap();
        // ConfigManager::new 接受相对路径，先切换进程 cwd 会污染其它测试，
        // 因此这里直接构造 manager 并写入其内部路径对应的文件
        let manager = ConfigManager::for_tests(dir.join("config.json"), dir.join("system.json"));
        Settings::new(std::sync::Arc::new(manager))
    }

    #[test]
    fn extract_config_inherits_from_named_model() {
        let s = settings_with(serde_json::json!({
            "llm": { "models": [
                {"name": "a", "protocol": "ollama", "url": "http://a:11434", "key": "ka", "model": "qa-model"},
                {"name": "b", "protocol": "openai", "url": "https://api.example.com/v1", "key": "kb", "model": "qb-model"}
            ]},
            "llm_extract": {"from": "b", "model": "extract-model"},
            "vector_dbs": []
        }));
        let cfg = s.llm_extract_config();
        assert_eq!(cfg.protocol, "openai"); // 继承自 b
        assert_eq!(cfg.url, "https://api.example.com/v1"); // 继承自 b
        assert_eq!(cfg.key, "kb"); // 继承自 b
        assert_eq!(cfg.model, "extract-model"); // 显式覆盖
    }

    #[test]
    fn extract_config_defaults_to_first_model() {
        let s = settings_with(serde_json::json!({
            "llm": { "models": [
                {"name": "a", "protocol": "openai", "url": "http://a:11434", "key": "ka", "model": "qa-model"}
            ]},
            "llm_extract": {"from": "", "protocol": "", "url": "", "key": "", "model": ""},
            "vector_dbs": []
        }));
        let cfg = s.llm_extract_config();
        assert_eq!(cfg.protocol, "openai");
        assert_eq!(cfg.url, "http://a:11434");
        assert_eq!(cfg.key, "ka");
        assert_eq!(cfg.model, "qa-model");
    }

    #[test]
    fn extract_config_falls_back_when_no_models_configured() {
        let s = settings_with(serde_json::json!({
            "llm": { "models": [] },
            "llm_extract": {},
            "vector_dbs": []
        }));
        let cfg = s.llm_extract_config();
        assert_eq!(cfg.protocol, "ollama");
        assert_eq!(cfg.url, "http://127.0.0.1:11434");
        assert_eq!(cfg.model, "qwen3.5:9b");
    }

    #[test]
    fn embedding_is_independent_from_llm() {
        let s = settings_with(serde_json::json!({
            "llm": { "models": [
                {"name": "qa", "protocol": "openai", "url": "https://api.openai.com/v1", "key": "sk-llm", "model": "gpt-4o"}
            ]},
            "llm_extract": {"from": "qa", "model": "gpt-4o-mini"},
            "vector_dbs": [{
                "id": "default", "type": "qdrant", "enabled": true,
                "embedding": {"protocol": "ollama", "url": "http://embed:11434", "key": "k-embed", "model": "bge-m3"}
            }]
        }));
        let cfg = s.embedding_config_for(None);
        assert_eq!(cfg.protocol, "ollama");
        assert_eq!(cfg.url, "http://embed:11434");
        assert_eq!(cfg.key, "k-embed");
        assert_eq!(cfg.model, "bge-m3");
        // LLM 与抽取配置不受 embedding 影响
        assert_eq!(s.llm_extract_config().model, "gpt-4o-mini");
        assert_eq!(s.llm_models().len(), 1);
    }

    #[test]
    fn embedding_inherits_first_model_when_fields_empty() {
        let s = settings_with(serde_json::json!({
            "llm": { "models": [
                {"name": "a", "protocol": "openai", "url": "https://api.openai.com", "key": "sk", "model": "gpt"}
            ]},
            "llm_extract": {},
            "vector_dbs": [{"id": "default", "enabled": true, "embedding": {}}]
        }));
        let cfg = s.embedding_config_for(None);
        assert_eq!(cfg.protocol, "openai");
        assert_eq!(cfg.url, "https://api.openai.com");
        assert_eq!(cfg.key, "sk");
        // model 不回退到 LLM 模型名（语义不同）
        assert_eq!(cfg.model, "nomic-embed-text");
        // name 对 embedding 无意义，保持为空
        assert_eq!(cfg.name, "");
    }

    #[test]
    fn llm_models_returns_pool_in_order() {
        let s = settings_with(serde_json::json!({
            "llm": { "models": [
                {"name": "a", "protocol": "ollama", "url": "http://a:11434", "key": "", "model": "m1"},
                {"name": "b", "protocol": "openai", "url": "http://b:1234", "key": "kb", "model": "m2"}
            ]},
            "llm_extract": {},
            "vector_dbs": []
        }));
        let models = s.llm_models();
        assert_eq!(models.len(), 2);
        assert_eq!(models[0].name, "a");
        assert_eq!(models[1].model, "m2");
        assert_eq!(models[1].protocol, "openai");
    }

    #[test]
    fn llm_model_config_picks_by_name_and_defaults_to_first() {
        let s = settings_with(serde_json::json!({
            "llm": { "models": [
                {"name": "a", "protocol": "ollama", "url": "http://a:11434", "key": "", "model": "m1"},
                {"name": "b", "protocol": "openai", "url": "http://b:1234", "key": "kb", "model": "m2"}
            ]},
            "llm_extract": {},
            "vector_dbs": []
        }));

        // 命中指定名称
        let b = s.llm_model_config(Some("b"));
        assert_eq!(b.model, "m2");
        assert_eq!(b.protocol, "openai");

        // 未命中 / 空名称 / None 均回退第一项
        assert_eq!(s.llm_model_config(Some("missing")).model, "m1");
        assert_eq!(s.llm_model_config(Some("  ")).model, "m1");
        assert_eq!(s.llm_model_config(None).model, "m1");
    }
}
