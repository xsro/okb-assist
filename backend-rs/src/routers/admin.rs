//! 管理路由。
//!
//! 与 Python 版 `app/routers/admin.py` 保持 API 契约一致，
//! 同时补充前端 `frontend/src/api/admin.ts` 所需的字段。

use std::sync::Arc;

use axum::extract::{Path, Query};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Json, Response};
use axum::routing::{get, post};
use axum::Extension;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::config::Settings;
use crate::database::Database;
use crate::models::{DocStatus, Document};
use crate::paths;
use crate::services::vector_db::get_vector_db;
use crate::utils::calculate_file_hash;

pub fn router() -> axum::Router<()> {
    axum::Router::new()
        .route("/assist/api/admin/", get(status))
        .route("/assist/api/admin", get(status))
        .route("/assist/api/admin/services/", get(services_status))
        .route("/assist/api/admin/services", get(services_status))
        .route("/assist/api/admin/services/status/", get(services_status))
        .route("/assist/api/admin/services/status", get(services_status))
        .route("/assist/api/admin/services/test/", get(services_status))
        .route("/assist/api/admin/services/test", get(services_status))
        .route("/assist/api/admin/reconnect/", post(reconnect_services))
        .route("/assist/api/admin/reconnect", post(reconnect_services))
        .route("/assist/api/admin/stats/", get(stats))
        .route("/assist/api/admin/stats", get(stats))
        .route("/assist/api/admin/mineru/tasks/", get(mineru_tasks))
        .route("/assist/api/admin/mineru/tasks", get(mineru_tasks))
        .route("/assist/api/admin/qdrant/collections/", get(qdrant_collections))
        .route("/assist/api/admin/qdrant/collections", get(qdrant_collections))
        .route("/assist/api/admin/qdrant/point/:point_id/", get(qdrant_point))
        .route("/assist/api/admin/qdrant/point/:point_id", get(qdrant_point))
        .route("/assist/api/admin/db/migrate/", post(migrate))
        .route("/assist/api/admin/db/migrate", post(migrate))
        .route("/assist/api/admin/recalculate-hashes/", post(recalculate_hashes))
        .route("/assist/api/admin/recalculate-hashes", post(recalculate_hashes))
        .route("/assist/api/admin/reset-index/", post(reset_index))
        .route("/assist/api/admin/reset-index", post(reset_index))
}

async fn status() -> Json<Value> {
    Json(json!({
        "status": "running",
        "version": "0.1.0-rust",
        "backend": "rust"
    }))
}

/// 检查 MinerU 健康
async fn check_mineru(settings: &Settings) -> Value {
    let url = settings.mineru_url();
    let key = settings.mineru_key();
    let mut item = json!({"status": "unknown", "url": url});
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()
        .unwrap_or_default();
    let mut req = client.get(format!("{}/health", url));
    if !key.is_empty() {
        req = req.bearer_auth(key);
    }
    match req.send().await {
        Ok(resp) if resp.status().is_success() => {
            if let Ok(health) = resp.json::<Value>().await {
                item["status"] = json!("connected");
                item["version"] = health.get("version").cloned().unwrap_or(json!("unknown"));
                item["queued_tasks"] = health.get("queued_tasks").cloned().unwrap_or(json!(0));
                item["processing_tasks"] = health.get("processing_tasks").cloned().unwrap_or(json!(0));
                item["completed_tasks"] = health.get("completed_tasks").cloned().unwrap_or(json!(0));
                item["failed_tasks"] = health.get("failed_tasks").cloned().unwrap_or(json!(0));
                item["max_concurrent"] = health.get("max_concurrent_requests").cloned().unwrap_or(json!(0));
            } else {
                item["status"] = json!("connected");
            }
        }
        Ok(resp) => {
            item["status"] = json!("error");
            item["error"] = json!(format!("HTTP {}", resp.status()));
        }
        Err(e) => {
            item["status"] = json!("disconnected");
            item["error"] = json!(e.to_string());
        }
    }
    item
}

