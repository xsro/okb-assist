//! OKB-Assist 后端 — Rust 重实现。
//!
//! 基于 Axum + SQLx + Tokio，保持与 Python 版本 API 接口兼容。

use std::sync::Arc;

use clap::Parser;
use axum::Extension;
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
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

/// 日志级别
#[derive(clap::ValueEnum, Clone, Copy, Debug)]
enum LogLevel {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
}

impl LogLevel {
    fn as_str(self) -> &'static str {
        match self {
            LogLevel::Trace => "trace",
            LogLevel::Debug => "debug",
            LogLevel::Info => "info",
            LogLevel::Warn => "warn",
            LogLevel::Error => "error",
        }
    }
}

/// 命令行参数
#[derive(Parser, Debug)]
#[command(name = "okb_assist", version, about = "OKB-Assist 后端（Rust 版）")]
struct Args {
    /// 监听地址
    #[arg(long, default_value = "0.0.0.0")]
    host: String,

    /// 监听端口
    #[arg(long, default_value = "5001", value_parser = clap::value_parser!(u16))]
    port: u16,

    /// 日志级别
    #[arg(long, value_enum, default_value = "info")]
    log_level: LogLevel,

    /// system.json 文件路径（config.json 路径由此文件中的 config_path 字段确定）
    #[arg(long, default_value = "system.json")]
    system_path: String,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();

    // 初始化日志：RUST_LOG 环境变量优先，否则使用 --log-level 指定的级别。
    let level = args.log_level.as_str();
    let filter = tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
        tracing_subscriber::EnvFilter::new(format!("okb_assist={level},tower_http={level}"))
    });

    // 先加载 system.json 确定 log_path
    let config_manager = Arc::new(ConfigManager::new(&args.system_path));
    let system = config_manager.load_system_config();
    let log_path_raw = system
        .get("log_path")
        .and_then(|v| v.as_str())
        .unwrap_or("stdout")
        .to_string();
    // 应用变量替换
    let log_path = ConfigManager::substitute_path_variables(
        &log_path_raw,
        0,
        &config_manager.system_dir(),
        &config_manager.system_path(),
    );

    if log_path != "stdout" && !log_path.is_empty() {
        let log_file = std::fs::File::create(&log_path)
            .map_err(|e| anyhow::anyhow!("无法创建日志文件 {}: {}", log_path, e))?;
        tracing_subscriber::fmt()
            .with_env_filter(filter)
            .with_writer(std::sync::Mutex::new(log_file))
            .init();
    } else {
        tracing_subscriber::fmt().with_env_filter(filter).init();
    }
    let settings = Arc::new(Settings::new(config_manager.clone()));
    settings::init_settings(settings.clone());

    // 初始化数据库
    let db = Database::new(&settings.database_url()).await?;
    db.init().await?;
    tracing::info!("数据库初始化完成: {}", settings.database_url());
    let db_arc = Arc::new(db);

    // 构建应用
    let app = create_app(db_arc.clone(), config_manager.clone(), settings.clone());

    let addr = format!("{}:{}", args.host, args.port);
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    tracing::info!("OKB-Assist (Rust) listening on http://{}", addr);
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
        .layer(
            TraceLayer::new_for_http()
                .make_span_with(
                    tower_http::trace::DefaultMakeSpan::new().level(tracing::Level::INFO),
                )
                .on_response(
                    tower_http::trace::DefaultOnResponse::new().level(tracing::Level::INFO),
                ),
        )
        .layer(cors)
        .layer(Extension(config_manager))
        .layer(Extension(settings.clone()))
        .layer(Extension(db.clone()))
        .layer(middleware::from_fn(error_log_middleware))
        .layer(middleware::from_fn(token_middleware));

    let app = axum::Router::new()
        .merge(routers::documents::router())
        .merge(routers::pipeline::router())
        .merge(routers::admin::router())
        .merge(routers::config::router())
        .merge(routers::openapi::router())
        .route("/assist/mcp/stream", post(mcp_server::mcp_stream_handler))
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
        "SELECT id, filename, file_hash, title, authors, CAST(NULLIF(year, '') AS INTEGER) AS year, doi, source, journal, \
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

