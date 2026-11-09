//! 管理路由。
//!
//! 提供统计、服务状态与索引重置等管理端点，
//! 字段与前端 `frontend/src/api/admin.ts` 对应。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use axum::extract::{Path, Query};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Json, Response};
use axum::routing::{get, post};
use axum::Extension;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::auth;
use crate::config::Settings;
use crate::database::Database;
use crate::models::DocStatus;
use crate::paths;
use crate::services::mineru::MineruType;
use crate::services::vector_db::get_vector_db;
use crate::utils::calculate_file_hash_async;

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
        .route("/assist/api/admin/pdf-health/", get(pdf_health_overview))
        .route("/assist/api/admin/pdf-health", get(pdf_health_overview))
        .route("/assist/api/admin/mineru/tasks/", get(mineru_tasks))
        .route("/assist/api/admin/mineru/tasks", get(mineru_tasks))
        .route("/assist/api/admin/qdrant/collections/", get(qdrant_collections))
        .route("/assist/api/admin/qdrant/collections", get(qdrant_collections))
        .route("/assist/api/admin/qdrant/point/:point_id/", get(qdrant_point))
        .route("/assist/api/admin/qdrant/point/:point_id", get(qdrant_point))
        .route("/assist/api/admin/recalculate-hashes/", post(recalculate_hashes))
        .route("/assist/api/admin/recalculate-hashes", post(recalculate_hashes))
        .route("/assist/api/admin/check-pdf-health/", post(check_pdf_health_task))
        .route("/assist/api/admin/check-pdf-health", post(check_pdf_health_task))
        .route(
            "/assist/api/admin/documents/:id/check-pdf-health/",
            post(check_document_pdf_health),
        )
        .route(
            "/assist/api/admin/documents/:id/check-pdf-health",
            post(check_document_pdf_health),
        )
        .route("/assist/api/admin/sync-file-sizes/", post(sync_file_sizes))
        .route("/assist/api/admin/sync-file-sizes", post(sync_file_sizes))
        .route("/assist/api/admin/file-state/reconcile/", get(file_state_reconcile))
        .route("/assist/api/admin/file-state/reconcile", get(file_state_reconcile))
        .route("/assist/api/admin/dedup/", post(dedup))
        .route("/assist/api/admin/dedup", post(dedup))
        .route("/assist/api/admin/logs/", get(get_logs))
        .route("/assist/api/admin/logs", get(get_logs))
}

async fn dedup(
    Extension(role): Extension<auth::Role>,
    Extension(db): Extension<Arc<Database>>,
) -> Json<Value> {
    if let Err(_resp) = auth::assert_role(&role, &["admin"]) {
        return Json(json!({"detail": "权限不足"}));
    }
    // 查找所有有 file_hash 的文档，按 hash 分组
    let rows: Vec<(String, i64, String)> = sqlx::query_as(
        "SELECT file_hash, id, filename FROM documents WHERE file_hash IS NOT NULL ORDER BY file_hash, id",
    )
    .fetch_all(db.pool())
    .await
    .unwrap_or_default();

    // 按 hash 分组
    use std::collections::HashMap;
    let mut groups: HashMap<String, Vec<(i64, String)>> = HashMap::new();
    for (hash, id, filename) in &rows {
        groups.entry(hash.clone()).or_default().push((*id, filename.clone()));
    }

    let mut merged_groups = 0;
    let mut deleted_docs = 0;

    for (hash, docs) in &groups {
        if docs.len() <= 1 {
            continue;
        }
        // 保留第一个（最小 id），删除其余
        let keep_id = docs[0].0;
        for (del_id, _) in &docs[1..] {
            // 删除 Qdrant 向量
            if let Ok(adapter) = get_vector_db(None) {
                let _ = adapter.delete_document(0, *del_id).await;
            }
            // 删除文档记录
            let _ = sqlx::query("DELETE FROM documents WHERE id = ?")
                .bind(del_id)
                .execute(db.pool())
                .await;
            // 同步失效图片缓存（行已删，缓存不应继续占内存）
            crate::services::image_cache::invalidate(*del_id);
            deleted_docs += 1;
        }
        merged_groups += 1;
    }

    Json(json!({
        "detail": format!("去重完成: {} 组重复, 删除 {} 个文档", merged_groups, deleted_docs),
        "merged_groups": merged_groups,
        "deleted_docs": deleted_docs,
    }))
}

async fn status() -> Json<Value> {
    let build_time = std::time::UNIX_EPOCH
        + std::time::Duration::from_secs(
            env!("BUILD_TIME").parse::<u64>().unwrap_or(0),
        );
    let build_time_str = chrono::DateTime::<chrono::Utc>::from(build_time)
        .format("%Y-%m-%dT%H:%M:%SZ")
        .to_string();

    Json(json!({
        "status": "running",
        "name": env!("CARGO_PKG_NAME"),
        "version": env!("CARGO_PKG_VERSION"),
        "build_time": build_time_str,
        "build_profile": env!("BUILD_PROFILE"),
        "rustc": env!("RUSTC_VERSION"),
        "target": env!("BUILD_HOST"),
        "backend": "rust",
        // PDF 引擎版本：外部 `mutool -v` + （编入时）内置 mupdf 的版本
        "pdf": crate::services::pdf_engine::version_report().await,
    }))
}

