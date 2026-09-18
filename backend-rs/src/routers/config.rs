//! 配置路由。
//!
//! 与 Python 版 `app/routers/config.py` 保持 API 契约一致，
//! 并补充前端 `frontend/src/api/config.ts` 的 `/test-connection` 端点。

use std::sync::Arc;

use axum::http::StatusCode;
use axum::response::{IntoResponse, Json, Response};
use axum::routing::{get, post, put};
use axum::Extension;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::config::Settings;
use crate::config_manager::ConfigManager;
use crate::services::mineru::MineruType;

pub fn router() -> axum::Router<()> {
    axum::Router::new()
        .route("/assist/api/config/", get(get_config))
        .route("/assist/api/config", get(get_config))
        .route("/assist/api/config/", put(update_config))
        .route("/assist/api/config", put(update_config))
        .route("/assist/api/config/reload/", get(reload_config))
        .route("/assist/api/config/reload", get(reload_config))
        .route("/assist/api/config/reload/", post(reload_config))
        .route("/assist/api/config/reload", post(reload_config))
        .route("/assist/api/config/test/", post(test_service))
        .route("/assist/api/config/test", post(test_service))
        .route("/assist/api/config/test-connection/", post(test_connection))
        .route("/assist/api/config/test-connection", post(test_connection))
        .route("/assist/api/config/system/", get(get_system_config))
        .route("/assist/api/config/system", get(get_system_config))
        .route("/assist/api/config/system/", put(update_system_config))
        .route("/assist/api/config/system", put(update_system_config))
        .route("/assist/api/config/vector-dbs/", get(list_vector_dbs))
        .route("/assist/api/config/vector-dbs", get(list_vector_dbs))
}

async fn get_config(Extension(cm): Extension<Arc<ConfigManager>>) -> Json<Value> {
    let config = cm.get_config();
    Json(cm.mask_sensitive(&config))
}

async fn get_system_config(Extension(cm): Extension<Arc<ConfigManager>>) -> Json<Value> {
    let config = cm.get_system_config();
    Json(cm.mask_system_config(&config))
}

async fn update_config(
    Extension(cm): Extension<Arc<ConfigManager>>,
    Json(body): Json<Value>,
) -> Response {
    let mut config = cm.get_service_config();

    // 合并 mineru / ollama 字段
    if let Some(mineru) = body.get("mineru").and_then(|v| v.as_object()) {
        let base = config
            .get("mineru")
            .and_then(|v| v.as_object())
            .cloned()
            .unwrap_or_default();
        let mut merged = serde_json::Map::new();
        merged.extend(base);
        merged.extend(mineru.clone());
        config["mineru"] = Value::Object(merged);
    }

    if let Some(ollama) = body.get("ollama").and_then(|v| v.as_object()) {
        let base = config
            .get("ollama")
            .and_then(|v| v.as_object())
            .cloned()
            .unwrap_or_default();
        let mut merged = serde_json::Map::new();
        merged.extend(base);
        merged.extend(ollama.clone());
        config["ollama"] = Value::Object(merged);
    }

    if let Some(vector_dbs) = body.get("vector_dbs").and_then(|v| v.as_array()) {
        // 校验每个 vector_db 必须有 id 和 type
        for db in vector_dbs {
            let id = db.get("id").and_then(|v| v.as_str()).unwrap_or("");
            let db_type = db.get("type").and_then(|v| v.as_str()).unwrap_or("");
            if id.is_empty() {
                return (StatusCode::BAD_REQUEST, Json(json!({"detail": "向量数据库配置必须包含 id 字段"})))
                    .into_response();
            }
            if db_type.is_empty() {
                return (StatusCode::BAD_REQUEST, Json(json!({"detail": "向量数据库配置必须包含 type 字段"})))
                    .into_response();
            }
            if !["qdrant", "milvus", "chroma"].contains(&db_type) {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(json!({"detail": format!("不支持的向量数据库类型: {}", db_type)})),
                )
                    .into_response();
            }
        }
        config["vector_dbs"] = Value::Array(vector_dbs.clone());
    }

    if let Err(e) = cm.save_config(&config) {
        return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"detail": e.to_string()})))
            .into_response();
    }

    let saved = cm.get_config();
    Json(json!({"detail": "配置已保存", "config": cm.mask_sensitive(&saved)})).into_response()
}

async fn reload_config(Extension(cm): Extension<Arc<ConfigManager>>) -> Json<Value> {
    let config = cm.reload_config();
    Json(json!({"detail": "配置已重新加载", "config": cm.mask_sensitive(&config)}))
}

async fn update_system_config(
    Extension(cm): Extension<Arc<ConfigManager>>,
    Json(body): Json<Value>,
) -> Response {
    if let Err(e) = cm.update_system_config(&body) {
        return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"detail": e.to_string()})))
            .into_response();
    }
    Json(json!({"status": "ok"})).into_response()
}

async fn list_vector_dbs(Extension(cm): Extension<Arc<ConfigManager>>) -> Json<Value> {
    let dbs = cm.list_vector_dbs();
    Json(json!({"vector_dbs": dbs}))
}