/// 错误日志中间件：当响应状态码为 4xx/5xx 时，在控制台打印错误详情。
async fn error_log_middleware(req: axum::extract::Request, next: Next) -> Response {
    let method = req.method().clone();
    let uri = req.uri().clone();

    let response = next.run(req).await;

    let status = response.status();
    if !(status.is_client_error() || status.is_server_error()) {
        return response;
    }

    // 读取响应体以提取错误详情（错误响应通常为 JSON `{"detail": ...}`）
    let (parts, body) = response.into_parts();
    let body_bytes = axum::body::to_bytes(body, 1024 * 1024).await.unwrap_or_default();
    let body_text = String::from_utf8_lossy(&body_bytes).to_string();
    let detail = serde_json::from_str::<serde_json::Value>(&body_text)
        .ok()
        .and_then(|v| v.get("detail").and_then(|d| d.as_str()).map(|s| s.to_string()))
        .unwrap_or_else(|| format!("(非 JSON 响应，{} 字节)", body_bytes.len()));

    if status.is_server_error() {
        tracing::error!(
            method = %method,
            uri = %uri,
            status = %status.as_u16(),
            detail = %detail,
            "API 请求失败"
        );
    } else {
        tracing::warn!(
            method = %method,
            uri = %uri,
            status = %status.as_u16(),
            detail = %detail,
            "API 请求返回错误"
        );
    }

    axum::response::Response::from_parts(parts, axum::body::Body::from(body_bytes))
}

/// Token 鉴权中间件。
///
/// 仅保护 `/assist/api/*`，与 Python `TokenMiddleware` 行为一致：
/// - token 为 `change-me` 或未设置时跳过校验；
/// - 来自 192.168.1.0/24 局域网的请求免校验；
/// - 通过 `X-Token` 头或 `token` 查询参数校验；
/// - `/assist/api/documents/` 下含 `/image/` 的图片 URL 放行。
async fn token_middleware(req: axum::extract::Request, next: Next) -> Response {
    let path = req.uri().path().to_string();

    // 仅保护 API 路径
    if !path.starts_with("/assist/api/") {
        return next.run(req).await;
    }

    // 图片 URL 放行
    if path.contains("/image/") {
        return next.run(req).await;
    }

    let settings = req.extensions().get::<Arc<Settings>>().cloned();
    if let Some(settings) = settings {
        let token = settings.token();

        // token 未设置或为 change-me 时跳过校验
        if token.is_empty() || token == "change-me" {
            return next.run(req).await;
        }

        // 局域网 192.168.1.0/24 免校验
        let client_ip = req
            .headers()
            .get("x-forwarded-for")
            .and_then(|v| v.to_str().ok())
            .map(|s| s.split(',').next().unwrap_or("").trim().to_string());
        if let Some(ip) = client_ip {
            if ip.starts_with("192.168.1.") {
                return next.run(req).await;
            }
        }

        // 校验 X-Token 头或 token 查询参数
        let provided = req
            .headers()
            .get("x-token")
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string())
            .or_else(|| {
                req.uri().query().and_then(|q| {
                    q.split('&').find_map(|kv| {
                        let (k, v) = kv.split_once('=')?;
                        if k == "token" {
                            Some(v.to_string())
                        } else {
                            None
                        }
                    })
                })
            });

        if provided.as_deref() == Some(token.as_str()) {
            return next.run(req).await;
        }

        return (
            http::StatusCode::UNAUTHORIZED,
            axum::Json(serde_json::json!({"detail": "未授权：无效或缺失的 token"})),
        )
            .into_response();
    }

    next.run(req).await
}

