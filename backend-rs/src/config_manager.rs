//! JSON 配置文件管理模块。
//!
//! 配置分为两个文件：
//! - config.json: 服务配置（MinerU、Ollama、向量数据库），可通过前端修改
//! - system.json: 系统配置（token、并发数、数据库、上传目录），仅限手动修改

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};

/// 默认服务配置
pub fn default_config() -> serde_json::Value {
    serde_json::json!({
        "base_url": "",
        "alias_expiration_hours": 1,
        "mineru": [
            {
                "type": "local",
                "url": "http://127.0.0.1:8002",
                "key": "key",
                "max_tasks": 3,
                "task_timeout": 300,
                "model_version": "vlm"
            }
        ],
        "ollama": {
            "url": "http://127.0.0.1:11434",
            "key": "",
            "model": "qwen3.5:9b"
        },
        "vector_dbs": [
            {
                "id": "default",
                "name": "默认 Qdrant",
                "type": "qdrant",
                "enabled": true,
                "url": "http://127.0.0.1:6333",
                "collection": "documents",
                "embedding": {
                    "source": "ollama",
                    "model": "nomic-embed-text"
                }
            }
        ]
    })
}

/// 默认系统配置
pub fn default_system() -> serde_json::Value {
    serde_json::json!({
        "token": "change-me",
        "mcp_token": "change-me",
        "max_concurrent_tasks": 3,
        "database_url": "sqlite:///data/okb_assist.db",
        "cwd": "{system_dir}",
        "uploads_folder": "data/_uploads",
        "markdown_path": "data/markdowns/{id}.md",
        "info_path": "data/markdowns/{id}.json",
        "crossref_path": "data/markdowns/{id}_crossref.json",
        "markdown_asset_path": "data/pdfs/{id}/{id}.zip",
        "pdf_path": "data/pdfs/{id}/{id}.pdf",
        "config_path": "config.json",
        "log_path": "stdout"
    })
}

/// 配置管理器，带进程内缓存
#[derive(Clone)]
pub struct ConfigManager {
    config_file: PathBuf,
    system_file: PathBuf,
    cwd: String,
    cache: Arc<RwLock<Option<serde_json::Value>>>,
    system_cache: Arc<RwLock<Option<serde_json::Value>>>,
}

impl ConfigManager {
    /// 从 system.json 路径创建配置管理器。
    /// config.json 的路径从 system.json 的 config_path 字段读取，
    /// 若为相对路径则相对于 system.json 所在目录解析。
    /// cwd 和 config_path 支持 {system_dir} 变量替换。
    pub fn new(system_path: &str) -> Self {
        let system_path = PathBuf::from(system_path);
        let system = Self::load_json_file(&system_path, &default_system());
        let system_dir = system_path
            .parent()
            .unwrap_or(&PathBuf::from("."))
            .to_string_lossy()
            .to_string();
        let system_path_str = system_path.to_string_lossy().to_string();

        // 解析 cwd（支持 {system_dir} 替换）
        let cwd = system
            .get("cwd")
            .and_then(|v| v.as_str())
            .unwrap_or("{system_dir}")
            .to_string();
        let cwd = cwd.replace("{system_dir}", &system_dir).replace("{system_path}", &system_path_str);

        // 解析 config_path（支持 {system_dir} 替换）
        let config_path = system
            .get("config_path")
            .and_then(|v| v.as_str())
            .unwrap_or("config.json")
            .to_string();
        let config_path = config_path.replace("{system_dir}", &system_dir).replace("{system_path}", &system_path_str);
        // 若 config_path 是相对路径，相对于 system.json 所在目录解析
        let config_file = if PathBuf::from(&config_path).is_absolute() {
            PathBuf::from(&config_path)
        } else {
            system_path
                .parent()
                .unwrap_or(&PathBuf::from("."))
                .join(&config_path)
        };

        Self {
            config_file,
            system_file: system_path,
            cwd,
            cache: Arc::new(RwLock::new(None)),
            system_cache: Arc::new(RwLock::new(None)),
        }
    }

    /// 返回 system.json 所在目录的路径（绝对路径）。
    pub fn system_dir(&self) -> String {
        match self.system_file.parent() {
            Some(dir) => {
                let s = dir.to_string_lossy().to_string();
                if s.is_empty() {
                    ".".to_string()
                } else {
                    s
                }
            }
            None => ".".to_string(),
        }
    }

    /// 返回 system.json 的完整路径（绝对路径）。
    pub fn system_path(&self) -> String {
        self.system_file.to_string_lossy().to_string()
    }

    /// 返回解析后的工作目录（绝对路径）。
    pub fn cwd(&self) -> String {
        self.cwd.clone()
    }

