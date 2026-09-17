//! 配置路由。

use std::sync::Arc;

use axum::routing::{get, post, put};
use axum::Extension;
use axum::response::Json;
use serde_json::json;

use crate::config_manager::ConfigManager;
use crate::config::Settings;

pub fn router() -> axum::Router<()> {
    axum::Router::new()
        .route("/assist/api/config/", get(get_config))
        .route("/assist/api/config", get(get_config))
        .route("/assist/api/config/", put(update_config))
        .route("/assist/api/config", put(update_config))
        .route("/assist/api/config/reload/", get(reload_config))
        .route("/assist/api/config/reload", get(reload_config))
        .route("/assist/api/config/test/", post(test_service))
        .route("/assist/api/config/test", post(test_service))
        .route("/assist/api/config/system/", get(get_system_config))
        .route("/assist/api/config/system", get(get_system_config))
        .route("/assist/api/config/system/", put(update_system_config))
        .route("/assist/api/config/system", put(update_system_config))
        .route("/assist/api/config/vector-dbs/", get(list_vector_dbs))
        .route("/assist/api/config/vector-dbs", get(list_vector_dbs))
}

async fn get_config(
    Extension(cm): Extension<Arc<ConfigManager>>,
) -> Json<serde_json::Value> {
    let config = cm.get_config();
    Json(config)
}

async fn update_config(
    Extension(cm): Extension<Arc<ConfigManager>>,
    Extension(_settings): Extension<Arc<Settings>>,
    Json(body): Json<serde_json::Value>,
) -> Json<serde_json::Value> {
    let _ = cm.update_config(&body);
    Json(json!({"status": "ok"}))
}

async fn reload_config(
    Extension(cm): Extension<Arc<ConfigManager>>,
) -> Json<serde_json::Value> {
    cm.reload_config();
    Json(json!({"status": "ok"}))
}

async fn get_system_config(
    Extension(cm): Extension<Arc<ConfigManager>>,
) -> Json<serde_json::Value> {
    let sys = cm.get_system_config();
    Json(sys)
}

async fn update_system_config(
    Extension(cm): Extension<Arc<ConfigManager>>,
    Json(body): Json<serde_json::Value>,
) -> Json<serde_json::Value> {
    let _ = cm.update_system_config(&body);
    Json(json!({"status": "ok"}))
}

async fn list_vector_dbs(
    Extension(cm): Extension<Arc<ConfigManager>>,
) -> Json<serde_json::Value> {
    let dbs = cm.list_vector_dbs();
    Json(json!({"vector_dbs": dbs}))
}

async fn test_service(
    Extension(_cm): Extension<Arc<ConfigManager>>,
) -> Json<serde_json::Value> {
    Json(json!({"status": "ok"}))
}