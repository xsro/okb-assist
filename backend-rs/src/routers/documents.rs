//! 文档管理路由。

use std::sync::Arc;
use std::sync::RwLock;
use std::collections::{HashMap, HashSet};
use std::time::{SystemTime, UNIX_EPOCH};

use axum::extract::{Path, Query};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Json, Response};
use axum::routing::{get, head, post};
use serde::{de, Deserialize, Serialize};
use chrono::{DateTime, Utc};
use serde_json::{json, Value};

use crate::config::Settings;
use crate::database::Database;
use crate::models::Document;
use crate::paths;
use crate::services::pdf_meta::{
    extract_pdf_metadata_from_path_async, extract_pdf_report_from_path_async, normalize_doi,
};
use crate::services::image_cache;
use crate::services::bibtex;
use crate::services::file_state;
use crate::services::citation::{self, CitationRecord};
use crate::auth;
use crate::routers::pipeline::{list_to_db, run_crossref_override};

use axum::http::{HeaderMap, HeaderValue};
use tokio::io::{AsyncSeekExt, AsyncReadExt};
use crate::utils::{calculate_file_hash_async, now_datetime, now_iso, sha256_hex_async};

/// 单个文档上传（PDF / Markdown / 资源包）的大小上限，与 router 上的 `DefaultBodyLimit` 一致。
/// 上传过程是流式的：这里只是一个提前中止的保险，不再靠把整份文件读进内存来判大小。
const MAX_DOC_UPLOAD_BYTES: u64 = 200 * 1024 * 1024;

/// 全局文件别名表（内存，重启即丢失）
/// 存储格式：alias -> (doc_id, expires_at_unix_timestamp)
/// 有效期由 config.json 的 alias_expiration_hours 控制，默认 1 小时。
static FILE_ALIASES: std::sync::OnceLock<RwLock<std::collections::HashMap<String, (i64, u64)>>> =
    std::sync::OnceLock::new();

fn aliases() -> &'static RwLock<std::collections::HashMap<String, (i64, u64)>> {
    FILE_ALIASES.get_or_init(|| RwLock::new(std::collections::HashMap::new()))
}

/// 通过别名查找文档 ID（自动过滤已过期的别名）
pub fn get_doc_by_alias(alias: &str) -> Option<i64> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    aliases().read().unwrap().get(alias).and_then(|&(doc_id, expires_at)| {
        if now < expires_at {
            Some(doc_id)
        } else {
            None
        }
    })
}

/// 为文档生成/获取别名链接。
/// 返回 (alias, expires_at_iso_string)。
/// 如果已有未过期的别名则直接返回，否则生成新别名。
pub fn get_or_create_alias(doc_id: i64, year: Option<i64>, title: Option<&str>, expiration_hours: i64) -> (String, String) {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let expires_at = now + (expiration_hours as u64) * 3600;

    // 先查找已有别名
    if let Some(existing) = aliases().read().unwrap().iter().find(|(_, &(id, exp))| id == doc_id && now < exp) {
        let alias = existing.0.clone();
        let expires_iso = DateTime::from_timestamp(expires_at as i64, 0)
            .unwrap_or_else(|| Utc::now())
            .to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        return (alias, expires_iso);
    }

    // 生成新别名
    let y = year.map(|v| v.to_string()).unwrap_or_else(|| "unknown".to_string());
    // short_title：用 _ 替代空格，最大 20 字符
    let t: String = title
        .map(|t| {
            t.chars()
                .take(60)
                .collect::<String>()
                .replace(' ', "_")
        })
        .unwrap_or_else(|| "untitled".to_string());
    let rand_str: String = {
        use rand::Rng;
        let mut rng = rand::thread_rng();
        (0..6).map(|_| rng.gen_range(b'a'..=b'z') as char).collect()
    };
    let alias = format!("{}_{}_{}{}.pdf", y, t, rand_str, doc_id);
    aliases().write().unwrap().insert(alias.clone(), (doc_id, expires_at));

    let expires_iso = DateTime::from_timestamp(expires_at as i64, 0)
        .unwrap_or_else(|| Utc::now())
        .to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    (alias, expires_iso)
}

/// 从别名文件名中提取文档 ID。
///
/// 别名格式: `{year}_{short_title}_{6 位随机字母}{doc_id}.pdf`
/// 当别名已过期、内存中已不存在时，可借此从文件名恢复 doc_id。
pub fn extract_doc_id_from_alias(alias: &str) -> Option<i64> {
    let stem = alias.strip_suffix(".pdf")?;
    let last_part = stem.rsplit('_').next()?;
    // last_part = {6 位随机字母}{doc_id}
    if last_part.len() <= 6 {
        return None;
    }
    last_part[6..].parse::<i64>().ok()
}

// ── 流式上传工具 ────────────────────────────────────────

/// 临时上传文件的守卫：未被 `take()` 消费时，`Drop` 会删掉文件。
struct TempUpload {
    path: Option<std::path::PathBuf>,
}

impl TempUpload {
    fn path(&self) -> &std::path::Path {
        self.path.as_deref().expect("临时文件已被取走")
    }

    fn take(&mut self) -> std::path::PathBuf {
        self.path.take().expect("临时文件已被取走")
    }
}

impl Drop for TempUpload {
    fn drop(&mut self) {
        if let Some(p) = &self.path {
            let _ = std::fs::remove_file(p);
        }
    }
}

/// 上传临时目录：取 `pdf_path` 模板的上两级目录（`data/pdfs/{id}/{id}.pdf` → `data/pdfs`），
/// 保证与最终位置同一文件系统，后面可以 rename 而不是跨设备复制。
fn upload_temp_dir(settings: &Settings) -> std::path::PathBuf {
    let sample = paths::get_pdf_path(settings, 0);
    std::path::Path::new(&sample)
        .parent()
        .and_then(|p| p.parent())
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| std::path::PathBuf::from("data/pdfs"))
}

/// 把 multipart 的文件字段流式写进临时文件，边写边算 sha256。
///
/// 返回 `(临时文件守卫, 字节数, sha256 hex)`；超过 `max_bytes` 立即中止
/// 并删除临时文件。
async fn stream_field_to_temp_file(
    field: &mut axum::extract::multipart::Field<'_>,
    temp_dir: &std::path::Path,
    filename_hint: &str,
    max_bytes: u64,
) -> Result<(TempUpload, u64, String), String> {
    use sha2::{Digest, Sha256};
    use tokio::io::AsyncWriteExt;

    if let Err(e) = tokio::fs::create_dir_all(temp_dir).await {
        return Err(format!("创建上传临时目录失败: {e}"));
    }

    // 文件名只用来辨识临时文件，只用安全字符并截断
    let safe_hint: String = filename_hint
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .take(40)
        .collect();
    let tmp_path = temp_dir.join(format!(
        ".upload-{}-{}-{}.part",
        std::process::id(),
        chrono::Utc::now()
            .timestamp_nanos_opt()
            .unwrap_or_default(),
        safe_hint
    ));

    let guard = TempUpload {
        path: Some(tmp_path.clone()),
    };
    let mut file = match tokio::fs::File::create(&tmp_path).await {
        Ok(f) => f,
        Err(e) => return Err(format!("无法创建临时文件: {e}")),
    };

    let mut hasher = Sha256::new();
    let mut written: u64 = 0;
    let result: Result<(), String> = loop {
        match field.chunk().await {
            Ok(Some(chunk)) => {
                written += chunk.len() as u64;
                if written > max_bytes {
                    break Err(format!(
                        "文件过大（上限 {} MB）",
                        max_bytes / 1024 / 1024
                    ));
                }
                hasher.update(&chunk);
                if let Err(e) = file.write_all(&chunk).await {
                    break Err(format!("写入临时文件失败: {e}"));
                }
            }
            Ok(None) => break Ok(()),
            Err(e) => break Err(format!("读取上传数据失败: {e}")),
        }
    };
    let flushed = file.flush().await;
    drop(file);
    result?;
    flushed.map_err(|e| format!("刷新临时文件失败: {e}"))?;

    Ok((guard, written, hex::encode(hasher.finalize())))
}

/// 把临时文件搬到最终位置：优先 `rename`（同一文件系统，零拷贝），失败再退化为复制。
async fn persist_temp_file(mut temp: TempUpload, target: &str) -> std::io::Result<()> {
    let src = temp.take();
    if let Some(parent) = std::path::Path::new(target).parent() {
        let _ = tokio::fs::create_dir_all(parent).await;
    }
    match tokio::fs::rename(&src, target).await {
        Ok(()) => Ok(()),
        Err(_) => {
            let copied = tokio::fs::copy(&src, target).await.map(|_| ());
            let _ = tokio::fs::remove_file(&src).await;
            copied
        }
    }
}

/// 访客（未登录）可读的公开详情端点前缀（`system.json` 的 `public_detail_enabled` 控制是否放行）
pub const PUBLIC_DETAIL_PREFIX: &str = "/assist/api/documents/public/";

pub fn router() -> axum::Router<()> {
    axum::Router::new()
        // 访客公开详情：**必须**注册在 `/assist/api/documents/:id` 之前（静态段优先）
        .route("/assist/api/documents/public/:id/", get(get_public_document))
        .route("/assist/api/documents/public/:id", get(get_public_document))
        .route("/assist/api/documents/", get(list_documents).post(upload_document))
        .route("/assist/api/documents", get(list_documents).post(upload_document))
        .route("/assist/api/documents/upload/", post(upload_document))
        .route("/assist/api/documents/upload", post(upload_document))
        .route("/assist/api/documents/register/", post(register_document_by_path))
        .route("/assist/api/documents/register", post(register_document_by_path))
        .route("/assist/api/documents/import-bibtex/", post(import_bibtex))
        .route("/assist/api/documents/import-bibtex", post(import_bibtex))
        .route("/assist/api/documents/similar-titles/", get(find_similar_titles))
        .route("/assist/api/documents/similar-titles", get(find_similar_titles))
        .route("/assist/api/documents/similar-dois/", get(find_similar_dois))
        .route("/assist/api/documents/similar-dois", get(find_similar_dois))
        .route("/assist/api/documents/merge/", post(merge_documents))
        .route("/assist/api/documents/merge", post(merge_documents))
        .route("/assist/api/documents/vector-dbs/", get(list_vector_dbs))
        .route("/assist/api/documents/vector-dbs", get(list_vector_dbs))
        .route("/assist/api/documents/doc-types/", get(list_doc_types))
        .route("/assist/api/documents/doc-types", get(list_doc_types))
        .route("/assist/api/documents/search/", get(semantic_search))
        .route("/assist/api/documents/search", get(semantic_search))
        .route("/assist/api/documents/search-info/", get(search_info))
        .route("/assist/api/documents/search-info", get(search_info))
        .route("/assist/api/documents/grep-search/", get(grep_search))
        .route("/assist/api/documents/grep-search", get(grep_search))
        .route("/assist/api/documents/by-hash/:hash/", get(by_hash))
        .route("/assist/api/documents/by-hash/:hash", get(by_hash))
        .route("/assist/api/documents/by-doi/:doi/", get(by_doi))
        .route("/assist/api/documents/by-doi/:doi", get(by_doi))
        .route("/assist/api/documents/diff-dois/", post(diff_dois))
        .route("/assist/api/documents/diff-dois", post(diff_dois))
        .route("/assist/api/documents/diff-hashes/", post(diff_hashes))
        .route("/assist/api/documents/diff-hashes", post(diff_hashes))
        .route("/assist/api/documents/doi-hash-index/", get(doi_hash_index))
        .route("/assist/api/documents/doi-hash-index", get(doi_hash_index))
        .route("/assist/api/documents/:id/", get(get_document).put(update_document).delete(delete_document))
        .route("/assist/api/documents/:id", get(get_document).put(update_document).delete(delete_document))
        .route("/assist/api/documents/:id/info/", get(get_document_info).post(save_document_info))
        .route("/assist/api/documents/:id/info", get(get_document_info).post(save_document_info))
        .route("/assist/api/documents/:id/image/:filename/", get(get_image_from_zip))
        .route("/assist/api/documents/:id/image/:filename", get(get_image_from_zip))
        .route("/assist/api/documents/:id/markdown/", get(get_markdown).put(update_markdown))
        .route("/assist/api/documents/:id/markdown", get(get_markdown).put(update_markdown))
        .route("/assist/api/documents/:id/markdown/lines/", axum::routing::put(update_markdown_lines))
        .route("/assist/api/documents/:id/markdown/lines", axum::routing::put(update_markdown_lines))
        .route("/assist/api/documents/:id/toc/", get(get_toc))
        .route("/assist/api/documents/:id/toc", get(get_toc))
        .route("/assist/api/documents/:id/parse-result/", post(upload_parse_result))
        .route("/assist/api/documents/:id/parse-result", post(upload_parse_result))
        .route("/assist/api/documents/:id/pdf/", get(get_pdf).post(replace_pdf).head(check_pdf_exists))
        .route("/assist/api/documents/:id/pdf", get(get_pdf).post(replace_pdf).head(check_pdf_exists))
        .route("/assist/api/documents/:id/file-alias/", get(file_alias))
        .route("/assist/api/documents/:id/file-alias", get(file_alias))
        .route("/assist/api/documents/:id/rehash/", post(rehash_document))
        .route("/assist/api/documents/:id/rehash", post(rehash_document))
        .route("/assist/api/documents/:id/replace-markdown/", post(replace_markdown))
        .route("/assist/api/documents/:id/replace-markdown", post(replace_markdown))
        .route("/assist/api/documents/:id/replace-asset/", post(replace_asset))
        .route("/assist/api/documents/:id/replace-asset", post(replace_asset))
        .route("/assist/api/documents/:id/chunks/", get(get_markdown_chunks))
        .route("/assist/api/documents/:id/chunks", get(get_markdown_chunks))
        .route("/assist/api/documents/:id/pdf-info/", get(get_pdf_info))
        .route("/assist/api/documents/:id/pdf-info", get(get_pdf_info))
        .route("/assist/api/documents/:id/crossref/", get(get_crossref_record))
        .route("/assist/api/documents/:id/crossref", get(get_crossref_record))
        .route("/assist/api/documents/:id/citation/", get(get_citation))
        .route("/assist/api/documents/:id/citation", get(get_citation))
        .layer(axum::extract::DefaultBodyLimit::max(MAX_DOC_UPLOAD_BYTES as usize))
}

/// 列表查询参数
#[derive(Debug, Deserialize)]
pub struct ListQuery {
    pub page: Option<i64>,
    pub page_size: Option<i64>,
    pub q: Option<String>,
    pub doc_type_filter: Option<String>,
    pub search_fields: Option<String>,
    pub sort_by: Option<String>,
    pub sort_order: Option<String>,
    // 扩展筛选参数（除分页/关键词/排序外的精确条件）
    pub category: Option<String>,
    pub year: Option<i64>,
    pub year_min: Option<i64>,
    pub year_max: Option<i64>,
    /// 按文档 ID 精确筛选（对应 id:1）
    pub id: Option<i64>,
    /// 按文档 ID 范围筛选（对应 id:1-30 / id:>10）
    pub id_min: Option<i64>,
    pub id_max: Option<i64>,
    pub journal: Option<String>,
    /// 单一作者精确筛选（用于 author:xxx 语法）
    pub authors: Option<String>,

    // ── 文件事实筛选（取代原 `status_filter`）──
    /// PDF 三态：`present` / `absent` / `unknown`（逗号多选 = OR）
    pub pdf_state: Option<String>,
    /// Markdown 三态
    pub md_state: Option<String>,
    /// 图片资源包三态
    pub zip_state: Option<String>,
    /// 文件事实是否已同步过：`1` / `0`（与三态正交）
    pub synced: Option<String>,
    /// 便捷布尔：`has_pdf=1/0`（0 含未同步）
    pub has_pdf: Option<String>,
    pub has_md: Option<String>,
    pub has_zip: Option<String>,
    pub pdf_size_min: Option<i64>,
    pub pdf_size_max: Option<i64>,
    pub md_size_min: Option<i64>,
    pub md_size_max: Option<i64>,
    pub zip_size_min: Option<i64>,
    pub zip_size_max: Option<i64>,
    /// PDF 健康状态：`ok` / `warning` / `error` / `missing` / `unchecked`（逗号多选）
    pub pdf_health: Option<String>,
}

/// 文档输出格式
#[derive(Debug, Serialize)]
pub struct DocumentOut {
    pub id: i64,
    pub filename: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub markdown_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file_hash: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub authors: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub year: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub doi: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub journal: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub keywords: Option<String>,
    #[serde(rename = "abstract", skip_serializing_if = "Option::is_none")]
    pub abstract_text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub doc_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title_en: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub authors_en: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub keywords_en: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub abstract_en: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub journal_en: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mineru_task_id: Option<String>,
    /// 文件事实是否已同步过（false = 状态未知）
    pub synced: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fs_synced_at: Option<String>,
    pub has_pdf: bool,
    pub has_md: bool,
    pub has_zip: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub indexed_dbs: Option<Vec<String>>,
    pub pdf_size: Option<i64>,
    pub md_size: Option<i64>,
    pub zip_size: Option<i64>,
    /// PDF 健康检查结果：`ok` / `warning` / `error` / `missing`（`null` = 未检查）。
    /// 恒定下发（不跳过 `null`），便于调用方区分「未检查」与「字段缺失」。
    pub pdf_health: Option<String>,
    /// 健康检查原因
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pdf_health_message: Option<String>,
    /// 已写入的 Qdrant collection（数据库列，索引成功后回填）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub qdrant_collection: Option<String>,
    /// 上次索引使用的向量库 ID（数据库列）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vector_db_id: Option<String>,
    /// 进程内的活动任务（**不是**数据库记录；前端用它自行推导「解析中/进度」）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub runtime_task: Option<RuntimeTaskOut>,
    /// 进程内记录的最近一次失败原因（**不是**数据库记录；前端用它自行推导 `error`）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<String>,
}

/// 活动任务的输出结构（供前端自行推导「解析中」与进度）
#[derive(Debug, Serialize)]
pub struct RuntimeTaskOut {
    /// `parse` / `extract` / `index` / `hash` / `pdf-health` …
    pub task_type: String,
    pub message: String,
    /// 0-100
    pub progress: f64,
}

const DOC_COLUMNS: &str = "id, filename, file_hash, title, authors, CAST(NULLIF(year, '') AS INTEGER) AS year, doi, source, journal, \
    keywords, abstract, category, doc_type, language, title_en, authors_en, \
    keywords_en, abstract_en, journal_en, mineru_task_id, fs_synced_at, \
    qdrant_collection, vector_db_id, created_at, updated_at, \
    pdf_size, md_size, zip_size, pdf_health, pdf_health_message";

/// 查询文档已索引的向量数据库列表（仅 indexed 状态）
async fn indexed_dbs(db: &Database, doc_id: i64) -> Option<Vec<String>> {
    let rows: Vec<(String,)> = sqlx::query_as(
        "SELECT vector_db_id FROM document_vector_index \
         WHERE document_id = ? AND status = 'indexed' ORDER BY vector_db_id",
    )
    .bind(doc_id)
    .fetch_all(db.pool())
    .await
    .unwrap_or_default();

    if rows.is_empty() {
        None
    } else {
        Some(rows.into_iter().map(|(v,)| v).collect())
    }
}

fn file_size(path: &str) -> Option<i64> {
    std::fs::metadata(path).ok().map(|m| m.len() as i64)
}

/// 同步文档的文件大小信息到数据库（写时同步用）。
/// 对 pdf、md、zip 三个文件执行 stat 并更新数据库列（`fs_synced_at` 一并刷新）。
///
/// 这三列是「文件事实」的唯一来源：文献状态、列表筛选、批量选文档全靠它们，
/// **新增任何会动 PDF/MD/ZIP 的写路径都必须调用本函数**。
pub async fn sync_doc_file_sizes(db: &Database, settings: &Settings, doc_id: i64) {
    let pdf_size = file_size(&paths::get_pdf_path(settings, doc_id));
    let md_size = file_size(&paths::get_markdown_path(settings, doc_id));
    let zip_size = file_size(&paths::get_asset_path(settings, doc_id));
    let _ = sqlx::query(
        "UPDATE documents SET pdf_size = ?, md_size = ?, zip_size = ?, fs_synced_at = ?, updated_at = ? WHERE id = ?"
    )
    .bind(pdf_size)
    .bind(md_size)
    .bind(zip_size)
    .bind(now_iso())
    .bind(now_iso())
    .bind(doc_id)
    .execute(db.pool())
    .await;
}