/// 检查 Ollama 健康
async fn check_ollama(settings: &Settings) -> Value {
    let url = settings.ollama_url();
    let mut item = json!({"status": "unknown", "url": url});
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()
        .unwrap_or_default();
    match client.get(format!("{}/api/tags", url)).send().await {
        Ok(resp) if resp.status().is_success() => {
            if let Ok(data) = resp.json::<Value>().await {
                item["status"] = json!("connected");
                item["models"] = data.get("models").cloned().unwrap_or(json!([]));
            } else {
                item["status"] = json!("connected");
            }
        }
        Ok(resp) => {
            item["status"] = json!("error");
            item["error"] = json!(format!("HTTP {}", resp.status()));
        }
        Err(e) => {
            item["status"] = json!("disconnected");
            item["error"] = json!(e.to_string());
        }
    }
    item
}

/// 检查 FastEmbed 健康
async fn check_fastembed(settings: &Settings) -> Value {
    let url = settings.fastembed_url();
    let mut item = json!({"status": "unknown", "url": url});
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()
        .unwrap_or_default();
    match client.get(format!("{}/health", url)).send().await {
        Ok(resp) if resp.status().is_success() => {
            if let Ok(health) = resp.json::<Value>().await {
                item["status"] = json!("connected");
                item["default_model"] = health.get("default_model").cloned().unwrap_or(json!(""));
                item["loaded_models"] = health.get("loaded_models").cloned().unwrap_or(json!([]));
            } else {
                item["status"] = json!("connected");
            }
        }
        Ok(resp) => {
            item["status"] = json!("error");
            item["error"] = json!(format!("HTTP {}", resp.status()));
        }
        Err(e) => {
            item["status"] = json!("disconnected");
            item["error"] = json!(e.to_string());
        }
    }
    item
}

async fn services_status(
    Extension(settings): Extension<Arc<Settings>>,
) -> Json<Value> {
    let cm = crate::config_manager::ConfigManager::new();
    let cfg = cm.get_config();

    let (mineru, ollama, fastembed) = tokio::join!(
        check_mineru(&settings),
        check_ollama(&settings),
        check_fastembed(&settings),
    );

    let mut result = json!({
        "mineru": mineru,
        "ollama": ollama,
        "fastembed": fastembed,
    });

    // 检查所有向量数据库
    let mut vector_dbs = Vec::new();
    let mut qdrant_top: Option<Value> = None;
    if let Some(dbs) = cfg.get("vector_dbs").and_then(|v| v.as_array()) {
        for db_cfg in dbs {
            let mut db_info = json!({
                "id": db_cfg.get("id"),
                "name": db_cfg.get("name"),
                "type": db_cfg.get("type"),
                "enabled": db_cfg.get("enabled"),
                "url": db_cfg.get("url"),
            });
            let enabled = db_cfg.get("enabled").and_then(|e| e.as_bool()).unwrap_or(false);
            let db_id = db_cfg.get("id").and_then(|i| i.as_str()).unwrap_or("").to_string();
            if enabled {
                match get_vector_db(Some(&db_id)) {
                    Ok(adapter) => match adapter.health_check().await {
                        Ok(health) => {
                            for (k, v) in health.as_object().unwrap_or(&serde_json::Map::new()) {
                                db_info[k] = v.clone();
                            }
                        }
                        Err(e) => {
                            db_info["status"] = json!("error");
                            db_info["error"] = json!(e.to_string());
                        }
                    },
                    Err(e) => {
                        db_info["status"] = json!("error");
                        db_info["error"] = json!(e.to_string());
                    }
                }
            } else {
                db_info["status"] = json!("disabled");
            }

            if db_cfg.get("type").and_then(|t| t.as_str()) == Some("qdrant")
                && enabled
                && qdrant_top.is_none()
            {
                qdrant_top = Some(db_info.clone());
            }
            vector_dbs.push(db_info);
        }
    }
    result["vector_dbs"] = json!(vector_dbs);
    result["qdrant"] = qdrant_top.unwrap_or_else(|| json!({"status": "not_configured"}));

    Json(result)
}

