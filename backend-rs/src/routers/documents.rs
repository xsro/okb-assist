//! 文档管理路由。

use std::sync::Arc;
use std::sync::RwLock;

use axum::extract::{Path, Query};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Json, Response};
use axum::routing::{get, head, post};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::config::Settings;
use crate::database::Database;
use crate::models::Document;
use crate::paths;
use crate::services::pdf_meta::{extract_pdf_metadata, normalize_doi};
use crate::utils::{calculate_file_hash, now_datetime, now_iso, sha256_hex};

/// 全局文件别名表（内存，重启即丢失，与 Python 版一致）
static FILE_ALIASES: std::sync::OnceLock<RwLock<std::collections::HashMap<String, i64>>> =
    std::sync::OnceLock::new();

fn aliases() -> &'static RwLock<std::collections::HashMap<String, i64>> {
    FILE_ALIASES.get_or_init(|| RwLock::new(std::collections::HashMap::new()))
}

/// 通过别名查找文档 ID
pub fn get_doc_by_alias(alias: &str) -> Option<i64> {
    aliases().read().unwrap().get(alias).copied()
}

/// 为文档生成/获取别名
fn get_or_create_alias(doc_id: i64, year: Option<i64>, title: Option<&str>) -> String {
    // 先查找已有别名
    if let Some(existing) = aliases().read().unwrap().iter().find(|(_, &v)| v == doc_id) {
        return existing.0.clone();
    }
    // 生成新别名
    let y = year.map(|v| v.to_string()).unwrap_or_else(|| "unknown".to_string());
    let t: String = title
        .map(|t| t.chars().take(30).collect::<String>())
        .unwrap_or_else(|| "untitled".to_string());
    let rand_str: String = {
        use rand::Rng;
        let mut rng = rand::thread_rng();
        (0..6).map(|_| rng.gen_range(b'a'..=b'z') as char).collect()
    };
    let alias = format!("{}_{}_{}", y, t, rand_str);
    aliases().write().unwrap().insert(alias.clone(), doc_id);
    alias
}

pub fn router() -> axum::Router<()> {
    axum::Router::new()
        .route("/assist/api/documents/", get(list_documents).post(upload_document))
        .route("/assist/api/documents", get(list_documents).post(upload_document))
        .route("/assist/api/documents/upload/", post(upload_document))
        .route("/assist/api/documents/upload", post(upload_document))
        .route("/assist/api/documents/register/", post(register_document_by_path))
        .route("/assist/api/documents/register", post(register_document_by_path))
        .route("/assist/api/documents/similar-titles/", get(find_similar_titles))
        .route("/assist/api/documents/similar-titles", get(find_similar_titles))
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
        .route("/assist/api/documents/:id/", get(get_document).put(update_document).delete(delete_document))
        .route("/assist/api/documents/:id", get(get_document).put(update_document).delete(delete_document))
        .route("/assist/api/documents/:id/info/", get(get_document_info).post(save_document_info))
        .route("/assist/api/documents/:id/info", get(get_document_info).post(save_document_info))
        .route("/assist/api/documents/:id/image/:filename/", get(get_image_from_zip))
        .route("/assist/api/documents/:id/image/:filename", get(get_image_from_zip))
        .route("/assist/api/documents/:id/markdown/", get(get_markdown).put(update_markdown))
        .route("/assist/api/documents/:id/markdown", get(get_markdown).put(update_markdown))
        .route("/assist/api/documents/:id/parse-result/", post(upload_parse_result))
        .route("/assist/api/documents/:id/parse-result", post(upload_parse_result))
        .route("/assist/api/documents/:id/pdf/", get(get_pdf).post(replace_pdf).head(check_pdf_exists))
        .route("/assist/api/documents/:id/pdf", get(get_pdf).post(replace_pdf).head(check_pdf_exists))
        .route("/assist/api/documents/:id/file-alias/", get(file_alias))
        .route("/assist/api/documents/:id/file-alias", get(file_alias))
        .layer(axum::extract::DefaultBodyLimit::max(200 * 1024 * 1024))
}