async fn doc_to_out(doc: &Document, settings: &Settings, db: &Database) -> DocumentOut {
    // 只输出「数据库记录 + 运行时事实」，**不再**在这里推导 status / status_message：
    // 展示用的状态由前端（`utils/docStatus.ts`）根据这些原始字段自行组合，便于给出更细的文案。
    // 列表零 stat：所有文件大小都来自写时同步的列。
    let runtime_task = crate::routers::pipeline::runtime_task(doc.id).map(|t| RuntimeTaskOut {
        task_type: t.task_type,
        message: t.message,
        progress: t.progress,
    });
    let last_error = file_state::last_error(doc.id);

    DocumentOut {
        id: doc.id,
        filename: doc.filename.clone(),
        file_path: Some(paths::get_pdf_path(settings, doc.id)),
        markdown_path: Some(paths::get_markdown_path(settings, doc.id)),
        file_hash: doc.file_hash.clone(),
        title: doc.title.clone(),
        authors: doc.authors.clone(),
        year: doc.year,
        doi: doc.doi.clone(),
        source: doc.source.clone(),
        journal: doc.journal.clone(),
        keywords: doc.keywords.clone(),
        abstract_text: doc.abstract_text.clone(),
        category: doc.category.clone(),
        doc_type: doc.doc_type.clone(),
        language: doc.language.clone(),
        title_en: doc.title_en.clone(),
        authors_en: doc.authors_en.clone(),
        keywords_en: doc.keywords_en.clone(),
        abstract_en: doc.abstract_en.clone(),
        journal_en: doc.journal_en.clone(),
        mineru_task_id: doc.mineru_task_id.clone(),
        synced: doc
            .fs_synced_at
            .as_deref()
            .map(|s| !s.trim().is_empty())
            .unwrap_or(false),
        fs_synced_at: doc.fs_synced_at.clone(),
        has_pdf: doc.has_pdf(),
        has_md: doc.has_md(),
        has_zip: doc.has_zip(),
        indexed_dbs: indexed_dbs(db, doc.id).await,
        pdf_size: doc.pdf_size,
        md_size: doc.md_size,
        zip_size: doc.zip_size,
        pdf_health: doc.pdf_health.clone(),
        pdf_health_message: doc.pdf_health_message.clone(),
        qdrant_collection: doc.qdrant_collection.clone(),
        vector_db_id: doc.vector_db_id.clone(),
        runtime_task,
        last_error,
        created_at: doc.created_at.clone(),
        updated_at: doc.updated_at.clone(),
    }
}

pub(crate) async fn fetch_doc(db: &Database, id: i64) -> Result<Option<Document>, String> {
    sqlx::query_as::<_, Document>(&format!("SELECT {} FROM documents WHERE id = ?", DOC_COLUMNS))
        .bind(id)
        .fetch_optional(db.pool())
        .await
        .map_err(|e| e.to_string())
}

/// 查找下一个可用（最小的未被占用的）ID
async fn next_available_id(db: &Database) -> Option<i64> {
    let rows: Vec<(i64,)> = sqlx::query_as("SELECT id FROM documents ORDER BY id")
        .fetch_all(db.pool())
        .await
        .ok()?;
    let mut expected = 1;
    for (id,) in rows {
        if id != expected {
            return Some(expected);
        }
        expected += 1;
    }
    None
}

/// 可搜索字段名 → SQL 列名映射
const SEARCHABLE_COLUMNS: &[(&str, &str)] = &[
    ("title", "title"),
    ("authors", "authors"),
    ("keywords", "keywords"),
    ("abstract", "abstract"),
    ("journal", "journal"),
    ("doi", "doi"),
    ("source", "source"),
    ("filename", "filename"),
    ("category", "category"),
    ("doc_type", "doc_type"),
    ("language", "language"),
    ("title_en", "title_en"),
    ("authors_en", "authors_en"),
    ("keywords_en", "keywords_en"),
    ("abstract_en", "abstract_en"),
    ("journal_en", "journal_en"),
];

/// 可排序字段名 → SQL 列名映射
///
/// 注意：`status` 已不再可排序（它不再是数据库列）。
const SORTABLE_COLUMNS: &[(&str, &str)] = &[
    ("id", "id"),
    ("title", "title"),
    ("authors", "authors"),
    ("year", "year"),
    ("doc_type", "doc_type"),
    ("journal", "journal"),
    ("language", "language"),
    ("doi", "doi"),
    ("category", "category"),
    ("created_at", "created_at"),
    ("updated_at", "updated_at"),
    ("pdf_size", "pdf_size"),
    ("md_size", "md_size"),
    ("zip_size", "zip_size"),
    ("fs_synced_at", "fs_synced_at"),
];

fn column_for(field: &str, table: &[(&str, &str)]) -> Option<String> {
    table
        .iter()
        .find(|(k, _)| *k == field)
        .map(|(_, col)| col.to_string())
}

async fn list_documents(
    axum::Extension(db): axum::Extension<Arc<Database>>,
    axum::Extension(settings): axum::Extension<Arc<Settings>>,
    Query(params): Query<ListQuery>,
) -> Response {
    let page = params.page.unwrap_or(1).max(1);
    let page_size = params.page_size.unwrap_or(50).clamp(1, 500);
    let offset = (page - 1) * page_size;

    let mut conditions: Vec<String> = Vec::new();
    let mut binds: Vec<String> = Vec::new();

    if let Some(q) = &params.q {
        if !q.trim().is_empty() {
            // 解析搜索字段；为空时使用默认字段（title/authors/filename）
            let mut cols: Vec<String> = Vec::new();
            if let Some(sf) = &params.search_fields {
                for f in sf.split(',').map(|s| s.trim()).filter(|s| !s.is_empty()) {
                    if let Some(c) = column_for(f, SEARCHABLE_COLUMNS) {
                        cols.push(c);
                    }
                }
            }
            if cols.is_empty() {
                cols = vec!["title".to_string(), "authors".to_string(), "filename".to_string()];
            }
            let like = format!("%{}%", q.trim());
            let ors = cols
                .iter()
                .map(|c| format!("{} LIKE ?", c))
                .collect::<Vec<_>>()
                .join(" OR ");
            conditions.push(format!("({})", ors));
            for _ in 0..cols.len() {
                binds.push(like.clone());
            }
        }
    }
    // ── 文件事实筛选（取代原 `status_filter`）──
    for (kind, raw) in [
        (file_state::FileKind::Pdf, params.pdf_state.as_deref()),
        (file_state::FileKind::Md, params.md_state.as_deref()),
        (file_state::FileKind::Zip, params.zip_state.as_deref()),
    ] {
        let Some(raw) = raw.filter(|s| !s.trim().is_empty()) else { continue };
        let mut ors: Vec<String> = Vec::new();
        for st in raw.split(',').map(|s| s.trim()).filter(|s| !s.is_empty()) {
            match file_state::state_condition(kind, st) {
                Some(cond) => ors.push(cond),
                None => {
                    return (
                        StatusCode::BAD_REQUEST,
                        Json(json!({
                            "detail": format!(
                                "{}_state 取值非法：{}（仅支持 present / absent / unknown）",
                                kind.key(), st
                            )
                        })),
                    )
                        .into_response();
                }
            }
        }
        if ors.len() == 1 {
            conditions.push(ors.remove(0));
        } else if !ors.is_empty() {
            conditions.push(format!("({})", ors.join(" OR ")));
        }
    }

    if let Some(raw) = params.synced.as_deref().filter(|s| !s.trim().is_empty()) {
        match parse_bool_param(raw) {
            Some(true) => conditions.push("fs_synced_at IS NOT NULL".to_string()),
            Some(false) => conditions.push("fs_synced_at IS NULL".to_string()),
            None => {
                return (StatusCode::BAD_REQUEST, Json(json!({"detail": "synced 只接受 1/0"})))
                    .into_response()
            }
        }
    }

    for (kind, raw) in [
        (file_state::FileKind::Pdf, params.has_pdf.as_deref()),
        (file_state::FileKind::Md, params.has_md.as_deref()),
        (file_state::FileKind::Zip, params.has_zip.as_deref()),
    ] {
        let Some(raw) = raw.filter(|s| !s.trim().is_empty()) else { continue };
        match parse_bool_param(raw) {
            Some(want) => conditions.push(file_state::has_condition(kind, want)),
            None => {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(json!({"detail": format!("has_{} 只接受 1/0", kind.key())})),
                )
                    .into_response();
            }
        }
    }

    for (column, min, max) in [
        ("pdf_size", params.pdf_size_min, params.pdf_size_max),
        ("md_size", params.md_size_min, params.md_size_max),
        ("zip_size", params.zip_size_min, params.zip_size_max),
    ] {
        if let Some(v) = min {
            conditions.push(format!("COALESCE({}, 0) >= CAST(? AS INTEGER)", column));
            binds.push(v.to_string());
        }
        if let Some(v) = max {
            conditions.push(format!("COALESCE({}, 0) <= CAST(? AS INTEGER)", column));
            binds.push(v.to_string());
        }
    }

    if let Some(raw) = params.pdf_health.as_deref().filter(|s| !s.trim().is_empty()) {
        let mut values: Vec<String> = Vec::new();
        let mut include_unchecked = false;
        for item in raw.split(',').map(|s| s.trim()).filter(|s| !s.is_empty()) {
            match item.to_ascii_lowercase().as_str() {
                "ok" | "warning" | "error" | "missing" => values.push(item.to_ascii_lowercase()),
                "unchecked" | "none" | "null" => include_unchecked = true,
                _ => {
                    return (
                        StatusCode::BAD_REQUEST,
                        Json(json!({
                            "detail": format!(
                                "pdf_health 取值非法：{}（仅支持 ok / warning / error / missing / unchecked）",
                                item
                            )
                        })),
                    )
                        .into_response();
                }
            }
        }
        if !values.is_empty() {
            let placeholders = values.iter().map(|_| "?").collect::<Vec<_>>().join(",");
            let cond = format!("pdf_health IN ({})", placeholders);
            conditions.push(if include_unchecked {
                format!("({} OR pdf_health IS NULL)", cond)
            } else {
                format!("({})", cond)
            });
            binds.extend(values);
        } else if include_unchecked {
            conditions.push("pdf_health IS NULL".to_string());
        }
    }
    if let Some(dt) = &params.doc_type_filter {
        let types: Vec<&str> = dt.split(',').map(|s| s.trim()).filter(|s| !s.is_empty()).collect();
        if !types.is_empty() {
            let placeholders = types.iter().map(|_| "?").collect::<Vec<_>>().join(",");
            conditions.push(format!("doc_type IN ({})", placeholders));
            for t in types {
                binds.push(t.to_string());
            }
        }
    }
    if let Some(year) = params.year {
        conditions.push("year = ?".to_string());
        binds.push(year.to_string());
    }
    if let Some(year_min) = params.year_min {
        conditions.push("year >= ?".to_string());
        binds.push(year_min.to_string());
    }
    if let Some(year_max) = params.year_max {
        conditions.push("year <= ?".to_string());
        binds.push(year_max.to_string());
    }
    if let Some(id) = params.id {
        conditions.push("id = ?".to_string());
        binds.push(id.to_string());
    }
    if let Some(id_min) = params.id_min {
        conditions.push("id >= ?".to_string());
        binds.push(id_min.to_string());
    }
    if let Some(id_max) = params.id_max {
        conditions.push("id <= ?".to_string());
        binds.push(id_max.to_string());
    }
    if let Some(journal) = &params.journal {
        if !journal.trim().is_empty() {
            conditions.push("journal LIKE ?".to_string());
            binds.push(format!("%{}%", journal.trim()));
        }
    }
    if let Some(authors) = &params.authors {
        if !authors.trim().is_empty() {
            conditions.push("authors LIKE ?".to_string());
            binds.push(format!("%{}%", authors.trim()));
        }
    }

    let where_clause = if conditions.is_empty() {
        String::new()
    } else {
        format!(" WHERE {}", conditions.join(" AND "))
    };

    let count_sql = format!("SELECT COUNT(*) FROM documents{}", where_clause);
    let mut count_query = sqlx::query_scalar::<_, i64>(&count_sql);
    for b in &binds {
        count_query = count_query.bind(b);
    }
    let total: i64 = count_query.fetch_one(db.pool()).await.unwrap_or(0);

    // 排序：sort_by + sort_order（默认 created_at desc）
    let sort_col = params
        .sort_by
        .as_deref()
        .and_then(|f| column_for(f, SORTABLE_COLUMNS))
        .unwrap_or_else(|| "created_at".to_string());
    let order_dir = if params.sort_order.as_deref() == Some("asc") { "ASC" } else { "DESC" };
    let order = format!("ORDER BY {} {}", sort_col, order_dir);

    let sql = format!(
        "SELECT {} FROM documents{} {} LIMIT ? OFFSET ?",
        DOC_COLUMNS, where_clause, order
    );

    let mut query = sqlx::query_as::<_, Document>(&sql);
    for b in &binds {
        query = query.bind(b);
    }
    let docs: Vec<Document> = query
        .bind(page_size)
        .bind(offset)
        .fetch_all(db.pool())
        .await
        .unwrap_or_default();

    let mut items: Vec<DocumentOut> = Vec::with_capacity(docs.len());
    for d in &docs {
        items.push(doc_to_out(d, &settings, &db).await);
    }
    let total_pages = (total as f64 / page_size as f64).ceil() as i64;

    Json(json!({
        "items": items,
        "total": total,
        "page": page,
        "page_size": page_size,
        "total_pages": total_pages,
    }))
    .into_response()
}

/// 解析 `1/0`、`true/false`、`yes/no` 形式的布尔参数
fn parse_bool_param(raw: &str) -> Option<bool> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "y" | "on" => Some(true),
        "0" | "false" | "no" | "n" | "off" => Some(false),
        _ => None,
    }
}

async fn get_document(
    axum::Extension(db): axum::Extension<Arc<Database>>,
    axum::Extension(settings): axum::Extension<Arc<Settings>>,
    Path(id): Path<i64>,
) -> Response {
    // 单篇详情：实时 stat 一次并回写（列表用同步列，详情页保持与磁盘一致）
    if matches!(fetch_doc(&db, id).await, Ok(Some(_))) {
        sync_doc_file_sizes(&db, &settings, id).await;
    }
    match fetch_doc(&db, id).await {
        Ok(Some(doc)) => (StatusCode::OK, Json(json!(doc_to_out(&doc, &settings, &db).await))).into_response(),
        Ok(None) => (StatusCode::NOT_FOUND, Json(json!({"detail": "Document not found"}))).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"detail": e}))).into_response(),
    }
}

/// 访客可见的文献公开信息（`GET /assist/api/documents/public/:id`）。
///
/// 字段刻意收窄：只含书目元数据与摘要（含双语字段），**不含**文件名、文件哈希、
/// 文件大小、PDF 健康、已索引库、运行时任务/错误与时间戳。
#[derive(Debug, Serialize)]
pub struct PublicDocumentOut {
    pub id: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title_en: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub authors: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub authors_en: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub journal: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub journal_en: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub year: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub doc_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub doi: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub keywords: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub keywords_en: Option<String>,
    #[serde(rename = "abstract", skip_serializing_if = "Option::is_none")]
    pub abstract_text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub abstract_en: Option<String>,
}

impl PublicDocumentOut {
    /// 从数据库记录里只取出可公开的字段（其余一律不下发）
    pub fn from_doc(doc: &Document) -> Self {
        Self {
            id: doc.id,
            title: doc.title.clone(),
            title_en: doc.title_en.clone(),
            authors: doc.authors.clone(),
            authors_en: doc.authors_en.clone(),
            journal: doc.journal.clone(),
            journal_en: doc.journal_en.clone(),
            year: doc.year,
            doc_type: doc.doc_type.clone(),
            doi: doc.doi.clone(),
            source: doc.source.clone(),
            category: doc.category.clone(),
            language: doc.language.clone(),
            keywords: doc.keywords.clone(),
            keywords_en: doc.keywords_en.clone(),
            abstract_text: doc.abstract_text.clone(),
            abstract_en: doc.abstract_en.clone(),
        }
    }
}

/// 访客公开详情（匿名可读，仅在 `system.json` 的 `public_detail_enabled = true` 时由
/// `token_middleware` 放行）。不做实时 stat、不查向量索引，避免任何副作用与信息泄露。
async fn get_public_document(
    axum::Extension(db): axum::Extension<Arc<Database>>,
    Path(id): Path<i64>,
) -> Response {
    match fetch_doc(&db, id).await {
        Ok(Some(doc)) => {
            (StatusCode::OK, Json(json!(PublicDocumentOut::from_doc(&doc)))).into_response()
        }
        Ok(None) => (StatusCode::NOT_FOUND, Json(json!({"detail": "Document not found"}))).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"detail": e}))).into_response(),
    }
}

/// 元数据更新请求体。
///
/// 语义（与旧的 `COALESCE` 实现不同，现在可以真正清空字段）：
/// - **键缺省** = 不修改该字段；
/// - **空串 / 纯空白** = 清空为 `NULL`；
/// - 其他值 = 写入（自动去首尾空白，DOI / `doc_type` 额外归一化）。
#[derive(Debug, Deserialize)]
pub struct UpdateDocumentBody {
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub authors: Option<String>,
    /// 年份：键缺省 = 不修改；`null` / 空串 = 清空；数字（或数字字符串）= 写入。
    /// 用 `Option<Option<i64>>` 才能把「没说」和「显式清空」区分开，
    /// 也因此能把前端 `<input type="number">` 清空后拿到的 `""` 当成清空而不是 422。
    #[serde(default, deserialize_with = "de_optional_year")]
    pub year: Option<Option<i64>>,
    #[serde(default)]
    pub doi: Option<String>,
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub journal: Option<String>,
    #[serde(default)]
    pub keywords: Option<String>,
    /// 摘要：`DocumentOut` 下发的键是 `abstract`，这里两种写法都接受
    /// （历史上前端只发 `abstract`，结构体却只认 `abstract_text`，导致摘要永远存不进去）。
    #[serde(default, alias = "abstract")]
    pub abstract_text: Option<String>,
    #[serde(default)]
    pub category: Option<String>,
    #[serde(default)]
    pub doc_type: Option<String>,
    #[serde(default)]
    pub language: Option<String>,
    #[serde(default)]
    pub title_en: Option<String>,
    #[serde(default)]
    pub authors_en: Option<String>,
    #[serde(default)]
    pub keywords_en: Option<String>,
    #[serde(default)]
    pub abstract_en: Option<String>,
    #[serde(default)]
    pub journal_en: Option<String>,
}

/// 空串 / 纯空白 → `NULL`（元数据里 `""` 与 `NULL` 语义相同，统一成 `NULL` 便于比较与展示）
fn blank_to_none(raw: String) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

/// `year` 的宽松反序列化：`null` / `""` / 纯空白 → 清空；数字或数字字符串 → 写入。
/// 键缺省时由 `#[serde(default)]` 得到 `None`（= 不修改），不会走到这里。
fn de_optional_year<'de, D>(deserializer: D) -> Result<Option<Option<i64>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let raw = Value::deserialize(deserializer)?;
    Ok(match raw {
        Value::Null => Some(None),
        Value::Number(n) => match n.as_i64() {
            Some(y) => Some(Some(y)),
            None => return Err(de::Error::custom("year 需为整数或 null")),
        },
        Value::String(s) => {
            let t = s.trim();
            if t.is_empty() {
                Some(None)
            } else {
                match t.parse::<i64>() {
                    Ok(y) => Some(Some(y)),
                    Err(_) => return Err(de::Error::custom(format!("year 不是整数：{s}"))),
                }
            }
        }
        _ => return Err(de::Error::custom("year 需为整数或 null")),
    })
}

