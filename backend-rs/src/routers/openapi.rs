//! OpenAPI 文档路由。

use axum::routing::get;
use axum::response::Json;
use serde_json::json;

pub fn router() -> axum::Router<()> {
    axum::Router::new()
        .route("/assist/openapi/", get(openapi))
        .route("/assist/openapi", get(openapi))
        .route("/assist/openapi.json", get(openapi_json))
}

async fn openapi() -> Json<serde_json::Value> {
    Json(json!({
        "openapi": "3.0.0",
        "info": {
            "title": "OKB-Assist API (Rust)",
            "version": "0.1.0"
        }
    }))
}

async fn openapi_json() -> Json<serde_json::Value> {
    Json(json!({
        "openapi": "3.0.0",
        "info": {
            "title": "OKB-Assist API (Rust)",
            "version": "0.1.0"
        }
    }))
}