/// 检查 MinerU 健康（遍历所有配置）
async fn check_mineru(settings: &Settings) -> Value {
    let configs = settings.mineru_configs();
    let mut items: Vec<Value> = Vec::new();

    for config in &configs {
        let mut item = json!({"status": "unknown", "url": config.url, "type": config.mineru_type});
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(5))
            .build()
            .unwrap_or_default();

        // 根据 MinerU 类型选择正确的健康检查方式
        let mineru_type = MineruType::from_str(&config.mineru_type);
        match mineru_type {
            MineruType::Local => {
                // 自部署 V1 API：使用 /v1/health
                let mut req = client.get(format!("{}/v1/health", config.url));
                if !config.token.is_empty() {
                    req = req.bearer_auth(&config.token);
                }
                match req.send().await {
                    Ok(resp) if resp.status().is_success() => {
                        item["status"] = json!("connected");
                        item["api_version"] = json!("v1");
                        if let Ok(health) = resp.json::<Value>().await {
                            if let Some(ver) = health.get("version") {
                                item["version"] = ver.clone();
                            }
                            if let Some(features) = health.get("features") {
                                item["features"] = features.clone();
                            }
                            if let Some(tiers) = health.get("tiers") {
                                item["tiers"] = tiers.clone();
                            }
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
            }
            MineruType::Official => {
                // 官方 V4 API：用 Bearer token 请求根路径（接受 200 或 404）
                let resp = client.get(&config.url).bearer_auth(&config.token).send().await;
                match resp {
                    Ok(resp) if resp.status().is_success() || resp.status().as_u16() == 404 => {
                        item["status"] = json!("connected");
                        item["api_version"] = json!("v4");
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
            }
            MineruType::OfficialLightweight => {
                // 轻量 API
                let resp = client.get(format!("{}/v1/agent/parse/health", config.url)).send().await;
                match resp {
                    Ok(resp) if resp.status().is_success() => {
                        item["status"] = json!("connected");
                        item["api_version"] = json!("v1-lightweight");
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
            }
            MineruType::Datalab => {
                // Datalab API：使用 /api/v1/user_health 检查
                let resp = client
                    .get("https://www.datalab.to/api/v1/user_health")
                    .header("X-API-Key", &config.token)
                    .send()
                    .await;
                match resp {
                    Ok(resp) if resp.status().is_success() => {
                        item["status"] = json!("connected");
                        item["api_version"] = json!("datalab-v1");
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
            }
        }
        items.push(item);
    }

    json!({"mineru": items})
}

/// 检查 LLM 服务健康（探测信息补全所用的模型端点，支持 Ollama 原生 / OpenAI 兼容协议）
async fn check_llm(settings: &Settings) -> Value {
    let extract = settings.llm_extract_config();
    let mut item = json!({
        "status": "unknown",
        "url": &extract.url,
        "protocol": &extract.protocol,
        "model": &extract.model,
    });
    match extract.client().probe(5).await {
        Ok(models) => {
            item["status"] = json!("connected");
            item["models"] = json!(models);
        }
        Err(e) => {
            let (status, detail) = e.status_and_detail();
            item["status"] = json!(status);
            item["error"] = json!(detail);
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
    Extension(role): Extension<auth::Role>,
    Extension(settings): Extension<Arc<Settings>>,
) -> Json<Value> {
    if let Err(_resp) = auth::assert_role(&role, &["admin", "view-only", "view-upload"]) {
        return Json(json!({"detail": "权限不足"}));
    }
    let cm = crate::config_manager::ConfigManager::new("system.json");
    let cfg = cm.get_config();

    let (mineru, llm, fastembed) = tokio::join!(
        check_mineru(&settings),
        check_llm(&settings),
        check_fastembed(&settings),
    );

    let mut result = json!({
        "mineru": mineru,
        "llm": llm,
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
    Extension(role): Extension<auth::Role>,
    Extension(cm): Extension<Arc<ConfigManager>>,
) -> Json<Value> {
    if let Err(_resp) = auth::assert_role(&role, &["admin"]) {
        return Json(json!({"detail": "权限不足"}));
    }
    cm.reload_config();
    Json(json!({"status": "ok", "detail": "配置已重新加载"}))
}

use crate::config_manager::ConfigManager;

async fn stats(
    Extension(role): Extension<auth::Role>,
    Extension(db): Extension<Arc<Database>>,
    Extension(settings): Extension<Arc<Settings>>,
) -> Json<Value> {
    if let Err(_resp) = auth::assert_role(&role, &["admin", "view-only", "view-upload"]) {
        return Json(json!({"detail": "权限不足"}));
    }
    let doc_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM documents")
        .fetch_one(db.pool())
        .await
        .unwrap_or(0);

    // 状态由文件事实推导（见 `services::file_state`）：
    // uploaded / markdown_done 用 size 列聚合，parsing / error 来自运行时。
    let (uploaded, markdown_done, unsynced): (i64, i64, i64) = sqlx::query_as(
        "SELECT \
           SUM(CASE WHEN COALESCE(md_size,0) = 0 THEN 1 ELSE 0 END), \
           SUM(CASE WHEN COALESCE(md_size,0) > 0 THEN 1 ELSE 0 END), \
           SUM(CASE WHEN fs_synced_at IS NULL THEN 1 ELSE 0 END) \
         FROM documents",
    )
    .fetch_one(db.pool())
    .await
    .unwrap_or((0, 0, 0));

    let runtime_errors = crate::services::file_state::error_snapshot().len() as i64;
    let mut status_counts = serde_json::Map::new();
    status_counts.insert(DocStatus::Uploaded.as_str().to_string(), json!(uploaded));
    status_counts.insert(DocStatus::MarkdownDone.as_str().to_string(), json!(markdown_done));
    status_counts.insert(DocStatus::Error.as_str().to_string(), json!(runtime_errors));
    status_counts.insert(
        DocStatus::Parsing.as_str().to_string(),
        json!(crate::routers::pipeline::runtime_task_count()),
    );
    status_counts.insert("unsynced".to_string(), json!(unsynced));

    // 索引状态记在 document_vector_index 上（documents 不再有解析状态列）
    let indexed_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(DISTINCT document_id) FROM document_vector_index WHERE status = 'indexed'",
    )
    .fetch_one(db.pool())
    .await
    .unwrap_or(0);
    let error_count = status_counts.get("error").and_then(|v| v.as_i64()).unwrap_or(0);

    // 从数据库直接汇总文件大小（避免遍历文件系统）
    let total_pdf_size: i64 = sqlx::query_scalar("SELECT COALESCE(SUM(pdf_size), 0) FROM documents")
        .fetch_one(db.pool())
        .await
        .unwrap_or(0);
    let total_md_size: i64 = sqlx::query_scalar("SELECT COALESCE(SUM(md_size), 0) FROM documents")
        .fetch_one(db.pool())
        .await
        .unwrap_or(0);
    let total_zip_size: i64 = sqlx::query_scalar("SELECT COALESCE(SUM(zip_size), 0) FROM documents")
        .fetch_one(db.pool())
        .await
        .unwrap_or(0);
    let total_size: i64 = total_pdf_size + total_md_size + total_zip_size;

    // PDF 健康分布（ok / warning / error / missing；NULL 或空串 = 从未检查）
    let mut pdf_health_counts = serde_json::Map::new();
    for health in [
        crate::services::pdf_health::HEALTH_OK,
        crate::services::pdf_health::HEALTH_WARNING,
        crate::services::pdf_health::HEALTH_ERROR,
        crate::services::pdf_health::HEALTH_MISSING,
    ] {
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM documents WHERE pdf_health = ?")
            .bind(health)
            .fetch_one(db.pool())
            .await
            .unwrap_or(0);
        pdf_health_counts.insert(health.to_string(), json!(count));
    }
    let pdf_health_unchecked: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM documents WHERE pdf_health IS NULL OR pdf_health = ''",
    )
    .fetch_one(db.pool())
    .await
    .unwrap_or(0);

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
        "documents": doc_count,
        "status_counts": status_counts,
        "qdrant_status": qdrant_status,
        "qdrant_collections": qdrant_collections,
        "total_documents": doc_count,
        "indexed_count": indexed_count,
        "error_count": error_count,
        "total_size": total_size,
        "pdf_size": total_pdf_size,
        "md_size": total_md_size,
        "zip_size": total_zip_size,
        "pdf_health_counts": pdf_health_counts,
        "pdf_health_unchecked": pdf_health_unchecked,
    }))
}

/// PDF 健康概览（admin）：分布计数 + 非 `ok` 文档总数。
///
/// `pdf_health` 取值 `ok` / `warning` / `error` / `missing`，`NULL` / 空串表示从未检查。
/// 具体文档清单不在本端点返回：前端从「监控面板」跳到文献列表（`?pdf_health=warning,error,missing`）查看。
async fn pdf_health_overview(
    Extension(role): Extension<auth::Role>,
    Extension(db): Extension<Arc<Database>>,
) -> Json<Value> {
    if let Err(_resp) = auth::assert_role(&role, &["admin"]) {
        return Json(json!({"detail": "权限不足"}));
    }

    let total: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM documents")
        .fetch_one(db.pool())
        .await
        .unwrap_or(0);

    let mut counts = serde_json::Map::new();
    for health in [
        crate::services::pdf_health::HEALTH_OK,
        crate::services::pdf_health::HEALTH_WARNING,
        crate::services::pdf_health::HEALTH_ERROR,
        crate::services::pdf_health::HEALTH_MISSING,
    ] {
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM documents WHERE pdf_health = ?")
            .bind(health)
            .fetch_one(db.pool())
            .await
            .unwrap_or(0);
        counts.insert(health.to_string(), json!(count));
    }

    let unchecked: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM documents WHERE pdf_health IS NULL OR pdf_health = ''",
    )
    .fetch_one(db.pool())
    .await
    .unwrap_or(0);

    let problem_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM documents WHERE pdf_health IN ('error', 'missing', 'warning')",
    )
    .fetch_one(db.pool())
    .await
    .unwrap_or(0);

    Json(json!({
        "total": total,
        "counts": counts,
        "unchecked": unchecked,
        "problem_count": problem_count,
    }))
}

async fn mineru_tasks(
    Extension(role): Extension<auth::Role>,
    Extension(settings): Extension<Arc<Settings>>,
) -> Json<Value> {
    if let Err(_resp) = auth::assert_role(&role, &["admin"]) {
        return Json(json!({"detail": "权限不足"}));
    }
    let url = settings.mineru_url();
    let key = settings.mineru_key();
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .unwrap_or_default();

    // V1 API：查询用量统计 /v1/usage
    let mut req = client.get(format!("{}/v1/usage", url));
    if !key.is_empty() {
        req = req.bearer_auth(key);
    }
    match req.send().await {
        Ok(resp) if resp.status().is_success() => match resp.json::<Value>().await {
            Ok(data) => Json(json!({
                "usage": data.get("data").or(Some(&data)).cloned().unwrap_or(json!({})),
                "api_version": "v1"
            })),
            Err(e) => Json(json!({"error": e.to_string(), "api_version": "v1"})),
        },
        Ok(resp) => {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            Json(json!({"error": format!("HTTP {}", status), "detail": text, "api_version": "v1"}))
        }
        Err(e) => Json(json!({"error": e.to_string(), "api_version": "v1"})),
    }
}

async fn qdrant_collections(
    Extension(role): Extension<auth::Role>,
) -> Json<Value> {
    if let Err(_resp) = auth::assert_role(&role, &["admin"]) {
        return Json(json!({"detail": "权限不足"}));
    }
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
    Extension(role): Extension<auth::Role>,
    Path(point_id): Path<String>,
    Query(query): Query<PointQuery>,
) -> Response {
    if let Err(resp) = auth::assert_role(&role, &["admin"]) {
        return resp;
    }
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

/// 重算哈希后台任务是否正在执行（进程内标记，防止重复提交；重启即复位）。
static HASH_RECALC_RUNNING: AtomicBool = AtomicBool::new(false);

/// 任务结束（包括 panic）时复位运行标记。
struct HashRecalcGuard;

impl Drop for HashRecalcGuard {
    fn drop(&mut self) {
        HASH_RECALC_RUNNING.store(false, Ordering::SeqCst);
    }
}

/// 维护任务的筛选参数（`recalculate-hashes` / `check-pdf-health` / `sync-file-sizes` 共用）
///
/// `?only_missing=1` 只处理该任务对应字段为空（`NULL` / 空串）的文档：
/// 「补缺」不影响已有数据，适合日常点一下；不带参数则是全量重做。
#[derive(Debug, Deserialize)]
pub struct MaintenanceQuery {
    #[serde(default, deserialize_with = "de_flag")]
    pub only_missing: bool,
}

/// 查询参数里的布尔开关：接受 `1` / `true` / `yes` / `on`（不区分大小写），
/// 空值（`?only_missing`）也当作 `true`，其余都被视为 `false`。
fn de_flag<'de, D>(deserializer: D) -> Result<bool, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::Deserialize as _;
    let raw = Option::<String>::deserialize(deserializer)?;
    Ok(match raw.as_deref().map(|s| s.trim().to_ascii_lowercase()) {
        None => false,
        Some(s) if s.is_empty() => true,
        Some(s) => matches!(s.as_str(), "1" | "true" | "yes" | "on"),
    })
}

/// POST /assist/api/admin/recalculate-hashes —— 重算文档的 `file_hash`。
///
/// **非阻塞**：请求立即返回，后台**逐个串行**计算（不做并发，避免同时开多个
/// PDF 读句柄/抢 IO）；每条开始时在「活跃任务」里登记一行，文案带 `第 i/N 条`
/// 与累计统计，进度可在 `GET /assist/api/pipeline/tasks/active` 查看。
///
/// `?only_missing=1` 时只处理 `file_hash` 为 `NULL` / 空串的文档。
async fn recalculate_hashes(
    Extension(role): Extension<auth::Role>,
    Extension(db): Extension<Arc<Database>>,
    Extension(settings): Extension<Arc<Settings>>,
    Query(q): Query<MaintenanceQuery>,
) -> Json<Value> {
    if let Err(_resp) = auth::assert_role(&role, &["admin"]) {
        return Json(json!({"detail": "权限不足"}));
    }
    if HASH_RECALC_RUNNING
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return Json(json!({
            "detail": "哈希重算已在后台执行中，请等待完成",
            "started": false,
        }));
    }

    let only_missing = q.only_missing;

    // 后台执行，不阻塞当前请求
    tokio::spawn(async move {
        let _guard = HashRecalcGuard;

        let sql = if only_missing {
            "SELECT id, file_hash FROM documents WHERE file_hash IS NULL OR file_hash = '' ORDER BY id"
        } else {
            "SELECT id, file_hash FROM documents ORDER BY id"
        };
        let docs: Vec<(i64, Option<String>)> = sqlx::query_as(sql)
            .fetch_all(db.pool())
            .await
            .unwrap_or_default();
        let total = docs.len() as i64;
        tracing::info!(
            "开始重算哈希（{}）：共 {} 条，逐个串行执行",
            if only_missing { "只补缺失" } else { "全量" },
            total
        );

        let mut updated = 0i64;
        let mut unchanged = 0i64;
        let mut skipped = 0i64;
        let mut errors = 0i64;
        // 用于「先登记下一行、再撤掉上一行」，保证活跃任务里始终有一条进度可见
        let mut prev_doc_id: Option<i64> = None;

        for (idx, (doc_id, old_hash)) in docs.iter().enumerate() {
            let doc_id = *doc_id;
            crate::routers::pipeline::track_maintenance_task(
                doc_id,
                "hash",
                &format!(
                    "重算哈希：第 {}/{} 条（已更新 {}，未变 {}，跳过 {}，失败 {}）",
                    idx + 1,
                    total,
                    updated,
                    unchanged,
                    skipped,
                    errors
                ),
                idx as i32,
                total as i32,
            );
            if let Some(prev) = prev_doc_id.take() {
                crate::routers::pipeline::track_task_end(prev);
            }
            prev_doc_id = Some(doc_id);

            let pdf_path = paths::get_pdf_path(&settings, doc_id);
            if !std::path::Path::new(&pdf_path).exists() {
                skipped += 1;
                continue;
            }
            match calculate_file_hash_async(pdf_path.clone()).await {
                Ok(hash) => {
                    // 哈希没变化就不写库，省掉无谓的 SQLite 写
                    if old_hash.as_deref() == Some(hash.as_str()) {
                        unchanged += 1;
                        continue;
                    }
                    match sqlx::query("UPDATE documents SET file_hash = ? WHERE id = ?")
                        .bind(&hash)
                        .bind(doc_id)
                        .execute(db.pool())
                        .await
                    {
                        Ok(_) => updated += 1,
                        Err(e) => {
                            tracing::warn!("重算哈希写库失败 doc_id={}: {}", doc_id, e);
                            errors += 1;
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!(
                        "重算哈希失败 doc_id={} path={}: {}",
                        doc_id,
                        pdf_path,
                        e
                    );
                    errors += 1;
                }
            }
        }
        if let Some(prev) = prev_doc_id {
            crate::routers::pipeline::track_task_end(prev);
        }

        tracing::info!(
            "哈希重算完成：共 {} 条，更新 {} 条，未变 {} 条，跳过 {} 条（PDF 缺失），失败 {} 条",
            total,
            updated,
            unchanged,
            skipped,
            errors
        );
    });

    Json(json!({
        "detail": if only_missing {
            "缺失哈希补齐任务已提交，后台逐个计算中（进度见「活跃任务」）"
        } else {
            "哈希重算任务已提交，后台逐个计算中（进度见「活跃任务」）"
        },
        "started": true,
        "only_missing": only_missing,
    }))
}

/// PDF 健康检查后台任务是否正在执行（进程内标记，防止重复提交；重启即复位）。
static PDF_HEALTH_RUNNING: AtomicBool = AtomicBool::new(false);

/// 任务结束（包括 panic）时复位运行标记。
struct PdfHealthGuard;

impl Drop for PdfHealthGuard {
    fn drop(&mut self) {
        PDF_HEALTH_RUNNING.store(false, Ordering::SeqCst);
    }
}

/// POST /assist/api/admin/check-pdf-health —— 批量检查 PDF 健康。
///
/// **非阻塞**：请求立即返回，后台**逐个串行**检查（不并发），结果写入
/// `documents.pdf_health`（`ok` / `warning` / `error` / `missing`）与
/// `documents.pdf_health_message`（原因）；进度同重算哈希，见「活跃任务」。
///
/// `?only_missing=1` 时只检查 `pdf_health` 为 `NULL` / 空串（从未检查过）的文档。
async fn check_pdf_health_task(
    Extension(role): Extension<auth::Role>,
    Extension(db): Extension<Arc<Database>>,
    Extension(settings): Extension<Arc<Settings>>,
    Query(q): Query<MaintenanceQuery>,
) -> Json<Value> {
    if let Err(_resp) = auth::assert_role(&role, &["admin"]) {
        return Json(json!({"detail": "权限不足"}));
    }
    if PDF_HEALTH_RUNNING
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return Json(json!({
            "detail": "PDF 健康检查已在后台执行中，请等待完成",
            "started": false,
        }));
    }

    let only_missing = q.only_missing;

    tokio::spawn(async move {
        let _guard = PdfHealthGuard;

        let sql = if only_missing {
            "SELECT id FROM documents WHERE pdf_health IS NULL OR pdf_health = '' ORDER BY id"
        } else {
            "SELECT id FROM documents ORDER BY id"
        };
        let docs: Vec<(i64,)> = sqlx::query_as(sql)
            .fetch_all(db.pool())
            .await
            .unwrap_or_default();
        let total = docs.len() as i64;
        tracing::info!(
            "开始批量 PDF 健康检查（{}）：共 {} 条，逐个串行执行",
            if only_missing { "只补缺失" } else { "全量" },
            total
        );

        let mut ok = 0i64;
        let mut warning = 0i64;
        let mut error = 0i64;
        let mut missing = 0i64;
        let mut prev_doc_id: Option<i64> = None;

        for (idx, (doc_id,)) in docs.iter().enumerate() {
            let doc_id = *doc_id;
            crate::routers::pipeline::track_maintenance_task(
                doc_id,
                "pdf-health",
                &format!(
                    "PDF 健康检查：第 {}/{} 条（正常 {}，警告 {}，异常 {}，缺失 {}）",
                    idx + 1,
                    total,
                    ok,
                    warning,
                    error,
                    missing
                ),
                idx as i32,
                total as i32,
            );
            if let Some(prev) = prev_doc_id.take() {
                crate::routers::pipeline::track_task_end(prev);
            }
            prev_doc_id = Some(doc_id);

            let pdf_path = paths::get_pdf_path(&settings, doc_id);
            let health = crate::services::pdf_health::check_pdf_health(pdf_path.clone()).await;

            match health.status {
                crate::services::pdf_health::HEALTH_OK => ok += 1,
                crate::services::pdf_health::HEALTH_WARNING => warning += 1,
                crate::services::pdf_health::HEALTH_MISSING => missing += 1,
                _ => error += 1,
            }
            if health.status != crate::services::pdf_health::HEALTH_OK {
                tracing::warn!(
                    "PDF 健康检查异常 doc_id={} path={} status={} pages={:?} msg={}",
                    doc_id,
                    pdf_path,
                    health.status,
                    health.page_count,
                    health.message
                );
            }

            if let Err(e) = sqlx::query(
                "UPDATE documents SET pdf_health = ?, pdf_health_message = ? WHERE id = ?",
            )
            .bind(health.status)
            .bind(&health.message)
            .bind(doc_id)
            .execute(db.pool())
            .await
            {
                tracing::warn!("PDF 健康结果写库失败 doc_id={}: {}", doc_id, e);
            }
        }
        if let Some(prev) = prev_doc_id {
            crate::routers::pipeline::track_task_end(prev);
        }

        tracing::info!(
            "PDF 健康检查完成：共 {} 条，正常 {}，警告 {}，异常 {}，缺失 {}",
            total,
            ok,
            warning,
            error,
            missing
        );
    });

    Json(json!({
        "detail": if only_missing {
            "缺失 PDF 健康信息补齐任务已提交，后台逐个执行中（进度见「活跃任务」）"
        } else {
            "PDF 健康检查任务已提交，后台逐个执行中（进度见「活跃任务」）"
        },
        "started": true,
        "only_missing": only_missing,
    }))
}

/// POST /assist/api/admin/documents/:id/check-pdf-health —— **单篇同步**健康检查。
///
/// 与批量端点不同，这个接口**当场返回结果**（单文件几十毫秒），并写回
/// `documents.pdf_health` / `pdf_health_message`。用于文档管理页的「检查 PDF 健康」按钮。
///
/// 响应：`{id, status, message, page_count, engine, pdf_path}`，
/// `status` 为 `ok` / `warning` / `error` / `missing`。
async fn check_document_pdf_health(
    Extension(role): Extension<auth::Role>,
    Extension(db): Extension<Arc<Database>>,
    Extension(settings): Extension<Arc<Settings>>,
    Path(id): Path<i64>,
) -> Response {
    if let Err(resp) = auth::assert_role(&role, &["admin"]) {
        return resp;
    }

    // 文档必须存在（否则 pdf_path 模板也能拼出一个不存在的路径，报“缺失”会误导）
    let found: Result<Option<(i64,)>, _> = sqlx::query_as("SELECT id FROM documents WHERE id = ?")
        .bind(id)
        .fetch_optional(db.pool())
        .await;
    match found {
        Ok(Some(_)) => {}
        Ok(None) => {
            return (StatusCode::NOT_FOUND, Json(json!({"detail": format!("文档 {id} 不存在")})))
                .into_response()
        }
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"detail": format!("查询文档失败: {e}")})),
            )
                .into_response()
        }
    }

    let pdf_path = paths::get_pdf_path(&settings, id);
    let health = crate::services::pdf_health::check_pdf_health(pdf_path.clone()).await;

    if let Err(e) = sqlx::query(
        "UPDATE documents SET pdf_health = ?, pdf_health_message = ? WHERE id = ?",
    )
    .bind(health.status)
    .bind(&health.message)
    .bind(id)
    .execute(db.pool())
    .await
    {
        tracing::warn!("PDF 健康结果写库失败 doc_id={}: {}", id, e);
    }

    tracing::info!(
        "单篇 PDF 健康检查 doc_id={} path={} status={} pages={:?} engine={} msg={}",
        id,
        pdf_path,
        health.status,
        health.page_count,
        crate::services::pdf_engine::active().as_str(),
        health.message
    );

    Json(json!({
        "id": id,
        "status": health.status,
        "message": health.message,
        "page_count": health.page_count,
        "engine": crate::services::pdf_engine::active().as_str(),
        "pdf_path": pdf_path,
    }))
    .into_response()
}