async fn update_document(
    axum::Extension(role): axum::Extension<auth::Role>,
    axum::Extension(db): axum::Extension<Arc<Database>>,
    axum::Extension(settings): axum::Extension<Arc<Settings>>,
    Path(id): Path<i64>,
    Json(body): Json<UpdateDocumentBody>,
) -> Response {
    if let Err(resp) = auth::assert_role(&role, &["admin"]) {
        return resp;
    }

    // 先读当前记录，再在内存里按「键缺省 = 不改、空串 = 清空」合并，最后整行写回：
    // 这样既能清空字段，也不需要对 16 个字段拼动态 SQL。
    let mut doc = match fetch_doc(&db, id).await {
        Ok(Some(d)) => d,
        Ok(None) => {
            return (StatusCode::NOT_FOUND, Json(json!({"detail": "Document not found"}))).into_response()
        }
        Err(e) => {
            return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"detail": e}))).into_response()
        }
    };

    macro_rules! merge_text {
        ($field:ident) => {
            if let Some(raw) = body.$field {
                doc.$field = blank_to_none(raw);
            }
        };
    }

    merge_text!(title);
    merge_text!(authors);
    merge_text!(source);
    merge_text!(journal);
    merge_text!(keywords);
    merge_text!(abstract_text);
    merge_text!(category);
    merge_text!(language);
    merge_text!(title_en);
    merge_text!(authors_en);
    merge_text!(keywords_en);
    merge_text!(abstract_en);
    merge_text!(journal_en);

    // 年份：`Some(None)` = 显式清空，`Some(Some(y))` = 写入，`None` = 不修改
    if let Some(year) = body.year {
        doc.year = year;
    }

    // DOI 写入前统一归一化（去 `https://doi.org/` 等前缀、去尾部标点），
    // 这样按 DOI 比对时才不会被 Zotero/Crossref 的写法差异影响；空串表示「清空」。
    if let Some(raw) = body.doi.as_deref() {
        doc.doi = normalize_doi(raw).and_then(|d| blank_to_none(d));
    }

    // 类型与抽取路径走同一套 Zotero 标准映射，避免手工填的值和模型填的值互不相认
    if let Some(raw) = body.doc_type {
        doc.doc_type = match blank_to_none(raw) {
            None => None,
            Some(raw) => Some(crate::routers::pipeline::normalize_doc_type(&raw)),
        };
    }

    let result = sqlx::query(
        "UPDATE documents SET title = ?, authors = ?, year = ?, doi = ?, source = ?, \
         journal = ?, keywords = ?, abstract = ?, category = ?, doc_type = ?, language = ?, \
         title_en = ?, authors_en = ?, keywords_en = ?, abstract_en = ?, journal_en = ?, \
         updated_at = datetime('now') WHERE id = ?",
    )
    .bind(&doc.title)
    .bind(&doc.authors)
    .bind(doc.year)
    .bind(&doc.doi)
    .bind(&doc.source)
    .bind(&doc.journal)
    .bind(&doc.keywords)
    .bind(&doc.abstract_text)
    .bind(&doc.category)
    .bind(&doc.doc_type)
    .bind(&doc.language)
    .bind(&doc.title_en)
    .bind(&doc.authors_en)
    .bind(&doc.keywords_en)
    .bind(&doc.abstract_en)
    .bind(&doc.journal_en)
    .bind(id)
    .execute(db.pool())
    .await;

    match result {
        // 回传更新后的完整文档：前端此前把 `{status:"ok"}` 当成 Document 赋回，导致保存后页面字段全空
        Ok(r) if r.rows_affected() > 0 => {
            (StatusCode::OK, Json(json!(doc_to_out(&doc, &settings, &db).await))).into_response()
        }
        Ok(_) => (StatusCode::NOT_FOUND, Json(json!({"detail": "Document not found"}))).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"detail": e.to_string()}))).into_response(),
    }
}

async fn delete_document(
    axum::Extension(role): axum::Extension<auth::Role>,
    axum::Extension(db): axum::Extension<Arc<Database>>,
    axum::Extension(settings): axum::Extension<Arc<Settings>>,
    Path(id): Path<i64>,
) -> Response {
    if let Err(resp) = auth::assert_role(&role, &["admin"]) {
        return resp;
    }
    match fetch_doc(&db, id).await {
        Ok(Some(doc)) => {
            // 删除本地文件
            let _ = std::fs::remove_file(paths::get_pdf_path(&settings, id));
            let _ = std::fs::remove_file(paths::get_markdown_path(&settings, id));
            // 图片缓存同步失效，避免已删文档的 zip 继续占着内存
            image_cache::invalidate(id);
            let _ = doc;

            let result = sqlx::query("DELETE FROM documents WHERE id = ?")
                .bind(id)
                .execute(db.pool())
                .await;

            match result {
                Ok(r) if r.rows_affected() > 0 => (StatusCode::OK, Json(json!({"status": "ok", "deleted": id}))).into_response(),
                Ok(_) => (StatusCode::NOT_FOUND, Json(json!({"detail": "Document not found"}))).into_response(),
                Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"detail": e.to_string()}))).into_response(),
            }
        }
        Ok(None) => (StatusCode::NOT_FOUND, Json(json!({"detail": "Document not found"}))).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"detail": e}))).into_response(),
    }
}

/// 合并请求体：将 source_ids 中的所有文档合并进 target_id。
#[derive(Debug, Deserialize)]
pub struct MergeDocumentsBody {
    pub target_id: i64,
    pub source_ids: Vec<i64>,
}

/// 将 source 的非空元数据填充到 target 的空字段中（去重合并时用）。
fn merge_metadata_into(target: &mut Document, source: &Document) {
    let fill = |t: &mut Option<String>, s: &Option<String>| {
        if t.as_deref().map_or(true, |v| v.trim().is_empty()) {
            *t = s.clone();
        }
    };
    fill(&mut target.title, &source.title);
    fill(&mut target.authors, &source.authors);
    if target.year.is_none() {
        target.year = source.year;
    }
    fill(&mut target.doi, &source.doi);
    fill(&mut target.source, &source.source);
    fill(&mut target.journal, &source.journal);
    fill(&mut target.keywords, &source.keywords);
    fill(&mut target.abstract_text, &source.abstract_text);
    fill(&mut target.category, &source.category);
    fill(&mut target.doc_type, &source.doc_type);
    fill(&mut target.language, &source.language);
    fill(&mut target.title_en, &source.title_en);
    fill(&mut target.authors_en, &source.authors_en);
    fill(&mut target.keywords_en, &source.keywords_en);
    fill(&mut target.abstract_en, &source.abstract_en);
    fill(&mut target.journal_en, &source.journal_en);
}

/// 将重复文献合并进目标文献：填充目标空字段，删除源文献（文件 + 向量 + 记录）。
async fn merge_documents(
    axum::Extension(role): axum::Extension<auth::Role>,
    axum::Extension(db): axum::Extension<Arc<Database>>,
    axum::Extension(settings): axum::Extension<Arc<Settings>>,
    Json(body): Json<MergeDocumentsBody>,
) -> Response {
    if let Err(resp) = auth::assert_role(&role, &["admin"]) {
        return resp;
    }
    // 去重并排除目标自身
    let mut source_ids: Vec<i64> = body
        .source_ids
        .into_iter()
        .filter(|id| *id != body.target_id)
        .collect();
    source_ids.sort_unstable();
    source_ids.dedup();

    let mut target = match fetch_doc(&db, body.target_id).await {
        Ok(Some(doc)) => doc,
        Ok(None) => {
            return (StatusCode::NOT_FOUND, Json(json!({"detail": "目标文档不存在"}))).into_response();
        }
        Err(e) => {
            return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"detail": e}))).into_response();
        }
    };

    // 逐个拉取源文档并合并元数据
    let mut merged_sources: Vec<i64> = Vec::new();
    for sid in &source_ids {
        if let Ok(Some(src)) = fetch_doc(&db, *sid).await {
            merge_metadata_into(&mut target, &src);
            merged_sources.push(*sid);
        }
    }

    if merged_sources.is_empty() {
        return (StatusCode::BAD_REQUEST, Json(json!({"detail": "没有可合并的源文档"}))).into_response();
    }

    // 写回合并后的目标元数据
    let update = sqlx::query(
        "UPDATE documents SET title = ?, authors = ?, year = ?, doi = ?, source = ?, \
         journal = ?, keywords = ?, abstract = ?, category = ?, doc_type = ?, language = ?, \
         title_en = ?, authors_en = ?, keywords_en = ?, abstract_en = ?, journal_en = ?, \
         updated_at = datetime('now') WHERE id = ?",
    )
    .bind(&target.title)
    .bind(&target.authors)
    .bind(target.year)
    .bind(&target.doi)
    .bind(&target.source)
    .bind(&target.journal)
    .bind(&target.keywords)
    .bind(&target.abstract_text)
    .bind(&target.category)
    .bind(&target.doc_type)
    .bind(&target.language)
    .bind(&target.title_en)
    .bind(&target.authors_en)
    .bind(&target.keywords_en)
    .bind(&target.abstract_en)
    .bind(&target.journal_en)
    .bind(target.id)
    .execute(db.pool())
    .await;

    if let Err(e) = update {
        return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"detail": e.to_string()}))).into_response();
    }

    // 删除源文档：向量点 + 本地文件 + 索引记录 + 数据库记录
    let mut deleted: Vec<i64> = Vec::new();
    for sid in &merged_sources {
        // 删除向量点（异步，失败不影响主流程）
        if let Ok(adapter) = crate::services::vector_db::get_vector_db(None) {
            let _ = adapter.delete_document(0, *sid).await;
        }
        // 删除本地文件（PDF、Markdown、元信息、资产包、Crossref 缓存）
        let _ = std::fs::remove_file(paths::get_pdf_path(&settings, *sid));
        let _ = std::fs::remove_file(paths::get_markdown_path(&settings, *sid));
        let _ = std::fs::remove_file(paths::get_info_path(&settings, *sid));
        let _ = std::fs::remove_file(paths::get_asset_path(&settings, *sid));
        let _ = std::fs::remove_file(paths::get_crossref_path(&settings, *sid));
        // 源文档的图片缓存同步失效
        image_cache::invalidate(*sid);
        // 删除向量索引记录
        let _ = sqlx::query("DELETE FROM document_vector_index WHERE document_id = ?")
            .bind(sid)
            .execute(db.pool())
            .await;
        // 删除数据库记录
        if let Ok(r) = sqlx::query("DELETE FROM documents WHERE id = ?")
            .bind(sid)
            .execute(db.pool())
            .await
        {
            if r.rows_affected() > 0 {
                deleted.push(*sid);
            }
        }
    }

    (StatusCode::OK, Json(json!({
        "status": "ok",
        "target_id": target.id,
        "merged_ids": deleted,
        "detail": format!("已合并 {} 篇文献到文档 {}", deleted.len(), target.id),
    }))).into_response()
}

/// 从 PDF 元数据填充文档空字段
fn fill_meta_fields(doc: &mut Document, meta: &Value) {
    if doc.title.is_none() {
        if let Some(t) = meta.get("title").and_then(|v| v.as_str()) {
            doc.title = Some(t.to_string());
        }
    }
    if doc.authors.is_none() {
        if let Some(a) = meta.get("authors").and_then(|v| v.as_array()) {
            let authors: Vec<String> = a.iter().filter_map(|x| x.as_str().map(String::from)).collect();
            if !authors.is_empty() {
                // 列表列统一存 `; ` 分隔纯文本（历史数据是 JSON 数组字符串）
                doc.authors = Some(list_to_db(&authors));
            }
        }
    }
    if doc.year.is_none() {
        if let Some(y) = meta.get("year").and_then(|v| v.as_i64()) {
            doc.year = Some(y);
        }
    }
    if doc.doi.is_none() {
        if let Some(d) = meta.get("doi").and_then(|v| v.as_str()) {
            doc.doi = normalize_doi(d);
        }
    }
    if doc.keywords.is_none() {
        if let Some(k) = meta.get("keywords").and_then(|v| v.as_array()) {
            let kws: Vec<String> = k.iter().filter_map(|x| x.as_str().map(String::from)).collect();
            if !kws.is_empty() {
                doc.keywords = Some(list_to_db(&kws));
            }
        }
    }
    if doc.abstract_text.is_none() {
        if let Some(a) = meta.get("abstract").and_then(|v| v.as_str()) {
            doc.abstract_text = Some(a.to_string());
        }
    }
    if doc.journal.is_none() {
        if let Some(j) = meta.get("journal").and_then(|v| v.as_str()) {
            if !j.trim().is_empty() {
                doc.journal = Some(j.to_string());
            }
        }
    }
    if doc.journal_en.is_none() {
        if let Some(j) = meta.get("journal_en").and_then(|v| v.as_str()) {
            if !j.trim().is_empty() {
                doc.journal_en = Some(j.to_string());
            }
        }
    }
    if doc.title_en.is_none() {
        if let Some(t) = meta.get("title_en").and_then(|v| v.as_str()) {
            if !t.trim().is_empty() {
                doc.title_en = Some(t.to_string());
            }
        }
    }
    if doc.authors_en.is_none() {
        if let Some(a) = meta.get("authors_en").and_then(|v| v.as_array()) {
            let authors: Vec<String> = a.iter().filter_map(|x| x.as_str().map(String::from)).collect();
            if !authors.is_empty() {
                doc.authors_en = Some(list_to_db(&authors));
            }
        }
    }
    if doc.keywords_en.is_none() {
        if let Some(k) = meta.get("keywords_en").and_then(|v| v.as_array()) {
            let kws: Vec<String> = k.iter().filter_map(|x| x.as_str().map(String::from)).collect();
            if !kws.is_empty() {
                doc.keywords_en = Some(list_to_db(&kws));
            }
        }
    }
    if doc.abstract_en.is_none() {
        if let Some(a) = meta.get("abstract_en").and_then(|v| v.as_str()) {
            if !a.trim().is_empty() {
                doc.abstract_en = Some(a.to_string());
            }
        }
    }
}

/// 插入新文档记录
async fn insert_document(db: &Database, doc: Document) -> Result<i64, String> {
    let now = now_datetime();
    let result = sqlx::query(
        "INSERT INTO documents (filename, file_hash, title, authors, year, doi, source, journal, \
         keywords, abstract, category, doc_type, language, title_en, authors_en, keywords_en, \
         abstract_en, journal_en, created_at, updated_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&doc.filename)
    .bind(&doc.file_hash)
    .bind(&doc.title)
    .bind(&doc.authors)
    .bind(doc.year)
    .bind(&doc.doi)
    .bind(&doc.source)
    .bind(&doc.journal)
    .bind(&doc.keywords)
    .bind(&doc.abstract_text)
    .bind(&doc.category)
    .bind(&doc.doc_type)
    .bind(&doc.language)
    .bind(&doc.title_en)
    .bind(&doc.authors_en)
    .bind(&doc.keywords_en)
    .bind(&doc.abstract_en)
    .bind(&doc.journal_en)
    .bind(&now)
    .bind(&now)
    .execute(db.pool())
    .await
    .map_err(|e| e.to_string())?;

    Ok(result.last_insert_rowid())
}

async fn upload_document(
    axum::Extension(role): axum::Extension<auth::Role>,
    axum::Extension(db): axum::Extension<Arc<Database>>,
    axum::Extension(settings): axum::Extension<Arc<Settings>>,
    mut multipart: axum::extract::Multipart,
) -> Response {
    if let Err(resp) = auth::assert_role(&role, &["admin", "view-upload"]) {
        return resp;
    }
    let mut filename: Option<String> = None;
    // 流式落盘：整个上传过程不再把 PDF 读进内存，哈希在写入时顺带算好
    let mut uploaded: Option<(TempUpload, u64, String)> = None;
    let mut force = false;
    let mut auto_process = true;
    // 调用方可以显式声明 DOI（客户端从 Zotero 已知），这样服务端不用抽 PDF 元数据就能做 DOI 去重；
    // 只有显式给出时才会做 DOI 去重（PDF 元数据里抽到的 DOI 不参与去重，避免影响既有上传流程）。
    let mut doi_field: Option<String> = None;
    // 命中重复时的行为：error（默认，409）| skip（200 + 已存在文档）| replace（用本次上传覆盖已有 PDF）
    let mut on_duplicate = "error".to_string();
    // 是否做 PDF 元数据启发式抽取（默认 true，保持 Web 上传行为不变）。
    // 批量上传（client-rs）会传 false：那一步要起几个 mutool 子进程、把整份文档的正文抽一遍，
    // 是大文件上传后「等服务端处理」的主要耗时；不抽也能先把记录建好，之后再补。
    let mut extract_meta = true;
    let temp_dir = upload_temp_dir(&settings);

    while let Ok(Some(mut field)) = multipart.next_field().await {
        let Some(name) = field.name().map(|s| s.to_string()) else {
            continue;
        };
        match name.as_str() {
            "file" => {
                let hint = field.file_name().unwrap_or("upload.pdf").to_string();
                filename = Some(hint.clone());
                // 只保留最后一个文件字段（旧守卫在这里被 drop，临时文件随之删除）
                let _ = uploaded.take();
                match stream_field_to_temp_file(
                    &mut field,
                    &temp_dir,
                    &hint,
                    MAX_DOC_UPLOAD_BYTES,
                )
                .await
                {
                    Ok(v) => uploaded = Some(v),
                    Err(e) => {
                        return (StatusCode::BAD_REQUEST, Json(json!({"detail": e}))).into_response()
                    }
                }
            }
            "force" => {
                let val = field.bytes().await.unwrap_or_default();
                force = std::str::from_utf8(&val).unwrap_or("false") == "true";
            }
            "auto_process" => {
                let val = field.bytes().await.unwrap_or_default();
                auto_process = std::str::from_utf8(&val).unwrap_or("true") != "false";
            }
            "extract_meta" => {
                let val = field.bytes().await.unwrap_or_default();
                extract_meta = std::str::from_utf8(&val).unwrap_or("true") != "false";
            }
            "doi" => {
                let val = field.bytes().await.unwrap_or_default();
                doi_field = normalize_doi(&String::from_utf8_lossy(&val));
            }
            "on_duplicate" => {
                let val = field.bytes().await.unwrap_or_default();
                let v = String::from_utf8_lossy(&val).trim().to_lowercase();
                on_duplicate = match v.as_str() {
                    "" | "error" => "error".to_string(),
                    "skip" => "skip".to_string(),
                    "replace" => "replace".to_string(),
                    other => {
                        return (
                            StatusCode::BAD_REQUEST,
                            Json(json!({
                                "detail": format!("on_duplicate 只支持 error / skip / replace，收到: {}", other)
                            })),
                        )
                            .into_response()
                    }
                };
            }
            _ => {
                // 未知字段：读完丢弃，保持流位置正确
                let _ = field.bytes().await;
            }
        }
    }

    let filename = match filename {
        Some(f) => f,
        None => return (StatusCode::BAD_REQUEST, Json(json!({"detail": "缺少文件"}))).into_response(),
    };

    if !filename.to_lowercase().ends_with(".pdf") {
        return (StatusCode::BAD_REQUEST, Json(json!({"detail": "只支持 PDF 文件"}))).into_response();
    }

    let (temp_upload, upload_size, file_hash) = match uploaded {
        Some(v) => v,
        None => return (StatusCode::BAD_REQUEST, Json(json!({"detail": "缺少文件"}))).into_response(),
    };
    if upload_size == 0 {
        return (StatusCode::BAD_REQUEST, Json(json!({"detail": "文件为空"}))).into_response();
    }

    // ── 去重 ────────────────────────────────────────────────────────────
    // 1. 哈希去重：只查 `file_hash` 索引，完全不碰 PDF 内容（原来的实现先抽元数据再比对，
    //    重复上传时要白白拆一次 PDF）；
    // 2. DOI 去重：只在调用方**显式**传入 `doi` 时做（PDF 里抽到的 DOI 不参与，保持既有行为），
    //    这样免掉拆 PDF 就能拦住 DOI 重复。
    // 命中重复时临时文件由 `TempUpload` 的 Drop 自动删除。
    let mut dup: Option<(Document, &'static str)> = None;
    if !force {
        if let Some(existing) = find_doc_by_hash(&db, &file_hash).await {
            dup = Some((existing, "hash"));
        } else if let Some(doi) = doi_field.clone() {
            if let Some(existing) = find_doc_by_doi(&db, &doi).await {
                dup = Some((existing, "doi"));
            }
        }
    }

    if let Some((existing, kind)) = dup {
        let existing_id = existing.id;
        let same_file = existing.file_hash.as_deref() == Some(file_hash.as_str());
        match on_duplicate.as_str() {
            // 跳过：直接把已存在文档返回给调用方（200，不新建）
            "skip" => {
                let out = doc_to_out(&existing, &settings, &db).await;
                return (
                    StatusCode::OK,
                    Json(json!({
                        "status": "skipped",
                        "skipped": true,
                        "reason": kind,
                        "existing_id": existing_id,
                        "same_file": same_file,
                        // 兼容只读 `id` 的客户端
                        "id": existing_id,
                        "doc": out,
                    })),
                )
                    .into_response();
            }
            // 覆盖：用本次上传的 PDF 替掉已存在文档的文件（文件完全相同则等价于跳过）
            "replace" => {
                if same_file {
                    let out = doc_to_out(&existing, &settings, &db).await;
                    return (
                        StatusCode::OK,
                        Json(json!({
                            "status": "skipped",
                            "skipped": true,
                            "reason": kind,
                            "existing_id": existing_id,
                            "same_file": true,
                            "id": existing_id,
                            "doc": out,
                        })),
                    )
                        .into_response();
                }
                if let Err(e) = persist_pdf_for_doc(
                    &db,
                    &settings,
                    existing_id,
                    temp_upload,
                    &file_hash,
                    doi_field.clone(),
                )
                .await
                {
                    return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"detail": e})))
                        .into_response();
                }
                return match fetch_doc(&db, existing_id).await {
                    Ok(Some(doc)) => (
                        StatusCode::OK,
                        Json(json!({
                            "status": "replaced",
                            "replaced": true,
                            "reason": kind,
                            "existing_id": existing_id,
                            "id": existing_id,
                            "doc": doc_to_out(&doc, &settings, &db).await,
                        })),
                    )
                        .into_response(),
                    _ => (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(json!({"detail": "文档更新失败"})),
                    )
                        .into_response(),
                };
            }
            // 默认：409，把命中的原因与已有文档的关键信息都带回去，方便客户端决策
            _ => {
                let message = if kind == "hash" {
                    format!(
                        "文件已存在 (hash: {}...)",
                        &file_hash[..16.min(file_hash.len())]
                    )
                } else {
                    format!(
                        "DOI 已存在: {}（文档 {}）",
                        existing.doi.clone().unwrap_or_default(),
                        existing_id
                    )
                };
                return (
                    StatusCode::CONFLICT,
                    Json(json!({
                        "error": "duplicate",
                        "kind": kind,
                        "message": message,
                        "existing_id": existing_id,
                        "same_file": same_file,
                        "file_hash": file_hash,
                        "existing": {
                            "id": existing_id,
                            "filename": existing.filename,
                            "doi": existing.doi,
                            "file_hash": existing.file_hash,
                            "pdf_health": existing.pdf_health,
                            "pdf_health_message": existing.pdf_health_message,
                        },
                    })),
                )
                    .into_response();
            }
        }
    }

    // PDF 元数据（后台线程读临时文件，不占用请求内存）；`extract_meta=false` 时整段跳过
    let meta = if extract_meta {
        extract_pdf_metadata_from_path_async(
            temp_upload.path().to_string_lossy().to_string(),
            Some(filename.clone()),
            None,
        )
        .await
    } else {
        serde_json::Value::Object(serde_json::Map::new())
    };

    let mut doc = Document {
        id: 0,
        filename: filename.clone(),
        file_hash: Some(file_hash),
        title: None,
        authors: None,
        year: None,
        doi: doi_field.clone(),
        source: None,
        journal: None,
        keywords: None,
        abstract_text: None,
        category: None,
        doc_type: None,
        language: None,
        title_en: None,
        authors_en: None,
        keywords_en: None,
        abstract_en: None,
        journal_en: None,
        mineru_task_id: None,
        fs_synced_at: None,
        qdrant_collection: None,
        vector_db_id: None,
        created_at: None,
        updated_at: None,
        pdf_size: None,
        md_size: None,
        zip_size: None,
        pdf_health: None,
        pdf_health_message: None,
    };
    fill_meta_fields(&mut doc, &meta);

    // 复用最小可用 ID
    let doc_id = match next_available_id(&db).await {
        Some(id) => {
            doc.id = id;
            // 用指定 ID 插入
            let now = now_datetime();
            let result = sqlx::query(
                "INSERT INTO documents (id, filename, file_hash, title, authors, year, doi, source, journal, \
                 keywords, abstract, category, doc_type, language, title_en, authors_en, keywords_en, \
                 abstract_en, journal_en, created_at, updated_at) \
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            )
            .bind(id)
            .bind(&doc.filename)
            .bind(&doc.file_hash)
            .bind(&doc.title)
            .bind(&doc.authors)
            .bind(doc.year)
            .bind(&doc.doi)
            .bind(&doc.source)
            .bind(&doc.journal)
            .bind(&doc.keywords)
            .bind(&doc.abstract_text)
            .bind(&doc.category)
            .bind(&doc.doc_type)
            .bind(&doc.language)
            .bind(&doc.title_en)
            .bind(&doc.authors_en)
            .bind(&doc.keywords_en)
            .bind(&doc.abstract_en)
            .bind(&doc.journal_en)
            .bind(&now)
            .bind(&now)
            .execute(db.pool())
            .await;

            match result {
                Ok(_) => id,
                Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"detail": e.to_string()}))).into_response(),
            }
        }
        None => match insert_document(&db, doc.clone()).await {
            Ok(id) => id,
            Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"detail": e}))).into_response(),
        },
    };

    // 保存 PDF 文件：临时文件已被流式写好，这里只是搬位置（同盘 rename，零拷贝）
    let pdf_path = paths::get_pdf_path(&settings, doc_id);
    if let Err(e) = persist_temp_file(temp_upload, &pdf_path).await {
        return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"detail": format!("保存文件失败: {}", e)}))).into_response();
    }

    // 写时同步：更新文件大小到数据库
    sync_doc_file_sizes(&db, &settings, doc_id).await;

    // 如果有 DOI 且 auto_process 为 true，自动触发 Crossref 获取权威元数据
    if auto_process && doc.doi.is_some() {
        let db = db.clone();
        let settings = settings.clone();
        tokio::spawn(run_crossref_override(db, settings, doc_id));
    }

    // 返回文档信息
    match fetch_doc(&db, doc_id).await {
        Ok(Some(doc)) => (StatusCode::OK, Json(json!(doc_to_out(&doc, &settings, &db).await))).into_response(),
        _ => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"detail": "文档创建失败"}))).into_response(),
    }
}