// ── 连接测试 ──

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
pub struct TestServiceRequest {
    #[serde(rename = "type")]
    pub service_type: String,
    pub url: Option<String>,
    pub key: Option<String>,
    pub model: Option<String>,
    pub embed_model: Option<String>,
    pub collection: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct TestConnectionRequest {
    pub service: String,
}

async fn test_service(Json(req): Json<TestServiceRequest>) -> Response {
    match req.service_type.as_str() {
        "mineru" => {
            let url = req.url.unwrap_or_default().trim_end_matches('/').to_string();
            let mineru_type = req.key.clone().unwrap_or_else(|| "local".to_string());
            // 实际上 type 字段应该从 mineru_type 传入，这里从 key 字段临时读取
            // 前端提交的 TestServiceRequest 中 key 字段实际存储的是 mineru type
            Json(test_mineru(&url, "", &mineru_type).await).into_response()
        }
        "ollama" => {
            let url = req.url.unwrap_or_default().trim_end_matches('/').to_string();
            Json(test_ollama(&url, req.model.unwrap_or_default().as_str()).await).into_response()
        }
        "qdrant" => {
            let url = req.url.unwrap_or_default().trim_end_matches('/').to_string();
            Json(test_qdrant(&url).await).into_response()
        }
        "milvus" | "chroma" => Json(json!({
            "status": "unsupported",
            "detail": format!("{} 暂未实现连接测试", req.service_type),
        }))
        .into_response(),
        other => (
            StatusCode::BAD_REQUEST,
            Json(json!({"detail": format!("未知的服务类型: {}", other)})),
        )
            .into_response(),
    }
}

async fn test_connection(
    Extension(settings): Extension<Arc<Settings>>,
    Json(req): Json<TestConnectionRequest>,
) -> Response {
    match req.service.as_str() {
        "mineru" => {
            let url = settings.mineru_url();
            let key = settings.mineru_key();
            let mineru_type = settings.mineru_type();
            Json(test_mineru(&url, &key, &mineru_type).await).into_response()
        }
        "ollama" => Json(test_ollama(&settings.ollama_url(), &settings.ollama_model()).await).into_response(),
        "qdrant" => Json(test_qdrant(&settings.qdrant_url()).await).into_response(),
        "fastembed" => Json(test_fastembed(&settings.fastembed_url()).await).into_response(),
        other => (
            StatusCode::BAD_REQUEST,
            Json(json!({"detail": format!("未知的服务类型: {}", other)})),
        )
            .into_response(),
    }
}

async fn test_mineru(url: &str, key: &str, mineru_type: &str) -> Value {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()
        .unwrap_or_default();

    match MineruType::from_str(mineru_type) {
        MineruType::Local => {
            let mut req = client.get(format!("{}/health", url));
            if !key.is_empty() {
                req = req.bearer_auth(key);
            }
            match req.send().await {
                Ok(resp) if resp.status().is_success() => {
                    let data = resp.json::<Value>().await.unwrap_or(json!({}));
                    json!({
                        "status": "connected",
                        "detail": "MinerU 本地服务连接成功",
                        "version": data.get("version").unwrap_or(&json!("unknown")),
                    })
                }
                Ok(resp) => json!({"status": "error", "detail": format!("HTTP {}", resp.status())}),
                Err(e) => json!({"status": "disconnected", "detail": format!("无法连接到 {}: {}", url, e)}),
            }
        }
        MineruType::Official => {
            // 官方精准解析 API：通过请求 API 根路径验证连通性
            let resp = client.get(url).bearer_auth(key).send().await;
            match resp {
                Ok(resp) if resp.status().is_success() || resp.status().as_u16() == 404 => {
                    json!({
                        "status": "connected",
                        "detail": "MinerU 官方 API 连接成功",
                        "version": json!("v4"),
                    })
                }
                Ok(resp) => json!({"status": "error", "detail": format!("HTTP {}", resp.status())}),
                Err(e) => json!({"status": "disconnected", "detail": format!("无法连接到官方 API {}: {}", url, e)}),
            }
        }
    }
}

async fn test_ollama(url: &str, model: &str) -> Value {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()
        .unwrap_or_default();
    match client.get(format!("{}/api/tags", url)).send().await {
        Ok(resp) if resp.status().is_success() => {
            let data = resp.json::<Value>().await.unwrap_or(json!({}));
            let models: Vec<String> = data["models"]
                .as_array()
                .map(|arr| {
                    arr.iter()
                        .filter_map(|m| m["name"].as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default();
            let model_found = model.is_empty() || models.iter().any(|m| m.contains(model));
            json!({
                "status": "connected",
                "detail": "Ollama 连接成功",
                "models": models,
                "model_available": model_found,
            })
        }
        Ok(resp) => json!({"status": "error", "detail": format!("HTTP {}", resp.status())}),
        Err(e) => json!({"status": "disconnected", "detail": format!("无法连接到 {}: {}", url, e)}),
    }
}

async fn test_qdrant(url: &str) -> Value {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()
        .unwrap_or_default();
    match client.get(format!("{}/collections", url)).send().await {
        Ok(resp) if resp.status().is_success() => {
            let data = resp.json::<Value>().await.unwrap_or(json!({}));
            let collections: Vec<String> = data["result"]["collections"]
                .as_array()
                .map(|arr| {
                    arr.iter()
                        .filter_map(|c| c["name"].as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default();
            json!({
                "status": "connected",
                "detail": "Qdrant 连接成功",
                "collections": collections,
            })
        }
        Ok(resp) => json!({"status": "error", "detail": format!("HTTP {}", resp.status())}),
        Err(e) => json!({"status": "disconnected", "detail": format!("无法连接到 {}: {}", url, e)}),
    }
}

async fn test_fastembed(url: &str) -> Value {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()
        .unwrap_or_default();
    match client.get(format!("{}/health", url)).send().await {
        Ok(resp) if resp.status().is_success() => {
            json!({"status": "connected", "detail": "FastEmbed 连接成功"})
        }
        Ok(resp) => json!({"status": "error", "detail": format!("HTTP {}", resp.status())}),
        Err(e) => json!({"status": "disconnected", "detail": format!("无法连接到 {}: {}", url, e)}),
    }
}
