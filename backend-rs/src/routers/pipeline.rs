//! 流水线路由。

use axum::routing::{get, post};
use axum::Extension;
use axum::response::Json;
use serde_json::json;

use crate::config::Settings;
use crate::database::Database;
use crate::models::{Document, DocStatus};

pub fn router() -> axum::Router<()> {
    axum::Router::new()
        .route("/assist/api/pipeline/", get(status))
        .route("/assist/api/pipeline", get(status))
        .route("/assist/api/pipeline/batch/start/", post(batch_start))
        .route("/assist/api/pipeline/batch/start", post(batch_start))
        .route("/assist/api/pipeline/batch/pause/", post(batch_pause))
        .route("/assist/api/pipeline/batch/pause", post(batch_pause))
        .route("/assist/api/pipeline/batch/resume/", post(batch_resume))
        .route("/assist/api/pipeline/batch/resume", post(batch_resume))
        .route("/assist/api/pipeline/batch/progress/", get(batch_progress))
        .route("/assist/api/pipeline/batch/progress", get(batch_progress))
        .route("/assist/api/pipeline/batch/reset/", post(batch_reset))
        .route("/assist/api/pipeline/batch/reset", post(batch_reset))
        .route("/assist/api/pipeline/batch/start-parse/", post(batch_start_parse))
        .route("/assist/api/pipeline/batch/start-parse", post(batch_start_parse))
        .route("/assist/api/pipeline/batch/start-extract/", post(batch_start_extract))
        .route("/assist/api/pipeline/batch/start-extract", post(batch_start_extract))
        .route("/assist/api/pipeline/batch/start-index/", post(batch_start_index))
        .route("/assist/api/pipeline/batch/start-index", post(batch_start_index))
        .route("/assist/api/pipeline/batch/start-full/", post(batch_start_full))
        .route("/assist/api/pipeline/batch/start-full", post(batch_start_full))
        .route("/assist/api/pipeline/parse/:id/", post(parse))
        .route("/assist/api/pipeline/parse/:id", post(parse))
        .route("/assist/api/pipeline/extract/:id/", post(extract))
        .route("/assist/api/pipeline/extract/:id", post(extract))
        .route("/assist/api/pipeline/index/:id/", post(index))
        .route("/assist/api/pipeline/index/:id", post(index))
        .route("/assist/api/pipeline/full/:id/", post(full))
        .route("/assist/api/pipeline/full/:id", post(full))
        .route("/assist/api/pipeline/reset/:id/", post(reset))
        .route("/assist/api/pipeline/reset/:id", post(reset))
        .route("/assist/api/pipeline/stop/:id/", post(stop))
        .route("/assist/api/pipeline/stop/:id", post(stop))
}

async fn status() -> Json<serde_json::Value> {
    Json(json!({"status": "ok"}))
}

async fn batch_start() -> Json<serde_json::Value> {
    Json(json!({"status": "ok", "message": "Use batch/start-full or individual endpoints"}))
}

async fn batch_pause() -> Json<serde_json::Value> {
    Json(json!({"status": "ok"}))
}

async fn batch_resume() -> Json<serde_json::Value> {
    Json(json!({"status": "ok"}))
}

async fn batch_progress() -> Json<serde_json::Value> {
    Json(json!({"progress": 0, "total": 0, "status": "idle"}))
}

async fn batch_reset() -> Json<serde_json::Value> {
    Json(json!({"status": "ok"}))
}

async fn batch_start_parse() -> Json<serde_json::Value> {
    Json(json!({"status": "ok"}))
}

async fn batch_start_extract() -> Json<serde_json::Value> {
    Json(json!({"status": "ok"}))
}

async fn batch_start_index() -> Json<serde_json::Value> {
    Json(json!({"status": "ok"}))
}

async fn batch_start_full() -> Json<serde_json::Value> {
    Json(json!({"status": "ok"}))
}

async fn parse() -> Json<serde_json::Value> {
    Json(json!({"status": "ok"}))
}

async fn extract() -> Json<serde_json::Value> {
    Json(json!({"status": "ok"}))
}

async fn index() -> Json<serde_json::Value> {
    Json(json!({"status": "ok"}))
}

async fn full() -> Json<serde_json::Value> {
    Json(json!({"status": "ok"}))
}

async fn reset() -> Json<serde_json::Value> {
    Json(json!({"status": "ok"}))
}

async fn stop() -> Json<serde_json::Value> {
    Json(json!({"status": "ok"}))
}