    /// 解析路径模板中的变量。
    ///
    /// 支持的变量：
    /// - `{id}` — 文档 ID
    /// - `{env:VAR_NAME}` — 环境变量 VAR_NAME 的值
    pub fn substitute_path_variables(template: &str, doc_id: i64) -> String {
        let mut result = template.to_string();

        // 替换 {id}
        result = result.replace("{id}", &doc_id.to_string());

        // 替换 {env:VAR_NAME}
        Self::substitute_env_vars(&result)
    }

    /// 扫描字符串中的 {env:VAR_NAME} 模式并替换为环境变量值。
    fn substitute_env_vars(s: &str) -> String {
        let mut result = s.to_string();
        let mut start = 0;
        while let Some(pos) = result[start..].find("{env:") {
            let abs_pos = start + pos;
            if let Some(end_pos) = result[abs_pos..].find('}') {
                let end_abs = abs_pos + end_pos;
                let var_expr = &result[abs_pos + 5..end_abs];
                if let Some(env_val) = std::env::var(var_expr).ok() {
                    let before = &result[..abs_pos];
                    let after = &result[end_abs + 1..];
                    result = format!("{}{}{}", before, env_val, after);
                    start = abs_pos + env_val.len();
                } else {
                    start = end_abs + 1;
                }
            } else {
                break;
            }
        }
        result
    }

    fn deep_merge(base: &serde_json::Value, override_val: &serde_json::Value) -> serde_json::Value {
        match (base, override_val) {
            (serde_json::Value::Object(b), serde_json::Value::Object(o)) => {
                let mut result = b.clone();
                for (k, v) in o {
                    if let Some(bv) = result.get(k) {
                        result[k] = Self::deep_merge(bv, v);
                    } else {
                        result.insert(k.clone(), v.clone());
                    }
                }
                serde_json::Value::Object(result)
            }
            _ => override_val.clone(),
        }
    }

    fn load_json_file(filepath: &PathBuf, defaults: &serde_json::Value) -> serde_json::Value {
        if filepath.exists() {
            match fs::read_to_string(filepath) {
                Ok(content) => {
                    match serde_json::from_str(&content) {
                        Ok(data) => Self::deep_merge(defaults, &data),
                        Err(_) => defaults.clone(),
                    }
                }
                Err(_) => defaults.clone(),
            }
        } else {
            if let Some(parent) = filepath.parent() {
                fs::create_dir_all(parent).ok();
            }
            let defaults_str = serde_json::to_string_pretty(defaults).unwrap_or_default();
            fs::write(filepath, defaults_str).ok();
            defaults.clone()
        }
    }

    fn write_json_file(filepath: &PathBuf, data: &serde_json::Value) -> anyhow::Result<()> {
        if let Some(parent) = filepath.parent() {
            fs::create_dir_all(parent)?;
        }
        let content = serde_json::to_string_pretty(data)?;
        fs::write(filepath, content)?;
        Ok(())
    }

    pub fn load_config(&self) -> serde_json::Value {
        {
            let cache = self.cache.read().unwrap();
            if let Some(ref cached) = *cache {
                return cached.clone();
            }
        }
        let mut cache = self.cache.write().unwrap();
        if cache.is_none() {
            *cache = Some(Self::load_json_file(&self.config_file, &default_config()));
        }
        cache.as_ref().unwrap().clone()
    }

    pub fn load_system_config(&self) -> serde_json::Value {
        {
            let cache = self.system_cache.read().unwrap();
            if let Some(ref cached) = *cache {
                return cached.clone();
            }
        }
        let mut cache = self.system_cache.write().unwrap();
        if cache.is_none() {
            *cache = Some(Self::load_json_file(&self.system_file, &default_system()));
        }
        cache.as_ref().unwrap().clone()
    }

    pub fn get_config(&self) -> serde_json::Value {
        let mut config = self.load_config();
        let system = self.load_system_config();
        if let (Some(c), Some(s)) = (config.as_object_mut(), system.as_object()) {
            for (k, v) in s {
                c.insert(k.clone(), v.clone());
            }
        }
        config
    }

    pub fn get_service_config(&self) -> serde_json::Value {
        self.load_config()
    }

    pub fn get_system_config(&self) -> serde_json::Value {
        self.load_system_config()
    }

