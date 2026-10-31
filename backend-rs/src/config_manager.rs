//! JSON 配置文件管理模块。
//!
//! 配置分为两个文件：
//! - config.json: 服务配置（MinerU、LLM、向量数据库），可通过前端修改
//! - system.json: 系统配置（并发数、数据库、上传目录、静态挂载等），仅限手动修改

use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, RwLock};

/// 不允许被 `save_config()`（即 `PUT /assist/api/config`）覆写的配置段。
///
/// `users` 段持有密码哈希与 token 哈希，只能通过 `users` 模块的专用端点与
/// [`ConfigManager::update_users_section`] 修改；否则前端配置表单回传旧快照就会
/// 覆盖账号或注入自定义 `password_hash`。
const PROTECTED_CONFIG_SECTIONS: &[&str] = &["users", "drive"];

/// 归一化并迁移服务配置的旧格式，使其符合“LLM 模型池 + 抽取专用模型”的新结构。
///
/// 支持的历史格式：
///
/// 1. 最早版本（≤ 0.9）——单一 Ollama 配置：
/// ```json
/// { "ollama": { "url": "...", "key": "...", "model": "..." } }
/// ```
/// 2. 中间版本——单一 LLM 配置 + 向量库 `embedding.source`：
/// ```json
/// { "llm": { "protocol": "ollama|openai", "url": "...", "key": "...", "model": "..." },
///   "vector_dbs": [{ "embedding": { "source": "ollama", "model": "..." } }] }
/// ```
///
/// 目标格式：
/// ```json
/// { "llm": { "models": [{ "name": "...", "protocol": "...", "url": "...", "key": "...", "model": "..." }] },
///   "llm_extract": { "from": "", "protocol": "", "url": "", "key": "", "model": "",
///                    "prompt": "", "max_input_chars": 6000, "max_tokens": 2048, "overwrite": true },
///   "vector_dbs": [{ "embedding": { "protocol": "...", "url": "...", "key": "...", "model": "..." } }] }
/// ```
///
/// 补齐规则：
/// - `llm` 的旧单对象形式被包装为 `llm.models` 的单元素数组（`name` = "default"）。
/// - `llm.models` 每项补齐 `name`（`model-N`）与 `protocol`（`ollama`）；`tool_mode`
///   缺省视为 `auto`（先试原生 function calling，不支持时自动降级为提示词工具协议）。
/// - `llm_extract` 的字段留空即表示“从 `from` 指定的模型项继承”（`from` 为空则取第一项），
///   因此迁移时保持全空，不复制取值。该段还承载与模型无关的抽取策略：
///   `prompt`（自定义提示词模板，空 = 内置默认）、`max_input_chars`、`max_tokens`、
///   `overwrite`（是否允许用抽取结果覆盖/清空已有元数据）。
/// - `vector_dbs[].embedding` 的 `protocol` 依次回退到 `embedding.source`、`llm.models[0].protocol`；
///   `url`/`key` 回退到 `llm.models[0]`；`model` 缺失时回退 `nomic-embed-text`。
///
/// 该函数作用于磁盘原始内容（与默认值合并之前），因此文件中的旧取值优先于默认值；
/// 迁移后旧的 `ollama` 键被移除。函数是幂等的。
fn migrate_legacy_service_config(mut raw: serde_json::Value) -> serde_json::Value {
    // 1) 归一化 llm：ollama 对象 / 单对象 llm → llm.models 数组
    let legacy_llm = match raw.get("llm") {
        // 新格式（已含 models 数组）无需处理
        Some(llm) if llm.get("models").is_some() => None,
        Some(llm) if llm.is_object() => Some(llm.clone()),
        _ => raw.get("ollama").cloned(),
    };
    if let Some(old) = legacy_llm {
        let mut entry = old.as_object().cloned().unwrap_or_default();
        entry
            .entry("name".to_string())
            .or_insert_with(|| serde_json::json!("default"));
        entry
            .entry("protocol".to_string())
            .or_insert_with(|| serde_json::json!("ollama"));
        raw["llm"] = serde_json::json!({ "models": [serde_json::Value::Object(entry)] });
    }
    if let Some(obj) = raw.as_object_mut() {
        obj.remove("ollama");
    }

    // 保证 llm.models 是数组且每项有 name / protocol。
    // 注意：文件未提供 llm 时不要凭空创建空池，否则会盖掉默认模型池
    // （deep_merge 对数组是整体替换）。
    if let Some(models) = raw.get_mut("llm").and_then(|l| l.get_mut("models")) {
        if !models.is_array() {
            *models = serde_json::json!([]);
        }
        if let Some(arr) = models.as_array_mut() {
            for (i, entry) in arr.iter_mut().enumerate() {
                if !entry.is_object() {
                    *entry = serde_json::json!({});
                }
                let obj = entry.as_object_mut().unwrap();
                obj.entry("name".to_string())
                    .or_insert_with(|| serde_json::json!(format!("model-{}", i + 1)));
                obj.entry("protocol".to_string())
                    .or_insert_with(|| serde_json::json!("ollama"));
            }
        }
    }

    // 1.5) users 段归一化：必须是对象，且 items 必须是数组
    //      （手改配置写成字符串/对象时不应让整个用户模块不可用）
    match raw.get_mut("users") {
        Some(users) if users.is_object() => {
            if !users.get("items").map(|v| v.is_array()).unwrap_or(false) {
                users["items"] = serde_json::json!([]);
            }
        }
        _ => {
            raw["users"] = default_users_section();
        }
    }

    // 2) embedding 段补齐：缺失的字段回退到 llm.models 的第一项
    //    （旧版本 embedding 完全依赖 Ollama 配置）
    let first_llm = raw
        .get("llm")
        .and_then(|l| l.get("models"))
        .and_then(|m| m.as_array())
        .and_then(|arr| arr.first())
        .cloned()
        .unwrap_or_else(|| serde_json::json!({}));
    let llm_protocol = first_llm["protocol"].as_str().unwrap_or("ollama").to_string();
    let llm_url = first_llm["url"].as_str().unwrap_or("").to_string();
    let llm_key = first_llm["key"].as_str().unwrap_or("").to_string();

    if let Some(dbs) = raw.get_mut("vector_dbs").and_then(|v| v.as_array_mut()) {
        for db in dbs.iter_mut() {
            if !db.get("embedding").map(|e| e.is_object()).unwrap_or(false) {
                db["embedding"] = serde_json::json!({});
            }
            let emb = match db.get_mut("embedding").and_then(|e| e.as_object_mut()) {
                Some(e) => e,
                None => continue,
            };
            // 旧字段 source（"ollama"/"openai"）作为 protocol 的回退
            let legacy_protocol = emb
                .get("source")
                .and_then(|v| v.as_str())
                .filter(|s| *s == "ollama" || *s == "openai")
                .map(String::from);
            if !emb.contains_key("protocol") {
                emb.insert(
                    "protocol".to_string(),
                    serde_json::json!(legacy_protocol.unwrap_or_else(|| llm_protocol.clone())),
                );
            }
            if !emb.contains_key("url") && !llm_url.is_empty() {
                emb.insert("url".to_string(), serde_json::json!(llm_url.clone()));
            }
            if !emb.contains_key("key") && !llm_key.is_empty() {
                emb.insert("key".to_string(), serde_json::json!(llm_key.clone()));
            }
            // model 无法从 LLM 段回退（语义不同），缺失时补默认值
            let model_missing = emb
                .get("model")
                .and_then(|v| v.as_str())
                .map(|s| s.trim().is_empty())
                .unwrap_or(true);
            if model_missing {
                emb.insert(
                    "model".to_string(),
                    serde_json::json!("nomic-embed-text"),
                );
            }
        }
    }

    raw
}