#[derive(Debug, Deserialize)]
pub struct RegisterByPath {
    pub file_path: String,
    #[serde(default)]
    pub force: bool,
}

async fn register_document_by_path(
    axum::Extension(role): axum::Extension<auth::Role>,
    axum::Extension(db): axum::Extension<Arc<Database>>,
    axum::Extension(settings): axum::Extension<Arc<Settings>>,
    Json(data): Json<RegisterByPath>,
) -> Response {
    if let Err(resp) = auth::assert_role(&role, &["admin", "view-upload"]) {
        return resp;
    }
    if !std::path::Path::new(&data.file_path).exists() {
        return (StatusCode::BAD_REQUEST, Json(json!({"detail": format!("文件不存在: {}", data.file_path)}))).into_response();
    }
    if !data.file_path.to_lowercase().ends_with(".pdf") {
        return (StatusCode::BAD_REQUEST, Json(json!({"detail": "只支持 PDF 文件"}))).into_response();
    }

    // 直接从磁盘路径算哈希、抽元数据（流式读，不把整份 PDF 读进内存）
    let file_hash = match calculate_file_hash_async(data.file_path.clone()).await {
        Ok(h) => h,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"detail": e.to_string()}))).into_response(),
    };
    let meta = extract_pdf_metadata_from_path_async(data.file_path.clone(), None, None).await;

    // 检查重复
    if !data.force {
        let duplicate: Option<(i64,)> = sqlx::query_as("SELECT id FROM documents WHERE file_hash = ? LIMIT 1")
            .bind(&file_hash)
            .fetch_optional(db.pool())
            .await
            .unwrap_or(None);
        if let Some((existing_id,)) = duplicate {
            return (StatusCode::CONFLICT, Json(json!({
                "error": "duplicate",
                "message": format!("文件已存在 (hash: {}...)", &file_hash[..16.min(file_hash.len())]),
                "existing_id": existing_id,
            }))).into_response();
        }
    }

    let filename = std::path::Path::new(&data.file_path)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "document.pdf".to_string());

    let mut doc = Document {
        id: 0,
        filename,
        file_hash: Some(file_hash),
        title: None,
        authors: None,
        year: None,
        doi: None,
        source: None,
        journal: None,
        keywords: None,
        abstract_text: None,
        category: None,
        doc_type: None,
        language: None,
        title_en: None,
        authors_en: None,
        keywords_en: None,
        abstract_en: None,
        journal_en: None,
        mineru_task_id: None,
        fs_synced_at: None,
        qdrant_collection: None,
        vector_db_id: None,
        created_at: None,
        updated_at: None,
        pdf_size: None,
        md_size: None,
        zip_size: None,
        pdf_health: None,
        pdf_health_message: None,
    };
    fill_meta_fields(&mut doc, &meta);

    let doc_id = match insert_document(&db, doc).await {
        Ok(id) => id,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"detail": e}))).into_response(),
    };

    // 复制文件（tokio::fs::copy 走内核复制，不在用户态缓冲整份文件）
    let pdf_path = paths::get_pdf_path(&settings, doc_id);
    if let Some(parent) = std::path::Path::new(&pdf_path).parent() {
        let _ = tokio::fs::create_dir_all(parent).await;
    }
    if let Err(e) = tokio::fs::copy(&data.file_path, &pdf_path).await {
        return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"detail": format!("复制文件失败: {}", e)}))).into_response();
    }

    // 写时同步：更新文件大小到数据库
    sync_doc_file_sizes(&db, &settings, doc_id).await;

    match fetch_doc(&db, doc_id).await {
        Ok(Some(doc)) => (StatusCode::OK, Json(json!(doc_to_out(&doc, &settings, &db).await))).into_response(),
        _ => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"detail": "文档创建失败"}))).into_response(),
    }
}


// ── BibTeX 批量新建条目 ──────────────────────────────────

/// `.bib` 文本内容上限（纯文本，8 MB 足够放下几万条）
const MAX_BIBTEX_BYTES: usize = 8 * 1024 * 1024;
/// 单次导入处理的条目上限（防止一个巨型文件长时间占住请求）
const MAX_BIBTEX_ENTRIES: usize = 2000;
/// 返回给前端的逐条结果上限（超出只给统计，避免响应过大）
const MAX_BIBTEX_ITEMS: usize = 500;

/// BibTeX 批量导入请求体
#[derive(Debug, Deserialize)]
pub struct ImportBibtexRequest {
    /// `.bib` 文件的原始内容
    pub content: String,
    /// `true` = 只预览（不写库）
    #[serde(default)]
    pub dry_run: bool,
}

/// 批量新建「库里缺失」的条目（来自 BibTeX）。
///
/// 只做新建，不改已有文档：命中已有文档（DOI 优先，其次归一化标题）时跳过并回传
/// `existing_id`。新建出来的文档**没有 PDF**，`pdf_health` 直接标 `missing`、
/// 状态保持 `uploaded`（上传 PDF 后即可走解析流水线）。
async fn import_bibtex(
    axum::Extension(role): axum::Extension<auth::Role>,
    axum::Extension(db): axum::Extension<Arc<Database>>,
    Json(data): Json<ImportBibtexRequest>,
) -> Response {
    if let Err(resp) = auth::assert_role(&role, &["admin", "view-upload"]) {
        return resp;
    }
    if data.content.trim().is_empty() {
        return (StatusCode::BAD_REQUEST, Json(json!({"detail": "BibTeX 内容为空"}))).into_response();
    }
    if data.content.len() > MAX_BIBTEX_BYTES {
        return (
            StatusCode::PAYLOAD_TOO_LARGE,
            Json(json!({
                "detail": format!(
                    "BibTeX 文件过大（{} 字节），上限 {} 字节",
                    data.content.len(),
                    MAX_BIBTEX_BYTES
                )
            })),
        )
            .into_response();
    }

    let parsed = bibtex::parse_bibtex(&data.content);

    // 现有库索引：一次读出 id/标题/DOI/年份，之后全在内存里比对
    // （DOI 统一走 `doi_key` 归一化，兼容历史数据里的 `https://doi.org/` 前缀与大小写差异）
    //
    // ⚠ `year` 必须 CAST：老库里存在写成 `''`（TEXT 类型）的年份，sqlx 按 `Option<i64>`
    // 解码 TEXT 会直接报 `mismatched types`，整条 `fetch_all` 失败。以前这里用
    // `unwrap_or_default()` 把错误吞掉 → 索引变成空表 → 所有条目都被判成「库里缺失」
    // （看起来就是「检测不出重复」）。读取失败必须显式报错，不能静默当成空库。
    let rows: Vec<(i64, Option<String>, Option<String>, Option<i64>)> = match sqlx::query_as(
        "SELECT id, title, doi, CAST(NULLIF(TRIM(year), '') AS INTEGER) AS year FROM documents",
    )
    .fetch_all(db.pool())
    .await
    {
        Ok(rows) => rows,
        Err(e) => {
            tracing::error!("BibTeX 导入：读取库内文献索引失败: {}", e);
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"detail": format!("读取文献库索引失败：{}", e)})),
            )
                .into_response();
        }
    };
    let mut doi_index: HashMap<String, i64> = HashMap::new();
    let mut title_index: HashMap<String, Vec<(Option<i64>, i64)>> = HashMap::new();
    for (id, title, doi, year) in rows {
        if let Some(key) = doi.as_deref().and_then(doi_key) {
            doi_index.entry(key).or_insert(id);
        }
        if let Some(title) = title.as_deref() {
            let norm = normalize_title(title);
            if !norm.is_empty() {
                title_index.entry(norm).or_default().push((year, id));
            }
        }
    }

    let total = parsed.entries.len();
    let mut processed = 0usize;
    let mut missing_count = 0usize;
    let mut existing_count = 0usize;
    let mut duplicate_count = 0usize;
    let mut invalid_count = 0usize;
    let mut failed_count = 0usize;
    let mut created: Vec<Value> = Vec::new();
    let mut items: Vec<Value> = Vec::new();
    let mut items_truncated = false;
    // 文件内部去重：同一个文件里重复出现的条目不再新建
    let mut seen_doi: HashSet<String> = HashSet::new();
    let mut seen_title: HashSet<String> = HashSet::new();

    for entry in parsed.entries.iter().take(MAX_BIBTEX_ENTRIES) {
        processed += 1;
        let rec = match entry.to_record() {
            Some(r) => r,
            None => {
                invalid_count += 1;
                push_bibtex_item(
                    &mut items,
                    &mut items_truncated,
                    bibtex_item("invalid", &entry.key, "", None, "", None, "no-title", None, "缺少标题与 DOI，已跳过"),
                );
                continue;
            }
        };

        let doi_k = doi_key(&rec.doi);
        let title_k = normalize_title(&rec.title);

        // 1) 文件内部重复（同 DOI 或同归一化标题）
        let dup_doi = doi_k.as_ref().map(|k| !seen_doi.insert(k.clone())).unwrap_or(false);
        let dup_title = !title_k.is_empty() && !seen_title.insert(title_k.clone());
        if dup_doi || dup_title {
            duplicate_count += 1;
            push_bibtex_item(
                &mut items,
                &mut items_truncated,
                bibtex_item(
                    "duplicate",
                    &rec.key,
                    &rec.title,
                    rec.year,
                    &rec.doi,
                    None,
                    if dup_doi { "in-file-doi" } else { "in-file-title" },
                    None,
                    "文件内重复条目",
                ),
            );
            continue;
        }

        // 2) 库里已存在：DOI 优先，其次归一化标题（两边都有年份时才要求年份一致）
        if let Some((existing_id, reason)) = match_existing_bibtex_doc(
            doi_k.as_deref(),
            &title_k,
            rec.year,
            &doi_index,
            &title_index,
        ) {
            existing_count += 1;
            push_bibtex_item(
                &mut items,
                &mut items_truncated,
                bibtex_item(
                    "exists",
                    &rec.key,
                    &rec.title,
                    rec.year,
                    &rec.doi,
                    Some(existing_id),
                    reason,
                    None,
                    "库中已存在",
                ),
            );
            continue;
        }

        // 3) 库里缺失：预览只统计，正式导入才写库
        missing_count += 1;
        if data.dry_run {
            push_bibtex_item(
                &mut items,
                &mut items_truncated,
                bibtex_item("create", &rec.key, &rec.title, rec.year, &rec.doi, None, "", None, "可新建"),
            );
            continue;
        }

        match insert_bibtex_document(&db, &rec).await {
            Ok(id) => {
                // 新记录立刻进索引：同文件后续条目（同一篇的另一种写法）也能命中
                if let Some(k) = doi_key(&rec.doi) {
                    doi_index.entry(k).or_insert(id);
                }
                if !title_k.is_empty() {
                    title_index.entry(title_k.clone()).or_default().push((rec.year, id));
                }
                created.push(json!({
                    "id": id,
                    "key": rec.key,
                    "title": rec.title,
                    "doi": rec.doi,
                    "year": rec.year,
                }));
                push_bibtex_item(
                    &mut items,
                    &mut items_truncated,
                    bibtex_item("created", &rec.key, &rec.title, rec.year, &rec.doi, None, "", Some(id), "已新建"),
                );
            }
            Err(e) => {
                failed_count += 1;
                push_bibtex_item(
                    &mut items,
                    &mut items_truncated,
                    bibtex_item("error", &rec.key, &rec.title, rec.year, &rec.doi, None, "", None, &e),
                );
            }
        }
    }

    let detail = if data.dry_run {
        format!(
            "解析到 {} 条，其中 {} 条库中缺失（可新建），{} 条已存在，{} 条文件内重复",
            total, missing_count, existing_count, duplicate_count
        )
    } else {
        format!(
            "解析到 {} 条，新建 {} 条，已存在 {} 条，文件内重复 {} 条，无效 {} 条，失败 {} 条",
            total,
            created.len(),
            existing_count,
            duplicate_count,
            invalid_count,
            failed_count
        )
    };

    (
        StatusCode::OK,
        Json(json!({
            "dry_run": data.dry_run,
            "total": total,
            "processed": processed,
            "truncated": total > processed,
            "entry_limit": MAX_BIBTEX_ENTRIES,
            "missing": missing_count,
            "created_count": created.len(),
            "existing_count": existing_count,
            "duplicate_count": duplicate_count,
            "invalid_count": invalid_count,
            "failed_count": failed_count,
            "created": created,
            "items": items,
            "items_truncated": items_truncated,
            "warnings": parsed.warnings,
            "skipped_entries": parsed.skipped,
            "detail": detail,
        })),
    )
        .into_response()
}

/// 组装一条逐条结果（前端导入结果表格展示）
#[allow(clippy::too_many_arguments)]
fn bibtex_item(
    action: &str,
    key: &str,
    title: &str,
    year: Option<i64>,
    doi: &str,
    existing_id: Option<i64>,
    reason: &str,
    new_id: Option<i64>,
    detail: &str,
) -> Value {
    let mut item = json!({
        "action": action,
        "key": key,
        "title": title,
        "year": year,
        "doi": doi,
    });
    if let Some(id) = existing_id {
        item["existing_id"] = json!(id);
    }
    if let Some(id) = new_id {
        item["id"] = json!(id);
    }
    if !reason.is_empty() {
        item["reason"] = json!(reason);
    }
    if !detail.is_empty() {
        item["detail"] = json!(detail);
    }
    item
}

/// 逐条结果封顶（超出只保留统计）
fn push_bibtex_item(items: &mut Vec<Value>, truncated: &mut bool, item: Value) {
    if items.len() < MAX_BIBTEX_ITEMS {
        items.push(item);
    } else {
        *truncated = true;
    }
}

/// 判断条目是否已存在于库中：返回（已有文档 ID，命中依据 `doi` / `title`）
fn match_existing_bibtex_doc(
    doi_key: Option<&str>,
    title_key: &str,
    year: Option<i64>,
    doi_index: &HashMap<String, i64>,
    title_index: &HashMap<String, Vec<(Option<i64>, i64)>>,
) -> Option<(i64, &'static str)> {
    if let Some(key) = doi_key {
        if let Some(id) = doi_index.get(key) {
            return Some((*id, "doi"));
        }
    }
    if !title_key.is_empty() {
        if let Some(candidates) = title_index.get(title_key) {
            let hit = candidates.iter().find(|(doc_year, _)| match (year, *doc_year) {
                (Some(a), Some(b)) => a == b,
                // 任一方没有年份：只按标题认定（避免漏掉历史数据里年份为空的记录）
                _ => true,
            });
            if let Some((_, id)) = hit {
                return Some((*id, "title"));
            }
        }
    }
    None
}