/// POST /assist/api/admin/sync-file-sizes —— 同步文档的文件大小信息到数据库。
/// 非阻塞：请求立即返回，后台遍历文档执行 stat。
///
/// `?only_missing=1` 时只处理 `pdf_size` / `md_size` / `zip_size` 中有 `NULL` 的文档。
async fn sync_file_sizes(
    Extension(role): Extension<auth::Role>,
    Extension(db): Extension<Arc<Database>>,
    Extension(settings): Extension<Arc<Settings>>,
    Query(q): Query<MaintenanceQuery>,
) -> Json<Value> {
    if let Err(_resp) = auth::assert_role(&role, &["admin"]) {
        return Json(json!({"detail": "权限不足"}));
    }

    let only_missing = q.only_missing;

    // 后台执行，不阻塞当前请求
    let db_clone = db.clone();
    let settings_clone = settings.clone();
    tokio::spawn(async move {
        // 「补缺」按「任一项为空」筛选：三项都是有文件才有值，缺文件的项会一直是 NULL，
        // 重复处理也只是几次 stat，代价可以忽略。
        let sql = if only_missing {
            "SELECT id FROM documents WHERE pdf_size IS NULL OR md_size IS NULL OR zip_size IS NULL ORDER BY id"
        } else {
            "SELECT id FROM documents ORDER BY id"
        };
        let docs: Vec<(i64,)> = sqlx::query_as(sql)
            .fetch_all(db_clone.pool())
            .await
            .unwrap_or_default();
        let total = docs.len();
        for (doc_id,) in &docs {
            crate::routers::documents::sync_doc_file_sizes(&db_clone, &settings_clone, *doc_id).await;
        }
        tracing::info!(
            "文件大小同步完成（{}）: {} 条",
            if only_missing { "只补缺失" } else { "全量" },
            total
        );
    });

    Json(json!({
        "detail": if only_missing {
            "缺失文件大小补齐任务已提交，后台执行中"
        } else {
            "文件大小同步任务已提交，后台执行中"
        },
        "only_missing": only_missing,
    }))
}