/// 默认服务配置
pub fn default_config() -> serde_json::Value {
    serde_json::json!({
        "base_url": "",
        "alias_expiration_hours": 1,
        "active_mineru": "",
        "mineru": [
            {
                "type": "local",
                "url": "http://127.0.0.1:8000",
                "key": "key",
                "max_tasks": 3,
                "task_timeout": 300,
                "tier": "standard"
            }
        ],
        "llm": {
            "models": [
                {
                    "name": "default",
                    "protocol": "ollama",
                    "url": "http://127.0.0.1:11434",
                    "key": "",
                    "model": "qwen3.5:9b",
                    "tool_mode": "auto"
                }
            ]
        },
        "llm_extract": {
            "from": "",
            "protocol": "",
            "url": "",
            "key": "",
            "model": "",
            "prompt": "",
            "max_input_chars": 6000,
            "max_tokens": 2048,
            "overwrite": true
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
                    "protocol": "ollama",
                    "url": "http://127.0.0.1:11434",
                    "key": "",
                    "model": "nomic-embed-text"
                }
            }
        ],
        "users": default_users_section(),
        "drive": default_drive_section()
    })
}

/// 默认的 `drive` 段（用户网盘策略）。
///
/// - `enabled`：总开关；
/// - `max_upload_mb`：单个文件上传上限；
/// - `default_quota_mb` / `quota_overrides`：配额（0 = 不限）；
/// - `public_folders`：管理员创建的公有文件夹（`read_role`/`write_role` 为角色阈值）。
pub fn default_drive_section() -> serde_json::Value {
    serde_json::json!({
        "enabled": true,
        "max_upload_mb": 200,
        "default_quota_mb": 0,
        "quota_overrides": {},
        "public_folders": []
    })
}

