//! 文档管理路由。

use std::sync::Arc;
use std::sync::RwLock;

use axum::extract::{Path, Query};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Json, Response};
use axum::routing::{get, post};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::config::Settings;
use crate::database::Database;
use crate::models::Document;
use crate::paths;
use crate::services::pdf_meta::{extract_pdf_metadata, normalize_doi};
use crate::utils::calculate_file_hash;

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
        .route("/assist/api/documents/:id/", get(get_document).put(update_document).delete(delete_document))
        .route("/assist/api/documents/:id", get(get_document).put(update_document).delete(delete_document))
        .route("/assist/api/documents/:id/markdown/", get(get_markdown).put(update_markdown))
        .route("/assist/api/documents/:id/markdown", get(get_markdown).put(update_markdown))
        .route("/assist/api/documents/:id/pdf/", get(get_pdf))
        .route("/assist/api/documents/:id/pdf", get(get_pdf))
        .route("/assist/api/documents/:id/file-alias/", get(file_alias))
        .route("/assist/api/documents/:id/file-alias", get(file_alias))
}

/// 列表查询参数
#[derive(Debug, Deserialize)]
pub struct ListQuery {
    pub page: Option<i64>,
    pub page_size: Option<i64>,
    pub q: Option<String>,
    pub status: Option<String>,
    pub doc_type: Option<String>,
    pub category: Option<String>,
    pub year: Option<i64>,
    pub journal: Option<String>,
    pub sort: Option<String>,
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
    #[serde(skip_serializing_if = "Option::is_none")]
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

const DOC_COLUMNS: &str = "id, filename, file_hash, title, authors, year, doi, source, journal, \
    keywords, abstract, category, doc_type, language, title_en, authors_en, \
    keywords_en, abstract_en, journal_en, mineru_task_id, status, status_message, \
    progress, qdrant_collection, vector_db_id, created_at, updated_at";

fn doc_to_out(doc: &Document, settings: &Settings) -> DocumentOut {
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
        indexed_dbs: None,
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
            let like = format!("%{}%", q.trim());
            conditions.push("(title LIKE ? OR filename LIKE ? OR authors LIKE ? OR doi LIKE ?)".to_string());
            for _ in 0..4 {
                binds.push(like.clone());
            }
        }
    }
    if let Some(status) = &params.status {
        if !status.trim().is_empty() {
            conditions.push("status = ?".to_string());
            binds.push(status.clone());
        }
    }
    if let Some(dt) = &params.doc_type {
        if !dt.trim().is_empty() {
            conditions.push("doc_type = ?".to_string());
            binds.push(dt.clone());
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

    let order = match params.sort.as_deref() {
        Some("year_desc") => "ORDER BY year DESC",
        Some("year_asc") => "ORDER BY year ASC",
        Some("title") => "ORDER BY title",
        Some("created_desc") => "ORDER BY created_at DESC",
        _ => "ORDER BY id DESC",
    };

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

    let items: Vec<DocumentOut> = docs.iter().map(|d| doc_to_out(d, &settings)).collect();
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
        Ok(Some(doc)) => (StatusCode::OK, Json(json!(doc_to_out(&doc, &settings)))).into_response(),
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
    let result = sqlx::query(
        "INSERT INTO documents (filename, file_hash, title, authors, year, doi, source, journal, \
         keywords, abstract, category, doc_type, language, title_en, authors_en, keywords_en, \
         abstract_en, journal_en, status, progress) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 'uploaded', 0.0)",
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
                if let Ok(bytes) = field.bytes().await {
                    content = bytes.to_vec();
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
            let result = sqlx::query(
                "INSERT INTO documents (id, filename, file_hash, title, authors, year, doi, source, journal, \
                 keywords, abstract, category, doc_type, language, title_en, authors_en, keywords_en, \
                 abstract_en, journal_en, status, progress) \
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 'uploaded', 0.0)",
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
        Ok(Some(doc)) => (StatusCode::OK, Json(json!(doc_to_out(&doc, &settings)))).into_response(),
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
        Ok(Some(doc)) => (StatusCode::OK, Json(json!(doc_to_out(&doc, &settings)))).into_response(),
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
            let out = doc_to_out(doc, &settings);
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
    let cm = crate::config_manager::ConfigManager::new();
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

    let results: Vec<DocumentOut> = docs.iter().map(|d| doc_to_out(d, &settings)).collect();
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

/// 全文 grep 搜索
#[derive(Debug, Deserialize)]
pub struct GrepSearchQuery {
    pub q: Option<String>,
    pub limit: Option<i64>,
    pub context: Option<i64>,
    pub doc_ids: Option<String>,
    pub algorithm: Option<String>,
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

    // 构建搜索路径
    let settings = crate::settings::get_settings();
    let sample = paths::get_markdown_path(&settings, 0);
    let dir = match std::path::Path::new(&sample).parent() {
        Some(d) if d.is_dir() => d.to_path_buf(),
        _ => return Json(json!({"results": [], "query": q})).into_response(),
    };

    let search_paths: Vec<String> = if let Some(ids) = &id_list {
        ids.iter().map(|id| paths::get_markdown_path(&settings, *id)).filter(|p| std::path::Path::new(p).exists()).collect()
    } else {
        vec![dir.to_string_lossy().to_string()]
    };

    let results = crate::services::grep_search::grep_search(
        q.trim(),
        params.context.unwrap_or(2) as usize,
        params.limit.unwrap_or(10) as usize,
        id_list.as_deref(),
        params.algorithm.as_deref().unwrap_or("full"),
        params.regex.unwrap_or(true),
        &search_paths,
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
    let doc: Option<Document> = sqlx::query_as::<_, Document>(&format!(
        "SELECT {} FROM documents WHERE file_hash = ? LIMIT 1", DOC_COLUMNS
    ))
    .bind(&hash)
    .fetch_optional(db.pool())
    .await
    .unwrap_or(None);

    match doc {
        Some(d) => (StatusCode::OK, Json(json!(doc_to_out(&d, &settings)))).into_response(),
        None => (StatusCode::NOT_FOUND, Json(json!({"detail": "Document not found"}))).into_response(),
    }
}

async fn by_doi(
    axum::Extension(db): axum::Extension<Arc<Database>>,
    axum::Extension(settings): axum::Extension<Arc<Settings>>,
    Path(doi): Path<String>,
) -> Response {
    let doc: Option<Document> = sqlx::query_as::<_, Document>(&format!(
        "SELECT {} FROM documents WHERE doi = ? LIMIT 1", DOC_COLUMNS
    ))
    .bind(&doi)
    .fetch_optional(db.pool())
    .await
    .unwrap_or(None);

    match doc {
        Some(d) => (StatusCode::OK, Json(json!(doc_to_out(&d, &settings)))).into_response(),
        None => (StatusCode::NOT_FOUND, Json(json!({"detail": "Document not found"}))).into_response(),
    }
}

/// 将 Markdown 按段落分页
fn split_into_pages(content: &str, max_chars: usize) -> Vec<String> {
    if content.len() <= max_chars {
        return vec![content.to_string()];
    }

    let mut pages = Vec::new();
    let mut current_pos = 0;

    while current_pos < content.len() {
        if current_pos + max_chars >= content.len() {
            pages.push(content[current_pos..].to_string());
            break;
        }

        let search_start = current_pos + max_chars - 500;
        let search_end = (current_pos + max_chars).min(content.len());
        let chunk = &content[search_start..search_end];

        if let Some(break_pos) = chunk.rfind("\n\n") {
            pages.push(content[current_pos..search_start + break_pos].to_string());
            current_pos = search_start + break_pos + 2;
        } else if let Some(break_pos) = chunk.rfind('\n') {
            pages.push(content[current_pos..search_start + break_pos].to_string());
            current_pos = search_start + break_pos + 1;
        } else {
            pages.push(content[current_pos..current_pos + max_chars].to_string());
            current_pos += max_chars;
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
    axum::Extension(settings): axum::Extension<Arc<Settings>>,
    Path(id): Path<i64>,
) -> Response {
    match fetch_doc(&db, id).await {
        Ok(Some(doc)) => {
            let alias = get_or_create_alias(id, doc.year, doc.title.as_deref());
            Json(json!({
                "filename": alias,
                "url": format!("{}/assist/file/{}", settings.public_url(), alias),
            })).into_response()
        }
        Ok(None) => (StatusCode::NOT_FOUND, Json(json!({"detail": "文献不存在"}))).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"detail": e}))).into_response(),
    }
}