/// 列表查询参数（与 Python 版 /assist/api/documents/ 对齐）
#[derive(Debug, Deserialize)]
pub struct ListQuery {
    pub page: Option<i64>,
    pub page_size: Option<i64>,
    pub q: Option<String>,
    pub status_filter: Option<String>,
    pub doc_type_filter: Option<String>,
    pub search_fields: Option<String>,
    pub sort_by: Option<String>,
    pub sort_order: Option<String>,
    // 向后兼容旧参数（Python 版无这些，仅作扩展）
    pub category: Option<String>,
    pub year: Option<i64>,
    pub journal: Option<String>,
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
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status_message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub progress: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub indexed_dbs: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<String>,
}

const DOC_COLUMNS: &str = "id, filename, file_hash, title, authors, CAST(NULLIF(year, '') AS INTEGER) AS year, doi, source, journal, \
    keywords, abstract, category, doc_type, language, title_en, authors_en, \
    keywords_en, abstract_en, journal_en, mineru_task_id, status, status_message, \
    progress, qdrant_collection, vector_db_id, created_at, updated_at";

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

async fn doc_to_out(doc: &Document, settings: &Settings, db: &Database) -> DocumentOut {
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
        status: doc.status.clone(),
        status_message: doc.status_message.clone(),
        progress: Some(doc.progress),
        indexed_dbs: indexed_dbs(db, doc.id).await,
        created_at: doc.created_at.clone(),
        updated_at: doc.updated_at.clone(),
    }
}

async fn fetch_doc(db: &Database, id: i64) -> Result<Option<Document>, String> {
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

/// 可搜索字段名 → SQL 列名映射（与 Python 版一致）
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

/// 可排序字段名 → SQL 列名映射（与 Python 版一致）
const SORTABLE_COLUMNS: &[(&str, &str)] = &[
    ("id", "id"),
    ("title", "title"),
    ("authors", "authors"),
    ("year", "year"),
    ("doc_type", "doc_type"),
    ("status", "status"),
    ("journal", "journal"),
    ("language", "language"),
    ("doi", "doi"),
    ("category", "category"),
    ("created_at", "created_at"),
    ("updated_at", "updated_at"),
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
) -> Json<Value> {
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
    if let Some(status) = &params.status_filter {
        let statuses: Vec<&str> = status.split(',').map(|s| s.trim()).filter(|s| !s.is_empty()).collect();
        if !statuses.is_empty() {
            let placeholders = statuses.iter().map(|_| "?").collect::<Vec<_>>().join(",");
            conditions.push(format!("status IN ({})", placeholders));
            for s in statuses {
                binds.push(s.to_string());
            }
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
    if let Some(journal) = &params.journal {
        if !journal.trim().is_empty() {
            conditions.push("journal LIKE ?".to_string());
            binds.push(format!("%{}%", journal.trim()));
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
}

async fn get_document(
    axum::Extension(db): axum::Extension<Arc<Database>>,
    axum::Extension(settings): axum::Extension<Arc<Settings>>,
    Path(id): Path<i64>,
) -> Response {
    match fetch_doc(&db, id).await {
        Ok(Some(doc)) => (StatusCode::OK, Json(json!(doc_to_out(&doc, &settings, &db).await))).into_response(),
        Ok(None) => (StatusCode::NOT_FOUND, Json(json!({"detail": "Document not found"}))).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"detail": e}))).into_response(),
    }
}

#[derive(Debug, Deserialize)]
pub struct UpdateDocumentBody {
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub authors: Option<String>,
    #[serde(default)]
    pub year: Option<i64>,
    #[serde(default)]
    pub doi: Option<String>,
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub journal: Option<String>,
    #[serde(default)]
    pub keywords: Option<String>,
    #[serde(default)]
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

async fn update_document(
    axum::Extension(db): axum::Extension<Arc<Database>>,
    Path(id): Path<i64>,
    Json(body): Json<UpdateDocumentBody>,
) -> Response {
    let result = sqlx::query(
        "UPDATE documents SET title = COALESCE(?, title), authors = COALESCE(?, authors), \
         year = COALESCE(?, year), doi = COALESCE(?, doi), source = COALESCE(?, source), \
         journal = COALESCE(?, journal), keywords = COALESCE(?, keywords), \
         abstract = COALESCE(?, abstract), category = COALESCE(?, category), \
         doc_type = COALESCE(?, doc_type), language = COALESCE(?, language), \
         title_en = COALESCE(?, title_en), authors_en = COALESCE(?, authors_en), \
         keywords_en = COALESCE(?, keywords_en), abstract_en = COALESCE(?, abstract_en), \
         journal_en = COALESCE(?, journal_en), updated_at = datetime('now') \
         WHERE id = ?",
    )
    .bind(&body.title)
    .bind(&body.authors)
    .bind(body.year)
    .bind(&body.doi)
    .bind(&body.source)
    .bind(&body.journal)
    .bind(&body.keywords)
    .bind(&body.abstract_text)
    .bind(&body.category)
    .bind(&body.doc_type)
    .bind(&body.language)
    .bind(&body.title_en)
    .bind(&body.authors_en)
    .bind(&body.keywords_en)
    .bind(&body.abstract_en)
    .bind(&body.journal_en)
    .bind(id)
    .execute(db.pool())
    .await;

    match result {
        Ok(r) if r.rows_affected() > 0 => (StatusCode::OK, Json(json!({"status": "ok"}))).into_response(),
        Ok(_) => (StatusCode::NOT_FOUND, Json(json!({"detail": "Document not found"}))).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"detail": e.to_string()}))).into_response(),
    }
}