/// 插入「仅元数据」的文档（来自 BibTeX）：没有 PDF，`pdf_health` 直接标 `missing`。
async fn insert_bibtex_document(db: &Database, rec: &bibtex::BibRecord) -> Result<i64, String> {
    let now = now_datetime();
    // `authors` / `keywords` 列统一存 `; ` 分隔纯文本（不用 JSON 数组字符串）
    let authors = if rec.authors.is_empty() {
        None
    } else {
        Some(list_to_db(&rec.authors))
    };
    let keywords = if rec.keywords.is_empty() {
        None
    } else {
        Some(list_to_db(&rec.keywords))
    };
    // `source` 列沿用 Crossref 的语义（出版社），没有出版社时标出来源
    let source = if rec.publisher.trim().is_empty() {
        "bibtex".to_string()
    } else {
        rec.publisher.trim().to_string()
    };
    let doi = normalize_doi(&rec.doi);
    let filename = bibtex_filename(rec);
    let doc_type = if rec.doc_type.trim().is_empty() {
        "document".to_string()
    } else {
        rec.doc_type.trim().to_string()
    };

    let result = sqlx::query(
        "INSERT INTO documents (filename, title, authors, year, doi, source, journal, keywords, abstract, \
         doc_type, language, pdf_health, pdf_health_message, fs_synced_at, created_at, updated_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 'missing', ?, ?, ?, ?)",
    )
    .bind(&filename)
    .bind(non_empty_str(&rec.title))
    .bind(&authors)
    .bind(rec.year)
    .bind(&doi)
    .bind(&source)
    .bind(non_empty_str(&rec.journal))
    .bind(&keywords)
    .bind(non_empty_str(&rec.abstract_text))
    .bind(&doc_type)
    .bind(non_empty_str(&rec.language))
    .bind("未上传 PDF（条目来自 BibTeX 导入）")
    // 导入时就知道没有 PDF：直接记下同步时间，三个 size 保持 NULL = 全部 absent
    .bind(&now)
    .bind(&now)
    .bind(&now)
    .execute(db.pool())
    .await
    .map_err(|e| e.to_string())?;

    Ok(result.last_insert_rowid())
}

/// 空串落库为 `NULL`（列表/筛选用不上空字符串）
fn non_empty_str(value: &str) -> Option<&str> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed)
    }
}

/// 没有 PDF 时用引用键占位（形如 `li2020control.bib`），并去掉路径分隔符等危险字符
fn bibtex_filename(rec: &bibtex::BibRecord) -> String {
    let base = if !rec.key.trim().is_empty() {
        rec.key.trim().to_string()
    } else {
        rec.title.trim().chars().take(60).collect::<String>()
    };
    let safe: String = base
        .chars()
        .map(|c| {
            if matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') || c.is_control() {
                '_'
            } else {
                c
            }
        })
        .take(120)
        .collect();
    let safe = safe.trim().trim_matches('.').to_string();
    if safe.is_empty() {
        "entry.bib".to_string()
    } else {
        format!("{}.bib", safe)
    }
}

/// 归一化标题：转小写、非字母数字字符转为空格、合并连续空白、去首尾空白。
fn normalize_title(title: &str) -> String {
    let mut out = String::with_capacity(title.len());
    let mut prev_space = true;
    for c in title.to_lowercase().chars() {
        if c.is_alphanumeric() {
            out.push(c);
            prev_space = false;
        } else if !prev_space {
            out.push(' ');
            prev_space = true;
        }
    }
    out.trim().to_string()
}

/// Levenshtein 编辑距离（滚动数组，空间 O(min(m, n))）。
fn levenshtein_distance(s1: &str, s2: &str) -> usize {
    let mut a: Vec<char> = s1.chars().collect();
    let mut b: Vec<char> = s2.chars().collect();
    // 保证 a 较长、b 较短，缩小滚动数组宽度
    if a.len() < b.len() {
        std::mem::swap(&mut a, &mut b);
    }
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut curr = vec![0usize; b.len() + 1];
    for i in 1..=a.len() {
        curr[0] = i;
        for j in 1..=b.len() {
            let cost = if a[i - 1] == b[j - 1] { 0 } else { 1 };
            curr[j] = (prev[j] + 1).min(curr[j - 1] + 1).min(prev[j - 1] + cost);
        }
        std::mem::swap(&mut prev, &mut curr);
    }
    prev[b.len()]
}

/// 归一化后的标题相似度 = 1 - 编辑距离 / 较长标题长度，范围 [0, 1]。
fn title_similarity(a: &str, b: &str) -> f64 {
    let max_len = a.chars().count().max(b.chars().count());
    if max_len == 0 {
        return 1.0;
    }
    let dist = levenshtein_distance(a, b);
    1.0 - (dist as f64 / max_len as f64)
}

/// 相似标题判定阈值：归一化相似度 >= 该值视为重复。
const SIMILARITY_THRESHOLD: f64 = 0.75;

/// GET /assist/api/documents/similar-dois —— 按归一化 DOI 分组的重复文献。
///
/// DOI 比较统一走 [`doi_key`]（`normalize_doi` 去前缀/尾部标点 + 转小写），
/// 兼容历史数据里 `https://doi.org/…`、大写、尾部标点等写法差异。
/// 只返回组内 ≥ 2 篇的分组，供前端「手动去重」按 DOI 合并。
async fn find_similar_dois(
    axum::Extension(db): axum::Extension<Arc<Database>>,
    axum::Extension(settings): axum::Extension<Arc<Settings>>,
) -> Json<Value> {
    let docs: Vec<Document> = sqlx::query_as::<_, Document>(&format!(
        "SELECT {} FROM documents WHERE doi IS NOT NULL AND trim(doi) != '' ORDER BY id",
        DOC_COLUMNS
    ))
    .fetch_all(db.pool())
    .await
    .unwrap_or_default();

    // 1. 按归一化 DOI 分组
    let mut groups: HashMap<String, Vec<Document>> = HashMap::new();
    for doc in docs {
        if let Some(key) = doc.doi.as_deref().and_then(doi_key) {
            groups.entry(key).or_default().push(doc);
        }
    }

    // 2. 只留重复组，按组内最小 id 排序保证输出稳定
    let mut repeated: Vec<(String, Vec<Document>)> = groups
        .into_iter()
        .filter(|(_, docs)| docs.len() > 1)
        .collect();
    repeated.sort_by_key(|(_, docs)| docs.iter().map(|d| d.id).min().unwrap_or(0));

    // 3. 转输出结构（`doi` 给组内第一个非空原值，仅用于展示）
    let mut out_groups: Vec<Value> = Vec::new();
    for (key, docs) in repeated {
        let count = docs.len();
        let display_doi = docs
            .iter()
            .find_map(|d| d.doi.clone().filter(|s| !s.trim().is_empty()))
            .unwrap_or_else(|| key.clone());
        let mut out_docs: Vec<Value> = Vec::with_capacity(count);
        for d in &docs {
            out_docs.push(json!(doc_to_out(d, &settings, &db).await));
        }
        out_groups.push(json!({
            "doi_key": key,
            "doi": display_doi,
            "count": count,
            "documents": out_docs,
        }));
    }

    let total_documents: usize = out_groups
        .iter()
        .map(|g| g["documents"].as_array().map(|a| a.len()).unwrap_or(0))
        .sum();

    Json(json!({
        "groups": out_groups,
        "total_groups": out_groups.len(),
        "total_documents": total_documents,
    }))
}

async fn find_similar_titles(
    axum::Extension(db): axum::Extension<Arc<Database>>,
    axum::Extension(settings): axum::Extension<Arc<Settings>>,
) -> Json<Value> {
    let docs: Vec<Document> = sqlx::query_as::<_, Document>(&format!(
        "SELECT {} FROM documents WHERE title IS NOT NULL AND title != '' ORDER BY id",
        DOC_COLUMNS
    ))
    .fetch_all(db.pool())
    .await
    .unwrap_or_default();

    // 1. 归一化标题并按首字符分桶，缩小候选对规模
    let mut buckets: HashMap<char, Vec<(String, Document)>> = HashMap::new();
    for doc in docs {
        if let Some(title) = &doc.title {
            let norm = normalize_title(title);
            if norm.is_empty() {
                continue;
            }
            if let Some(first) = norm.chars().next() {
                buckets.entry(first).or_default().push((norm, doc));
            }
        }
    }

    // 2. 桶内贪心聚类：每组以第一个文档的归一化标题为代表
    let mut groups: Vec<(String, Vec<Document>)> = Vec::new();
    let mut reps: Vec<String> = Vec::new();

    for (_, items) in buckets {
        for (norm, doc) in items {
            let mut matched_group: Option<usize> = None;
            for (gi, rep) in reps.iter().enumerate() {
                // 长度差剪枝：编辑距离 >= 长度差，若按长度差已无法达到阈值则跳过
                let a_len = norm.chars().count();
                let b_len = rep.chars().count();
                let max_len = a_len.max(b_len);
                let len_diff = a_len.abs_diff(b_len);
                if max_len == 0 || (len_diff as f64) / (max_len as f64) > 1.0 - SIMILARITY_THRESHOLD {
                    continue;
                }
                if title_similarity(&norm, rep) >= SIMILARITY_THRESHOLD {
                    matched_group = Some(gi);
                    break;
                }
            }
            match matched_group {
                Some(gi) => groups[gi].1.push(doc),
                None => {
                    reps.push(norm.clone());
                    groups.push((norm, vec![doc]));
                }
            }
        }
    }

    // 3. 按每组最小 id 排序，保证输出顺序稳定
    groups.sort_by_key(|(_, docs)| docs.iter().map(|d| d.id).min().unwrap_or(0));

    // 4. 过滤单篇组并转为输出结构（异步生成 DocumentOut）
    let mut out_groups: Vec<Value> = Vec::new();
    for (norm, docs) in groups {
        if docs.len() <= 1 {
            continue;
        }
        let count = docs.len();
        let mut out_docs: Vec<Value> = Vec::with_capacity(count);
        for d in &docs {
            out_docs.push(json!(doc_to_out(d, &settings, &db).await));
        }
        out_groups.push(json!({
            "normalized_title": norm,
            "count": count,
            "documents": out_docs,
        }));
    }

    let total_documents: usize = out_groups
        .iter()
        .map(|g| g["documents"].as_array().map(|a| a.len()).unwrap_or(0))
        .sum();

    Json(json!({
        "groups": out_groups,
        "total_groups": out_groups.len(),
        "total_documents": total_documents,
    }))
}

async fn list_vector_dbs() -> Json<Value> {
    let cm = crate::config_manager::ConfigManager::new("system.json");
    let dbs = cm.list_vector_dbs();
    Json(json!({"vector_dbs": dbs}))
}

async fn list_doc_types(
    axum::Extension(db): axum::Extension<Arc<Database>>,
) -> Json<Value> {
    let types: Vec<(String,)> = sqlx::query_as(
        "SELECT DISTINCT doc_type FROM documents WHERE doc_type IS NOT NULL AND doc_type != '' ORDER BY doc_type"
    )
    .fetch_all(db.pool())
    .await
    .unwrap_or_default();

    Json(json!({"doc_types": types.into_iter().map(|(t,)| t).collect::<Vec<_>>()}))
}

/// 元数据搜索（标题、作者、期刊、关键词、摘要、DOI）
#[derive(Debug, Deserialize)]
pub struct SearchInfoQuery {
    pub q: Option<String>,
    pub page: Option<i64>,
    pub page_size: Option<i64>,
    /// PDF 健康筛选：`ok` / `warning` / `error` / `missing` / `unchecked`（逗号多选）
    ///
    /// 与 `q` 的高级语法叠加（AND），供文献列表页从监控面板带入 `?pdf_health=warning,error,missing`。
    pub pdf_health: Option<String>,
}

/// 解析 `pdf_health` 逗号多选参数，生成 SQL 条件与绑定值。
///
/// 返回 `Err(非法取值)` 时端点应回 400（与列表端点 `list_documents` 的校验保持一致）。
fn pdf_health_condition(raw: &str) -> Result<Option<(String, Vec<String>)>, String> {
    let mut values: Vec<String> = Vec::new();
    let mut include_unchecked = false;
    for item in raw.split(',').map(|s| s.trim()).filter(|s| !s.is_empty()) {
        match item.to_ascii_lowercase().as_str() {
            "ok" | "warning" | "error" | "missing" => values.push(item.to_ascii_lowercase()),
            "unchecked" | "none" | "null" => include_unchecked = true,
            _ => {
                return Err(format!(
                    "pdf_health 取值非法：{}（仅支持 ok / warning / error / missing / unchecked）",
                    item
                ))
            }
        }
    }

    if !values.is_empty() {
        let placeholders = values.iter().map(|_| "?").collect::<Vec<_>>().join(",");
        let cond = format!("pdf_health IN ({})", placeholders);
        let cond = if include_unchecked {
            format!("({} OR pdf_health IS NULL)", cond)
        } else {
            format!("({})", cond)
        };
        return Ok(Some((cond, values)));
    }
    if include_unchecked {
        return Ok(Some(("pdf_health IS NULL".to_string(), Vec::new())));
    }
    Ok(None)
}

/// 高级搜索端点。
///
/// 接收前端发来的 query string（如 `transformer title:attention year:>2020`），
/// 在后端解析并构建 SQL 查询，支持字段过滤、范围、OR、排除等高级语法。
async fn search_info(
    axum::Extension(db): axum::Extension<Arc<Database>>,
    axum::Extension(settings): axum::Extension<Arc<Settings>>,
    Query(params): Query<SearchInfoQuery>,
) -> Response {
    let q = params.q.unwrap_or_default();
    let page = params.page.unwrap_or(1).max(1);
    let page_size = params.page_size.unwrap_or(20).clamp(1, 100);

    // 额外的 PDF 健康筛选（来自文献列表页的 `?pdf_health=` 参数）
    let health_cond = match params.pdf_health.as_deref().filter(|s| !s.trim().is_empty()) {
        Some(raw) => match pdf_health_condition(raw) {
            Ok(c) => c,
            Err(detail) => {
                return (StatusCode::BAD_REQUEST, Json(json!({ "detail": detail })))
                    .into_response()
            }
        },
        None => None,
    };

    if q.trim().is_empty() && health_cond.is_none() {
        return Json(json!({"items": [], "query": q, "total": 0, "page": page, "page_size": page_size, "total_pages": 0})).into_response();
    }

    // 解析查询字符串
    let pq = crate::services::query_parser::parse_query(&q);

    // 生成 WHERE 子句和绑定值
    let (mut where_clause, mut binds) = crate::services::query_parser::build_where_clause(&pq, true);
    if let Some((cond, extra)) = health_cond {
        where_clause = format!("({}) AND {}", where_clause, cond);
        binds.extend(extra);
    }

    // 计数
    let count_sql = format!("SELECT COUNT(*) FROM documents WHERE {}", where_clause);
    let mut count_query = sqlx::query_scalar::<_, i64>(&count_sql);
    for b in &binds {
        count_query = count_query.bind(b.clone());
    }
    let total: i64 = count_query.fetch_one(db.pool()).await.unwrap_or(0);

    // 排序
    let sort_col = match pq.sort_by.as_deref() {
        Some("title") => "title",
        Some("authors") => "authors",
        Some("year") => "year",
        Some("id") => "id",
        Some("journal") => "journal",
        Some("doc_type") => "doc_type",
        Some("pdf_size") => "pdf_size",
        Some("md_size") => "md_size",
        Some("zip_size") => "zip_size",
        Some("updated_at") => "updated_at",
        Some("created_at") => "created_at",
        _ => "updated_at",
    };
    let sort_dir = match pq.sort_order.as_deref() {
        Some("asc") => "ASC",
        _ => "DESC",
    };
    let offset = (page - 1) * page_size;
    let search_sql = format!(
        "SELECT {} FROM documents WHERE {} ORDER BY {} {} LIMIT ? OFFSET ?",
        DOC_COLUMNS, where_clause, sort_col, sort_dir
    );
    let mut doc_query = sqlx::query_as::<_, Document>(&search_sql);
    for b in &binds {
        doc_query = doc_query.bind(b.clone());
    }
    doc_query = doc_query.bind(page_size).bind(offset);

    let docs: Vec<Document> = doc_query
        .fetch_all(db.pool())
        .await
        .unwrap_or_default();

    let mut items: Vec<DocumentOut> = Vec::with_capacity(docs.len());
    for d in &docs {
        items.push(doc_to_out(d, &settings, &db).await);
    }

    let total_pages = if total == 0 { 0 } else { (total + page_size - 1) / page_size };

    Json(json!({
        "items": items,
        "query": q,
        "total": total,
        "page": page,
        "page_size": page_size,
        "total_pages": total_pages
    }))
    .into_response()
}

/// 语义搜索
#[derive(Debug, Deserialize)]
pub struct SemanticSearchQuery {
    pub q: Option<String>,
    pub limit: Option<i64>,
    pub vector_db_id: Option<String>,
}

async fn semantic_search(
    axum::Extension(db): axum::Extension<Arc<Database>>,
    Query(params): Query<SemanticSearchQuery>,
) -> Response {
    let q = params.q.unwrap_or_default();
    if q.trim().is_empty() {
        return Json(json!({"results": [], "query": q})).into_response();
    }

    let settings = crate::settings::get_settings();

    // 获取 embedding：使用目标向量库自己的 embedding 配置（独立于 LLM 配置）
    let manager = crate::config_manager::ConfigManager::new("system.json");
    let vdb_config = match params.vector_db_id.as_deref() {
        Some(id) => manager.get_vector_db_by_id(id),
        None => manager.get_active_vector_db(),
    };
    let embed_client = settings.embedding_config_for(vdb_config.as_ref()).client();
    let embedding = match embed_client.get_embedding(q.trim()).await {
        Ok(e) => e,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"detail": format!("Embedding 失败: {}", e)}))).into_response(),
    };

    // 搜索向量库
    let adapter = match crate::services::vector_db::get_vector_db(params.vector_db_id.as_deref()) {
        Ok(a) => a,
        Err(e) => return (StatusCode::BAD_REQUEST, Json(json!({"detail": e.to_string()}))).into_response(),
    };

    let results = match adapter.search_similar(0, &embedding, params.limit.unwrap_or(5) as usize).await {
        Ok(r) => r,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"detail": format!("搜索失败: {}", e)}))).into_response(),
    };

    // 补充文档元数据
    let mut enriched = Vec::new();
    for hit in &results {
        let doc_id = hit["document_id"].as_i64();
        let mut doc_info = json!({});
        if let Some(did) = doc_id {
            if let Ok(Some(doc)) = fetch_doc(&db, did).await {
                // 同样不再给 `status`：只给文件事实，状态由调用方（前端）自行推导
                doc_info = json!({
                    "filename": doc.filename,
                    "authors": doc.authors,
                    "year": doc.year,
                    "journal": doc.journal,
                    "has_md": doc.has_md(),
                    "has_pdf": doc.has_pdf(),
                    "synced": doc.fs_synced_at.is_some(),
                    "pdf_health": doc.pdf_health,
                });
            }
        }
        let mut item = hit.clone();
        if let (Some(obj), Some(info)) = (item.as_object_mut(), doc_info.as_object()) {
            for (k, v) in info {
                obj.insert(k.clone(), v.clone());
            }
        }
        enriched.push(item);
    }

    Json(json!({"results": enriched, "query": q})).into_response()
}

/// 解析 query 中的布尔值（宽松取值：1/0、true/false、yes/no、on/off 等）。
fn de_flexible_bool<'de, D>(deserializer: D) -> Result<Option<bool>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let raw = String::deserialize(deserializer)?;
    let t = raw.trim().to_ascii_lowercase();
    Ok(match t.as_str() {
        "1" | "true" | "t" | "yes" | "y" | "on" => Some(true),
        "0" | "false" | "f" | "no" | "n" | "off" | "" => Some(false),
        // 无法识别的值按未提供处理（宽松，避免 400）
        _ => None,
    })
}

/// 全文 grep 搜索
#[derive(Debug, Deserialize)]
pub struct GrepSearchQuery {
    pub q: Option<String>,
    pub limit: Option<i64>,
    pub context: Option<i64>,
    pub doc_ids: Option<String>,
    pub algorithm: Option<String>,
    #[serde(default, deserialize_with = "de_flexible_bool")]
    pub regex: Option<bool>,
    pub journal: Option<String>,
    pub year_start: Option<i64>,
    pub year_end: Option<i64>,
    pub start_after_id: Option<i64>,
}