// ── 文件事实一致性报告（运维） ───────────────────────────

/// `GET /assist/api/admin/file-state/reconcile` 的查询参数。
#[derive(Debug, Deserialize)]
pub struct ReconcileQuery {
    /// `1` / `true` 时把差异文档的文件事实重新同步（默认只报告）
    pub fix: Option<String>,
}

/// 文件事实一致性报告：对比「数据库同步列」与「磁盘实时 stat」。
///
/// 取代原「批量修正文档状态」端点 —— 状态不再可手改，只能靠重新同步文件事实修正。
/// 三个桶：
/// - `unsynced`：`fs_synced_at` 为空（从未同步过，状态未知）
/// - `mismatch`：同步值与实时值不一致
/// - `ok`：一致
async fn file_state_reconcile(
    Extension(role): Extension<auth::Role>,
    Extension(db): Extension<Arc<Database>>,
    Extension(settings): Extension<Arc<Settings>>,
    Query(q): Query<ReconcileQuery>,
) -> Response {
    if let Err(resp) = auth::assert_role(&role, &["admin"]) {
        return resp;
    }
    let fix = matches!(
        q.fix.as_deref().map(|s| s.trim().to_ascii_lowercase()).as_deref(),
        Some("1") | Some("true") | Some("yes") | Some("on")
    );

    let rows: Vec<(i64, Option<i64>, Option<i64>, Option<i64>, Option<String>)> = sqlx::query_as(
        "SELECT id, pdf_size, md_size, zip_size, fs_synced_at FROM documents ORDER BY id",
    )
    .fetch_all(db.pool())
    .await
    .unwrap_or_default();

    let mut unsynced: Vec<i64> = Vec::new();
    let mut mismatch: Vec<Value> = Vec::new();
    let mut fixed = 0i64;

    for (id, pdf, md, zip, synced_at) in rows {
        if synced_at.as_deref().map(|s| s.trim().is_empty()).unwrap_or(true) {
            unsynced.push(id);
            if fix {
                crate::routers::documents::sync_doc_file_sizes(&db, &settings, id).await;
                fixed += 1;
            }
            continue;
        }
        let live = (
            std::fs::metadata(paths::get_pdf_path(&settings, id)).ok().map(|m| m.len() as i64),
            std::fs::metadata(paths::get_markdown_path(&settings, id)).ok().map(|m| m.len() as i64),
            std::fs::metadata(paths::get_asset_path(&settings, id)).ok().map(|m| m.len() as i64),
        );
        if (pdf, md, zip) != live {
            mismatch.push(json!({
                "id": id,
                "db": {"pdf_size": pdf, "md_size": md, "zip_size": zip},
                "disk": {"pdf_size": live.0, "md_size": live.1, "zip_size": live.2},
            }));
            if fix {
                crate::routers::documents::sync_doc_file_sizes(&db, &settings, id).await;
                fixed += 1;
            }
        }
    }

    let total = unsynced.len() + mismatch.len();
    Json(json!({
        "detail": if fix {
            format!("已重新同步 {} 条文档的文件事实", fixed)
        } else {
            format!("发现 {} 条文档的文件事实需要同步（unsynced {} / mismatch {}）",
                    total, unsynced.len(), mismatch.len())
        },
        "fix": fix,
        "total_documents": unsynced.len() + mismatch.len(),
        "unsynced": unsynced,
        "mismatch": mismatch,
        "fixed": fixed,
    }))
    .into_response()
}