async fn delete_document(
    axum::Extension(db): axum::Extension<Arc<Database>>,
    axum::Extension(settings): axum::Extension<Arc<Settings>>,
    Path(id): Path<i64>,
) -> Response {
    match fetch_doc(&db, id).await {
        Ok(Some(doc)) => {
            // 删除本地文件
            let _ = std::fs::remove_file(paths::get_pdf_path(&settings, id));
            let _ = std::fs::remove_file(paths::get_markdown_path(&settings, id));
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
                doc.authors = Some(serde_json::to_string(&authors).unwrap_or_default());
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
                doc.keywords = Some(serde_json::to_string(&kws).unwrap_or_default());
            }
        }
    }
    if doc.abstract_text.is_none() {
        if let Some(a) = meta.get("abstract").and_then(|v| v.as_str()) {
            doc.abstract_text = Some(a.to_string());
        }
    }
}

/// 插入新文档记录
async fn insert_document(db: &Database, doc: Document) -> Result<i64, String> {
    let now = now_datetime();
    let result = sqlx::query(
        "INSERT INTO documents (filename, file_hash, title, authors, year, doi, source, journal, \
         keywords, abstract, category, doc_type, language, title_en, authors_en, keywords_en, \
         abstract_en, journal_en, status, progress, created_at, updated_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 'uploaded', 0.0, ?, ?)",
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
    axum::Extension(db): axum::Extension<Arc<Database>>,
    axum::Extension(settings): axum::Extension<Arc<Settings>>,
    mut multipart: axum::extract::Multipart,
) -> Response {
    let mut filename: Option<String> = None;
    let mut content: Vec<u8> = Vec::new();

    while let Ok(Some(field)) = multipart.next_field().await {
        if let Some(name) = field.name() {
            if name == "file" {
                filename = field.file_name().map(String::from);
                match field.bytes().await {
                    Ok(bytes) => content = bytes.to_vec(),
                    Err(e) => {
                        return (StatusCode::BAD_REQUEST, Json(json!({"detail": format!("读取文件内容失败: {}", e)}))).into_response();
                    }
                }
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

    // 提取 PDF 元数据
    let meta = extract_pdf_metadata(&content, Some(&filename));
    let file_hash = {
        use sha2::{Digest, Sha256};
        format!("{:x}", Sha256::digest(&content))
    };

    let mut doc = Document {
        id: 0,
        filename: filename.clone(),
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
        status: "uploaded".to_string(),
        status_message: None,
        progress: 0.0,
        qdrant_collection: None,
        vector_db_id: None,
        created_at: None,
        updated_at: None,
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
                 abstract_en, journal_en, status, progress, created_at, updated_at) \
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 'uploaded', 0.0, ?, ?)",
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

    // 保存 PDF 文件
    let pdf_path = paths::get_pdf_path(&settings, doc_id);
    if let Some(parent) = std::path::Path::new(&pdf_path).parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Err(e) = std::fs::write(&pdf_path, &content) {
        return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"detail": format!("保存文件失败: {}", e)}))).into_response();
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
    axum::Extension(db): axum::Extension<Arc<Database>>,
    axum::Extension(settings): axum::Extension<Arc<Settings>>,
    Json(data): Json<RegisterByPath>,
) -> Response {
    if !std::path::Path::new(&data.file_path).exists() {
        return (StatusCode::BAD_REQUEST, Json(json!({"detail": format!("文件不存在: {}", data.file_path)}))).into_response();
    }
    if !data.file_path.to_lowercase().ends_with(".pdf") {
        return (StatusCode::BAD_REQUEST, Json(json!({"detail": "只支持 PDF 文件"}))).into_response();
    }

    let content = match std::fs::read(&data.file_path) {
        Ok(c) => c,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"detail": e.to_string()}))).into_response(),
    };
    let meta = extract_pdf_metadata(&content, None);
    let file_hash = match calculate_file_hash(&data.file_path) {
        Ok(h) => h,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"detail": e.to_string()}))).into_response(),
    };

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
        status: "uploaded".to_string(),
        status_message: None,
        progress: 0.0,
        qdrant_collection: None,
        vector_db_id: None,
        created_at: None,
        updated_at: None,
    };
    fill_meta_fields(&mut doc, &meta);

    let doc_id = match insert_document(&db, doc).await {
        Ok(id) => id,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"detail": e}))).into_response(),
    };

    // 复制文件
    let pdf_path = paths::get_pdf_path(&settings, doc_id);
    if let Some(parent) = std::path::Path::new(&pdf_path).parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Err(e) = std::fs::copy(&data.file_path, &pdf_path) {
        return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"detail": format!("复制文件失败: {}", e)}))).into_response();
    }

    match fetch_doc(&db, doc_id).await {
        Ok(Some(doc)) => (StatusCode::OK, Json(json!(doc_to_out(&doc, &settings, &db).await))).into_response(),
        _ => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"detail": "文档创建失败"}))).into_response(),
    }
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

    // 简单分组：按标题前 20 个字符（忽略大小写）分组
    let mut groups: std::collections::HashMap<String, Vec<DocumentOut>> = std::collections::HashMap::new();
    for doc in &docs {
        if let Some(title) = &doc.title {
            let key: String = title.chars().take(20).collect::<String>().to_lowercase();
            let out = doc_to_out(doc, &settings, &db).await;
            groups.entry(key).or_default().push(out);
        }
    }

    let groups: Vec<Value> = groups
        .into_iter()
        .filter(|(_, v)| v.len() > 1)
        .map(|(_, docs)| json!({"documents": docs}))
        .collect();

    let total_documents: usize = groups.iter().map(|g| g["documents"].as_array().map(|a| a.len()).unwrap_or(0)).sum();

    Json(json!({
        "groups": groups,
        "total_groups": groups.len(),
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
    pub limit: Option<i64>,
}

async fn search_info(
    axum::Extension(db): axum::Extension<Arc<Database>>,
    axum::Extension(settings): axum::Extension<Arc<Settings>>,
    Query(params): Query<SearchInfoQuery>,
) -> Json<Value> {
    let q = params.q.unwrap_or_default();
    if q.trim().is_empty() {
        return Json(json!({"results": [], "query": q, "total": 0}));
    }

    let like = format!("%{}%", q.trim());
    let sql = format!(
        "SELECT {} FROM documents WHERE title LIKE ? OR title_en LIKE ? OR authors LIKE ? \
         OR authors_en LIKE ? OR journal LIKE ? OR journal_en LIKE ? OR keywords LIKE ? \
         OR keywords_en LIKE ? OR abstract LIKE ? OR abstract_en LIKE ? OR doi LIKE ? \
         OR category LIKE ? OR source LIKE ? ORDER BY updated_at DESC LIMIT ?",
        DOC_COLUMNS
    );

    let limit = params.limit.unwrap_or(10).clamp(1, 100);
    let docs: Vec<Document> = sqlx::query_as::<_, Document>(&sql)
        .bind(&like).bind(&like).bind(&like).bind(&like)
        .bind(&like).bind(&like).bind(&like).bind(&like)
        .bind(&like).bind(&like).bind(&like).bind(&like)
        .bind(&like)
        .bind(limit)
        .fetch_all(db.pool())
        .await
        .unwrap_or_default();

    let mut results: Vec<DocumentOut> = Vec::with_capacity(docs.len());
    for d in &docs {
        results.push(doc_to_out(d, &settings, &db).await);
    }
    let total = results.len();
    Json(json!({"results": results, "query": q, "total": total}))
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

    // 获取 embedding
    let ollama = crate::services::ollama::OllamaClient::new(
        &settings.ollama_url(),
        Some(&settings.ollama_key()),
        &settings.embedding_model(),
    );
    let embedding = match ollama.get_embedding(q.trim()).await {
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
                doc_info = json!({
                    "filename": doc.filename,
                    "status": doc.status,
                    "authors": doc.authors,
                    "year": doc.year,
                    "journal": doc.journal,
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

/// 解析 query 中的布尔值（兼容 Python/Pydantic 的宽松 bool：1/0、true/false、yes/no、on/off 等）。
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
    )
    .await;

    // 补充文档元数据
    let mut enriched = Vec::new();
    for hit in &results {
        let doc_id = hit["document_id"].as_i64();
        let mut doc_info = json!({});
        if let Some(did) = doc_id {
            if let Ok(Some(doc)) = fetch_doc(&db, did).await {
                doc_info = json!({
                    "title": doc.title,
                    "filename": doc.filename,
                    "status": doc.status,
                    "authors": doc.authors,
                    "year": doc.year,
                    "journal": doc.journal,
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

async fn by_doi(
    axum::Extension(db): axum::Extension<Arc<Database>>,
    axum::Extension(settings): axum::Extension<Arc<Settings>>,
    Path(doi): Path<String>,
) -> Response {
    if doi.trim().is_empty() {
        return (StatusCode::BAD_REQUEST, Json(json!({"detail": "DOI 不能为空"}))).into_response();
    }
    let doc: Option<Document> = sqlx::query_as::<_, Document>(&format!(
        "SELECT {} FROM documents WHERE doi = ? LIMIT 1", DOC_COLUMNS
    ))
    .bind(&doi)
    .fetch_optional(db.pool())
    .await
    .unwrap_or(None);

    match doc {
        Some(d) => (StatusCode::OK, Json(json!(doc_to_out(&d, &settings, &db).await))).into_response(),
        None => (StatusCode::NOT_FOUND, Json(json!({"detail": "未找到匹配的文档"}))).into_response(),
    }
}

/// 将 Markdown 按段落分页
fn split_into_pages(content: &str, max_chars: usize) -> Vec<String> {
    // 使用字符数而非字节数判断，避免 UTF-8 多字节字符切片 panic
    let char_count: usize = content.chars().count();
    if char_count <= max_chars {
        return vec![content.to_string()];
    }

    let mut pages = Vec::new();
    let mut current_char_idx = 0;

    // 构建字符索引到字节偏移的映射，用于安全切片
    let char_offsets: Vec<usize> = content.char_indices().map(|(i, _)| i).collect();
    let content_len = content.len();

    while current_char_idx < char_count {
        if current_char_idx + max_chars >= char_count {
            let byte_start = char_offsets[current_char_idx];
            pages.push(content[byte_start..].to_string());
            break;
        }

        let search_start_char = if max_chars >= 500 {
            current_char_idx + max_chars - 500
        } else {
            current_char_idx
        };
        let search_end_char = (current_char_idx + max_chars).min(char_count);

        let byte_start = char_offsets[current_char_idx];
        let byte_search_start = char_offsets[search_start_char];
        let byte_search_end = char_offsets.get(search_end_char).copied().unwrap_or(content_len);

        let chunk = &content[byte_search_start..byte_search_end];

        if let Some(break_byte_offset) = chunk.rfind("\n\n") {
            let split_byte = byte_search_start + break_byte_offset;
            pages.push(content[byte_start..split_byte].to_string());
            // 找到 split_byte 之后第一个字符的索引
            let remainder = &content[split_byte..];
            let skip = remainder.chars().next().map(|c| c.len_utf8()).unwrap_or(0);
            let remainder_start = split_byte + skip;
            // 计算跳过的字符数
            let skipped = content[byte_start..remainder_start].chars().count();
            current_char_idx += skipped;
        } else if let Some(break_byte_offset) = chunk.rfind('\n') {
            let split_byte = byte_search_start + break_byte_offset;
            pages.push(content[byte_start..split_byte].to_string());
            let remainder = &content[split_byte..];
            let skip = remainder.chars().next().map(|c| c.len_utf8()).unwrap_or(0);
            let remainder_start = split_byte + skip;
            let skipped = content[byte_start..remainder_start].chars().count();
            current_char_idx += skipped;
        } else {
            let byte_end = char_offsets.get(current_char_idx + max_chars).copied().unwrap_or(content_len);
            pages.push(content[byte_start..byte_end].to_string());
            current_char_idx += max_chars;
        }
    }

    pages
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
    pub page: Option<i64>,
    pub page_size: Option<i64>,
    #[serde(default, deserialize_with = "de_flexible_bool")]
    pub full: Option<bool>,
}

async fn get_markdown(
    axum::Extension(db): axum::Extension<Arc<Database>>,
    axum::Extension(settings): axum::Extension<Arc<Settings>>,
    Path(id): Path<i64>,
    Query(params): Query<MarkdownQuery>,
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

    let content = rewrite_image_paths(&content, id);

    if params.full.unwrap_or(false) {
        let len = content.len();
        return Json(json!({
            "content": content,
            "page": 1,
            "total_pages": 1,
            "total_length": len,
        })).into_response();
    }

    let page_size = params.page_size.unwrap_or(100000).max(1) as usize;
    let pages = split_into_pages(&content, page_size);
    let total_pages = pages.len();
    let page = params.page.unwrap_or(1).clamp(1, total_pages as i64) as usize;
    let len = content.len();

    Json(json!({
        "content": pages.get(page - 1).cloned().unwrap_or_default(),
        "page": page,
        "total_pages": total_pages,
        "total_length": len,
    })).into_response()
}

#[derive(Debug, Deserialize)]
pub struct MarkdownUpdate {
    pub content: String,
}

async fn update_markdown(
    axum::Extension(db): axum::Extension<Arc<Database>>,
    axum::Extension(settings): axum::Extension<Arc<Settings>>,
    Path(id): Path<i64>,
    Json(data): Json<MarkdownUpdate>,
) -> Response {
    match fetch_doc(&db, id).await {
        Ok(Some(_)) => {}
        Ok(None) => return (StatusCode::NOT_FOUND, Json(json!({"detail": "文献不存在"}))).into_response(),
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"detail": e}))).into_response(),
    }

    let md_path = paths::get_markdown_path(&settings, id);
    if !std::path::Path::new(&md_path).exists() {
        return (StatusCode::BAD_REQUEST, Json(json!({"detail": "Markdown 文件尚未生成"}))).into_response();
    }

    let content = restore_image_paths(&data.content, id);
    match std::fs::write(&md_path, content) {
        Ok(_) => Json(json!({"detail": "Markdown 已更新"})).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"detail": e.to_string()}))).into_response(),
    }
}

/// 上传 MinerU 解析结果（Markdown 内容）到文档，并更新状态为 markdown_done。
///
/// 与 pipeline 的 parse 阶段完成等价，但由外部脚本直接上传解析结果。
#[derive(Debug, Deserialize)]
pub struct ParseResultBody {
    pub content: String,
}

async fn upload_parse_result(
    axum::Extension(db): axum::Extension<Arc<Database>>,
    axum::Extension(settings): axum::Extension<Arc<Settings>>,
    Path(id): Path<i64>,
    Json(data): Json<ParseResultBody>,
) -> Response {
    // 校验文档存在
    let doc = match fetch_doc(&db, id).await {
        Ok(Some(d)) => d,
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

    // 仅当处于解析前状态时升级为 markdown_done，不降级后续状态
    let new_status = match doc.status.as_str() {
        "uploaded" | "parsing" | "error" => "markdown_done",
        other => other,
    };
    let _ = sqlx::query(
        "UPDATE documents SET status = ?, status_message = ?, progress = ?, updated_at = ? WHERE id = ?",
    )
    .bind(new_status)
    .bind("PDF 解析完成")
    .bind(100.0)
    .bind(now_datetime())
    .bind(id)
    .execute(db.pool())
    .await;

    Json(json!({"id": id, "status": new_status, "markdown_path": md_path})).into_response()
}

async fn get_pdf(
    axum::Extension(db): axum::Extension<Arc<Database>>,
    axum::Extension(settings): axum::Extension<Arc<Settings>>,
    Path(id): Path<i64>,
) -> Response {
    match fetch_doc(&db, id).await {
        Ok(Some(_)) => {}
        Ok(None) => return (StatusCode::NOT_FOUND, Json(json!({"detail": "文献不存在"}))).into_response(),
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"detail": e}))).into_response(),
    }

    let pdf_path = paths::get_pdf_path(&settings, id);
    match std::fs::read(&pdf_path) {
        Ok(bytes) => (
            StatusCode::OK,
            [
                (header::CONTENT_TYPE, "application/pdf"),
                (header::CACHE_CONTROL, "public, max-age=86400"),
            ],
            bytes,
        ).into_response(),
        Err(_) => (StatusCode::NOT_FOUND, Json(json!({"detail": "PDF 文件不存在"}))).into_response(),
    }
}

