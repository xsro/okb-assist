//! OKB-Assist 后端 — Rust 重实现。
//!
//! 基于 Axum + SQLx + Tokio，保持与 Python 版本 API 接口兼容。

use std::sync::Arc;

use axum::Extension;
use axum::response::IntoResponse;
use axum::routing::get;
use tower::ServiceBuilder;
use tower_http::{cors::CorsLayer, trace::TraceLayer};

mod config_manager;
mod config;
mod database;
mod models;
mod paths;
mod settings;
mod utils;
mod services;
mod routers;
mod mcp_server;

use config::Settings;
use config_manager::ConfigManager;
use database::Database;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("okb_assist=info")),
        )
        .init();

    let config_manager = Arc::new(ConfigManager::new());
    let settings = Arc::new(Settings::new(config_manager.clone()));
    settings::init_settings(settings.clone());

    // 初始化数据库
    let db = Database::new(&settings.database_url()).await?;
    db.init().await?;
    let db_arc = Arc::new(db);

    // 构建应用
    let app = create_app(db_arc.clone(), config_manager.clone(), settings.clone());

    let listener = tokio::net::TcpListener::bind("0.0.0.0:5001").await?;
    tracing::info!("OKB-Assist (Rust) listening on http://0.0.0.0:5001");
    axum::serve::serve(listener, app).await?;

    Ok(())
}

fn create_app(
    db: Arc<Database>,
    config_manager: Arc<ConfigManager>,
    settings: Arc<Settings>,
) -> axum::Router<()> {
    let cors = CorsLayer::new()
        .allow_origin([
            "http://localhost:5173".parse().unwrap(),
            "http://localhost:5001".parse().unwrap(),
        ])
        .allow_credentials(true)
        .allow_methods([
            http::Method::GET,
            http::Method::POST,
            http::Method::PUT,
            http::Method::DELETE,
            http::Method::PATCH,
            http::Method::OPTIONS,
        ])
        .allow_headers([
            http::header::CONTENT_TYPE,
            http::header::AUTHORIZATION,
            http::HeaderName::from_static("x-token"),
        ]);

    let middleware = ServiceBuilder::new()
        .layer(TraceLayer::new_for_http())
        .layer(cors)
        .layer(Extension(config_manager))
        .layer(Extension(settings.clone()))
        .layer(Extension(db.clone()));

    let app = axum::Router::new()
        .merge(routers::documents::router())
        .merge(routers::pipeline::router())
        .merge(routers::admin::router())
        .merge(routers::config::router())
        .merge(routers::openapi::router())
        .route("/assist/file/:filename", get(serve_file_alias))
        .route("/", get(root_redirect))
        .route("/redirect/:doc_id", get(redirect_by_network))
        .nest_service("/assist/uploads", tower_http::services::ServeDir::new(settings.uploads_folder()))
        .layer(middleware);

    // SPA fallback
    app.fallback(spa_fallback)
}

/// 根路由重定向到 /assist
async fn root_redirect() -> impl IntoResponse {
    (
        http::StatusCode::FOUND,
        [(http::header::LOCATION, "/assist")],
    )
}

/// 根据请求 Host 自动跳转到对应地址的详情页
async fn redirect_by_network(
    axum::extract::Path(doc_id): axum::extract::Path<i64>,
    axum::Extension(settings): axum::Extension<Arc<Settings>>,
    headers: http::HeaderMap,
) -> impl IntoResponse {
    let host = headers.get("host").and_then(|v| v.to_str().ok()).unwrap_or("");
    // 从 public_url 提取 host（如 http://localhost:5001 → localhost:5001）
    let public_host = settings
        .public_url()
        .trim_start_matches("http://")
        .trim_start_matches("https://")
        .split('/')
        .next()
        .unwrap_or("")
        .to_string();
    let base = if host == public_host {
        settings.public_url()
    } else {
        settings.subnet_url()
    };
    let target = format!("{}/assist/detail/{}", base, doc_id);
    (
        http::StatusCode::FOUND,
        [(http::header::LOCATION, target)],
    )
}

/// 通过文件别名提供 PDF（无需 token，URL 不可猜测）
async fn serve_file_alias(
    axum::extract::Path(filename): axum::extract::Path<String>,
    axum::Extension(db): axum::Extension<Arc<Database>>,
    axum::Extension(settings): axum::Extension<Arc<Settings>>,
) -> impl IntoResponse {
    use crate::routers::documents::get_doc_by_alias;

    let doc_id = match get_doc_by_alias(&filename) {
        Some(id) => id,
        None => return axum::Json(serde_json::json!({"detail": "文件不存在或链接已过期"})).into_response(),
    };

    let doc: Option<crate::models::Document> = sqlx::query_as(
        "SELECT id, filename, file_hash, title, authors, year, doi, source, journal, \
         keywords, abstract, category, doc_type, language, title_en, authors_en, \
         keywords_en, abstract_en, journal_en, mineru_task_id, status, status_message, \
         progress, qdrant_collection, vector_db_id, created_at, updated_at \
         FROM documents WHERE id = ?",
    )
    .bind(doc_id)
    .fetch_optional(db.pool())
    .await
    .unwrap_or(None);

    let doc = match doc {
        Some(d) => d,
        None => return axum::Json(serde_json::json!({"detail": "文件不存在"})).into_response(),
    };

    let pdf_path = crate::paths::get_pdf_path(&settings, doc.id);
    match std::fs::read(&pdf_path) {
        Ok(bytes) => (
            http::StatusCode::OK,
            [
                (http::header::CONTENT_TYPE, "application/pdf"),
                (http::header::CACHE_CONTROL, "public, max-age=86400"),
            ],
            bytes,
        ).into_response(),
        Err(_) => axum::Json(serde_json::json!({"detail": "文件不存在"})).into_response(),
    }
}

async fn spa_fallback(
    Extension(_settings): Extension<Arc<Settings>>,
) -> impl IntoResponse {
    let dist_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("frontend")
        .join("dist");
    let index = dist_dir.join("index.html");

    if index.exists() {
        (
            [(http::header::CONTENT_TYPE, "text/html; charset=utf-8")],
            std::fs::read(&index).unwrap_or_default(),
        )
    } else {
        (
            [(http::header::CONTENT_TYPE, "text/plain; charset=utf-8")],
            b"Frontend not built. Run: cd frontend && pnpm run build".to_vec(),
        )
    }
}