/// SPA fallback：命中 frontend/dist 真实文件则返回文件，否则返回 index.html。
///
/// 与 Python `serve_spa` 对齐：
/// - api/、mcp/、uploads/、file/ 前缀应由其他路由处理，未命中则 404；
/// - 命中真实文件时按扩展名返回 MIME 与缓存策略；
/// - 其余客户端路由回退到 index.html。
async fn spa_fallback(uri: axum::http::Uri) -> Response {
    let dist_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("frontend")
        .join("dist");
    let index = dist_dir.join("index.html");

    let path = uri.path();
    let full_path = match path.strip_prefix("/assist") {
        Some(rest) => rest.trim_start_matches('/'),
        None => {
            // 仅处理 /assist 前缀的 SPA 路径，其余 404（对齐 Python）
            return (
                http::StatusCode::NOT_FOUND,
                axum::Json(serde_json::json!({"detail": "Not Found"})),
            )
                .into_response();
        }
    };

    // 这些前缀应由其他路由处理，未命中则 404
    for prefix in ["api/", "mcp/", "uploads/", "file/"] {
        if full_path.starts_with(prefix) {
            return (
                http::StatusCode::NOT_FOUND,
                axum::Json(serde_json::json!({"detail": "Not Found"})),
            )
                .into_response();
        }
    }

    // 命中真实文件（防路径穿越）
    if !full_path.is_empty() {
        if let Some(file) = resolve_frontend_file(&dist_dir, full_path) {
            if let Ok(bytes) = std::fs::read(&file) {
                return (
                    http::StatusCode::OK,
                    [
                        (http::header::CONTENT_TYPE, mime_for_path(&file)),
                        (http::header::CACHE_CONTROL, cache_control_for(&file)),
                    ],
                    bytes,
                )
                    .into_response();
            }
        }
    }

    // 返回 index.html
    if index.exists() {
        (
            http::StatusCode::OK,
            [
                (http::header::CONTENT_TYPE, "text/html; charset=utf-8"),
                (http::header::CACHE_CONTROL, "no-cache, must-revalidate"),
            ],
            std::fs::read(&index).unwrap_or_default(),
        )
            .into_response()
    } else {
        (
            http::StatusCode::OK,
            [(http::header::CONTENT_TYPE, "text/plain; charset=utf-8")],
            b"Frontend not built. Run: cd frontend && pnpm run build".to_vec(),
        )
            .into_response()
    }
}

/// 在 frontend/dist 内解析请求路径（防路径穿越）。
fn resolve_frontend_file(dist_dir: &std::path::Path, path: &str) -> Option<std::path::PathBuf> {
    let normalized = path.trim_start_matches('/');
    let candidate = dist_dir.join(normalized);
    let candidate = candidate.canonicalize().ok()?;
    let dist_canon = dist_dir.canonicalize().ok()?;
    if candidate.starts_with(&dist_canon) && candidate.is_file() {
        Some(candidate)
    } else {
        None
    }
}

/// 根据扩展名返回 MIME 类型。
fn mime_for_path(path: &std::path::Path) -> &'static str {
    match path.extension().and_then(|e| e.to_str()).unwrap_or("") {
        "html" => "text/html; charset=utf-8",
        "js" | "mjs" => "application/javascript",
        "css" => "text/css; charset=utf-8",
        "json" | "map" => "application/json",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "avif" => "image/avif",
        "ico" => "image/x-icon",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "ttf" => "font/ttf",
        "eot" => "application/vnd.ms-fontobject",
        "txt" => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}

/// 与 Python `_get_cache_control` 对齐的缓存策略。
fn cache_control_for(path: &std::path::Path) -> &'static str {
    let basename = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase();
    if basename == "index.html" || ext.is_empty() {
        return "no-cache, must-revalidate";
    }
    match ext.as_str() {
        "js" | "css" | "png" | "jpg" | "jpeg" | "gif" | "svg" | "ico" | "woff"
        | "woff2" | "ttf" | "eot" | "map" | "webp" | "avif" => {
            "public, max-age=31536000, immutable"
        }
        _ => "public, max-age=86400",
    }
}