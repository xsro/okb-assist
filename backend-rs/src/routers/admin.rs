//! 管理路由。

use std::sync::Arc;

use axum::routing::get;
use axum::Extension;
use axum::response::Json;
use serde_json::json;

use crate::config::Settings;
use crate::database::Database;

pub fn router() -> axum::Router<()> {
    axum::Router::new()
        .route("/assist/api/admin/", get(status))
        .route("/assist/api/admin", get(status))
        .route("/assist/api/admin/services/", get(services_status))
        .route("/assist/api/admin/services", get(services_status))
        .route("/assist/api/admin/services/test/", get(test_services))
        .route("/assist/api/admin/services/test", get(test_services))
        .route("/assist/api/admin/reconnect/", get(reconnect_services))
        .route("/assist/api/admin/reconnect", get(reconnect_services))
        .route("/assist/api/admin/stats/", get(stats))
        .route("/assist/api/admin/stats", get(stats))
        .route("/assist/api/admin/migrate/", get(migrate))
        .route("/assist/api/admin/migrate", get(migrate))
        .route("/assist/api/admin/reset-index/", get(reset_index))
        .route("/assist/api/admin/reset-index", get(reset_index))
}

async fn status() -> Json<serde_json::Value> {
    Json(json!({
        "status": "running",
        "version": "0.1.0-rust",
        "backend": "rust"
    }))
}

async fn services_status(
    Extension(_settings): Extension<Arc<Settings>>,
) -> Json<serde_json::Value> {
    Json(json!({
        "services": {
            "mineru": {"status": "not_checked"},
            "ollama": {"status": "not_checked"},
            "vector_db": {"status": "not_checked"},
            "fastembed": {"status": "not_checked"}
        }
    }))
}

async fn test_services() -> Json<serde_json::Value> {
    Json(json!({"services": {}}))
}

async fn reconnect_services() -> Json<serde_json::Value> {
    Json(json!({"status": "ok"}))
}

async fn stats(
    axum::Extension(db): axum::Extension<Arc<Database>>,
) -> Json<serde_json::Value> {
    let total = sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM documents")
        .fetch_one(db.pool())
        .await
        .unwrap_or(0);

    let indexed = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM documents WHERE status = 'indexed'"
    )
        .fetch_one(db.pool())
        .await
        .unwrap_or(0);

    Json(json!({
        "total_documents": total,
        "indexed_documents": indexed,
        "parsing": 0,
        "extracting": 0,
        "indexing": 0,
        "error": 0,
        "uploaded": total - indexed,
        "markdown_done": 0,
        "meta_done": 0,
    }))
}

async fn migrate() -> Json<serde_json::Value> {
    Json(json!({"status": "ok", "migrated": 0}))
}

async fn reset_index() -> Json<serde_json::Value> {
    Json(json!({"status": "ok", "reset": 0}))
}