async fn grep_search(
    axum::Extension(db): axum::Extension<Arc<Database>>,
    Query(params): Query<GrepSearchQuery>,
) -> Response {
    let q = params.q.unwrap_or_default();
    if q.trim().is_empty() {
        return Json(json!({"results": [], "query": q})).into_response();
    }

    // 解析 doc_ids
    let id_list = match crate::services::grep_search::parse_doc_ids(params.doc_ids.as_deref().unwrap_or("")) {
        Ok(ids) => ids,
        Err(_) => return (StatusCode::BAD_REQUEST, Json(json!({"detail": "doc_ids 格式无效，支持逗号与区间，如 1,2,5-100"}))).into_response(),
    };

    let settings = crate::settings::get_settings();
    let results = crate::services::grep_search::grep_search(
        q.trim(),
        params.context.unwrap_or(2) as usize,
        params.limit.unwrap_or(10) as usize,
        id_list.as_deref(),
        params.algorithm.as_deref().unwrap_or("full"),
        params.regex.unwrap_or(true),
        db.pool(),
        &settings,
        params.journal.as_deref(),
        params.year_start,
        params.year_end,
        params.start_after_id,
    )
    .await;

    // 补充文档元数据（批量查询，避免 N+1）
    let mut enriched = Vec::new();
    let ids: Vec<i64> = results.iter().filter_map(|hit| hit["id"].as_i64()).collect();
    let mut doc_map: std::collections::HashMap<i64, serde_json::Value> = std::collections::HashMap::new();
    if !ids.is_empty() {
        let ids_str: Vec<String> = ids.iter().map(|id| id.to_string()).collect();
        let sql = format!("SELECT {} FROM documents WHERE id IN ({})", DOC_COLUMNS, ids_str.join(","));
        if let Ok(rows) = sqlx::query_as::<_, Document>(&sql).fetch_all(db.pool()).await {
            for doc in rows {
                // 同样不再给 `status`：只给文件事实
                doc_map.insert(doc.id, json!({
                    "title": doc.title,
                    "filename": doc.filename,
                    "authors": doc.authors,
                    "year": doc.year,
                    "journal": doc.journal,
                    "has_md": doc.has_md(),
                    "has_pdf": doc.has_pdf(),
                    "synced": doc.fs_synced_at.is_some(),
                    "pdf_health": doc.pdf_health,
                }));
            }
        }
    }
    for hit in &results {
        let mut item = hit.clone();
        if let Some(did) = hit["id"].as_i64() {
            if let Some(info) = doc_map.get(&did) {
                if let (Some(obj), Some(info_obj)) = (item.as_object_mut(), info.as_object()) {
                    for (k, v) in info_obj {
                        obj.insert(k.clone(), v.clone());
                    }
                }
            }
        }
        enriched.push(item);
    }

    let last_id = results.iter()
        .filter_map(|r| r["id"].as_i64())
        .max();

    Json(json!({"results": enriched, "query": q, "last_id": last_id})).into_response()
}

async fn by_hash(
    axum::Extension(db): axum::Extension<Arc<Database>>,
    axum::Extension(settings): axum::Extension<Arc<Settings>>,
    Path(hash): Path<String>,
) -> Response {
    if hash.len() != 64 || !hash.chars().all(|c| c.is_ascii_hexdigit()) {
        return (StatusCode::BAD_REQUEST, Json(json!({"detail": "哈希格式无效，需要 64 位 SHA256"}))).into_response();
    }
    let doc: Option<Document> = sqlx::query_as::<_, Document>(&format!(
        "SELECT {} FROM documents WHERE file_hash = ? LIMIT 1", DOC_COLUMNS
    ))
    .bind(&hash)
    .fetch_optional(db.pool())
    .await
    .unwrap_or(None);

    match doc {
        Some(d) => (StatusCode::OK, Json(json!(doc_to_out(&d, &settings, &db).await))).into_response(),
        None => (StatusCode::NOT_FOUND, Json(json!({"detail": "未找到匹配的文档"}))).into_response(),
    }
}

/// DOI 比对键：先归一化（去 `https://doi.org/` 等前缀、去尾部标点）再转小写。
///
/// DOI 本身大小写不敏感，而库里既有 `fill_meta_fields` / `pipeline` 归一化后的值，
/// 也有历史数据里原样保存的 Zotero 写法（可能带 `https://doi.org/` 前缀）。
/// 比对时统一压平成这个键。
fn doi_key(raw: &str) -> Option<String> {
    normalize_doi(raw).map(|d| d.to_lowercase())
}

/// 按 DOI 查找文档（宽松匹配）。
///
/// 1. 先原样精确匹配（走索引，命中已归一化的数据，绝大多数情况在这里返回）；
/// 2. 未命中则用 `LIKE` 粗筛（SQLite 的 LIKE 对 ASCII 大小写不敏感）+ `doi_key` 校验，
///    兼容历史数据里的前缀/大小写差异与尾部标点。
async fn find_doc_by_doi(db: &Database, raw: &str) -> Option<Document> {
    if let Ok(Some(doc)) = sqlx::query_as::<_, Document>(&format!(
        "SELECT {} FROM documents WHERE doi = ? LIMIT 1", DOC_COLUMNS
    ))
    .bind(raw)
    .fetch_optional(db.pool())
    .await
    {
        return Some(doc);
    }

    let target = doi_key(raw)?;
    // LIKE 的通配符需转义，否则 DOI 里的 `_` 会当成单字符通配
    let pattern = format!(
        "%{}%",
        target
            .replace('\\', "\\\\")
            .replace('%', "\\%")
            .replace('_', "\\_")
    );
    let rows: Vec<Document> = sqlx::query_as::<_, Document>(&format!(
        "SELECT {} FROM documents WHERE doi IS NOT NULL AND doi <> '' AND doi LIKE ? ESCAPE '\\' LIMIT 200",
        DOC_COLUMNS
    ))
    .bind(pattern)
    .fetch_all(db.pool())
    .await
    .unwrap_or_default();

    rows.into_iter()
        .find(|d| d.doi.as_deref().and_then(doi_key).as_deref() == Some(target.as_str()))
}

/// 按文件哈希查找文档（上传去重使用）。
///
/// 这是「便宜」的那一路：只查 `file_hash` 索引，完全不碰 PDF 内容。
async fn find_doc_by_hash(db: &Database, hash: &str) -> Option<Document> {
    sqlx::query_as::<_, Document>(&format!(
        "SELECT {} FROM documents WHERE file_hash = ? LIMIT 1", DOC_COLUMNS
    ))
    .bind(hash)
    .fetch_optional(db.pool())
    .await
    .unwrap_or(None)
}

/// 把已落盘的临时 PDF 应用到**已有**文档：搬文件 → 同步体积 → 更新哈希与健康状态。
///
/// 上传命中重复且 `on_duplicate=replace`、以及 `POST /{id}/pdf` 替换 PDF 都走这里。
/// `pdf_health` 一律置空（旧结果对新文件无效）。
///
/// **替换 PDF 不影响解析阶段**：状态由文件事实推导（有 Markdown 就是 `markdown_done`），
/// 不再有 `reset_status` 开关；需要重新解析请调 `POST /assist/api/pipeline/{id}/reset`
/// （「清空解析产物」）。
/// `doi` 为 `Some` 时只在该文档原本没有 DOI 时补写（已有 DOI 不覆盖）。
async fn persist_pdf_for_doc(
    db: &Database,
    settings: &Settings,
    doc_id: i64,
    temp_upload: TempUpload,
    file_hash: &str,
    doi: Option<String>,
) -> Result<(), String> {
    let pdf_path = paths::get_pdf_path(settings, doc_id);
    persist_temp_file(temp_upload, &pdf_path)
        .await
        .map_err(|e| e.to_string())?;

    // 写时同步：更新文件大小到数据库
    sync_doc_file_sizes(db, settings, doc_id).await;

    let sql = "UPDATE documents SET file_hash = ?, \
         pdf_health = NULL, pdf_health_message = NULL, \
         doi = COALESCE(NULLIF(doi, ''), ?), updated_at = ? WHERE id = ?";
    if let Err(e) = sqlx::query(sql)
    .bind(file_hash)
    .bind(doi)
    .bind(now_iso())
    .bind(doc_id)
    .execute(db.pool())
    .await
    {
        return Err(e.to_string());
    }
    Ok(())
}

async fn by_doi(
    axum::Extension(db): axum::Extension<Arc<Database>>,
    axum::Extension(settings): axum::Extension<Arc<Settings>>,
    Path(doi): Path<String>,
) -> Response {
    if doi_key(&doi).is_none() {
        return (StatusCode::BAD_REQUEST, Json(json!({"detail": "DOI 不能为空"}))).into_response();
    }

    match find_doc_by_doi(&db, &doi).await {
        Some(d) => Json(json!({
            "found": true,
            "doc": doc_to_out(&d, &settings, &db).await,
        })).into_response(),
        None => Json(json!({
            "found": false,
            "doi": doi,
            "doi_key": doi_key(&doi),
        })).into_response(),
    }
}

/// 从 Markdown 文本中提取目录（标题 + 行号）
fn extract_toc(content: &str) -> Vec<Value> {
    let mut toc = Vec::new();
    let re = regex::Regex::new(r"^(#{1,6})\s+(.+)$").unwrap();

    for (line_idx, line) in content.lines().enumerate() {
        if let Some(caps) = re.captures(line) {
            let level = caps.get(1).unwrap().as_str().len();
            let title = caps.get(2).unwrap().as_str().trim().to_string();
            if !title.is_empty() {
                toc.push(json!({
                    "level": level,
                    "title": title,
                    "line": line_idx,
                }));
            }
        }
    }
    toc
}

/// 重写 Markdown 中的图片路径
fn rewrite_image_paths(content: &str, doc_id: i64) -> String {
    let re = regex::Regex::new(r"\]\(([^)]+)\)").unwrap();
    re.replace_all(content, |caps: &regex::Captures| {
        let img_path = &caps[1];
        if img_path.starts_with("http://") || img_path.starts_with("https://") || img_path.starts_with("/assist/") {
            format!("]({})", img_path)
        } else {
            let filename = std::path::Path::new(img_path)
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| img_path.to_string());
            format!("](/assist/api/documents/{}/image/{})", doc_id, filename)
        }
    })
    .to_string()
}

/// 反向还原图片路径
fn restore_image_paths(content: &str, doc_id: i64) -> String {
    let pattern = format!(r"\]\(/assist/api/documents/{}/image/([^)]+)\)", doc_id);
    let re = regex::Regex::new(&pattern).unwrap();
    re.replace_all(content, "](images/$1)").to_string()
}

#[derive(Debug, Deserialize)]
pub struct MarkdownQuery {
    pub line_start: Option<i64>,
    pub line_count: Option<i64>,
    #[serde(default, deserialize_with = "de_flexible_bool")]
    pub full: Option<bool>,
    /// 返回文件原始内容（不做图片路径重写）。源码编辑器用它保证「读-改-写」无损。
    #[serde(default, deserialize_with = "de_flexible_bool")]
    pub raw: Option<bool>,
}

/// GET /assist/api/documents/:id/markdown
///
/// 按行范围读取 Markdown 内容。
/// 参数：
///   - line_start: 起始行号（0-indexed，默认 0）
///   - line_count: 返回行数（默认 -1 即到末尾）
///   - full: 忽略其他参数，返回全部内容
async fn get_markdown(
    axum::Extension(role): axum::Extension<auth::Role>,
    axum::Extension(db): axum::Extension<Arc<Database>>,
    axum::Extension(settings): axum::Extension<Arc<Settings>>,
    Path(id): Path<i64>,
    Query(params): Query<MarkdownQuery>,
) -> Response {
    if let Err(resp) = auth::assert_role(&role, &["admin", "view-only", "view-upload"]) {
        return resp;
    }
    match fetch_doc(&db, id).await {
        Ok(Some(_)) => {}
        Ok(None) => return (StatusCode::NOT_FOUND, Json(json!({"detail": "文献不存在"}))).into_response(),
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"detail": e}))).into_response(),
    }

    let md_path = paths::get_markdown_path(&settings, id);
    if !std::path::Path::new(&md_path).exists() {
        return (StatusCode::NOT_FOUND, Json(json!({"detail": "Markdown 文件尚未生成"}))).into_response();
    }

    let raw_content = match std::fs::read_to_string(&md_path) {
        Ok(c) => c,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"detail": e.to_string()}))).into_response(),
    };

    // 文件版本（乐观并发控制）：基于文件真实字节计算，与 raw 与否无关
    let hash = sha256_hex_async(raw_content.clone().into_bytes()).await;

    // 默认把本地图片路径重写为可访问的 API URL；源码面板用 raw=1 拿原始路径
    let content = if params.raw.unwrap_or(false) {
        raw_content
    } else {
        rewrite_image_paths(&raw_content, id)
    };
    let total_length = content.len();
    let total_lines = content.lines().count();

    if params.full.unwrap_or(false) {
        return Json(json!({
            "content": content,
            "total_length": total_length,
            "total_lines": total_lines,
            "line_start": 0,
            "line_count": total_lines as i64,
            "lines_returned": total_lines as i64,
            "hash": hash,
        })).into_response();
    }

    let line_start = params.line_start.unwrap_or(0).max(0) as usize;
    let line_count = params.line_count.unwrap_or(-1);

    let lines: Vec<&str> = content.lines().collect();
    let actual_start = line_start.min(lines.len());
    let actual_end = if line_count < 0 {
        lines.len()
    } else {
        (actual_start + line_count as usize).min(lines.len())
    };
    let lines_returned = actual_end - actual_start;
    let slice = if lines_returned > 0 {
        lines[actual_start..actual_end].join("\n")
    } else {
        String::new()
    };

    Json(json!({
        "content": slice,
        "total_length": total_length,
        "total_lines": total_lines as i64,
        "line_start": actual_start,
        "line_count": line_count,
        "lines_returned": lines_returned as i64,
        "hash": hash,
    })).into_response()
}

/// 一个 Markdown 区块
#[derive(Debug, Serialize)]
struct ChunkItem {
    pub id: usize,
    pub start_line: usize,
    pub end_line: usize,
    pub content: String,
    pub heading: Option<String>,
}

/// GET /assist/api/documents/:id/chunks
///
/// 将 Markdown 文件按 heading 边界分为~200-500行的语义完整区块。
/// 每个 chunk 自然从 heading 开始，前端用于增量加载和渲染。
async fn get_markdown_chunks(
    axum::Extension(role): axum::Extension<auth::Role>,
    axum::Extension(settings): axum::Extension<Arc<Settings>>,
    Path(id): Path<i64>,
) -> Response {
    if let Err(resp) = auth::assert_role(&role, &["admin", "view-only", "view-upload"]) {
        return resp;
    }

    let md_path = paths::get_markdown_path(&settings, id);
    if !std::path::Path::new(&md_path).exists() {
        return (StatusCode::NOT_FOUND, Json(json!({"detail": "Markdown 文件尚未生成"}))).into_response();
    }

    let content = match std::fs::read_to_string(&md_path) {
        Ok(c) => c,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"detail": e.to_string()}))).into_response(),
    };
    let content = rewrite_image_paths(&content, id);
    let total_lines = content.lines().count();
    if total_lines == 0 {
        return Json(json!({"chunks": [], "total_lines": 0, "doc_id": id})).into_response();
    }

    // 找出所有 heading 行号（行以 # 开头）
    let lines: Vec<&str> = content.lines().collect();
    let heading_indices: Vec<usize> = lines.iter().enumerate()
        .filter(|(_, line)| line.starts_with('#'))
        .map(|(i, _)| i)
        .collect();

    const MIN_CHUNK: usize = 200;
    const MAX_CHUNK: usize = 500;

    let mut chunks: Vec<ChunkItem> = Vec::new();
    let mut start = 0usize;

    while start < total_lines {
        // 理想结束点：在 [start+MIN_CHUNK, start+MAX_CHUNK) 内找最近的 heading
        let search_lo = (start + MIN_CHUNK).min(total_lines);
        let search_hi = (start + MAX_CHUNK).min(total_lines);

        let mut end = search_hi - 1; // 默认在 MAX_CHUNK 处截断

        // 优先在 heading 处切分
        for &h in &heading_indices {
            if h >= search_lo && h < search_hi {
                end = h.saturating_sub(1); // heading 放到下一个 chunk
                break;
            }
        }

        // 边界保护：剩余行数不足 MIN_CHUNK 时合并到最后 chunk
        if total_lines - end <= MIN_CHUNK && !chunks.is_empty() {
            // 合并到最后 chunk
            let last = chunks.last_mut().unwrap();
            let old_end = last.end_line;
            let new_content = lines[old_end + 1..total_lines].join("\n");
            last.content.push_str("\n");
            last.content.push_str(&new_content);
            last.end_line = total_lines - 1;
            break;
        }

        let chunk_lines = &lines[start..=end];
        let first_line = chunk_lines.first().unwrap_or(&"");
        let heading = if first_line.starts_with('#') {
            Some(first_line.trim_start_matches('#').trim().to_string())
        } else {
            None
        };

        chunks.push(ChunkItem {
            id: chunks.len(),
            start_line: start,
            end_line: end,
            content: chunk_lines.join("\n"),
            heading,
        });

        start = end + 1;
    }

    Json(json!({
        "chunks": chunks,
        "total_lines": total_lines,
        "doc_id": id,
    })).into_response()
}

/// GET /assist/api/documents/:id/toc
///
/// 从完整 Markdown 文件中提取目录（标题 + 行号）。
async fn get_toc(
    axum::Extension(db): axum::Extension<Arc<Database>>,
    axum::Extension(settings): axum::Extension<Arc<Settings>>,
    Path(id): Path<i64>,
) -> Response {
    match fetch_doc(&db, id).await {
        Ok(Some(_)) => {}
        Ok(None) => return (StatusCode::NOT_FOUND, Json(json!({"detail": "文献不存在"}))).into_response(),
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"detail": e}))).into_response(),
    }

    let md_path = paths::get_markdown_path(&settings, id);
    if !std::path::Path::new(&md_path).exists() {
        return (StatusCode::NOT_FOUND, Json(json!({"detail": "Markdown 文件尚未生成"}))).into_response();
    }

    let content = match std::fs::read_to_string(&md_path) {
        Ok(c) => c,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"detail": e.to_string()}))).into_response(),
    };

    let toc = extract_toc(&content);
    Json(json!({"toc": toc})).into_response()
}

#[derive(Debug, Deserialize)]
pub struct MarkdownUpdate {
    pub content: String,
    /// 乐观并发控制：客户端读取时的文件 hash；不传则不做校验
    #[serde(default)]
    pub base_hash: Option<String>,
}

/// 校验乐观并发控制：文件当前 hash 与客户端携带的 base_hash 不一致时返回 409。
fn check_base_hash(base_hash: &Option<String>, current: &str) -> Option<Response> {
    match base_hash {
        Some(expected) if !expected.is_empty() && expected != current => Some(
            (StatusCode::CONFLICT, Json(json!({
                "detail": "文件已被其它操作修改，请重新加载后再保存",
                "hash": current,
                "code": "conflict",
            })))
            .into_response(),
        ),
        _ => None,
    }
}

async fn update_markdown(
    axum::Extension(role): axum::Extension<auth::Role>,
    axum::Extension(db): axum::Extension<Arc<Database>>,
    axum::Extension(settings): axum::Extension<Arc<Settings>>,
    Path(id): Path<i64>,
    Json(data): Json<MarkdownUpdate>,
) -> Response {
    if let Err(resp) = auth::assert_role(&role, &["admin"]) {
        return resp;
    }
    match fetch_doc(&db, id).await {
        Ok(Some(_)) => {}
        Ok(None) => return (StatusCode::NOT_FOUND, Json(json!({"detail": "文献不存在"}))).into_response(),
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"detail": e}))).into_response(),
    }

    let md_path = paths::get_markdown_path(&settings, id);
    if !std::path::Path::new(&md_path).exists() {
        return (StatusCode::BAD_REQUEST, Json(json!({"detail": "Markdown 文件尚未生成"}))).into_response();
    }

    // 并发校验：基于写之前的文件内容
    let current_hash = match std::fs::read(&md_path) {
        Ok(bytes) => sha256_hex_async(bytes).await,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"detail": e.to_string()}))).into_response(),
    };
    if let Some(resp) = check_base_hash(&data.base_hash, &current_hash) {
        return resp;
    }

    let content = restore_image_paths(&data.content, id);
    match write_atomic(&md_path, &content) {
        Ok(_) => {
            // 同步文件大小，避免列表页 md_size 过期
            sync_doc_file_sizes(&db, &settings, id).await;
            let hash = sha256_hex_async(content.into_bytes()).await;
            Json(json!({"detail": "Markdown 已更新", "hash": hash})).into_response()
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"detail": e.to_string()}))).into_response(),
    }
}

#[derive(Debug, Deserialize)]
pub struct MarkdownLinesUpdate {
    /// 起始行号（0-indexed，含）
    pub line_start: i64,
    /// 要替换的行数（0 表示纯插入）
    pub line_count: i64,
    /// 替换后的文本；其行数 = `content.split('\n').count()`，与「按行切片再 join」互为逆运算。
    /// 空字符串表示 1 个空行（不表示删除）。
    #[serde(default)]
    pub content: String,
    /// 显式删除：true 时直接删除 [line_start, line_start + line_count) 这些行（忽略 content）。
    /// 因为空字符串固定等于 1 个空行，删行必须由本字段显式表达。
    #[serde(default)]
    pub delete: Option<bool>,
    /// 乐观并发控制：客户端加载该行范围时拿到的文件 hash；不传则跳过校验
    #[serde(default)]
    pub base_hash: Option<String>,
}