// ── 实时日志（短轮询环形缓冲） ───────────────────────────

/// 单次返回的默认条数（前端可传 `limit` 调整）
const LOG_DEFAULT_LIMIT: usize = 500;

/// 日志查询参数。
///
/// 数据源是 `logger` 的进程内环形缓冲（最近 [`crate::logger::RING_CAPACITY`] 条），
/// **不再读日志文件**；历史日志请直接看 `system.json` 的 `log_path` 指向的文件。
#[derive(Debug, Deserialize)]
pub struct LogQuery {
    /// 增量游标：取自上次响应的 `next_seq`（只返回序号更大的记录）。
    /// 缺省 / 0 = 从缓冲区里最早的记录开始。
    pub since: Option<u64>,
    /// 最低等级：trace / debug / info / warn / error（缺省 info，语义为「该等级及以上」）
    pub level: Option<String>,
    /// 单次返回上限（默认 500，上限 = 环形缓冲容量）
    pub limit: Option<usize>,
    /// 是否把「本接口自身的请求日志」也算进来（默认隐藏，避免看日志时被自己的轮询刷屏）。
    /// 取 `1` / `true` / `yes` / `on` 时包含。
    pub include_self: Option<String>,
}

/// 本接口自身的请求路径：默认把它产生的日志过滤掉。
const SELF_LOG_PATH: &str = "/assist/api/admin/logs";

