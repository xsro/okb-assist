//! 流水线路由。
//!
//! 与 Python 版 `app/routers/pipeline.py` 保持 API 契约一致。
//! 后台任务使用 `tokio::spawn` + 全局 `Semaphore` 模拟 FastAPI 的 `BackgroundTasks`
//! + `asyncio.Semaphore`，批量进度与暂停标志为进程内全局状态（重启即丢失）。

use std::collections::HashMap;
use std::future::Future;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock, RwLock};

use axum::extract::{Path, Query};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Json, Response};
use axum::routing::{get, post};
use axum::Extension;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::config::Settings;
use crate::database::Database;
use crate::models::{Document, DocStatus, DocumentVectorIndex};
use crate::paths;
use crate::services::crossref::CrossrefClient;
use crate::services::mineru::MinerUClient;
use crate::services::ollama::OllamaClient;
use crate::services::pdf_meta::{extract_pdf_metadata, looks_like_placeholder, normalize_doi};
use crate::services::vector_db::{chunk_text_by_markdown, get_vector_db};
use crate::utils::{absolute_path, now_iso};

/// 与 documents.rs 保持一致的文档列清单
const DOC_COLUMNS: &str = "id, filename, file_hash, title, authors, year, doi, source, journal, \
    keywords, abstract, category, doc_type, language, title_en, authors_en, \
    keywords_en, abstract_en, journal_en, mineru_task_id, status, status_message, \
    progress, qdrant_collection, vector_db_id, created_at, updated_at";

/// 默认 Qdrant user id（无鉴权）
const QDRANT_USER_ID: i64 = 0;

/// Markdown 分块默认参数（与 Python qdrant.py 一致）
const CHUNK_SIZE: usize = 1000;
const MIN_CHUNK_SIZE: usize = 100;

/// Zotero 标准类型映射（镜像 Python `_TYPE_NORMALIZE`）
fn normalize_doc_type(raw: &str) -> String {
    let raw = raw.trim();
    if raw.is_empty() {
        return String::new();
    }
    let lower = raw.to_lowercase();
    match lower.as_str() {
        "article" | "journalarticle" | "journal article" => "journalArticle".to_string(),
        "conference" | "conferencepaper" | "conference paper" | "inproceedings" | "conference proceedings" => "conferencePaper".to_string(),
        "thesis" | "dissertation" | "硕士学位论文" | "博士学位论文" | "mastersthesis" | "phdthesis" => "thesis".to_string(),
        "book" => "book".to_string(),
        "booksection" | "book section" => "bookSection".to_string(),
        "preprint" => "preprint".to_string(),
        "report" => "report".to_string(),
        "webpage" => "webpage".to_string(),
        "document" => "document".to_string(),
        "presentation" => "presentation".to_string(),
        "manuscript" => "manuscript".to_string(),
        "patent" => "patent".to_string(),
        "review" => "review".to_string(),
        _ => "document".to_string(),
    }
}

// ── 全局状态（进程内，重启即丢失，与 Python 模块级全局一致） ──

static TASK_SEMAPHORE: OnceLock<Arc<tokio::sync::Semaphore>> = OnceLock::new();
static RUNNING_TASKS: AtomicUsize = AtomicUsize::new(0);
static BATCH_PAUSED: AtomicBool = AtomicBool::new(false);

#[derive(Clone, serde::Serialize)]
struct ActiveTask {
    task_type: String,
    started_at: String,
    status_message: String,
}

static ACTIVE_TASKS: OnceLock<RwLock<HashMap<i64, ActiveTask>>> = OnceLock::new();
static BATCH_PROGRESS: OnceLock<RwLock<Value>> = OnceLock::new();

fn task_semaphore() -> Arc<tokio::sync::Semaphore> {
    TASK_SEMAPHORE
        .get_or_init(|| {
            let n = crate::settings::get_settings().max_concurrent_tasks().max(1);
            Arc::new(tokio::sync::Semaphore::new(n))
        })
        .clone()
}

fn active_tasks_map() -> &'static RwLock<HashMap<i64, ActiveTask>> {
    ACTIVE_TASKS.get_or_init(|| RwLock::new(HashMap::new()))
}

fn batch_progress_map() -> &'static RwLock<Value> {
    BATCH_PROGRESS.get_or_init(|| {
        RwLock::new(json!({
            "active": false,
            "stage": "",
            "vector_db_id": "",
            "total": 0,
            "processed": 0,
            "current_batch": 0,
            "total_batches": 0,
            "batch_size": 0,
            "pause_seconds": 0,
            "started_at": null,
            "errors": 0,
        }))
    })
}

fn max_concurrent_tasks() -> usize {
    crate::settings::get_settings().max_concurrent_tasks().max(1)
}

fn track_task_start(doc_id: i64, task_type: &str) {
    active_tasks_map().write().unwrap().insert(
        doc_id,
        ActiveTask {
            task_type: task_type.to_string(),
            started_at: now_iso(),
            status_message: "正在处理...".to_string(),
        },
    );
}

fn track_task_update(doc_id: i64, status_message: &str) {
    if let Some(t) = active_tasks_map().write().unwrap().get_mut(&doc_id) {
        t.status_message = status_message.to_string();
    }
}

fn track_task_end(doc_id: i64) {
    active_tasks_map().write().unwrap().remove(&doc_id);
}

// ── 数据库辅助函数 ──

async fn fetch_doc(db: &Database, doc_id: i64) -> Option<Document> {
    sqlx::query_as::<_, Document>(&format!("SELECT {} FROM documents WHERE id = ?", DOC_COLUMNS))
        .bind(doc_id)
        .fetch_optional(db.pool())
        .await
        .unwrap_or(None)
}

async fn update_doc_status(
    db: &Database,
    doc_id: i64,
    status: DocStatus,
    message: Option<&str>,
    progress: Option<f64>,
) {
    let mut sql = "UPDATE documents SET status = ?".to_string();
    if message.is_some() {
        sql.push_str(", status_message = ?");
    }
    if progress.is_some() {
        sql.push_str(", progress = ?");
    }
    sql.push_str(", updated_at = ? WHERE id = ?");

    let mut q = sqlx::query(&sql).bind(status.as_str());
    if let Some(m) = message {
        q = q.bind(m);
    }
    if let Some(p) = progress {
        q = q.bind(p);
    }
    q = q.bind(now_iso()).bind(doc_id);
    let _ = q.execute(db.pool()).await;
}

async fn count_status(db: &Database, status: &str) -> i64 {
    sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM documents WHERE status = ?")
        .bind(status)
        .fetch_one(db.pool())
        .await
        .unwrap_or(0)
}

async fn save_mineru_task_id(db: &Database, doc_id: i64, task_id: &str) {
    let _ = sqlx::query("UPDATE documents SET mineru_task_id = ? WHERE id = ?")
        .bind(task_id)
        .bind(doc_id)
        .execute(db.pool())
        .await;
}

async fn clear_mineru_task_id(db: &Database, doc_id: i64) {
    let _ = sqlx::query("UPDATE documents SET mineru_task_id = NULL WHERE id = ?")
        .bind(doc_id)
        .execute(db.pool())
        .await;
}

/// 递归删除目录（忽略错误）
fn remove_dir_all_ignore(path: &std::path::Path) {
    let _ = std::fs::remove_dir_all(path);
}

// ── 后台任务实现 ──