/// 原子写文件：先写同目录临时文件再 rename，避免崩溃/磁盘满导致截断。
pub(crate) fn write_atomic(path: &str, content: &str) -> std::io::Result<()> {
    let target = std::path::Path::new(path);
    let tmp = target.with_extension(format!(
        "{}.tmp",
        target.extension().map(|e| e.to_string_lossy().to_string()).unwrap_or_default()
    ));
    std::fs::write(&tmp, content)?;
    match std::fs::rename(&tmp, target) {
        Ok(_) => Ok(()),
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            Err(e)
        }
    }
}

/// PUT /assist/api/documents/:id/markdown/lines
///
/// 按行范围替换 Markdown 源码（就地编辑用）。
/// 语义：
///   - line_start 为 0-indexed 且包含该行；
///   - 被替换的区间为 [line_start, line_start + line_count)；
///   - line_count = 0 时为纯插入；
///   - 新行数 = content.split('\n').count()，这与「读切片 join('\n')」严格互逆，
///     因此「读出来原样写回去」字节不变（含空行、末尾换行）；
///   - `delete: true` 时直接删掉整段行范围（忽略 content）——空字符串固定等于 1 个空行，
///     因此删行必须显式声明；
///   - 写入的是文件真实内容，不做图片路径重写/还原（与 raw=1 读取配对）；
///   - 携带 base_hash（GET 返回的 hash）时做乐观并发校验，不匹配返回 409。
async fn update_markdown_lines(
    axum::Extension(role): axum::Extension<auth::Role>,
    axum::Extension(db): axum::Extension<Arc<Database>>,
    axum::Extension(settings): axum::Extension<Arc<Settings>>,
    Path(id): Path<i64>,
    Json(data): Json<MarkdownLinesUpdate>,
) -> Response {
    if let Err(resp) = auth::assert_role(&role, &["admin"]) {
        return resp;
    }
    match fetch_doc(&db, id).await {
        Ok(Some(_)) => {}
        Ok(None) => return (StatusCode::NOT_FOUND, Json(json!({"detail": "文献不存在"}))).into_response(),
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"detail": e}))).into_response(),
    }

    let md_path = paths::get_markdown_path(&settings, id);
    if !std::path::Path::new(&md_path).exists() {
        return (StatusCode::BAD_REQUEST, Json(json!({"detail": "Markdown 文件尚未生成"}))).into_response();
    }

    let raw = match std::fs::read_to_string(&md_path) {
        Ok(c) => c,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"detail": e.to_string()}))).into_response(),
    };

    let ends_with_newline = raw.ends_with('\n');
    let lines: Vec<&str> = raw.lines().collect();

    // 乐观并发校验：基于本次读取到的文件内容
    let current_hash = sha256_hex_async(raw.clone().into_bytes()).await;
    if let Some(resp) = check_base_hash(&data.base_hash, &current_hash) {
        return resp;
    }

    if data.line_start < 0 || data.line_count < 0 {
        return (StatusCode::BAD_REQUEST, Json(json!({"detail": "line_start / line_count 不能为负数"}))).into_response();
    }

    let start = (data.line_start as usize).min(lines.len());
    let end = (start + data.line_count as usize).min(lines.len());

    // 严格互逆：line_count=1 取回的空行是 ''，写回时也应得到 1 个空行；
    // 删行由 delete=true 显式表达
    let new_lines: Vec<&str> = if data.delete.unwrap_or(false) {
        Vec::new()
    } else {
        data.content.split('\n').collect()
    };
    let lines_written = new_lines.len();

    let mut merged: Vec<&str> = Vec::with_capacity(lines.len() + lines_written);
    merged.extend_from_slice(&lines[..start]);
    merged.extend_from_slice(&new_lines);
    merged.extend_from_slice(&lines[end..]);

    let mut result = merged.join("\n");
    if ends_with_newline && !result.is_empty() {
        result.push('\n');
    }

    if let Err(e) = write_atomic(&md_path, &result) {
        return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"detail": e.to_string()}))).into_response();
    }

    // 同步文件大小，避免列表页 md_size 过期
    sync_doc_file_sizes(&db, &settings, id).await;

    let total_lines = result.lines().count();
    let hash = sha256_hex_async(result.clone().into_bytes()).await;
    Json(json!({
        "detail": "Markdown 已更新",
        "line_start": start,
        "line_count": data.line_count,
        "lines_written": lines_written,
        "total_lines": total_lines,
        "md_size": file_size(&md_path),
        "hash": hash,
    }))
    .into_response()
}

/// 上传 MinerU 解析结果（Markdown 内容）到文档。
///
/// 与 pipeline 的 parse 阶段完成等价，但由外部脚本直接上传解析结果。
/// 状态不需要手写：写完 Markdown 并同步文件事实后，派生状态自然变为 `markdown_done`。
#[derive(Debug, Deserialize)]
pub struct ParseResultBody {
    pub content: String,
}

async fn upload_parse_result(
    axum::Extension(role): axum::Extension<auth::Role>,
    axum::Extension(db): axum::Extension<Arc<Database>>,
    axum::Extension(settings): axum::Extension<Arc<Settings>>,
    Path(id): Path<i64>,
    Json(data): Json<ParseResultBody>,
) -> Response {
    if let Err(resp) = auth::assert_role(&role, &["admin"]) {
        return resp;
    }
    // 校验文档存在
    match fetch_doc(&db, id).await {
        Ok(Some(_)) => {}
        Ok(None) => return (StatusCode::NOT_FOUND, Json(json!({"detail": "文献不存在"}))).into_response(),
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"detail": e}))).into_response(),
    };

    // 写入 Markdown 文件（不存在则创建）
    let md_path = paths::get_markdown_path(&settings, id);
    if let Some(parent) = std::path::Path::new(&md_path).parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Err(e) = std::fs::write(&md_path, &data.content) {
        return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"detail": format!("写入 Markdown 失败: {}", e)}))).into_response();
    }

    // 写时同步文件事实（md_size 更新后，派生状态即为 markdown_done）
    sync_doc_file_sizes(&db, &settings, id).await;
    crate::services::file_state::clear_error(&settings, id);

    Json(json!({
        "id": id,
        "status": "markdown_done",
        "markdown_path": md_path,
    }))
    .into_response()
}

/// 带 HTTP Range / ETag / If-None-Match 支持的 PDF 文件响应。
///
/// 浏览器原生 PDF 查看器依赖 Range 头实现分页渲染和滚动跳转。
/// ETag 基于文件修改时间+文件大小生成，支持条件请求（304 Not Modified）。
pub async fn serve_pdf_file(pdf_path: &str, headers: &HeaderMap) -> Response {
    let file = match tokio::fs::File::open(pdf_path).await {
        Ok(f) => f,
        Err(_) => {
            return (StatusCode::NOT_FOUND, Json(json!({"detail": "PDF 文件不存在"}))).into_response();
        }
    };

    let meta = match file.metadata().await {
        Ok(m) => m,
        Err(_) => {
            return (StatusCode::NOT_FOUND, Json(json!({"detail": "PDF 文件不存在"}))).into_response();
        }
    };

    let file_size = meta.len();
    let etag = make_etag(&meta);

    // --- If-None-Match: 资源未变更时返回 304 ---
    if let Some(h) = headers.get(header::IF_NONE_MATCH) {
        if let Ok(val) = h.to_str() {
            if val == "*" || val == etag || val == format!("W/{}", etag) {
                let mut resp = Response::new(axum::body::Body::empty());
                *resp.status_mut() = StatusCode::NOT_MODIFIED;
                resp.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("public, max-age=86400"));
                resp.headers_mut().insert(header::ETAG, etag.parse().unwrap());
                return resp;
            }
        }
    }

    // --- Range 请求: 返回 206 Partial Content ---
    let range_result = headers
        .get(header::RANGE)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| parse_range(s, file_size));

    if let Some((start, end)) = range_result {
        let length = (end - start + 1) as usize;
        let mut buf = vec![0u8; length];
        let mut file = file;
        let ok = file.seek(std::io::SeekFrom::Start(start)).await.is_ok()
            && file.read_exact(&mut buf).await.is_ok();

        if ok {
            let body = axum::body::Body::from(buf);
            let mut resp = Response::new(body);
            *resp.status_mut() = StatusCode::PARTIAL_CONTENT;
            resp.headers_mut().insert(header::CONTENT_TYPE, HeaderValue::from_static("application/pdf"));
            resp.headers_mut().insert(header::CONTENT_RANGE, format!("bytes {}-{}/{}", start, end, file_size).parse().unwrap());
            resp.headers_mut().insert(header::CONTENT_LENGTH, HeaderValue::from_str(&length.to_string()).unwrap());
            resp.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("public, max-age=86400"));
            resp.headers_mut().insert(header::ETAG, etag.parse().unwrap());
            resp.headers_mut().insert(header::ACCEPT_RANGES, HeaderValue::from_static("bytes"));
            return resp;
        }
    }

    // --- 不支持 Range 或解析失败 → 返回完整文件 (200 OK) ---
    match tokio::fs::read(pdf_path).await {
        Ok(bytes) => {
            let mut resp = Response::new(axum::body::Body::from(bytes));
            *resp.status_mut() = StatusCode::OK;
            resp.headers_mut().insert(header::CONTENT_TYPE, HeaderValue::from_static("application/pdf"));
            resp.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("public, max-age=86400"));
            resp.headers_mut().insert(header::ETAG, etag.parse().unwrap());
            resp.headers_mut().insert(header::ACCEPT_RANGES, HeaderValue::from_static("bytes"));
            resp
        }
        Err(_) => (StatusCode::NOT_FOUND, Json(json!({"detail": "PDF 文件不存在"}))).into_response(),
    }
}

/// 基于文件修改时间和大小生成弱 ETag。
fn make_etag(meta: &std::fs::Metadata) -> String {
    let ts = meta.modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("\"{:x}-{:x}\"", ts, meta.len())
}

/// 解析 HTTP Range 请求头，支持三种格式：
/// - `bytes=start-end` — 闭区间
/// - `bytes=start-` — 开区间（起始到末尾）
/// - `bytes=-suffix` — 末尾 N 字节
///
/// 仅处理首个 range（浏览器原生 PDF 查看器只发单 range）。
fn parse_range(range_str: &str, file_size: u64) -> Option<(u64, u64)> {
    let s = range_str.trim();
    if !s.starts_with("bytes=") {
        return None;
    }
    let spec = s[6..].split(',').next()?.trim();
    if spec.is_empty() {
        return None;
    }

    if let Some(rest) = spec.strip_prefix('-') {
        // bytes=-500: 最后 500 字节
        let count: u64 = rest.parse().ok()?;
        if count == 0 {
            return None;
        }
        let start = file_size.saturating_sub(count);
        return Some((start, file_size - 1));
    }

    if let Some((a, b)) = spec.split_once('-') {
        let start: u64 = a.parse().ok()?;
        if start >= file_size {
            return None;
        }
        if b.is_empty() {
            // bytes=1024-: 从 start 到末尾
            return Some((start, file_size - 1));
        }
        let end: u64 = b.parse().ok()?;
        if start > end {
            return None;
        }
        return Some((start, end.min(file_size - 1)));
    }

    None
}

async fn get_pdf(
    axum::Extension(role): axum::Extension<auth::Role>,
    axum::Extension(db): axum::Extension<Arc<Database>>,
    axum::Extension(settings): axum::Extension<Arc<Settings>>,
    Path(id): Path<i64>,
    headers: HeaderMap,
) -> Response {
    if let Err(resp) = auth::assert_role(&role, &["admin", "view-only", "view-upload"]) {
        return resp;
    }
    match fetch_doc(&db, id).await {
        Ok(Some(_)) => {}
        Ok(None) => return (StatusCode::NOT_FOUND, Json(json!({"detail": "文献不存在"}))).into_response(),
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"detail": e}))).into_response(),
    }

    let pdf_path = paths::get_pdf_path(&settings, id);
    serve_pdf_file(&pdf_path, &headers).await
}

/// 通过别名获取文档信息（用于 /assist/file/ 路由）
async fn file_alias(
    axum::Extension(role): axum::Extension<auth::Role>,
    axum::Extension(db): axum::Extension<Arc<Database>>,
    axum::Extension(settings): axum::Extension<Arc<Settings>>,
    Path(id): Path<i64>,
) -> Response {
    if let Err(resp) = auth::assert_role(&role, &["admin", "view-only", "view-upload"]) {
        return resp;
    }
    match fetch_doc(&db, id).await {
        Ok(Some(doc)) => {
            let (alias, expires_iso) = get_or_create_alias(id, doc.year, doc.title.as_deref(), settings.alias_expiration_hours());
            Json(json!({
                "url": format!("/assist/file/{}", alias),
                "expires_at": expires_iso,
            })).into_response()
        }
        Ok(None) => (StatusCode::NOT_FOUND, Json(json!({"detail": "文献不存在"}))).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"detail": e}))).into_response(),
    }
}

/// GET /assist/api/documents/doi-hash-index/ —— 一次性导出库内全部文献的 DOI / 文件哈希 / PDF 健康状态
///
/// 客户端（`client-rs` 的 Zotero 导入）拉一次即可在本地完成整库比对，
/// 不必再按 DOI 逐条打 `by-doi`（几百条文献就是几百次往返）。
/// 只返回比对必需的最小字段，不带标题/摘要这类大文本。
async fn doi_hash_index(axum::Extension(db): axum::Extension<Arc<Database>>) -> Response {
    let rows: Vec<(i64, Option<String>, Option<String>, Option<String>)> = sqlx::query_as(
        "SELECT id, doi, file_hash, pdf_health FROM documents ORDER BY id",
    )
    .fetch_all(db.pool())
    .await
    .unwrap_or_default();

    let mut documents: Vec<Value> = Vec::with_capacity(rows.len());
    let mut with_doi = 0usize;
    let mut with_hash = 0usize;
    for (id, doi, file_hash, pdf_health) in rows {
        let doi = doi.filter(|d| !d.trim().is_empty());
        let key = doi.as_deref().and_then(doi_key);
        if key.is_some() {
            with_doi += 1;
        }
        let file_hash = file_hash.filter(|h| !h.trim().is_empty());
        if file_hash.is_some() {
            with_hash += 1;
        }
        documents.push(json!({
            "id": id,
            "doi": doi,
            "doi_key": key,
            "file_hash": file_hash,
            "pdf_health": pdf_health,
        }));
    }

    Json(json!({
        "count": documents.len(),
        "with_doi": with_doi,
        "with_hash": with_hash,
        "generated_at": now_iso(),
        "documents": documents,
    }))
    .into_response()
}

#[derive(Debug, Deserialize)]
pub struct DiffDoisRequest {
    pub dois: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub struct DiffHashesRequest {
    pub hashes: Vec<String>,
}

/// POST /assist/api/documents/diff-hashes/ —— 判断哪些文件哈希已在库中
async fn diff_hashes(
    axum::Extension(db): axum::Extension<Arc<Database>>,
    Json(payload): Json<DiffHashesRequest>,
) -> Response {
    let submitted = payload.hashes.iter().filter(|h| !h.trim().is_empty() && h.len() == 64 && h.chars().all(|c| c.is_ascii_hexdigit())).map(|h| h.trim().to_string()).collect::<Vec<_>>();

    let mut present: Vec<String> = Vec::new();
    let mut missing: Vec<String> = Vec::new();
    let mut invalid: Vec<String> = Vec::new();
    for hash in &payload.hashes {
        let h = hash.trim();
        if h.is_empty() || h.len() != 64 || !h.chars().all(|c| c.is_ascii_hexdigit()) {
            invalid.push(hash.clone());
            continue;
        }
        let exists: Option<(i64,)> = sqlx::query_as("SELECT id FROM documents WHERE file_hash = ? LIMIT 1")
            .bind(h)
            .fetch_optional(db.pool())
            .await
            .unwrap_or(None);
        if exists.is_some() {
            present.push(h.to_string());
        } else {
            missing.push(h.to_string());
        }
    }
    present.sort();

    Json(json!({
        "submitted": submitted,
        "present": present,
        "missing": missing,
        "invalid": invalid,
        "present_count": present.len(),
        "missing_count": missing.len(),
        "invalid_count": invalid.len(),
    })).into_response()
}

/// POST /assist/api/documents/diff-dois/ —— 判断哪些 DOI 已在库中
async fn diff_dois(
    axum::Extension(db): axum::Extension<Arc<Database>>,
    Json(payload): Json<DiffDoisRequest>,
) -> Response {
    let submitted = payload.dois.iter().filter(|d| !d.trim().is_empty()).map(|d| d.trim().to_string()).collect::<Vec<_>>();

    // 一次性取回库中全部非空 DOI 建归一化键集合，避免逐条查询（也顺带统一了写法差异）
    let existing: HashSet<String> = sqlx::query_as::<_, (String,)>(
        "SELECT doi FROM documents WHERE doi IS NOT NULL AND doi <> ''",
    )
    .fetch_all(db.pool())
    .await
    .map(|rows| rows.into_iter().filter_map(|(d,)| doi_key(&d)).collect())
    .unwrap_or_default();

    let mut present: Vec<String> = Vec::new();
    let mut missing: Vec<String> = Vec::new();
    for doi in &submitted {
        if doi_key(doi).is_some_and(|k| existing.contains(&k)) {
            present.push(doi.clone());
        } else {
            missing.push(doi.clone());
        }
    }
    present.sort();

    Json(json!({
        "submitted": submitted,
        "present": present,
        "missing": missing,
        "present_count": present.len(),
        "missing_count": missing.len(),
    })).into_response()
}

#[derive(Debug, Deserialize)]
pub struct InfoPayload {
    #[serde(default)]
    pub info: serde_json::Map<String, serde_json::Value>,
}

/// POST /assist/api/documents/{id}/info —— 合并写入 markdowns/{id}.json
async fn save_document_info(
    axum::Extension(db): axum::Extension<Arc<Database>>,
    axum::Extension(settings): axum::Extension<Arc<Settings>>,
    Path(id): Path<i64>,
    Json(payload): Json<InfoPayload>,
) -> Response {
    match fetch_doc(&db, id).await {
        Ok(Some(_)) => {}
        Ok(None) => return (StatusCode::NOT_FOUND, Json(json!({"detail": "文献不存在"}))).into_response(),
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"detail": e}))).into_response(),
    }

    let info_path = std::path::PathBuf::from(paths::get_info_path(&settings, id));
    let info_path_read = info_path.clone();
    if let Some(parent) = info_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }

    // 读取已有 info 文件（后台线程，不阻塞 async 运行时）
    let existing_raw = tokio::task::spawn_blocking(move || {
        if info_path_read.exists() {
            std::fs::read_to_string(&info_path_read).ok()
        } else {
            None
        }
    }).await.unwrap_or(None);

    let mut existing: serde_json::Map<String, serde_json::Value> = serde_json::Map::new();
    if let Some(raw) = existing_raw {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&raw) {
            if let Some(obj) = v.as_object() {
                existing = obj.clone();
            }
        }
    }

    for (k, v) in payload.info.iter() {
        let is_empty = match v {
            serde_json::Value::String(s) => s.trim().is_empty(),
            serde_json::Value::Null => true,
            _ => false,
        };
        if !is_empty {
            existing.insert(k.clone(), v.clone());
        }
    }

    let written = serde_json::to_string_pretty(&serde_json::Value::Object(existing.clone())).unwrap_or_default();
    let info_path_write = info_path.clone();
    if let Err(e) = tokio::task::spawn_blocking(move || std::fs::write(&info_path_write, written)).await.unwrap_or_else(|_| Err(std::io::Error::other("spawn_blocking panicked"))) {
        return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"detail": e.to_string()}))).into_response();
    }

    Json(json!({"id": id, "info_path": info_path.to_string_lossy(), "keys": existing.len()})).into_response()
}

/// GET /assist/api/documents/{id}/info —— 读取 markdowns/{id}.json
async fn get_document_info(
    axum::Extension(db): axum::Extension<Arc<Database>>,
    axum::Extension(settings): axum::Extension<Arc<Settings>>,
    Path(id): Path<i64>,
) -> Response {
    match fetch_doc(&db, id).await {
        Ok(Some(_)) => {}
        Ok(None) => return (StatusCode::NOT_FOUND, Json(json!({"detail": "文献不存在"}))).into_response(),
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"detail": e}))).into_response(),
    }

    let info_path = std::path::PathBuf::from(paths::get_info_path(&settings, id));
    if !info_path.exists() {
        return (StatusCode::NOT_FOUND, Json(json!({"detail": "信息文件不存在"}))).into_response();
    }
    let info_path_clone = info_path.clone();
    match tokio::task::spawn_blocking(move || std::fs::read_to_string(&info_path_clone)).await {
        Ok(Ok(raw)) => match serde_json::from_str::<serde_json::Value>(&raw) {
            Ok(v) => Json(json!({"id": id, "info": v})).into_response(),
            Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"detail": e.to_string()}))).into_response(),
        },
        _ => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"detail": "读取信息文件失败"}))).into_response(),
    }
}