    pub fn save_config(&self, config: &serde_json::Value) -> anyhow::Result<()> {
        let service_keys: Vec<String> = default_config()
            .as_object()
            .map(|o| o.keys().cloned().collect())
            .unwrap_or_default();
        let mut service_config = serde_json::Map::new();
        if let Some(obj) = config.as_object() {
            for (k, v) in obj {
                if service_keys.contains(k) {
                    service_config.insert(k.clone(), v.clone());
                }
            }
        }
        let result = serde_json::Value::Object(service_config);
        Self::write_json_file(&self.config_file, &result)?;
        *self.cache.write().unwrap() = Some(result);
        Ok(())
    }

    pub fn reload_config(&self) -> serde_json::Value {
        *self.cache.write().unwrap() = None;
        *self.system_cache.write().unwrap() = None;
        self.get_config()
    }

    /// 更新服务配置（前端提交的完整 config.json）
    pub fn update_config(&self, config: &serde_json::Value) -> anyhow::Result<()> {
        self.save_config(config)
    }

    /// 更新系统配置（仅允许修改部分字段）
    pub fn update_system_config(&self, config: &serde_json::Value) -> anyhow::Result<()> {
        let current = self.load_system_config();
        let merged = Self::deep_merge(&current, config);
        let result = Self::write_json_file(&self.system_file, &merged)?;
        *self.system_cache.write().unwrap() = Some(merged);
        Ok(result)
    }

    /// 列出所有向量数据库配置
    pub fn list_vector_dbs(&self) -> Vec<serde_json::Value> {
        self.get_config()
            .get("vector_dbs")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default()
    }

    pub fn get_active_vector_db(&self) -> Option<serde_json::Value> {
        let cfg = self.get_config();
        cfg.get("vector_dbs")
            .and_then(|v| v.as_array())
            .and_then(|arr| arr.iter().find(|db| db.get("enabled").and_then(|e| e.as_bool()).unwrap_or(false)))
            .cloned()
    }

    pub fn get_vector_db_by_id(&self, db_id: &str) -> Option<serde_json::Value> {
        let cfg = self.get_config();
        cfg.get("vector_dbs")
            .and_then(|v| v.as_array())
            .and_then(|arr| arr.iter().find(|db| db.get("id").and_then(|i| i.as_str()) == Some(db_id)))
            .cloned()
    }

    /// 敏感字段脱敏
    pub fn mask_sensitive(&self, config: &serde_json::Value) -> serde_json::Value {
        let mut masked = config.clone();
        if let Some(obj) = masked.as_object_mut() {
            // MinerU key（数组格式）
            if let Some(mineru) = obj.get_mut("mineru") {
                if let Some(arr) = mineru.as_array_mut() {
                    for item in arr.iter_mut() {
                        if let Some(key) = item.get_mut("key") {
                            if let Some(k) = key.as_str() {
                                if k.len() > 4 {
                                    *key = serde_json::Value::String(format!("{}***", &k[..4]));
                                } else {
                                    *key = serde_json::Value::String("***".to_string());
                                }
                            }
                        }
                    }
                }
            }
            // Ollama key
            if let Some(ollama) = obj.get_mut("ollama") {
                if let Some(key) = ollama.get_mut("key") {
                    if let Some(k) = key.as_str() {
                        if k.len() > 4 {
                            *key = serde_json::Value::String(format!("{}***", &k[..4]));
                        } else {
                            *key = serde_json::Value::String("***".to_string());
                        }
                    }
                }
            }
            // Token
            if let Some(token) = obj.get_mut("token") {
                if let Some(t) = token.as_str() {
                    if t.len() > 4 {
                        *token = serde_json::Value::String(format!("{}***", &t[..4]));
                    } else {
                        *token = serde_json::Value::String("***".to_string());
                    }
                }
            }
            // Vector DB api_key
            if let Some(dbs) = obj.get_mut("vector_dbs") {
                if let Some(arr) = dbs.as_array_mut() {
                    for db in arr.iter_mut() {
                        if let Some(key) = db.get_mut("api_key") {
                            if let Some(k) = key.as_str() {
                                if k.len() > 4 {
                                    *key = serde_json::Value::String(format!("{}***", &k[..4]));
                                } else {
                                    *key = serde_json::Value::String("***".to_string());
                                }
                            }
                        }
                    }
                }
            }
        }
        masked
    }

    pub fn mask_system_config(&self, config: &serde_json::Value) -> serde_json::Value {
        let mut masked = config.clone();
        if let Some(obj) = masked.as_object_mut() {
            if let Some(token) = obj.get_mut("token") {
                if let Some(t) = token.as_str() {
                    if t.len() > 4 {
                        *token = serde_json::Value::String(format!("{}***", &t[..4]));
                    } else {
                        *token = serde_json::Value::String("***".to_string());
                    }
                }
            }
        }
        masked
    }
}

impl Default for ConfigManager {
    fn default() -> Self {
        Self::new("system.json")
    }
}