async fn do_parse_impl(db: Arc<Database>, settings: Arc<Settings>, doc_id: i64) {
    let mineru = MinerUClient::new(
        &settings.mineru_url(),
        &settings.mineru_key(),
        &settings.mineru_type(),
        &settings.mineru_model_version(),
    );
    let uploads_folder = settings.uploads_folder();
    let pdf_path = paths::get_pdf_path(&settings, doc_id);
    let abs_file_path = absolute_path(&pdf_path);
    let output_dir = std::path::Path::new(&uploads_folder).join(doc_id.to_string());
    let _ = std::fs::create_dir_all(&output_dir);

    update_doc_status(&db, doc_id, DocStatus::Parsing, Some("正在解析 PDF..."), Some(10.0)).await;

    // 检查是否存在之前的 MinerU 任务
    let mut existing_task_id: Option<String> = fetch_doc(&db, doc_id)
        .await
        .and_then(|d| d.mineru_task_id)
        .filter(|t| !t.is_empty());

    if let Some(ref task_id) = existing_task_id {
        update_doc_status(&db, doc_id, DocStatus::Parsing, Some("正在检查之前的解析任务..."), Some(15.0)).await;
        let status_result = mineru.check_task_status(task_id).await;
        let status = status_result["status"].as_str().unwrap_or("unknown").to_string();

        match status.as_str() {
            "completed" => {
                update_doc_status(&db, doc_id, DocStatus::Parsing, Some("之前的任务已完成，正在获取结果..."), Some(50.0)).await;
                if let Ok(md_path) = mineru.get_task_result(task_id, output_dir.to_str().unwrap_or(""), Some(doc_id)).await {
                    finish_parse_result(&db, &settings, doc_id, &md_path).await;
                } else {
                    update_doc_status(&db, doc_id, DocStatus::Error, Some("解析失败: 获取之前任务结果失败"), None).await;
                }
                return;
            }
            "failed" => {
                update_doc_status(&db, doc_id, DocStatus::Parsing, Some("之前的任务失败，重新提交..."), Some(10.0)).await;
                clear_mineru_task_id(&db, doc_id).await;
                existing_task_id = None;
            }
            "processing" | "pending" => {
                update_doc_status(&db, doc_id, DocStatus::Parsing, Some("正在等待之前的解析任务完成..."), Some(20.0)).await;
                match mineru.poll_task(task_id, settings.mineru_task_timeout()).await {
                    Ok(_) => {
                        if let Ok(md_path) = mineru.get_task_result(task_id, output_dir.to_str().unwrap_or(""), Some(doc_id)).await {
                            finish_parse_result(&db, &settings, doc_id, &md_path).await;
                        } else {
                            update_doc_status(&db, doc_id, DocStatus::Error, Some("解析失败: 获取结果失败"), None).await;
                        }
                        return;
                    }
                    Err(e) => {
                        let msg = e.to_string();
                        if msg.to_lowercase().contains("timed out") {
                            update_doc_status(&db, doc_id, DocStatus::Error, Some(&format!("解析失败: {}", msg)), None).await;
                            return;
                        }
                        clear_mineru_task_id(&db, doc_id).await;
                        existing_task_id = None;
                    }
                }
            }
            _ => {
                clear_mineru_task_id(&db, doc_id).await;
                existing_task_id = None;
            }
        }
    }

    // 提交新任务
    let task_id = match mineru.submit_parse_task(&abs_file_path).await {
        Ok(t) => t,
        Err(e) => {
            update_doc_status(&db, doc_id, DocStatus::Error, Some(&format!("解析失败: {}", e)), None).await;
            return;
        }
    };

    save_mineru_task_id(&db, doc_id, &task_id).await;
    update_doc_status(&db, doc_id, DocStatus::Parsing, Some("正在解析 PDF，已提交任务..."), Some(20.0)).await;

    if let Err(e) = mineru.poll_task(&task_id, settings.mineru_task_timeout()).await {
        let msg = e.to_string();
        if msg.to_lowercase().contains("timed out") {
            update_doc_status(&db, doc_id, DocStatus::Error, Some(&format!("解析失败: {}", msg)), None).await;
            return;
        }
        update_doc_status(&db, doc_id, DocStatus::Error, Some(&format!("解析失败: {}", msg)), None).await;
        return;
    }

    update_doc_status(&db, doc_id, DocStatus::Parsing, Some("正在获取解析结果..."), Some(80.0)).await;
    match mineru.get_task_result(&task_id, output_dir.to_str().unwrap_or(""), Some(doc_id)).await {
        Ok(md_path) => finish_parse_result(&db, &settings, doc_id, &md_path).await,
        Err(e) => update_doc_status(&db, doc_id, DocStatus::Error, Some(&format!("解析失败: {}", e)), None).await,
    }
}

/// 完成解析：复制文件并更新状态（async 版本）
async fn finish_parse_result(db: &Database, settings: &Settings, doc_id: i64, md_path: &str) {
    let md_path_buf = std::path::PathBuf::from(md_path);
    let target_md = std::path::PathBuf::from(paths::get_markdown_path(settings, doc_id));
    if let Some(parent) = target_md.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let md_copied = std::fs::copy(&md_path_buf, &target_md).is_ok();

    let images_zip = md_path_buf.parent().map(|p| p.join("images.zip"));
    let mut zip_copied = true;
    if let Some(zip) = &images_zip {
        if zip.exists() {
            let target_zip = std::path::PathBuf::from(paths::get_asset_path(settings, doc_id));
            if let Some(parent) = target_zip.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            zip_copied = std::fs::copy(zip, &target_zip).is_ok();
        }
    }

    if md_copied && zip_copied {
        if let Some(staging_dir) = md_path_buf.parent() {
            remove_dir_all_ignore(staging_dir);
        }
    }

    let _ = sqlx::query(
        "UPDATE documents SET status = 'markdown_done', status_message = 'PDF 解析完成', \
         progress = 100, mineru_task_id = NULL, updated_at = ? WHERE id = ?",
    )
    .bind(now_iso())
    .bind(doc_id)
    .execute(db.pool())
    .await;
}

