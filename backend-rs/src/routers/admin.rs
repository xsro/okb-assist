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
use crate::services::mineru::MineruType;
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
        .route("/assist/api/admin/recalculate-hashes/", post(recalculate_hashes))
        .route("/assist/api/admin/recalculate-hashes", post(recalculate_hashes))
        .route("/assist/api/admin/dedup/", post(dedup))
        .route("/assist/api/admin/dedup", post(dedup))
        .route("/assist/api/admin/logs/", get(get_logs))
        .route("/assist/api/admin/logs", get(get_logs))
}

async fn dedup(Extension(db): Extension<Arc<Database>>) -> Json<Value> {
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
        }
        items.push(item);
    }

    json!({"mineru": items})
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
    let cm = crate::config_manager::ConfigManager::new("system.json");
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

    // 计算 PDF 总大小（后台线程，不阻塞 async 运行时）
    let total_size: i64 = {
        let docs: Vec<(i64,)> = sqlx::query_as("SELECT id FROM documents")
            .fetch_all(db.pool())
            .await
            .unwrap_or_default();
        let paths: Vec<String> = docs.iter().map(|(id,)| paths::get_pdf_path(&settings, *id)).collect();
        tokio::task::spawn_blocking(move || {
            let mut total: i64 = 0;
            for p in &paths {
                if let Ok(meta) = std::fs::metadata(p) {
                    total += meta.len() as i64;
                }
            }
            total
        }).await.unwrap_or(0)
    };

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

async fn recalculate_hashes(
    Extension(db): Extension<Arc<Database>>,
    Extension(settings): Extension<Arc<Settings>>,
) -> Json<Value> {
    let docs: Vec<Document> = sqlx::query_as::<_, Document>(
        "SELECT id, filename, file_hash, title, authors, CAST(NULLIF(year, '') AS INTEGER) AS year, doi, source, journal, \
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

// ── 日志查看 ────────────────────────────────────────────

/// 日志查询参数
#[derive(Debug, Deserialize)]
pub struct LogQuery {
    /// 返回最近多少行（默认 100）
    pub lines: Option<usize>,
    /// 按日志级别过滤（trace / debug / info / warn / error）
    pub level: Option<String>,
    /// 文本关键词过滤
    pub q: Option<String>,
}

async fn get_logs(
    Extension(settings): Extension<Arc<Settings>>,
    Query(params): Query<LogQuery>,
) -> Response {
    let log_path_raw = settings.log_path();
    let log_path = std::path::PathBuf::from(
        crate::config_manager::ConfigManager::substitute_path_variables(&log_path_raw, 0)
    );

    if !log_path.exists() {
        return (StatusCode::NOT_FOUND, Json(json!({
            "entries": [],
            "total": 0,
            "file": log_path.to_string_lossy().to_string(),
            "error": "日志文件不存在"
        }))).into_response();
    }

    let max_lines = params.lines.unwrap_or(100).min(5000);
    let level_filter = params.level.as_deref().unwrap_or("").to_lowercase();
    let keyword = params.q.as_deref().unwrap_or("").to_lowercase();

    // 读取日志文件
    let content = match std::fs::read_to_string(&log_path) {
        Ok(c) => c,
        Err(e) => {
            return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({
                "entries": [],
                "total": 0,
                "file": log_path.to_string_lossy().to_string(),
                "error": format!("读取日志文件失败: {}", e)
            }))).into_response();
        }
    };

    let mut entries: Vec<Value> = Vec::new();

    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() { continue; }

        // 尝试解析 JSONL
        let entry: Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => {
                // 非 JSONL 行（如进程启动时的消息），作为纯文本条目
                entries.push(json!({
                    "message": line,
                    "level": "info",
                    "timestamp": "",
                    "target": ""
                }));
                continue;
            }
        };

        // 按级别过滤
        if !level_filter.is_empty() {
            let lvl = entry["level"]
                .as_str()
                .unwrap_or("")
                .to_lowercase();
            if lvl != level_filter { continue; }
        }

        // 按关键词过滤
        if !keyword.is_empty() {
            let raw = serde_json::to_string(&entry).unwrap_or_default().to_lowercase();
            if !raw.contains(&keyword) { continue; }
        }

        entries.push(entry);
    }

    // 取最近的 N 条
    let total = entries.len();
    let start = if entries.len() > max_lines { entries.len() - max_lines } else { 0 };
    let slice: Vec<Value> = entries.drain(start..).collect();

    Json(json!({
        "entries": slice,
        "total": total,
        "file": log_path.to_string_lossy().to_string(),
        "displayed": slice.len(),
    })).into_response()
}