/// GET /assist/api/documents/{id}/pdf-info —— PDF Info 字典 + 页数 + 启发式抽取结果
async fn get_pdf_info(
    axum::Extension(role): axum::Extension<auth::Role>,
    axum::Extension(db): axum::Extension<Arc<Database>>,
    axum::Extension(settings): axum::Extension<Arc<Settings>>,
    Path(id): Path<i64>,
) -> Response {
    if let Err(resp) = auth::assert_role(&role, &["admin", "view-only", "view-upload"]) {
        return resp;
    }
    let doc = match fetch_doc(&db, id).await {
        Ok(Some(d)) => d,
        Ok(None) => return (StatusCode::NOT_FOUND, Json(json!({"detail": "文献不存在"}))).into_response(),
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"detail": e}))).into_response(),
    };

    let pdf_path = paths::get_pdf_path(&settings, id);
    if !std::path::Path::new(&pdf_path).exists() {
        return (StatusCode::NOT_FOUND, Json(json!({"detail": "PDF 文件不存在"}))).into_response();
    }

    // 只传路径：mutool / mupdf 直接读原文件，不把整份 PDF 读进内存
    let report = extract_pdf_report_from_path_async(pdf_path, Some(doc.filename.clone()), Some(id)).await;
    Json(json!({
        "id": id,
        "filename": doc.filename,
        "pdf": report,
    }))
    .into_response()
}

/// 读取 Crossref 原始记录（文件中的 `message` 部分）
fn read_crossref_message(settings: &Settings, id: i64) -> Option<Value> {
    let path = paths::get_crossref_path(settings, id);
    let raw = std::fs::read_to_string(&path).ok()?;
    let value: Value = serde_json::from_str(&raw).ok()?;
    // 文件内容为 Crossref API 的完整应答（含 message），也兼容仅保存 message 的情况
    if value.get("message").is_some() {
        Some(value["message"].clone())
    } else if value.get("DOI").is_some() {
        Some(value)
    } else {
        None
    }
}

/// GET /assist/api/documents/{id}/crossref —— 读取已保存的 Crossref 记录
async fn get_crossref_record(
    axum::Extension(role): axum::Extension<auth::Role>,
    axum::Extension(db): axum::Extension<Arc<Database>>,
    axum::Extension(settings): axum::Extension<Arc<Settings>>,
    Path(id): Path<i64>,
) -> Response {
    if let Err(resp) = auth::assert_role(&role, &["admin", "view-only", "view-upload"]) {
        return resp;
    }
    match fetch_doc(&db, id).await {
        Ok(Some(_)) => {}
        Ok(None) => return (StatusCode::NOT_FOUND, Json(json!({"detail": "文献不存在"}))).into_response(),
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"detail": e}))).into_response(),
    }

    let crossref_path = paths::get_crossref_path(&settings, id);
    let path_buf = std::path::PathBuf::from(&crossref_path);
    if !path_buf.exists() {
        return Json(json!({
            "id": id,
            "exists": false,
            "path": crossref_path,
            "raw": Value::Null,
            "parsed": Value::Null,
            "modified_at": Value::Null,
        }))
        .into_response();
    }

    // 文件修改时间（即抓取时间）
    let modified_at = std::fs::metadata(&path_buf)
        .ok()
        .and_then(|m| m.modified().ok())
        .map(|t| {
            let dt: DateTime<Utc> = t.into();
            dt.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
        });

    let path_clone = path_buf.clone();
    let raw_text = match tokio::task::spawn_blocking(move || std::fs::read_to_string(&path_clone)).await {
        Ok(Ok(t)) => t,
        _ => {
            return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"detail": "读取 Crossref 记录失败"})))
                .into_response()
        }
    };

    let raw: Value = match serde_json::from_str(&raw_text) {
        Ok(v) => v,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"detail": format!("Crossref 记录不是合法 JSON: {e}")})),
            )
                .into_response()
        }
    };

    // 解析后的摘要（字段与元数据补全保持一致）
    let message = raw.get("message").cloned().unwrap_or_else(|| raw.clone());
    let parsed = crate::services::crossref::parse_crossref_item(&message).unwrap_or(Value::Null);

    Json(json!({
        "id": id,
        "exists": true,
        "path": crossref_path,
        "raw": raw,
        "parsed": parsed,
        "modified_at": modified_at,
    }))
    .into_response()
}

#[derive(Debug, Deserialize)]
pub struct CitationQuery {
    /// 输出格式：bibtex（默认）/ gbt7714
    pub format: Option<String>,
    /// 数据来源：auto（默认）/ crossref / document
    pub source: Option<String>,
}

/// GET /assist/api/documents/{id}/citation —— 由 Crossref 记录（或文档元数据）生成引用条目
async fn get_citation(
    axum::Extension(role): axum::Extension<auth::Role>,
    axum::Extension(db): axum::Extension<Arc<Database>>,
    axum::Extension(settings): axum::Extension<Arc<Settings>>,
    Path(id): Path<i64>,
    Query(query): Query<CitationQuery>,
) -> Response {
    if let Err(resp) = auth::assert_role(&role, &["admin", "view-only", "view-upload"]) {
        return resp;
    }
    let doc = match fetch_doc(&db, id).await {
        Ok(Some(d)) => d,
        Ok(None) => return (StatusCode::NOT_FOUND, Json(json!({"detail": "文献不存在"}))).into_response(),
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"detail": e}))).into_response(),
    };

    let format_arg = query.format.as_deref().unwrap_or("");
    let format = match citation::normalize_format(format_arg) {
        Some(f) => f,
        None => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({"detail": "不支持的引用格式，可选 bibtex / gbt7714"})),
            )
                .into_response()
        }
    };

    let source_arg = query.source.as_deref().unwrap_or("auto").trim().to_lowercase();
    let crossref_message = if source_arg == "document" {
        None
    } else {
        read_crossref_message(&settings, id)
    };

    let record = match (source_arg.as_str(), crossref_message) {
        ("crossref", None) => {
            return (StatusCode::NOT_FOUND, Json(json!({"detail": "该文献暂无 Crossref 记录"})))
                .into_response()
        }
        (_, Some(message)) => CitationRecord::from_crossref(&message, "crossref"),
        (_, None) => CitationRecord::from_document(&doc, "document"),
    };

    let content = citation::render(&record, format);
    Json(json!({
        "id": id,
        "format": format,
        "source": record.source,
        "key": citation::citation_key(&record),
        "content": content,
    }))
    .into_response()
}

fn image_mime(filename: &str) -> &'static str {
    let ext = std::path::Path::new(filename)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        "jpg" | "jpeg" => "image/jpeg",
        "png" => "image/png",
        "gif" => "image/gif",
        "svg" => "image/svg+xml",
        _ => "application/octet-stream",
    }
}

/// GET /assist/api/documents/{id}/image/{filename} —— 从图片资源 zip 中读取图片
async fn get_image_from_zip(
    axum::Extension(role): axum::Extension<auth::Role>,
    axum::Extension(db): axum::Extension<Arc<Database>>,
    axum::Extension(settings): axum::Extension<Arc<Settings>>,
    Path((id, filename)): Path<(i64, String)>,
) -> Response {
    if let Err(resp) = auth::assert_role(&role, &["admin", "view-only", "view-upload"]) {
        return resp;
    }
    match fetch_doc(&db, id).await {
        Ok(Some(_)) => {}
        Ok(None) => return (StatusCode::NOT_FOUND, Json(json!({"detail": "文档不存在"}))).into_response(),
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"detail": e}))).into_response(),
    }

    let zip_path = paths::get_asset_path(&settings, id);
    if !std::path::Path::new(&zip_path).exists() {
        return (StatusCode::NOT_FOUND, Json(json!({"detail": "图片包不存在"}))).into_response();
    }

    // 按 doc 缓存 zip 字节（LRU）：同一篇文档的 N 张图片只读一次盘
    let raw = match image_cache::load(id, &zip_path).await {
        Some(r) => r,
        None => return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"detail": "读取图片包失败"}))).into_response(),
    };

    // zip 解析与解压是 CPU 操作，放后台线程
    let want = filename.clone();
    let img_data = match tokio::task::spawn_blocking(move || image_cache::read_image(&raw, &want)).await {
        Ok(Some(data)) => data,
        Ok(None) => return (StatusCode::NOT_FOUND, Json(json!({"detail": "图片不存在"}))).into_response(),
        Err(_) => return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"detail": "读取图片失败"}))).into_response(),
    };

    let mime = image_mime(&filename);
    (StatusCode::OK, [(header::CONTENT_TYPE, mime), (header::CACHE_CONTROL, "public, max-age=86400")], img_data).into_response()
}

/// POST /assist/api/documents/{id}/pdf/ —— 替换已有文档的 PDF
async fn replace_pdf(
    axum::Extension(db): axum::Extension<Arc<Database>>,
    axum::Extension(settings): axum::Extension<Arc<Settings>>,
    Path(id): Path<i64>,
    mut multipart: axum::extract::Multipart,
) -> Response {
    match fetch_doc(&db, id).await {
        Ok(Some(_)) => {}
        Ok(None) => return (StatusCode::NOT_FOUND, Json(json!({"detail": "文档不存在"}))).into_response(),
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"detail": e}))).into_response(),
    }

    let mut file_name: Option<String> = None;
    let mut uploaded: Option<(TempUpload, u64, String)> = None;
    let temp_dir = upload_temp_dir(&settings);
    while let Ok(Some(mut field)) = multipart.next_field().await {
        let Some(name) = field.name().map(|s| s.to_string()) else {
            continue;
        };
        if name == "file" {
            let hint = field.file_name().unwrap_or("replace.pdf").to_string();
            file_name = Some(hint.clone());
            let _ = uploaded.take();
            match stream_field_to_temp_file(&mut field, &temp_dir, &hint, MAX_DOC_UPLOAD_BYTES).await {
                Ok(v) => uploaded = Some(v),
                Err(e) => {
                    return (StatusCode::BAD_REQUEST, Json(json!({"detail": e}))).into_response()
                }
            }
        } else {
            // `reset_status` 等旧字段已删除：忽略并提示（替换 PDF 不再影响解析阶段）
            if name == "reset_status" {
                tracing::warn!(
                    "文档 {} 收到已删除的 reset_status 字段，已忽略；需要重解析请调 /assist/api/pipeline/{}/reset",
                    id, id
                );
            }
            let _ = field.bytes().await;
        }
    }

    let file_name = match file_name {
        Some(f) => f,
        None => return (StatusCode::BAD_REQUEST, Json(json!({"detail": "缺少文件"}))).into_response(),
    };

    if !file_name.to_lowercase().ends_with(".pdf") {
        return (StatusCode::BAD_REQUEST, Json(json!({"detail": "只支持 PDF 文件"}))).into_response();
    }

    let (temp_upload, upload_size, file_hash) = match uploaded {
        Some(v) => v,
        None => return (StatusCode::BAD_REQUEST, Json(json!({"detail": "缺少文件"}))).into_response(),
    };
    if upload_size == 0 {
        return (StatusCode::BAD_REQUEST, Json(json!({"detail": "文件为空"}))).into_response();
    }

    if let Err(e) =
        persist_pdf_for_doc(&db, &settings, id, temp_upload, &file_hash, None).await
    {
        return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"detail": e}))).into_response();
    }
    match fetch_doc(&db, id).await {
        Ok(Some(doc)) => (StatusCode::OK, Json(json!(doc_to_out(&doc, &settings, &db).await))).into_response(),
        _ => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"detail": "文档更新失败"}))).into_response(),
    }
}

/// HEAD /assist/api/documents/{id}/pdf —— 检查 PDF 是否存在
async fn check_pdf_exists(
    axum::Extension(db): axum::Extension<Arc<Database>>,
    axum::Extension(settings): axum::Extension<Arc<Settings>>,
    Path(id): Path<i64>,
) -> Response {
    match fetch_doc(&db, id).await {
        Ok(Some(_)) => {}
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(_) => return StatusCode::NOT_FOUND.into_response(),
    }
    let pdf_path = std::path::PathBuf::from(paths::get_pdf_path(&settings, id));
    if pdf_path.exists() {
        StatusCode::OK.into_response()
    } else {
        StatusCode::NOT_FOUND.into_response()
    }
}

/// POST /assist/api/documents/{id}/replace-markdown/ —— 替换文档的 Markdown 文件
async fn replace_markdown(
    axum::Extension(db): axum::Extension<Arc<Database>>,
    axum::Extension(settings): axum::Extension<Arc<Settings>>,
    Path(id): Path<i64>,
    mut multipart: axum::extract::Multipart,
) -> Response {
    match fetch_doc(&db, id).await {
        Ok(Some(_)) => {}
        Ok(None) => return (StatusCode::NOT_FOUND, Json(json!({"detail": "文档不存在"}))).into_response(),
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"detail": e}))).into_response(),
    }

    let mut file_name: Option<String> = None;
    let mut uploaded: Option<(TempUpload, u64, String)> = None;
    let temp_dir = upload_temp_dir(&settings);
    while let Ok(Some(mut field)) = multipart.next_field().await {
        let Some(name) = field.name().map(|s| s.to_string()) else {
            continue;
        };
        if name == "file" {
            let hint = field.file_name().unwrap_or("replace.md").to_string();
            file_name = Some(hint.clone());
            let _ = uploaded.take();
            match stream_field_to_temp_file(&mut field, &temp_dir, &hint, MAX_DOC_UPLOAD_BYTES).await {
                Ok(v) => uploaded = Some(v),
                Err(e) => {
                    return (StatusCode::BAD_REQUEST, Json(json!({"detail": e}))).into_response()
                }
            }
        } else {
            let _ = field.bytes().await;
        }
    }

    let file_name = match file_name {
        Some(f) => f,
        None => return (StatusCode::BAD_REQUEST, Json(json!({"detail": "缺少文件"}))).into_response(),
    };
    if !file_name.to_lowercase().ends_with(".md") {
        return (StatusCode::BAD_REQUEST, Json(json!({"detail": "只支持 Markdown (.md) 文件"}))).into_response();
    }

    let (temp_upload, upload_size, _hash) = match uploaded {
        Some(v) => v,
        None => return (StatusCode::BAD_REQUEST, Json(json!({"detail": "缺少文件"}))).into_response(),
    };
    if upload_size == 0 {
        return (StatusCode::BAD_REQUEST, Json(json!({"detail": "文件为空"}))).into_response();
    }

    let md_path = paths::get_markdown_path(&settings, id);
    if let Err(e) = persist_temp_file(temp_upload, &md_path).await {
        return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"detail": e.to_string()}))).into_response();
    }

    // 写时同步：更新文件大小到数据库
    sync_doc_file_sizes(&db, &settings, id).await;

    match fetch_doc(&db, id).await {
        Ok(Some(doc)) => (StatusCode::OK, Json(json!(doc_to_out(&doc, &settings, &db).await))).into_response(),
        _ => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"detail": "文档更新失败"}))).into_response(),
    }
}

/// POST /assist/api/documents/{id}/replace-asset/ —— 替换文档的图片资源包 (ZIP)
async fn replace_asset(
    axum::Extension(db): axum::Extension<Arc<Database>>,
    axum::Extension(settings): axum::Extension<Arc<Settings>>,
    Path(id): Path<i64>,
    mut multipart: axum::extract::Multipart,
) -> Response {
    match fetch_doc(&db, id).await {
        Ok(Some(_)) => {}
        Ok(None) => return (StatusCode::NOT_FOUND, Json(json!({"detail": "文档不存在"}))).into_response(),
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"detail": e}))).into_response(),
    }

    let mut file_name: Option<String> = None;
    let mut uploaded: Option<(TempUpload, u64, String)> = None;
    let temp_dir = upload_temp_dir(&settings);
    while let Ok(Some(mut field)) = multipart.next_field().await {
        let Some(name) = field.name().map(|s| s.to_string()) else {
            continue;
        };
        if name == "file" {
            let hint = field.file_name().unwrap_or("replace.zip").to_string();
            file_name = Some(hint.clone());
            let _ = uploaded.take();
            match stream_field_to_temp_file(&mut field, &temp_dir, &hint, MAX_DOC_UPLOAD_BYTES).await {
                Ok(v) => uploaded = Some(v),
                Err(e) => {
                    return (StatusCode::BAD_REQUEST, Json(json!({"detail": e}))).into_response()
                }
            }
        } else {
            let _ = field.bytes().await;
        }
    }

    let file_name = match file_name {
        Some(f) => f,
        None => return (StatusCode::BAD_REQUEST, Json(json!({"detail": "缺少文件"}))).into_response(),
    };
    if !file_name.to_lowercase().ends_with(".zip") {
        return (StatusCode::BAD_REQUEST, Json(json!({"detail": "只支持 ZIP 资源包文件"}))).into_response();
    }

    let (temp_upload, upload_size, _hash) = match uploaded {
        Some(v) => v,
        None => return (StatusCode::BAD_REQUEST, Json(json!({"detail": "缺少文件"}))).into_response(),
    };
    if upload_size == 0 {
        return (StatusCode::BAD_REQUEST, Json(json!({"detail": "文件为空"}))).into_response();
    }

    let asset_path = paths::get_asset_path(&settings, id);
    if let Err(e) = persist_temp_file(temp_upload, &asset_path).await {
        return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"detail": e.to_string()}))).into_response();
    }
    // 资源包已换：立即失效图片缓存（mtime 也可能因精度不足撞车）
    image_cache::invalidate(id);

    // 写时同步：更新文件大小到数据库
    sync_doc_file_sizes(&db, &settings, id).await;

    match fetch_doc(&db, id).await {
        Ok(Some(doc)) => (StatusCode::OK, Json(json!(doc_to_out(&doc, &settings, &db).await))).into_response(),
        _ => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"detail": "文档更新失败"}))).into_response(),
    }
}

/// POST /assist/api/documents/{id}/rehash/ —— 重算文献 PDF 的哈希值并更新数据库
async fn rehash_document(
    axum::Extension(db): axum::Extension<Arc<Database>>,
    axum::Extension(settings): axum::Extension<Arc<Settings>>,
    Path(id): Path<i64>,
) -> Response {
    let doc = match fetch_doc(&db, id).await {
        Ok(Some(d)) => d,
        Ok(None) => return (StatusCode::NOT_FOUND, Json(json!({"detail": "文档不存在"}))).into_response(),
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"detail": e}))).into_response(),
    };

    let pdf_path = std::path::PathBuf::from(paths::get_pdf_path(&settings, id));
    if !pdf_path.exists() {
        return (StatusCode::NOT_FOUND, Json(json!({"detail": "PDF 文件不存在"}))).into_response();
    }

    let pdf_path_str = pdf_path.to_string_lossy().to_string();
    let file_hash = match calculate_file_hash_async(pdf_path_str).await {
        Ok(h) => h,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"detail": format!("计算哈希失败: {}", e)}))).into_response(),
    };

    let now = now_iso();
    if let Err(e) = sqlx::query("UPDATE documents SET file_hash = ?, updated_at = ? WHERE id = ?")
        .bind(&file_hash)
        .bind(&now)
        .bind(id)
        .execute(db.pool())
        .await
    {
        return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"detail": e.to_string()}))).into_response();
    }

    (StatusCode::OK, Json(json!({
        "id": id,
        "file_hash": file_hash,
        "old_file_hash": doc.file_hash,
        "updated_at": now,
    }))).into_response()
}

#[cfg(test)]
mod doi_tests {
    use super::*;

    #[test]
    fn doi_key_strips_prefix_and_punctuation() {
        for raw in [
            "10.1000/xyz123",
            "https://doi.org/10.1000/xyz123",
            "http://dx.doi.org/10.1000/xyz123",
            "doi:10.1000/xyz123",
            "  10.1000/xyz123. ",
            "10.1000/xyz123)",
        ] {
            assert_eq!(doi_key(raw).as_deref(), Some("10.1000/xyz123"), "raw={raw}");
        }
    }

    #[test]
    fn doi_key_ignores_case() {
        assert_eq!(doi_key("10.1000/ABC"), doi_key("10.1000/abc"));
        assert_eq!(doi_key("https://doi.org/10.1000/ABC"), doi_key("10.1000/abc"));
    }

    #[test]
    fn doi_key_rejects_empty() {
        assert_eq!(doi_key(""), None);
        assert_eq!(doi_key("   "), None);
        assert_eq!(doi_key("."), None);
    }
}