/// 从 Value 中取字符串字段（非空）
fn value_str(v: &Value, key: &str) -> Option<String> {
    v.get(key).and_then(|x| x.as_str()).map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

/// 从 Value 中取整数字段
fn value_i64(v: &Value, key: &str) -> Option<i64> {
    v.get(key).and_then(|x| x.as_i64())
}

/// 从 Value 中取字符串数组字段（转为 JSON 字符串）
fn value_list_json(v: &Value, key: &str) -> Option<String> {
    v.get(key)
        .and_then(|x| x.as_array())
        .filter(|a| !a.is_empty())
        .map(|a| serde_json::to_string(a).unwrap_or_else(|_| "[]".to_string()))
}

async fn do_extract_impl(db: Arc<Database>, settings: Arc<Settings>, doc_id: i64) {
    let doc = match fetch_doc(&db, doc_id).await {
        Some(d) => d,
        None => return,
    };

    let md_path = paths::get_markdown_path(&settings, doc_id);
    if !std::path::Path::new(&md_path).exists() {
        return;
    }

    // 已有标题、作者、年份则跳过提取
    if doc.title.as_deref().map(|s| !s.trim().is_empty()).unwrap_or(false)
        && doc.authors.as_deref().map(|s| !s.trim().is_empty()).unwrap_or(false)
        && doc.year.is_some()
    {
        return;
    }

    let markdown_content = match std::fs::read_to_string(&md_path) {
        Ok(c) => c,
        Err(_) => return,
    };

    let ollama_key = settings.ollama_key();
    let ollama = OllamaClient::new(
        &settings.ollama_url(),
        if ollama_key.is_empty() { None } else { Some(&ollama_key) },
        &settings.ollama_model(),
    );

    let metadata = match ollama.extract_metadata(&markdown_content).await {
        Ok(m) => m,
        Err(_) => return,
    };

    let title = value_str(&metadata, "title").unwrap_or_default();
    let authors = metadata.get("authors").and_then(|a| a.as_array()).cloned().unwrap_or_default();
    let doc_type = normalize_doc_type(metadata.get("type").and_then(|t| t.as_str()).unwrap_or(""));

    if title.is_empty() && authors.is_empty() {
        return;
    }

    // 仅填充空字段
    let authors_json = serde_json::to_string(&authors).unwrap_or_else(|_| "[]".to_string());
    let keywords_json = value_list_json(&metadata, "keywords");
    let authors_en_json = value_list_json(&metadata, "authors_en");
    let keywords_en_json = value_list_json(&metadata, "keywords_en");

    let language = value_str(&metadata, "language").unwrap_or_else(|| "en".to_string());

    // 使用显式 UPDATE 仅填充空字段（用 COALESCE 语义）
    let _ = sqlx::query(
        "UPDATE documents SET \
            title = COALESCE(NULLIF(title, ''), ?), \
            authors = COALESCE(NULLIF(authors, ''), ?), \
            year = COALESCE(year, ?), \
            doi = COALESCE(NULLIF(doi, ''), ?), \
            source = COALESCE(NULLIF(source, ''), ?), \
            journal = COALESCE(NULLIF(journal, ''), ?), \
            keywords = COALESCE(NULLIF(keywords, ''), ?), \
            abstract = COALESCE(NULLIF(abstract, ''), ?), \
            category = COALESCE(NULLIF(category, ''), ?), \
            doc_type = COALESCE(NULLIF(doc_type, ''), ?), \
            language = COALESCE(NULLIF(language, ''), ?), \
            updated_at = ? WHERE id = ?",
    )
    .bind(&title)
    .bind(&authors_json)
    .bind(value_i64(&metadata, "year"))
    .bind(value_str(&metadata, "doi").unwrap_or_default())
    .bind(value_str(&metadata, "source").unwrap_or_default())
    .bind(value_str(&metadata, "journal").unwrap_or_default())
    .bind(keywords_json.unwrap_or_default())
    .bind(value_str(&metadata, "abstract").unwrap_or_default())
    .bind(value_str(&metadata, "category").unwrap_or_default())
    .bind(&doc_type)
    .bind(&language)
    .bind(now_iso())
    .bind(doc_id)
    .execute(db.pool())
    .await;

    // English fields for non-English documents
    if language != "en" {
        let _ = sqlx::query(
            "UPDATE documents SET \
                title_en = COALESCE(NULLIF(title_en, ''), ?), \
                authors_en = COALESCE(NULLIF(authors_en, ''), ?), \
                keywords_en = COALESCE(NULLIF(keywords_en, ''), ?), \
                abstract_en = COALESCE(NULLIF(abstract_en, ''), ?), \
                journal_en = COALESCE(NULLIF(journal_en, ''), ?) \
                WHERE id = ?",
        )
        .bind(value_str(&metadata, "title_en").unwrap_or_default())
        .bind(authors_en_json.unwrap_or_default())
        .bind(keywords_en_json.unwrap_or_default())
        .bind(value_str(&metadata, "abstract_en").unwrap_or_default())
        .bind(value_str(&metadata, "journal_en").unwrap_or_default())
        .bind(doc_id)
        .execute(db.pool())
        .await;
    }
}

async fn do_index_impl(db: Arc<Database>, settings: Arc<Settings>, doc_id: i64, vector_db_id: String) {
    let vector_db_id = if vector_db_id.trim().is_empty() { "default".to_string() } else { vector_db_id };

    // 获取或创建索引状态记录
    let existing: Option<DocumentVectorIndex> = sqlx::query_as(
        "SELECT id, document_id, vector_db_id, collection_name, status, error_message, created_at, updated_at \
         FROM document_vector_index WHERE document_id = ? AND vector_db_id = ? LIMIT 1",
    )
    .bind(doc_id)
    .bind(&vector_db_id)
    .fetch_optional(db.pool())
    .await
    .unwrap_or(None);

    if existing.is_none() {
        let _ = sqlx::query(
            "INSERT INTO document_vector_index (document_id, vector_db_id, status, created_at, updated_at) \
             VALUES (?, ?, 'pending', ?, ?)",
        )
        .bind(doc_id)
        .bind(&vector_db_id)
        .bind(now_iso())
        .bind(now_iso())
        .execute(db.pool())
        .await;
    }

    let _ = sqlx::query(
        "UPDATE document_vector_index SET status = 'indexing', error_message = NULL, updated_at = ? \
         WHERE document_id = ? AND vector_db_id = ?",
    )
    .bind(now_iso())
    .bind(doc_id)
    .bind(&vector_db_id)
    .execute(db.pool())
    .await;

    let doc = match fetch_doc(&db, doc_id).await {
        Some(d) => d,
        None => {
            set_index_error(&db, doc_id, &vector_db_id, "文档不存在").await;
            return;
        }
    };

    let md_path = paths::get_markdown_path(&settings, doc_id);
    if !std::path::Path::new(&md_path).exists() {
        set_index_error(&db, doc_id, &vector_db_id, "Markdown 文件不存在").await;
        return;
    }

    // 获取向量数据库配置
    let manager = crate::config_manager::ConfigManager::new("system.json");
    let vdb_config = match manager.get_vector_db_by_id(&vector_db_id) {
        Some(c) => c,
        None => {
            let msg = format!("向量数据库 {} 配置不存在", vector_db_id);
            set_index_error(&db, doc_id, &vector_db_id, &msg).await;
            return;
        }
    };

    let markdown_content = match std::fs::read_to_string(&md_path) {
        Ok(c) => c,
        Err(e) => {
            set_index_error(&db, doc_id, &vector_db_id, &e.to_string()).await;
            return;
        }
    };

    let metadata = json!({
        "title": doc.title.clone().unwrap_or_default(),
        "authors": doc.authors.as_deref().and_then(|a| serde_json::from_str::<Value>(a).ok()).unwrap_or_else(|| json!([])),
        "year": doc.year,
        "type": doc.doc_type.clone().unwrap_or_default(),
        "keywords": doc.keywords.as_deref().and_then(|k| serde_json::from_str::<Value>(k).ok()).unwrap_or_else(|| json!([])),
    });

    let chunks = chunk_text_by_markdown(&markdown_content, CHUNK_SIZE, MIN_CHUNK_SIZE);
    if chunks.is_empty() {
        set_index_error(&db, doc_id, &vector_db_id, "Markdown 内容为空，无法索引").await;
        return;
    }

    // embedding 模型来自 vdb_config 而非全局 settings
    let embed_model = vdb_config["embedding"]["model"].as_str().unwrap_or("nomic-embed-text").to_string();
    let ollama_key = settings.ollama_key();
    let ollama = OllamaClient::new(
        &settings.ollama_url(),
        if ollama_key.is_empty() { None } else { Some(&ollama_key) },
        &embed_model,
    );

    let embeddings = match ollama.get_embeddings_batch(&chunks).await {
        Ok(e) => e,
        Err(e) => {
            set_index_error(&db, doc_id, &vector_db_id, &e.to_string()).await;
            return;
        }
    };

    let adapter = match get_vector_db(Some(&vector_db_id)) {
        Ok(a) => a,
        Err(e) => {
            set_index_error(&db, doc_id, &vector_db_id, &e.to_string()).await;
            return;
        }
    };

    let collection_name = match adapter.index_document(doc_id, QDRANT_USER_ID, &chunks, &embeddings, &metadata).await {
        Ok(c) => c,
        Err(e) => {
            set_index_error(&db, doc_id, &vector_db_id, &e.to_string()).await;
            return;
        }
    };

    let _ = sqlx::query(
        "UPDATE document_vector_index SET collection_name = ?, status = 'indexed', updated_at = ? \
         WHERE document_id = ? AND vector_db_id = ?",
    )
    .bind(&collection_name)
    .bind(now_iso())
    .bind(doc_id)
    .bind(&vector_db_id)
    .execute(db.pool())
    .await;

    let _ = sqlx::query(
        "UPDATE documents SET qdrant_collection = ?, vector_db_id = ?, \
         status_message = ?, progress = 100, updated_at = ? WHERE id = ?",
    )
    .bind(&collection_name)
    .bind(&vector_db_id)
    .bind(format!("已索引到 {}", vector_db_id))
    .bind(now_iso())
    .bind(doc_id)
    .execute(db.pool())
    .await;
}

async fn set_index_error(db: &Database, doc_id: i64, vector_db_id: &str, msg: &str) {
    let _ = sqlx::query(
        "UPDATE document_vector_index SET status = 'error', error_message = ?, updated_at = ? \
         WHERE document_id = ? AND vector_db_id = ?",
    )
    .bind(msg)
    .bind(now_iso())
    .bind(doc_id)
    .bind(vector_db_id)
    .execute(db.pool())
    .await;
}

async fn run_crossref(db: Arc<Database>, settings: Arc<Settings>, doc_id: i64) {
    let doc = match fetch_doc(&db, doc_id).await {
        Some(d) => d,
        None => return,
    };

    let client = CrossrefClient::new(None, None);
    let result = if let Some(doi) = doc.doi.as_deref() {
        if normalize_doi(doi).is_some() {
            client.lookup_by_doi(doi).await
        } else if let Some(title) = doc.title.as_deref() {
            client.lookup_by_title(title).await
        } else {
            None
        }
    } else if let Some(title) = doc.title.as_deref() {
        client.lookup_by_title(title).await
    } else {
        None
    };

    let result = match result {
        Some(r) => r,
        None => return,
    };

    // 保存原始 Crossref 返回
    let crossref_path = paths::get_crossref_path(&settings, doc_id);
    if let Some(parent) = std::path::Path::new(&crossref_path).parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(raw) = serde_json::to_string_pretty(&result["raw"]) {
        let _ = std::fs::write(&crossref_path, raw);
    }

    let parsed = &result["parsed"];
    let authors = parsed.get("authors").and_then(|a| a.as_array()).cloned().unwrap_or_default();
    let authors_json = serde_json::to_string(&authors).unwrap_or_else(|_| "[]".to_string());
    let keywords = parsed.get("keywords").and_then(|a| a.as_array()).cloned();
    let keywords_json = keywords.as_ref().map(|a| serde_json::to_string(a).unwrap_or_else(|_| "[]".to_string()));
    let authors_en = parsed.get("authors_en").and_then(|a| a.as_array()).cloned();
    let authors_en_json = authors_en.as_ref().map(|a| serde_json::to_string(a).unwrap_or_else(|_| "[]".to_string()));
    let keywords_en = parsed.get("keywords_en").and_then(|a| a.as_array()).cloned();
    let keywords_en_json = keywords_en.as_ref().map(|a| serde_json::to_string(a).unwrap_or_else(|_| "[]".to_string()));

    let doi_norm = value_str(parsed, "doi").and_then(|d| normalize_doi(&d));

    let _ = sqlx::query(
        "UPDATE documents SET \
            title = COALESCE(NULLIF(title, ''), ?), \
            authors = COALESCE(NULLIF(authors, ''), ?), \
            year = COALESCE(year, ?), \
            doi = COALESCE(NULLIF(doi, ''), ?), \
            source = COALESCE(NULLIF(source, ''), ?), \
            journal = COALESCE(NULLIF(journal, ''), ?), \
            keywords = COALESCE(NULLIF(keywords, ''), ?), \
            abstract = COALESCE(NULLIF(abstract, ''), ?), \
            doc_type = COALESCE(NULLIF(doc_type, ''), ?), \
            language = COALESCE(NULLIF(language, ''), ?), \
            title_en = COALESCE(NULLIF(title_en, ''), ?), \
            authors_en = COALESCE(NULLIF(authors_en, ''), ?), \
            journal_en = COALESCE(NULLIF(journal_en, ''), ?), \
            keywords_en = COALESCE(NULLIF(keywords_en, ''), ?), \
            abstract_en = COALESCE(NULLIF(abstract_en, ''), ?), \
            updated_at = ? WHERE id = ?",
    )
    .bind(value_str(parsed, "title").unwrap_or_default())
    .bind(&authors_json)
    .bind(value_i64(parsed, "year"))
    .bind(doi_norm.unwrap_or_default())
    .bind(value_str(parsed, "source").unwrap_or_default())
    .bind(value_str(parsed, "journal").unwrap_or_default())
    .bind(keywords_json.unwrap_or_default())
    .bind(value_str(parsed, "abstract").unwrap_or_default())
    .bind(value_str(parsed, "doc_type").unwrap_or_default())
    .bind(value_str(parsed, "language").unwrap_or_default())
    .bind(value_str(parsed, "title_en").unwrap_or_default())
    .bind(authors_en_json.unwrap_or_default())
    .bind(value_str(parsed, "journal_en").unwrap_or_default())
    .bind(keywords_en_json.unwrap_or_default())
    .bind(value_str(parsed, "abstract_en").unwrap_or_default())
    .bind(now_iso())
    .bind(doc_id)
    .execute(db.pool())
    .await;
}

async fn run_extract_pdf_meta(db: Arc<Database>, settings: Arc<Settings>, doc_id: i64) {
    let doc = match fetch_doc(&db, doc_id).await {
        Some(d) => d,
        None => return,
    };

    // 如果当前标题是占位符（如 "Entire document"），先清空以便后续更新
    if doc.title.as_deref().map_or(false, |t| looks_like_placeholder(t)) {
        let _ = sqlx::query("UPDATE documents SET title = '' WHERE id = ?")
            .bind(doc_id)
            .execute(db.pool())
            .await;
    }

    let pdf_path = paths::get_pdf_path(&settings, doc_id);
    let content = match std::fs::read(&pdf_path) {
        Ok(c) => c,
        Err(_) => return,
    };

    let meta = extract_pdf_metadata(&content, Some(&doc.filename), Some(&settings.pdfcpu_path()));

    let authors = meta.get("authors").and_then(|a| a.as_array()).cloned().unwrap_or_default();
    let authors_json = serde_json::to_string(&authors).unwrap_or_else(|_| "[]".to_string());
    let keywords = meta.get("keywords").and_then(|a| a.as_array()).cloned();
    let keywords_json = keywords.as_ref().map(|a| serde_json::to_string(a).unwrap_or_else(|_| "[]".to_string()));
    let doi_norm = value_str(&meta, "doi").and_then(|d| normalize_doi(&d));

    let _ = sqlx::query(
        "UPDATE documents SET \
            title = COALESCE(NULLIF(title, ''), ?), \
            authors = COALESCE(NULLIF(authors, ''), ?), \
            year = COALESCE(year, ?), \
            doi = COALESCE(NULLIF(doi, ''), ?), \
            keywords = COALESCE(NULLIF(keywords, ''), ?), \
            abstract = COALESCE(NULLIF(abstract, ''), ?), \
            updated_at = ? WHERE id = ?",
    )
    .bind(value_str(&meta, "title").unwrap_or_default())
    .bind(&authors_json)
    .bind(value_i64(&meta, "year"))
    .bind(doi_norm.unwrap_or_default())
    .bind(keywords_json.unwrap_or_default())
    .bind(value_str(&meta, "abstract").unwrap_or_default())
    .bind(now_iso())
    .bind(doc_id)
    .execute(db.pool())
    .await;
}

// ── 并发/追踪包装 ──

async fn run_tracked<F>(doc_id: i64, task_type: &'static str, fut: F)
where
    F: Future<Output = ()> + Send + 'static,
{
    let sem = task_semaphore();
    let _permit = sem.acquire().await.expect("task semaphore closed");
    RUNNING_TASKS.fetch_add(1, Ordering::SeqCst);
    track_task_start(doc_id, task_type);
    fut.await;
    track_task_end(doc_id);
    RUNNING_TASKS.fetch_sub(1, Ordering::SeqCst);
}

// ── 批量处理 ──

async fn process_stage_batch_parse(db: Arc<Database>, settings: Arc<Settings>) {
    let docs: Vec<Document> = sqlx::query_as::<_, Document>(&format!(
        "SELECT {} FROM documents WHERE status IN ('uploaded', 'error')", DOC_COLUMNS
    ))
    .fetch_all(db.pool())
    .await
    .unwrap_or_default();

    if docs.is_empty() {
        BATCH_PAUSED.store(false, Ordering::SeqCst);
        return;
    }

    let mut handles = Vec::new();
    for doc in docs {
        if BATCH_PAUSED.load(Ordering::SeqCst) {
            break;
        }
        let db = db.clone();
        let settings = settings.clone();
        handles.push(tokio::spawn(run_tracked(doc.id, "parse", do_parse_impl(db, settings, doc.id))));
    }
    for h in handles {
        let _ = h.await;
    }
    BATCH_PAUSED.store(false, Ordering::SeqCst);
}

async fn process_stage_batch_extract(db: Arc<Database>, settings: Arc<Settings>) {
    let docs: Vec<Document> = sqlx::query_as::<_, Document>(&format!(
        "SELECT {} FROM documents WHERE status = 'markdown_done'", DOC_COLUMNS
    ))
    .fetch_all(db.pool())
    .await
    .unwrap_or_default();

    if docs.is_empty() {
        BATCH_PAUSED.store(false, Ordering::SeqCst);
        return;
    }

    let mut handles = Vec::new();
    for doc in docs {
        if BATCH_PAUSED.load(Ordering::SeqCst) {
            break;
        }
        let db = db.clone();
        let settings = settings.clone();
        handles.push(tokio::spawn(run_tracked(doc.id, "extract", do_extract_impl(db, settings, doc.id))));
    }
    for h in handles {
        let _ = h.await;
    }
    BATCH_PAUSED.store(false, Ordering::SeqCst);
}

async fn process_index_batch(
    db: Arc<Database>,
    settings: Arc<Settings>,
    vector_db_id: String,
    limit: i64,
    batch_size: i64,
    pause_seconds: i64,
) {
    // 查询待索引文档（排除已索引到该库的）
    let rows: Vec<(i64,)> = sqlx::query_as(
        "SELECT d.id FROM documents d \
         WHERE d.status = 'markdown_done' \
         AND d.id NOT IN (SELECT document_id FROM document_vector_index WHERE vector_db_id = ? AND status = 'indexed') \
         ORDER BY d.id",
    )
    .bind(&vector_db_id)
    .fetch_all(db.pool())
    .await
    .unwrap_or_default();

    let mut doc_ids: Vec<i64> = rows.into_iter().map(|(id,)| id).collect();
    if limit > 0 {
        doc_ids.truncate(limit as usize);
    }

    if doc_ids.is_empty() {
        BATCH_PAUSED.store(false, Ordering::SeqCst);
        let mut p = batch_progress_map().write().unwrap();
        p["active"] = json!(false);
        return;
    }

    let total = doc_ids.len() as i64;
    let effective_batch_size = if batch_size > 0 { batch_size } else { total };
    let num_batches = (total + effective_batch_size - 1) / effective_batch_size;

    {
        let mut p = batch_progress_map().write().unwrap();
        *p = json!({
            "active": true,
            "stage": "index",
            "vector_db_id": vector_db_id,
            "total": total,
            "processed": 0,
            "current_batch": 0,
            "total_batches": num_batches,
            "batch_size": batch_size,
            "pause_seconds": pause_seconds,
            "started_at": now_iso(),
            "errors": 0,
        });
    }

    let mut processed = 0i64;

    if batch_size <= 0 || batch_size >= total {
        {
            let mut p = batch_progress_map().write().unwrap();
            p["current_batch"] = json!(1);
            p["total_batches"] = json!(1);
        }
        let mut handles = Vec::new();
        for id in &doc_ids {
            if BATCH_PAUSED.load(Ordering::SeqCst) {
                break;
            }
            let db = db.clone();
            let settings = settings.clone();
            let vdb = vector_db_id.clone();
            handles.push(tokio::spawn(run_tracked(*id, "index", do_index_impl(db, settings, *id, vdb))));
        }
        for h in handles {
            let _ = h.await;
        }
        processed = doc_ids.len() as i64;
        let mut p = batch_progress_map().write().unwrap();
        p["processed"] = json!(processed);
    } else {
        let batches: Vec<&[i64]> = doc_ids.chunks(batch_size as usize).collect();
        for (batch_idx, batch_ids) in batches.iter().enumerate() {
            if BATCH_PAUSED.load(Ordering::SeqCst) {
                break;
            }
            {
                let mut p = batch_progress_map().write().unwrap();
                p["current_batch"] = json!(batch_idx + 1);
            }
            let mut handles = Vec::new();
            for id in *batch_ids {
                if BATCH_PAUSED.load(Ordering::SeqCst) {
                    break;
                }
                let db = db.clone();
                let settings = settings.clone();
                let vdb = vector_db_id.clone();
                handles.push(tokio::spawn(run_tracked(*id, "index", do_index_impl(db, settings, *id, vdb))));
            }
            for h in handles {
                let _ = h.await;
            }
            processed += batch_ids.len() as i64;
            {
                let mut p = batch_progress_map().write().unwrap();
                p["processed"] = json!(processed);
            }
            if pause_seconds > 0 && batch_idx < batches.len() - 1 && !BATCH_PAUSED.load(Ordering::SeqCst) {
                tokio::time::sleep(std::time::Duration::from_secs(pause_seconds as u64)).await;
            }
        }
    }

    BATCH_PAUSED.store(false, Ordering::SeqCst);
    let mut p = batch_progress_map().write().unwrap();
    p["active"] = json!(false);
}

// ── 路由 ──

pub fn router() -> axum::Router<()> {
    axum::Router::new()
        .route("/assist/api/pipeline/queue/status/", get(queue_status))
        .route("/assist/api/pipeline/queue/status", get(queue_status))
        .route("/assist/api/pipeline/tasks/active/", get(active_tasks))
        .route("/assist/api/pipeline/tasks/active", get(active_tasks))
        .route("/assist/api/pipeline/batch/status/", get(batch_status))
        .route("/assist/api/pipeline/batch/status", get(batch_status))
        .route("/assist/api/pipeline/batch/progress/", get(batch_progress))
        .route("/assist/api/pipeline/batch/progress", get(batch_progress))
        .route("/assist/api/pipeline/batch/start/", post(batch_start))
        .route("/assist/api/pipeline/batch/start", post(batch_start))
        .route("/assist/api/pipeline/batch/start-parse/", post(batch_start_parse))
        .route("/assist/api/pipeline/batch/start-parse", post(batch_start_parse))
        .route("/assist/api/pipeline/batch/start-extract/", post(batch_start_extract))
        .route("/assist/api/pipeline/batch/start-extract", post(batch_start_extract))
        .route("/assist/api/pipeline/batch/start-index/", post(batch_start_index))
        .route("/assist/api/pipeline/batch/start-index", post(batch_start_index))
        .route("/assist/api/pipeline/batch/start-full/", post(batch_start_full))
        .route("/assist/api/pipeline/batch/start-full", post(batch_start_full))
        .route("/assist/api/pipeline/batch/pause/", post(batch_pause))
        .route("/assist/api/pipeline/batch/pause", post(batch_pause))
        .route("/assist/api/pipeline/batch/resume/", post(batch_resume))
        .route("/assist/api/pipeline/batch/resume", post(batch_resume))
        .route("/assist/api/pipeline/batch/reset/", post(batch_reset))
        .route("/assist/api/pipeline/batch/reset", post(batch_reset))
        .route("/assist/api/pipeline/batch/reset-errors/", post(batch_reset_errors))
        .route("/assist/api/pipeline/batch/reset-errors", post(batch_reset_errors))
        .route("/assist/api/pipeline/batch/reset-timeout-errors/", post(batch_reset_timeout_errors))
        .route("/assist/api/pipeline/batch/reset-timeout-errors", post(batch_reset_timeout_errors))
        .route("/assist/api/pipeline/batch/promote-ready/", post(batch_promote_ready))
        .route("/assist/api/pipeline/batch/promote-ready", post(batch_promote_ready))
        .route("/assist/api/pipeline/parse/:id/", post(parse))
        .route("/assist/api/pipeline/parse/:id", post(parse))
        .route("/assist/api/pipeline/extract/:id/", post(extract))
        .route("/assist/api/pipeline/extract/:id", post(extract))
        .route("/assist/api/pipeline/crossref/:id/", post(crossref))
        .route("/assist/api/pipeline/crossref/:id", post(crossref))
        .route("/assist/api/pipeline/extract-pdf-meta/:id/", post(extract_pdf_meta))
        .route("/assist/api/pipeline/extract-pdf-meta/:id", post(extract_pdf_meta))
        .route("/assist/api/pipeline/index/:id/", post(index))
        .route("/assist/api/pipeline/index/:id", post(index))
        .route("/assist/api/pipeline/indexes/:id/", get(indexes))
        .route("/assist/api/pipeline/indexes/:id", get(indexes))
        .route("/assist/api/pipeline/status/:id/", get(doc_status))
        .route("/assist/api/pipeline/status/:id", get(doc_status))
        .route("/assist/api/pipeline/reset/:id/", post(reset))
        .route("/assist/api/pipeline/reset/:id", post(reset))
        .route("/assist/api/pipeline/full/:id/", post(full))
        .route("/assist/api/pipeline/full/:id", post(full))
        .route("/assist/api/pipeline/stop/:id/", post(stop))
        .route("/assist/api/pipeline/stop/:id", post(stop))
}

// ── 查询参数 ──

#[derive(Debug, Deserialize)]
pub struct IndexQuery {
    pub vector_db_id: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ResetQuery {
    pub target_status: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct StartIndexQuery {
    pub vector_db_id: Option<String>,
    pub limit: Option<i64>,
    pub batch_size: Option<i64>,
    pub pause_seconds: Option<i64>,
}

// ── 状态端点 ──

async fn queue_status() -> Json<Value> {
    let running = RUNNING_TASKS.load(Ordering::SeqCst) as i64;
    let max = max_concurrent_tasks() as i64;
    Json(json!({
        "max_concurrent_tasks": max,
        "running_tasks": running,
        "available_slots": (max - running).max(0),
    }))
}

async fn active_tasks(Extension(db): Extension<Arc<Database>>) -> Json<Value> {
    let snapshot: Vec<(i64, ActiveTask)> = {
        let map = active_tasks_map().read().unwrap();
        map.iter().map(|(k, v)| (*k, v.clone())).collect()
    };

    let mut tasks = Vec::new();
    for (doc_id, info) in snapshot {
        if let Some(doc) = fetch_doc(&db, doc_id).await {
            tasks.push(json!({
                "doc_id": doc_id,
                "doc_title": doc.title.clone().unwrap_or_else(|| doc.filename.clone()),
                "task_type": info.task_type,
                "started_at": info.started_at,
                "status_message": doc.status_message.clone().unwrap_or(info.status_message),
                "status": doc.status.clone(),
            }));
        }
    }
    Json(json!({"tasks": tasks, "count": tasks.len()}))
}

async fn batch_status(Extension(db): Extension<Arc<Database>>) -> Json<Value> {
    let pending = count_status(&db, "uploaded").await + count_status(&db, "error").await;
    let processing = count_status(&db, "parsing").await;
    let total: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM documents")
        .fetch_one(db.pool())
        .await
        .unwrap_or(0);
    let timeout_error: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM documents WHERE status = 'error' AND status_message LIKE '%timed out%'",
    )
    .fetch_one(db.pool())
    .await
    .unwrap_or(0);
    let ready_to_promote: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM documents WHERE status = 'markdown_done' \
         AND title IS NOT NULL AND title != '' AND authors IS NOT NULL AND authors != ''",
    )
    .fetch_one(db.pool())
    .await
    .unwrap_or(0);

    let uploaded = count_status(&db, "uploaded").await;
    let error = count_status(&db, "error").await;
    let markdown_done = count_status(&db, "markdown_done").await;

    let batch_progress = batch_progress_map().read().unwrap().clone();

    Json(json!({
        "paused": BATCH_PAUSED.load(Ordering::SeqCst),
        "pending": pending,
        "processing": processing,
        "completed": 0,
        "total": total,
        "running_tasks": RUNNING_TASKS.load(Ordering::SeqCst),
        "max_concurrent": max_concurrent_tasks(),
        "stage_counts": {
            "uploaded": uploaded,
            "error": error,
            "timeout_error": timeout_error,
            "markdown_done": markdown_done,
            "ready_to_promote": ready_to_promote,
        },
        "batch_progress": batch_progress,
    }))
}

async fn batch_progress() -> Json<Value> {
    Json(batch_progress_map().read().unwrap().clone())
}

// ── 批量控制端点 ──

async fn batch_start(Extension(db): Extension<Arc<Database>>, Extension(settings): Extension<Arc<Settings>>) -> Response {
    let pending = count_status(&db, "uploaded").await + count_status(&db, "error").await;
    if pending == 0 {
        return Json(json!({"detail": "没有待解析的文档", "pending": 0})).into_response();
    }
    BATCH_PAUSED.store(false, Ordering::SeqCst);
    tokio::spawn(process_stage_batch_parse(db, settings));
    Json(json!({"detail": format!("批量解析已开始，共 {} 个文档待处理", pending), "pending": pending})).into_response()
}

async fn batch_pause() -> Json<Value> {
    BATCH_PAUSED.store(true, Ordering::SeqCst);
    Json(json!({"detail": "批量处理将在当前任务完成后暂停"}))
}

async fn batch_resume(Extension(db): Extension<Arc<Database>>, Extension(settings): Extension<Arc<Settings>>) -> Json<Value> {
    BATCH_PAUSED.store(false, Ordering::SeqCst);
    tokio::spawn(process_stage_batch_parse(db, settings));
    Json(json!({"detail": "批量解析已恢复"}))
}

async fn batch_reset() -> Json<Value> {
    Json(json!({"status": "ok"}))
}

async fn batch_start_parse(Extension(db): Extension<Arc<Database>>, Extension(settings): Extension<Arc<Settings>>) -> Response {
    let count = count_status(&db, "uploaded").await + count_status(&db, "error").await;
    if count == 0 {
        return Json(json!({"detail": "没有待解析的文档", "pending": 0})).into_response();
    }
    BATCH_PAUSED.store(false, Ordering::SeqCst);
    tokio::spawn(process_stage_batch_parse(db, settings));
    Json(json!({"detail": format!("批量解析已开始，共 {} 个文档", count), "pending": count})).into_response()
}

async fn batch_start_extract(Extension(db): Extension<Arc<Database>>, Extension(settings): Extension<Arc<Settings>>) -> Response {
    let count = count_status(&db, "markdown_done").await;
    if count == 0 {
        return Json(json!({"detail": "没有待提取元数据的文档", "pending": 0})).into_response();
    }
    BATCH_PAUSED.store(false, Ordering::SeqCst);
    tokio::spawn(process_stage_batch_extract(db, settings));
    Json(json!({"detail": format!("批量提取已开始，共 {} 个文档", count), "pending": count})).into_response()
}

async fn batch_start_index(
    Extension(db): Extension<Arc<Database>>,
    Extension(settings): Extension<Arc<Settings>>,
    Query(query): Query<StartIndexQuery>,
) -> Response {
    let vector_db_id = query.vector_db_id.unwrap_or_else(|| "default".to_string());

    let manager = crate::config_manager::ConfigManager::new("system.json");
    if manager.get_vector_db_by_id(&vector_db_id).is_none() {
        return (StatusCode::BAD_REQUEST, Json(json!({"detail": format!("向量数据库 {} 配置不存在", vector_db_id)}))).into_response();
    }

    let total_available: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM documents d \
         WHERE d.status = 'markdown_done' \
         AND d.id NOT IN (SELECT document_id FROM document_vector_index WHERE vector_db_id = ? AND status = 'indexed')",
    )
    .bind(&vector_db_id)
    .fetch_one(db.pool())
    .await
    .unwrap_or(0);

    if total_available == 0 {
        return Json(json!({"detail": "没有待索引的文档", "pending": 0})).into_response();
    }

    let limit = query.limit.unwrap_or(0);
    let batch_size = query.batch_size.unwrap_or(0);
    let pause_seconds = query.pause_seconds.unwrap_or(0);
    let actual_limit = if limit > 0 { limit.min(total_available) } else { total_available };

    BATCH_PAUSED.store(false, Ordering::SeqCst);
    tokio::spawn(process_index_batch(db, settings, vector_db_id.clone(), limit, batch_size, pause_seconds));

    let mut detail = format!("批量索引到 {} 已开始，共 {} 个文档", vector_db_id, actual_limit);
    if batch_size > 0 && batch_size < actual_limit {
        detail.push_str(&format!("，每批 {} 个", batch_size));
    }
    if pause_seconds > 0 {
        detail.push_str(&format!("，批间暂停 {}s", pause_seconds));
    }

    Json(json!({
        "detail": detail,
        "pending": actual_limit,
        "total_available": total_available,
    })).into_response()
}

async fn batch_start_full() -> Json<Value> {
    Json(json!({"status": "ok", "message": "请使用 batch/start-index 或单文档 full 端点"}))
}

// ── 批量错误重置端点 ──

async fn batch_reset_errors(
    Extension(db): Extension<Arc<Database>>,
    Extension(settings): Extension<Arc<Settings>>,
    Query(query): Query<ResetQuery>,
) -> Json<Value> {
    let docs: Vec<Document> = sqlx::query_as::<_, Document>(&format!(
        "SELECT {} FROM documents WHERE status = 'error'", DOC_COLUMNS
    ))
    .fetch_all(db.pool())
    .await
    .unwrap_or_default();

    if docs.is_empty() {
        return Json(json!({"detail": "没有错误状态的文档", "reset_count": 0}));
    }

    let mut reset_count = 0i64;
    for doc in &docs {
        let new_status = match query.target_status.as_deref() {
            Some("uploaded") => "uploaded",
            Some("markdown_done") => "markdown_done",
            _ => {
                if std::path::Path::new(&paths::get_markdown_path(&settings, doc.id)).exists() {
                    "markdown_done"
                } else {
                    "uploaded"
                }
            }
        };
        let _ = sqlx::query(
            "UPDATE documents SET status = ?, status_message = NULL, progress = 0, mineru_task_id = NULL WHERE id = ?",
        )
        .bind(new_status)
        .bind(doc.id)
        .execute(db.pool())
        .await;
        reset_count += 1;
    }

    Json(json!({"detail": format!("已重置 {} 个错误状态的文档", reset_count), "reset_count": reset_count}))
}

async fn batch_reset_timeout_errors(
    Extension(db): Extension<Arc<Database>>,
    Extension(settings): Extension<Arc<Settings>>,
    Query(query): Query<ResetQuery>,
) -> Json<Value> {
    let docs: Vec<Document> = sqlx::query_as::<_, Document>(&format!(
        "SELECT {} FROM documents WHERE status = 'error' AND status_message LIKE '%timed out%'", DOC_COLUMNS
    ))
    .fetch_all(db.pool())
    .await
    .unwrap_or_default();

    if docs.is_empty() {
        return Json(json!({"detail": "没有超时失败的文档", "reset_count": 0}));
    }

    let mut reset_count = 0i64;
    for doc in &docs {
        let new_status = match query.target_status.as_deref() {
            Some("uploaded") => "uploaded",
            Some("markdown_done") => "markdown_done",
            _ => {
                if std::path::Path::new(&paths::get_markdown_path(&settings, doc.id)).exists() {
                    "markdown_done"
                } else {
                    "uploaded"
                }
            }
        };
        let _ = sqlx::query(
            "UPDATE documents SET status = ?, status_message = NULL, progress = 0, mineru_task_id = NULL WHERE id = ?",
        )
        .bind(new_status)
        .bind(doc.id)
        .execute(db.pool())
        .await;
        reset_count += 1;
    }

    Json(json!({"detail": format!("已重置 {} 个超时失败的文档", reset_count), "reset_count": reset_count}))
}

async fn batch_promote_ready(Extension(db): Extension<Arc<Database>>) -> Json<Value> {
    let docs: Vec<Document> = sqlx::query_as::<_, Document>(&format!(
        "SELECT {} FROM documents WHERE status = 'markdown_done' \
         AND title IS NOT NULL AND title != '' AND authors IS NOT NULL AND authors != ''", DOC_COLUMNS
    ))
    .fetch_all(db.pool())
    .await
    .unwrap_or_default();

    if docs.is_empty() {
        return Json(json!({"detail": "没有符合条件的文档", "promote_count": 0}));
    }

    let promote_count = docs.len() as i64;

    Json(json!({"detail": format!("已找到 {} 个可提取元数据的文档", promote_count), "promote_count": promote_count}))
}

// ── 单文档端点 ──

async fn parse(
    Extension(db): Extension<Arc<Database>>,
    Extension(settings): Extension<Arc<Settings>>,
    Path(id): Path<i64>,
) -> Response {
    let doc = match fetch_doc(&db, id).await {
        Some(d) => d,
        None => return (StatusCode::NOT_FOUND, Json(json!({"detail": "文献不存在"}))).into_response(),
    };

    let status = doc.doc_status();
    if status != DocStatus::Uploaded && status != DocStatus::Error {
        return (StatusCode::BAD_REQUEST, Json(json!({"detail": format!("当前状态 {} 不允许解析", status)}))).into_response();
    }

    let _ = sqlx::query(
        "UPDATE documents SET status = 'parsing', status_message = '任务已提交，等待处理...', progress = 0, updated_at = ? WHERE id = ?",
    )
    .bind(now_iso())
    .bind(id)
    .execute(db.pool())
    .await;

    tokio::spawn(run_tracked(id, "parse", do_parse_impl(db, settings, id)));

    Json(json!({"detail": "解析任务已提交", "status": "parsing"})).into_response()
}

async fn extract(
    Extension(db): Extension<Arc<Database>>,
    Extension(settings): Extension<Arc<Settings>>,
    Path(id): Path<i64>,
) -> Response {
    let doc = match fetch_doc(&db, id).await {
        Some(d) => d,
        None => return (StatusCode::NOT_FOUND, Json(json!({"detail": "文献不存在"}))).into_response(),
    };

    if doc.doc_status() != DocStatus::MarkdownDone {
        return (StatusCode::BAD_REQUEST, Json(json!({"detail": format!("当前状态 {} 不允许提取元数据", doc.status)}))).into_response();
    }

    tokio::spawn(run_tracked(id, "extract", do_extract_impl(db, settings, id)));

    Json(json!({"detail": "元数据提取任务已提交", "status": "markdown_done"})).into_response()
}

async fn crossref(
    Extension(db): Extension<Arc<Database>>,
    Extension(settings): Extension<Arc<Settings>>,
    Path(id): Path<i64>,
) -> Response {
    if fetch_doc(&db, id).await.is_none() {
        return (StatusCode::NOT_FOUND, Json(json!({"detail": "文献不存在"}))).into_response();
    }
    tokio::spawn(run_crossref(db, settings, id));
    Json(json!({"detail": "Crossref 获取任务已提交", "status": "crossref"})).into_response()
}

async fn extract_pdf_meta(
    Extension(db): Extension<Arc<Database>>,
    Extension(settings): Extension<Arc<Settings>>,
    Path(id): Path<i64>,
) -> Response {
    if fetch_doc(&db, id).await.is_none() {
        return (StatusCode::NOT_FOUND, Json(json!({"detail": "文献不存在"}))).into_response();
    }
    tokio::spawn(run_extract_pdf_meta(db, settings, id));
    Json(json!({"detail": "从 PDF 元数据提取任务已提交", "status": "extract_pdf_meta"})).into_response()
}

async fn index(
    Extension(db): Extension<Arc<Database>>,
    Extension(settings): Extension<Arc<Settings>>,
    Path(id): Path<i64>,
    Query(query): Query<IndexQuery>,
) -> Response {
    let vector_db_id = query.vector_db_id.unwrap_or_else(|| "default".to_string());

    let doc = match fetch_doc(&db, id).await {
        Some(d) => d,
        None => return (StatusCode::NOT_FOUND, Json(json!({"detail": "文献不存在"}))).into_response(),
    };

    let allowed = [DocStatus::Error, DocStatus::MarkdownDone];
    if !allowed.contains(&doc.doc_status()) {
        return (StatusCode::BAD_REQUEST, Json(json!({"detail": format!("当前状态 {} 不允许索引", doc.status)}))).into_response();
    }

    if !std::path::Path::new(&paths::get_markdown_path(&settings, id)).exists() {
        return (StatusCode::BAD_REQUEST, Json(json!({"detail": "Markdown 文件不存在，无法索引"}))).into_response();
    }

    let manager = crate::config_manager::ConfigManager::new("system.json");
    if manager.get_vector_db_by_id(&vector_db_id).is_none() {
        return (StatusCode::BAD_REQUEST, Json(json!({"detail": format!("向量数据库 {} 配置不存在", vector_db_id)}))).into_response();
    }

    tokio::spawn(run_tracked(id, "index", do_index_impl(db, settings, id, vector_db_id.clone())));

    Json(json!({"detail": format!("索引任务已提交到 {}", vector_db_id), "status": "markdown_done"})).into_response()
}

async fn indexes(
    Extension(db): Extension<Arc<Database>>,
    Path(id): Path<i64>,
) -> Response {
    if fetch_doc(&db, id).await.is_none() {
        return (StatusCode::NOT_FOUND, Json(json!({"detail": "文献不存在"}))).into_response();
    }

    let rows: Vec<DocumentVectorIndex> = sqlx::query_as(
        "SELECT id, document_id, vector_db_id, collection_name, status, error_message, created_at, updated_at \
         FROM document_vector_index WHERE document_id = ?",
    )
    .bind(id)
    .fetch_all(db.pool())
    .await
    .unwrap_or_default();

    let indexes: Vec<Value> = rows.iter().map(|idx| {
        json!({
            "vector_db_id": idx.vector_db_id,
            "collection_name": idx.collection_name,
            "status": idx.status,
            "error_message": idx.error_message,
            "updated_at": idx.updated_at,
        })
    }).collect();

    Json(json!({"document_id": id, "indexes": indexes})).into_response()
}

async fn doc_status(
    Extension(db): Extension<Arc<Database>>,
    Extension(settings): Extension<Arc<Settings>>,
    Path(id): Path<i64>,
) -> Response {
    let doc = match fetch_doc(&db, id).await {
        Some(d) => d,
        None => return (StatusCode::NOT_FOUND, Json(json!({"detail": "文献不存在"}))).into_response(),
    };

    let has_markdown = std::path::Path::new(&paths::get_markdown_path(&settings, id)).exists();

    let rows: Vec<(String, String)> = sqlx::query_as(
        "SELECT vector_db_id, status FROM document_vector_index WHERE document_id = ? AND status = 'indexed'",
    )
    .bind(id)
    .fetch_all(db.pool())
    .await
    .unwrap_or_default();

    let indexed_dbs: Vec<Value> = rows.iter().map(|(vdb, status)| {
        json!({"vector_db_id": vdb, "status": status})
    }).collect();

    Json(json!({
        "status": doc.status,
        "status_message": doc.status_message,
        "progress": doc.progress,
        "has_markdown": has_markdown,
        "has_meta": doc.title.is_some(),
        "is_indexed": !indexed_dbs.is_empty(),
        "indexed_dbs": indexed_dbs,
    })).into_response()
}

async fn reset(
    Extension(db): Extension<Arc<Database>>,
    Extension(settings): Extension<Arc<Settings>>,
    Path(id): Path<i64>,
    Query(query): Query<ResetQuery>,
) -> Response {
    let doc = match fetch_doc(&db, id).await {
        Some(d) => d,
        None => return (StatusCode::NOT_FOUND, Json(json!({"detail": "文献不存在"}))).into_response(),
    };

    let current = doc.doc_status();
    let valid_targets: Vec<&str> = match current {
        DocStatus::Parsing => vec!["uploaded"],
        DocStatus::MarkdownDone => vec!["uploaded"],
        DocStatus::Error => vec!["uploaded", "markdown_done"],
        DocStatus::Uploaded => vec!["uploaded"],
    };

    let target = match query.target_status.as_deref() {
        Some(t) if valid_targets.contains(&t) => t.to_string(),
        _ => valid_targets.first().map(|s| s.to_string()).unwrap_or_else(|| "uploaded".to_string()),
    };

    // 清理下游数据
    if target == "uploaded" || target == "markdown_done" {
        let _ = sqlx::query("UPDATE documents SET qdrant_collection = NULL WHERE id = ?")
            .bind(id)
            .execute(db.pool())
            .await;
    }

    let _ = sqlx::query(
        "UPDATE documents SET status = ?, status_message = NULL, progress = 0, mineru_task_id = NULL, updated_at = ? WHERE id = ?",
    )
    .bind(&target)
    .bind(now_iso())
    .bind(id)
    .execute(db.pool())
    .await;

    let _ = (settings,);

    Json(json!({
        "detail": format!("状态已重置为 {}", target),
        "status": target,
        "valid_targets": valid_targets,
    })).into_response()
}

async fn full(
    Extension(db): Extension<Arc<Database>>,
    Extension(settings): Extension<Arc<Settings>>,
    Path(id): Path<i64>,
) -> Response {
    // 非 Python 端点：顺序触发 parse → extract → index。
    let doc = match fetch_doc(&db, id).await {
        Some(d) => d,
        None => return (StatusCode::NOT_FOUND, Json(json!({"detail": "文献不存在"}))).into_response(),
    };
    if doc.doc_status() != DocStatus::Uploaded && doc.doc_status() != DocStatus::Error {
        return (StatusCode::BAD_REQUEST, Json(json!({"detail": format!("当前状态 {} 不允许执行完整流水线", doc.status)}))).into_response();
    }

    let db2 = db.clone();
    let settings2 = settings.clone();
    tokio::spawn(async move {
        do_parse_impl(db2.clone(), settings2.clone(), id).await;
        let d = fetch_doc(&db2, id).await;
        if let Some(d) = d {
            if d.doc_status() == DocStatus::MarkdownDone {
                do_extract_impl(db2.clone(), settings2.clone(), id).await;
                let d2 = fetch_doc(&db2, id).await;
                if let Some(d2) = d2 {
                    // 提取完成后检查元数据是否已填充
                    let has_meta = d2.title.as_deref().map(|s| !s.trim().is_empty()).unwrap_or(false)
                        && d2.authors.as_deref().map(|s| !s.trim().is_empty()).unwrap_or(false);
                    if has_meta {
                        do_index_impl(db2.clone(), settings2.clone(), id, "default".to_string()).await;
                    }
                }
            }
        }
    });

    Json(json!({"detail": "完整流水线任务已提交", "status": "parsing"})).into_response()
}

async fn stop(
    Extension(db): Extension<Arc<Database>>,
    Path(id): Path<i64>,
) -> Response {
    // 非 Python 端点：将运行中的文档标记为 error（暂停其后台进度）。
    let doc = match fetch_doc(&db, id).await {
        Some(d) => d,
        None => return (StatusCode::NOT_FOUND, Json(json!({"detail": "文献不存在"}))).into_response(),
    };
    let status = doc.doc_status();
    if matches!(status, DocStatus::Parsing) {
        let _ = sqlx::query(
            "UPDATE documents SET status = 'error', status_message = '任务已停止', updated_at = ? WHERE id = ?",
        )
        .bind(now_iso())
        .bind(id)
        .execute(db.pool())
        .await;
        track_task_end(id);
    }
    Json(json!({"detail": "任务停止请求已处理", "status": doc.status})).into_response()
}