/// 默认的 `users` 段。
///
/// 字段含义见 `crate::users::UsersSection`（两处默认值必须一致，有单元测试守护）。
pub fn default_users_section() -> serde_json::Value {
    serde_json::json!({
        "version": 1,
        "allow_registration": true,
        "require_approval": true,
        "registration_role": "view-only",
        "session_ttl_hours": 168,
        "items": []
    })
}

/// 默认系统配置
pub fn default_system() -> serde_json::Value {
    serde_json::json!({
        "max_concurrent_tasks": 3,
        "database_url": "sqlite:///data/okb_assist.db",
        "cwd": "{system_dir}",
        "uploads_folder": "data/_uploads",
        // 静态目录挂载（把本地文件夹挂到 URL 前缀下），详见 crate::static_files
        "static_dirs": [],
        "markdown_path": "data/markdowns/{id}.md",
        "info_path": "data/markdowns/{id}.json",
        "crossref_path": "data/markdowns/{id}_crossref.json",
        "markdown_asset_path": "data/pdfs/{id}/{id}.zip",
        "pdf_path": "data/pdfs/{id}/{id}.pdf",
        "config_path": "config.json",
        "log_path": "okb-log.jsonl",
        "drive_root": "data/drive",
        "trusted_subnets": [],
        "trusted_subnet_enabled": false,
        "trusted_subnet_role": "admin"
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
    /// 写锁：串行化 config.json / system.json 的「读盘 → 改 → 原子写」序列，
    /// 避免「保存服务配置」与「保存 users 段」互相覆盖。
    write_lock: Arc<Mutex<()>>,
}

impl ConfigManager {
    /// 从 system.json 路径创建配置管理器。
    /// config.json 的路径从 system.json 的 config_path 字段读取，
    /// 若为相对路径则相对于 system.json 所在目录解析。
    /// cwd 支持 {system_dir} 变量替换，config_path 不支持变量替换。
    pub fn new(system_path: &str) -> Self {
        let system_path = PathBuf::from(system_path);
        let system = Self::load_json_file(&system_path, &default_system());
        let system_dir = system_path
            .parent()
            .unwrap_or(&PathBuf::from("."))
            .to_string_lossy()
            .to_string();
        let system_dir = if system_dir.is_empty() {
            ".".to_string()
        } else {
            system_dir
        };
        // 解析 cwd（支持 {system_dir} 替换）
        let cwd = system
            .get("cwd")
            .and_then(|v| v.as_str())
            .unwrap_or("{system_dir}")
            .to_string();
        let cwd = cwd.replace("{system_dir}", &system_dir);

        // 解析 config_path（无变量替换，为相对于 system.json 所在目录的路径）
        let config_path = system
            .get("config_path")
            .and_then(|v| v.as_str())
            .unwrap_or("config.json")
            .to_string();
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
            write_lock: Arc::new(Mutex::new(())),
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

    /// 测试用构造器：直接指定两个配置文件路径，不做 cwd / 路径推导处理。
    #[cfg(test)]
    pub fn for_tests(config_file: PathBuf, system_file: PathBuf) -> Self {
        Self {
            config_file,
            system_file,
            cwd: ".".to_string(),
            cache: Arc::new(RwLock::new(None)),
            system_cache: Arc::new(RwLock::new(None)),
            write_lock: Arc::new(Mutex::new(())),
        }
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
        Self::substitute_path_variables_str(template, &doc_id.to_string())
    }

    /// 同 [`Self::substitute_path_variables`]，但 `{id}` 为任意字符串
    /// （用于非数字 ID 的路径，例如问答会话文件 `data/chat_sessions/{id}.json`）。
    pub fn substitute_path_variables_str(template: &str, id: &str) -> String {
        let mut result = template.to_string();

        // 替换 {id}
        result = result.replace("{id}", id);

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
        // 原子写：先写同目录临时文件再 rename，避免写到一半崩溃/断电导致配置（现已含密码哈希）残缺
        let file_name = filepath
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "config.json".to_string());
        let tmp = filepath.with_file_name(format!(
            ".{}.tmp.{}.{}",
            file_name,
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        fs::write(&tmp, content)?;
        if let Err(e) = fs::rename(&tmp, filepath) {
            let _ = fs::remove_file(&tmp);
            return Err(e.into());
        }
        Ok(())
    }

    /// 直接从磁盘读取 config.json 原始内容（应用旧格式迁移，但**不与默认值合并**）。
    /// 供「只改某一段」的原子写入使用：未经合并的内容可直接写回，不会丢失未识别字段。
    fn read_raw_service_config(filepath: &PathBuf) -> serde_json::Value {
        match fs::read_to_string(filepath)
            .ok()
            .and_then(|content| serde_json::from_str::<serde_json::Value>(&content).ok())
        {
            Some(raw) => migrate_legacy_service_config(raw),
            None => default_config(),
        }
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
            *cache = Some(Self::load_service_config(&self.config_file));
        }
        cache.as_ref().unwrap().clone()
    }

    /// 读取 config.json：先对磁盘原始内容做旧格式迁移，再与默认值深合并。
    ///
    /// 迁移内容见 [`migrate_legacy_service_config`]（`ollama` → `llm`、
    /// 向量库 `embedding` 段补齐独立字段）。
    fn load_service_config(filepath: &PathBuf) -> serde_json::Value {
        let defaults = default_config();
        if !filepath.exists() {
            if let Some(parent) = filepath.parent() {
                fs::create_dir_all(parent).ok();
            }
            let defaults_str = serde_json::to_string_pretty(&defaults).unwrap_or_default();
            fs::write(filepath, defaults_str).ok();
            return defaults;
        }
        match fs::read_to_string(filepath)
            .ok()
            .and_then(|content| serde_json::from_str::<serde_json::Value>(&content).ok())
        {
            Some(raw) => Self::deep_merge(&defaults, &migrate_legacy_service_config(raw)),
            None => defaults,
        }
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
        let _guard = self.write_lock.lock().unwrap();
        let service_keys: Vec<String> = default_config()
            .as_object()
            .map(|o| o.keys().cloned().collect())
            .unwrap_or_default();
        let mut service_config = serde_json::Map::new();
        if let Some(obj) = config.as_object() {
            for (k, v) in obj {
                // 受保护段（users）不能由配置页覆写：跳过，随后从磁盘取回
                if PROTECTED_CONFIG_SECTIONS.contains(&k.as_str()) {
                    continue;
                }
                if service_keys.contains(k) {
                    service_config.insert(k.clone(), v.clone());
                }
            }
        }
        // 受保护段一律以磁盘上的值为准（不存在时用默认值），保证保存服务配置不会清空账号
        let on_disk = Self::read_raw_service_config(&self.config_file);
        for key in PROTECTED_CONFIG_SECTIONS {
            let value = on_disk
                .get(*key)
                .cloned()
                .unwrap_or_else(|| default_config()[*key].clone());
            service_config.insert(key.to_string(), value);
        }
        let result = serde_json::Value::Object(service_config);
        Self::write_json_file(&self.config_file, &result)?;
        *self.cache.write().unwrap() = Some(result);
        Ok(())
    }

    /// 原子地修改 `config.json` 的 `users` 段。
    ///
    /// 流程：加写锁 → 从磁盘重读原始内容 → 只改 `users` → 临时文件 + rename 写回。
    /// 闭包返回 `Err` 时**不会写盘**（`users` 模块据此实现「校验失败则不落盘」）。
    /// 写盘后缓存置空，下次读取重新与默认值合并。
    pub fn update_users_section<F>(&self, f: F) -> anyhow::Result<serde_json::Value>
    where
        F: FnOnce(&mut serde_json::Value) -> anyhow::Result<()>,
    {
        let _guard = self.write_lock.lock().unwrap();
        let mut raw = Self::read_raw_service_config(&self.config_file);
        if !raw.get("users").map(|v| v.is_object()).unwrap_or(false) {
            raw["users"] = default_users_section();
        }
        let section = raw.get_mut("users").expect("users 段已在上方补齐");
        if !section.get("items").map(|v| v.is_array()).unwrap_or(false) {
            section["items"] = serde_json::json!([]);
        }
        f(section)?;
        Self::write_json_file(&self.config_file, &raw)?;
        *self.cache.write().unwrap() = None;
        Ok(raw["users"].clone())
    }

    /// 原子地修改 `config.json` 的任意顶层段（`users` / `drive` 等）。
    ///
    /// 与 [`Self::update_users_section`] 相同：加写锁 → 从磁盘重读 → 只改该段 →
    /// 临时文件 + rename 写回；闭包返回 `Err` 时不写盘。
    pub fn update_section<F>(&self, key: &str, f: F) -> anyhow::Result<serde_json::Value>
    where
        F: FnOnce(&mut serde_json::Value) -> anyhow::Result<()>,
    {
        let _guard = self.write_lock.lock().unwrap();
        let mut raw = Self::read_raw_service_config(&self.config_file);
        if !raw.get(key).map(|v| v.is_object()).unwrap_or(false) {
            let default = default_config();
            raw[key] = default
                .get(key)
                .cloned()
                .unwrap_or_else(|| serde_json::json!({}));
        }
        let section = raw.get_mut(key).expect("该段已在上面补齐");
        f(section)?;
        Self::write_json_file(&self.config_file, &raw)?;
        *self.cache.write().unwrap() = None;
        Ok(raw[key].clone())
    }

    /// 原子地删除 `config.json` 的某个顶层键，返回被删除的值。
    ///
    /// 用于旧格式的一次性迁移（如 `permissions`）：先取值建用户，再把这个键从磁盘移除，
    /// 避免下次启动重复迁移。
    pub fn take_config_key(&self, key: &str) -> anyhow::Result<Option<serde_json::Value>> {
        let _guard = self.write_lock.lock().unwrap();
        let mut raw = Self::read_raw_service_config(&self.config_file);
        let removed = raw.as_object_mut().and_then(|obj| obj.remove(key));
        let Some(removed) = removed else {
            return Ok(None);
        };
        Self::write_json_file(&self.config_file, &raw)?;
        *self.cache.write().unwrap() = None;
        Ok(Some(removed))
    }

    /// `users` 段的只读摘要（供 `/assist/api/config` 返回，避免暴露密码哈希）
    pub fn users_summary(&self) -> serde_json::Value {
        let config = self.load_config();
        let users = &config["users"];
        let items = users["items"].as_array();
        serde_json::json!({
            "count": items.map(|a| a.len()).unwrap_or(0),
            "pending": items
                .map(|a| a.iter().filter(|u| u["status"] == "pending").count())
                .unwrap_or(0),
            "allow_registration": users["allow_registration"].as_bool().unwrap_or(true),
        })
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

    /// 更新系统配置（仅允许修改部分字段）。
    /// 写入文件时从磁盘重新读取以保留文件的 key 顺序。
    pub fn update_system_config(&self, config: &serde_json::Value) -> anyhow::Result<()> {
        let _guard = self.write_lock.lock().unwrap();
        // 从磁盘读取原始 JSON，保留文件中 key 的顺序
        let file_content = fs::read_to_string(&self.system_file).unwrap_or_default();
        let current = if file_content.is_empty() {
            default_system()
        } else {
            serde_json::from_str(&file_content).unwrap_or_else(|_| default_system())
        };
        let merged = Self::deep_merge(&current, config);
        Self::write_json_file(&self.system_file, &merged)?;
        *self.system_cache.write().unwrap() = Some(merged);
        Ok(())
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
}

impl Default for ConfigManager {
    fn default() -> Self {
        Self::new("system.json")
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    fn defaults() -> serde_json::Value {
        default_config()
    }

    #[test]
    fn default_config_has_model_pool_and_extract_section() {
        let d = defaults();
        assert!(d.get("ollama").is_none(), "默认配置不应再含 ollama 键");
        // LLM 模型池
        let models = d["llm"]["models"].as_array().expect("llm.models 应为数组");
        assert_eq!(models.len(), 1);
        assert_eq!(models[0]["protocol"], "ollama");
        assert_eq!(models[0]["url"], "http://127.0.0.1:11434");
        // 抽取专用配置：默认全空 = 继承模型池第一项
        for key in ["from", "protocol", "url", "key", "model", "prompt"] {
            assert_eq!(d["llm_extract"][key], "", "llm_extract.{} 默认应为空", key);
        }
        // 抽取策略默认值
        assert_eq!(d["llm_extract"]["max_input_chars"], 6000);
        assert_eq!(d["llm_extract"]["max_tokens"], 2048);
        assert_eq!(d["llm_extract"]["overwrite"], true);
        // embedding 独立配置
        assert_eq!(d["vector_dbs"][0]["embedding"]["protocol"], "ollama");
        assert!(d["vector_dbs"][0]["embedding"].get("url").is_some());
    }

    #[test]
    fn migrates_earliest_ollama_shape() {
        let raw = serde_json::json!({
            "ollama": {"url": "http://custom:11434", "key": "k1", "model": "m1"},
            "vector_dbs": [
                {"id": "default", "embedding": {"source": "ollama", "model": "nomic-embed-text"}}
            ]
        });
        let out = migrate_legacy_service_config(raw);
        assert!(out.get("ollama").is_none(), "旧键应被移除");
        let models = out["llm"]["models"].as_array().unwrap();
        assert_eq!(models.len(), 1);
        assert_eq!(models[0]["name"], "default");
        assert_eq!(models[0]["protocol"], "ollama");
        assert_eq!(models[0]["url"], "http://custom:11434");
        assert_eq!(models[0]["key"], "k1");
        assert_eq!(models[0]["model"], "m1");
        // embedding 段按模型池第一项补齐
        assert_eq!(out["vector_dbs"][0]["embedding"]["protocol"], "ollama");
        assert_eq!(out["vector_dbs"][0]["embedding"]["url"], "http://custom:11434");
        assert_eq!(out["vector_dbs"][0]["embedding"]["key"], "k1");
        assert_eq!(out["vector_dbs"][0]["embedding"]["model"], "nomic-embed-text");
    }

    #[test]
    fn migrates_single_llm_object_into_model_pool() {
        let raw = serde_json::json!({
            "llm": {"protocol": "openai", "url": "https://api.openai.com", "key": "sk-x", "model": "gpt-4o-mini"},
            "vector_dbs": [{"embedding": {"protocol": "openai", "model": "text-embedding-3-small"}}]
        });
        let out = migrate_legacy_service_config(raw);
        let models = out["llm"]["models"].as_array().unwrap();
        assert_eq!(models.len(), 1);
        assert_eq!(models[0]["protocol"], "openai");
        assert_eq!(models[0]["model"], "gpt-4o-mini");
        // embedding 未显式配置的 url/key 由模型池第一项补齐
        assert_eq!(out["vector_dbs"][0]["embedding"]["protocol"], "openai");
        assert_eq!(out["vector_dbs"][0]["embedding"]["url"], "https://api.openai.com");
        assert_eq!(out["vector_dbs"][0]["embedding"]["key"], "sk-x");
        assert_eq!(
            out["vector_dbs"][0]["embedding"]["model"],
            "text-embedding-3-small"
        );
    }

    #[test]
    fn keeps_existing_model_pool_untouched() {
        let raw = serde_json::json!({
            "llm": {"models": [
                {"name": "本地", "protocol": "ollama", "url": "http://a:11434", "key": "", "model": "qwen"},
                {"name": "云端", "protocol": "openai", "url": "https://api.openai.com", "key": "sk", "model": "gpt"}
            ]},
            "llm_extract": {"from": "云端", "model": "gpt-4o-mini"}
        });
        let out = migrate_legacy_service_config(raw);
        let models = out["llm"]["models"].as_array().unwrap();
        assert_eq!(models.len(), 2);
        assert_eq!(models[0]["model"], "qwen");
        assert_eq!(models[1]["name"], "云端");
        // 已有的 llm_extract 不被覆盖（未提供的字段留给默认值合并）
        assert_eq!(out["llm_extract"]["from"], "云端");
        assert_eq!(out["llm_extract"]["model"], "gpt-4o-mini");
        assert!(out["llm_extract"].get("url").is_none());
    }

    #[test]
    fn fills_model_pool_entry_defaults() {
        let raw = serde_json::json!({
            "llm": {"models": [{"model": "qwen"}, {"url": "http://b:11434", "model": "llama"}]}
        });
        let out = migrate_legacy_service_config(raw);
        let models = out["llm"]["models"].as_array().unwrap();
        assert_eq!(models[0]["name"], "model-1");
        assert_eq!(models[0]["protocol"], "ollama");
        assert_eq!(models[1]["name"], "model-2");
        assert_eq!(models[1]["protocol"], "ollama");
    }

    #[test]
    fn migration_uses_legacy_embedding_source_as_protocol() {
        let raw = serde_json::json!({
            "ollama": {"url": "http://h:11434", "key": "", "model": "m"},
            "vector_dbs": [{"embedding": {"source": "openai", "model": "e"}}]
        });
        let out = migrate_legacy_service_config(raw);
        assert_eq!(out["vector_dbs"][0]["embedding"]["protocol"], "openai");
    }

    #[test]
    fn migration_is_idempotent() {
        let raw = serde_json::json!({
            "ollama": {"url": "http://h:11434", "key": "", "model": "m"},
            "vector_dbs": [{"embedding": {"source": "ollama", "model": "e"}}]
        });
        let once = migrate_legacy_service_config(raw);
        let twice = migrate_legacy_service_config(once.clone());
        assert_eq!(once, twice);
    }

    #[test]
    fn deep_merge_fills_missing_sections_from_defaults() {
        let raw = migrate_legacy_service_config(serde_json::json!({
            "vector_dbs": [{"id": "default"}]
        }));
        let merged = ConfigManager::deep_merge(&defaults(), &raw);
        // 文件未提供 llm 时，由默认值提供模型池
        assert_eq!(merged["llm"]["models"][0]["protocol"], "ollama");
        assert_eq!(merged["llm_extract"]["model"], "");
        assert_eq!(merged["vector_dbs"][0]["embedding"]["model"], "nomic-embed-text");
    }

    // ── users 段：写入隔离、原子写、迁移归一化 ──────────────

    fn temp_cm() -> (tempfile::TempDir, ConfigManager) {
        let dir = tempfile::tempdir().unwrap();
        let cm = ConfigManager::for_tests(
            dir.path().join("config.json"),
            dir.path().join("system.json"),
        );
        (dir, cm)
    }

    #[test]
    fn default_config_has_users_section() {
        let d = defaults();
        assert!(d["users"].is_object());
        assert_eq!(d["users"]["version"], 1);
        assert_eq!(d["users"]["session_ttl_hours"], 168);
        assert_eq!(d["users"]["items"], serde_json::json!([]));
    }

    #[test]
    fn update_users_section_only_touches_users() {
        let (_dir, cm) = temp_cm();
        cm.save_config(&serde_json::json!({"base_url": "http://keep.test/"}))
            .unwrap();

        cm.update_users_section(|users| {
            users["items"] = serde_json::json!([{"id": "u_1", "username": "alice"}]);
            Ok(())
        })
        .unwrap();

        let cfg = cm.get_service_config();
        assert_eq!(cfg["base_url"], "http://keep.test/", "其他段不应被改");
        assert_eq!(cfg["users"]["items"][0]["username"], "alice");
        // 未手写的标志回退默认值
        assert_eq!(cfg["users"]["version"], 1);
        assert_eq!(cfg["users"]["allow_registration"], true);
    }

    #[test]
    fn update_users_section_skips_write_on_error() {
        let (dir, cm) = temp_cm();
        cm.update_users_section(|users| {
            users["items"] = serde_json::json!([{"id": "u_1", "username": "alice"}]);
            Ok(())
        })
        .unwrap();
        let before = std::fs::read_to_string(dir.path().join("config.json")).unwrap();

        let err = cm.update_users_section(|users| {
            users["items"] = serde_json::json!([]);
            anyhow::bail!("拒绝写入")
        });
        assert!(err.is_err());
        let after = std::fs::read_to_string(dir.path().join("config.json")).unwrap();
        assert_eq!(before, after, "闭包报错时不得写盘");
    }

    #[test]
    fn save_config_cannot_overwrite_users() {
        let (_dir, cm) = temp_cm();
        cm.update_users_section(|users| {
            users["items"] = serde_json::json!([{"id": "u_1", "username": "alice", "role": "admin"}]);
            Ok(())
        })
        .unwrap();

        // 配置页如果回传一个伪造的 users 段，必须被忽略
        cm.save_config(&serde_json::json!({
            "base_url": "http://x.test/",
            "users": {"items": [{"id": "u_evil", "username": "evil", "role": "admin"}]}
        }))
        .unwrap();

        cm.reload_config();
        let cfg = cm.get_service_config();
        assert_eq!(cfg["users"]["items"].as_array().unwrap().len(), 1);
        assert_eq!(cfg["users"]["items"][0]["username"], "alice");
        assert_eq!(cfg["base_url"], "http://x.test/");
    }

    #[test]
    fn write_json_file_leaves_no_temp_files() {
        let (dir, cm) = temp_cm();
        cm.save_config(&serde_json::json!({"base_url": "http://a.test/"}))
            .unwrap();
        cm.update_users_section(|users| {
            users["version"] = serde_json::json!(2);
            Ok(())
        })
        .unwrap();
        let leftovers: Vec<String> = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().to_string())
            .filter(|n| n.contains(".tmp"))
            .collect();
        assert!(leftovers.is_empty(), "不应残留临时文件: {:?}", leftovers);
        assert!(dir.path().join("config.json").exists());
    }

    #[test]
    fn migration_normalizes_users_section() {
        // users 被写成字符串 → 回退默认段
        let raw = migrate_legacy_service_config(serde_json::json!({"users": "oops"}));
        assert_eq!(raw["users"], default_users_section());

        // users.items 不是数组 → 补成空数组，其余字段保留
        let raw = migrate_legacy_service_config(serde_json::json!({
            "users": {"version": 2, "items": {"bad": true}}
        }));
        assert_eq!(raw["users"]["version"], 2);
        assert_eq!(raw["users"]["items"], serde_json::json!([]));

        // 缺 users 段 → 补默认值（幂等）
        let once = migrate_legacy_service_config(serde_json::json!({}));
        assert_eq!(once["users"], default_users_section());
        assert_eq!(migrate_legacy_service_config(once.clone()), once);
    }

    #[test]
    fn users_summary_counts_and_pending() {
        let (_dir, cm) = temp_cm();
        cm.update_users_section(|users| {
            users["items"] = serde_json::json!([
                {"id": "u_1", "username": "a", "status": "active"},
                {"id": "u_2", "username": "b", "status": "pending"}
            ]);
            Ok(())
        })
        .unwrap();
        let summary = cm.users_summary();
        assert_eq!(summary["count"], 2);
        assert_eq!(summary["pending"], 1);
        assert_eq!(summary["allow_registration"], true);
    }
}