async fn file_alias(
    axum::Extension(db): axum::Extension<Arc<Database>>,
    Path(id): Path<i64>,
) -> Response {
    match fetch_doc(&db, id).await {
        Ok(Some(doc)) => {
            let alias = get_or_create_alias(id, doc.year, doc.title.as_deref());
            Json(json!({
                "url": format!("/assist/file/{}", alias),
            })).into_response()
        }
        Ok(None) => (StatusCode::NOT_FOUND, Json(json!({"detail": "文献不存在"}))).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"detail": e}))).into_response(),
    }
}

#[derive(Debug, Deserialize)]
pub struct DiffDoisRequest {
    pub dois: Vec<String>,
}

/// POST /assist/api/documents/diff-dois/ —— 判断哪些 DOI 已在库中
async fn diff_dois(
    axum::Extension(db): axum::Extension<Arc<Database>>,
    Json(payload): Json<DiffDoisRequest>,
) -> Response {
    let submitted = payload.dois.iter().filter(|d| !d.trim().is_empty()).map(|d| d.trim().to_string()).collect::<Vec<_>>();

    let mut present: Vec<String> = Vec::new();
    let mut missing: Vec<String> = Vec::new();
    for doi in &submitted {
        let exists: Option<(i64,)> = sqlx::query_as("SELECT id FROM documents WHERE doi = ? LIMIT 1")
            .bind(doi)
            .fetch_optional(db.pool())
            .await
            .unwrap_or(None);
        if exists.is_some() {
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
    if let Some(parent) = info_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }

    let mut existing: serde_json::Map<String, serde_json::Value> = serde_json::Map::new();
    if info_path.exists() {
        if let Ok(raw) = std::fs::read_to_string(&info_path) {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&raw) {
                if let Some(obj) = v.as_object() {
                    existing = obj.clone();
                }
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
    if let Err(e) = std::fs::write(&info_path, written) {
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
    match std::fs::read_to_string(&info_path) {
        Ok(raw) => match serde_json::from_str::<serde_json::Value>(&raw) {
            Ok(v) => Json(json!({"id": id, "info": v})).into_response(),
            Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"detail": e.to_string()}))).into_response(),
        },
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"detail": e.to_string()}))).into_response(),
    }
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
    axum::Extension(db): axum::Extension<Arc<Database>>,
    axum::Extension(settings): axum::Extension<Arc<Settings>>,
    Path((id, filename)): Path<(i64, String)>,
) -> Response {
    match fetch_doc(&db, id).await {
        Ok(Some(_)) => {}
        Ok(None) => return (StatusCode::NOT_FOUND, Json(json!({"detail": "文档不存在"}))).into_response(),
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"detail": e}))).into_response(),
    }

    let zip_path = std::path::PathBuf::from(paths::get_asset_path(&settings, id));
    if !zip_path.exists() {
        return (StatusCode::NOT_FOUND, Json(json!({"detail": "图片包不存在"}))).into_response();
    }

    let raw = match std::fs::read(&zip_path) {
        Ok(r) => r,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"detail": e.to_string()}))).into_response(),
    };

    let cursor = std::io::Cursor::new(raw);
    let mut archive = match zip::ZipArchive::new(cursor) {
        Ok(a) => a,
        Err(_) => return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"detail": "图片包损坏"}))).into_response(),
    };

    let target = {
        let mut found = None;
        for i in 0..archive.len() {
            if let Ok(entry) = archive.by_index(i) {
                if let Some(name) = entry.enclosed_name() {
                    let base = std::path::Path::new(&name)
                        .file_name()
                        .map(|n| n.to_string_lossy().to_string())
                        .unwrap_or_default();
                    if base == filename {
                        found = Some(name.to_path_buf());
                        break;
                    }
                }
            }
        }
        found
    };

    let target = match target {
        Some(t) => t,
        None => return (StatusCode::NOT_FOUND, Json(json!({"detail": "图片不存在"}))).into_response(),
    };

    let mut img_data = Vec::new();
    if let Ok(mut entry) = archive.by_name(&target.to_string_lossy()) {
        use std::io::Read;
        if entry.read_to_end(&mut img_data).is_err() {
            return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"detail": "读取图片失败"}))).into_response();
        }
    }

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
    let mut content: Vec<u8> = Vec::new();
    while let Ok(Some(field)) = multipart.next_field().await {
        if field.name() == Some("file") {
            file_name = field.file_name().map(|s| s.to_string());
            match field.bytes().await {
                Ok(bytes) => content = bytes.to_vec(),
                Err(e) => {
                    return (StatusCode::BAD_REQUEST, Json(json!({"detail": format!("读取文件内容失败: {}", e)}))).into_response();
                }
            }
        }
    }

    let file_name = match file_name {
        Some(f) => f,
        None => return (StatusCode::BAD_REQUEST, Json(json!({"detail": "缺少文件"}))).into_response(),
    };

    if !file_name.to_lowercase().ends_with(".pdf") {
        return (StatusCode::BAD_REQUEST, Json(json!({"detail": "只支持 PDF 文件"}))).into_response();
    }

    let file_hash = sha256_hex(&content);
    let pdf_path = std::path::PathBuf::from(paths::get_pdf_path(&settings, id));
    if let Some(parent) = pdf_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Err(e) = std::fs::write(&pdf_path, &content) {
        return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"detail": e.to_string()}))).into_response();
    }

    let now = now_iso();
    if let Err(e) = sqlx::query("UPDATE documents SET file_hash = ?, status = 'uploaded', updated_at = ? WHERE id = ?")
        .bind(&file_hash)
        .bind(&now)
        .bind(id)
        .execute(db.pool())
        .await
    {
        return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"detail": e.to_string()}))).into_response();
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