/// 判定一条日志是否属于「本接口自身的请求」。
///
/// 三种来源都要覆盖：
/// - `request_start_log_middleware`：`fields.path`；
/// - `TraceLayer` 的响应日志：`fields.uri`；
/// - 上面两者都挂在 `request` span 上，span 字段里也有 `uri`。
fn is_self_request_log(entry: &Value) -> bool {
    let matches_path = |v: Option<&Value>| {
        v.and_then(Value::as_str)
            .is_some_and(|s| s.starts_with(SELF_LOG_PATH))
    };
    if matches_path(entry.pointer("/fields/path")) || matches_path(entry.pointer("/fields/uri")) {
        return true;
    }
    entry
        .get("spans")
        .and_then(Value::as_array)
        .is_some_and(|spans| spans.iter().any(|span| matches_path(span.get("uri"))))
}

/// `GET /assist/api/admin/logs?since=&level=&limit=`
///
/// 前端每 1 秒用 `since` 拉一次增量即可（纯内存操作，无磁盘 IO）。
/// 响应字段：
/// - `entries`：按时间正序的日志条目（结构与 JSONL 文件里的一行完全一致）；
/// - `next_seq`：下次请求用的游标；
/// - `truncated`：命中条数超过 `limit`，前端应立刻再拉一次把落后的部分追平；
/// - `lost`：`since` 之后被挤出缓冲区的条数（前端提示「中间有日志被丢弃」）。
async fn get_logs(
    Extension(role): Extension<auth::Role>,
    Extension(settings): Extension<Arc<Settings>>,
    Query(params): Query<LogQuery>,
) -> Response {
    if let Err(resp) = auth::assert_role(&role, &["admin"]) {
        return resp;
    }

    let since = params.since.unwrap_or(0);
    let min_level = params
        .level
        .as_deref()
        .and_then(crate::logger::level_from_name)
        .unwrap_or(tracing::Level::INFO);
    let limit = params
        .limit
        .unwrap_or(LOG_DEFAULT_LIMIT)
        .clamp(1, crate::logger::RING_CAPACITY);

    // 默认把「本接口自身的请求日志」过滤掉（否则每拉一次就多一条自己的请求日志）
    let include_self = params
        .include_self
        .as_deref()
        .map(|v| matches!(v.trim().to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on" | "include"))
        .unwrap_or(false);
    let snapshot = crate::logger::store().snapshot(since, min_level, limit, |entry| {
        include_self || !is_self_request_log(entry)
    });

    // 日志文件路径只用于前端提示「历史日志在这里」，处理器不读它
    let log_file = crate::config_manager::ConfigManager::substitute_path_variables(
        &settings.log_path(),
        0,
    );

    Json(json!({
        "entries": snapshot.entries,
        "count": snapshot.entries.len(),
        "matched": snapshot.matched,
        "next_seq": snapshot.next_seq,
        "latest_seq": snapshot.latest_seq,
        "oldest_seq": snapshot.oldest_seq,
        "lost": snapshot.lost,
        "truncated": snapshot.truncated,
        "level": crate::logger::level_name(min_level),
        "capacity": crate::logger::RING_CAPACITY,
        "file": log_file,
        "server_time": chrono::Utc::now().to_rfc3339(),
    }))
    .into_response()
}