async fn reconnect_services(
    Extension(cm): Extension<Arc<ConfigManager>>,
) -> Json<Value> {
    cm.reload_config();
    Json(json!({"status": "ok", "detail": "配置已重新加载"}))
}

use crate::config_manager::ConfigManager;

async fn stats(
    Extension(db): Extension<Arc<Database>>,
    Extension(settings): Extension<Arc<Settings>>,
) -> Json<Value> {
    let doc_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM documents")
        .fetch_one(db.pool())
        .await
        .unwrap_or(0);

    let mut status_counts = serde_json::Map::new();
    for status in [
        DocStatus::Uploaded,
        DocStatus::Parsing,
        DocStatus::MarkdownDone,
        DocStatus::Extracting,
        DocStatus::MetaDone,
        DocStatus::Indexing,
        DocStatus::Indexed,
        DocStatus::Error,
    ] {
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM documents WHERE status = ?")
            .bind(status.as_str())
            .fetch_one(db.pool())
            .await
            .unwrap_or(0);
        status_counts.insert(status.as_str().to_string(), json!(count));
    }

    let indexed_count = status_counts.get("indexed").and_then(|v| v.as_i64()).unwrap_or(0);
    let error_count = status_counts.get("error").and_then(|v| v.as_i64()).unwrap_or(0);

    // 计算 PDF 总大小
    let mut total_size: i64 = 0;
    let docs: Vec<(i64,)> = sqlx::query_as("SELECT id FROM documents")
        .fetch_all(db.pool())
        .await
        .unwrap_or_default();
    for (id,) in docs {
        let pdf_path = paths::get_pdf_path(&settings, id);
        if let Ok(meta) = std::fs::metadata(&pdf_path) {
            total_size += meta.len() as i64;
        }
    }

    // Qdrant 状态
    let mut qdrant_status = "connected".to_string();
    let mut qdrant_collections: Vec<String> = Vec::new();
    match get_vector_db(None) {
        Ok(adapter) => match adapter.list_collections().await {
            Ok(cols) => qdrant_collections = cols,
            Err(e) => qdrant_status = format!("error: {}", e),
        },
        Err(e) => qdrant_status = format!("error: {}", e),
    }

    Json(json!({
        // Python 契约字段
        "documents": doc_count,
        "status_counts": status_counts,
        "qdrant_status": qdrant_status,
        "qdrant_collections": qdrant_collections,
        // 前端契约字段
        "total_documents": doc_count,
        "indexed_count": indexed_count,
        "error_count": error_count,
        "total_size": total_size,
    }))
}

async fn mineru_tasks(Extension(settings): Extension<Arc<Settings>>) -> Json<Value> {
    let url = settings.mineru_url();
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .unwrap_or_default();
    let mut req = client.get(format!("{}/tasks", url));
    let key = settings.mineru_key();
    if !key.is_empty() {
        req = req.bearer_auth(key);
    }
    match req.send().await {
        Ok(resp) if resp.status().is_success() => match resp.json::<Value>().await {
            Ok(data) => Json(data),
            Err(e) => Json(json!({"error": e.to_string()})),
        },
        Ok(resp) => {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            Json(json!({"error": format!("HTTP {}", status), "detail": text}))
        }
        Err(e) => Json(json!({"error": e.to_string()})),
    }
}

async fn qdrant_collections() -> Json<Value> {
    let collections = match get_vector_db(None) {
        Ok(adapter) => adapter.list_collections().await.unwrap_or_default(),
        Err(_) => Vec::new(),
    };
    Json(json!({"collections": collections}))
}

#[derive(Debug, Deserialize)]
pub struct PointQuery {
    pub collection: Option<String>,
}

async fn qdrant_point(
    Path(point_id): Path<String>,
    Query(query): Query<PointQuery>,
) -> Response {
    let adapter = match get_vector_db(None) {
        Ok(a) => a,
        Err(e) => {
            return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"detail": e.to_string()})))
                .into_response();
        }
    };
    match adapter.get_point(&point_id, query.collection.as_deref()).await {
        Ok(Some(point)) => Json(point).into_response(),
        Ok(None) => (StatusCode::NOT_FOUND, Json(json!({"detail": "Point not found"}))).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"detail": e.to_string()}))).into_response(),
    }
}

async fn migrate(Extension(db): Extension<Arc<Database>>) -> Json<Value> {
    // Rust 版本建表时已包含全部列，无需迁移；此处做一次幂等建表确认。
    let _ = db.init().await;
    Json(json!({
        "detail": "Migration completed",
        "migrations": [
            "mineru_task_id column already exists",
            "vector_db_id column already exists",
        ],
    }))
}

async fn recalculate_hashes(
    Extension(db): Extension<Arc<Database>>,
    Extension(settings): Extension<Arc<Settings>>,
) -> Json<Value> {
    let docs: Vec<Document> = sqlx::query_as::<_, Document>(
        "SELECT id, filename, file_hash, title, authors, year, doi, source, journal, \
         keywords, abstract, category, doc_type, language, title_en, authors_en, \
         keywords_en, abstract_en, journal_en, mineru_task_id, status, status_message, \
         progress, qdrant_collection, vector_db_id, created_at, updated_at \
         FROM documents",
    )
    .fetch_all(db.pool())
    .await
    .unwrap_or_default();

    let mut updated = 0i64;
    let mut skipped = 0i64;
    let mut errors: Vec<Value> = Vec::new();

    for doc in &docs {
        let pdf_path = paths::get_pdf_path(&settings, doc.id);
        if !std::path::Path::new(&pdf_path).exists() {
            skipped += 1;
            continue;
        }
        match calculate_file_hash(&pdf_path) {
            Ok(hash) => {
                let _ = sqlx::query("UPDATE documents SET file_hash = ? WHERE id = ?")
                    .bind(&hash)
                    .bind(doc.id)
                    .execute(db.pool())
                    .await;
                updated += 1;
            }
            Err(e) => errors.push(json!({"id": doc.id, "error": e.to_string()})),
        }
    }

    Json(json!({
        "detail": format!("哈希重算完成: 更新 {} 条，跳过 {} 条", updated, skipped),
        "updated": updated,
        "skipped": skipped,
        "errors": errors,
    }))
}

async fn reset_index(Extension(db): Extension<Arc<Database>>) -> Json<Value> {
    let indexed_docs: Vec<Document> = sqlx::query_as::<_, Document>(
        "SELECT id, filename, file_hash, title, authors, year, doi, source, journal, \
         keywords, abstract, category, doc_type, language, title_en, authors_en, \
         keywords_en, abstract_en, journal_en, mineru_task_id, status, status_message, \
         progress, qdrant_collection, vector_db_id, created_at, updated_at \
         FROM documents WHERE status = 'indexed'",
    )
    .fetch_all(db.pool())
    .await
    .unwrap_or_default();

    let count = indexed_docs.len();

    // 收集需要删除的集合名
    let mut collections: Vec<String> = Vec::new();
    for doc in &indexed_docs {
        if let Some(col) = doc.qdrant_collection.as_deref() {
            if !col.is_empty() && !collections.contains(&col.to_string()) {
                collections.push(col.to_string());
            }
        }
    }

    // 删除集合
    let mut deleted: Vec<String> = Vec::new();
    if let Ok(adapter) = get_vector_db(None) {
        for col in &collections {
            if adapter.delete_collection(col).await.unwrap_or(false) {
                deleted.push(col.clone());
            }
        }
    }

    // 重置文档状态
    for doc in &indexed_docs {
        let _ = sqlx::query(
            "UPDATE documents SET status = 'meta_done', qdrant_collection = NULL, \
             status_message = NULL, progress = 0 WHERE id = ?",
        )
        .bind(doc.id)
        .execute(db.pool())
        .await;
    }

    Json(json!({
        "detail": format!("索引重置完成: {} 个文档已重置为已提取状态", count),
        "reset_count": count,
        "deleted_collections": deleted,
    }))
}
