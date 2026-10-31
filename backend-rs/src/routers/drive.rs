//! 用户网盘路由。
//!
//! 概念：
//! - **我的网盘**（`scope=private`）：每个用户一个私有目录，只有本人可读写；
//! - **公有文件夹**（`scope=public:<id>`）：管理员创建，权限按**角色阈值**（见
//!   `services::drive::PublicFolder`）判定读/写。
//!
//! 所有端点都要求已认证；`private` 作用域自动绑定当前用户，客户端无法指定别人的目录。
//!
//! 与文献库（`documents` + 解析流水线）**完全独立**：网盘文件不会入库、不会解析、不会被索引。

use std::path::Path;
use std::sync::Arc;

use axum::body::Body;
use axum::extract::{Extension, Multipart, RawQuery};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Json, Response};
use axum::routing::{delete, get, post, put};
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::io::AsyncReadExt;
use tokio::io::AsyncSeekExt;
use tokio_util::io::ReaderStream;

use crate::auth::{self, Principal, Role};
use crate::config_manager::ConfigManager;
use crate::config::Settings;
use crate::services::drive::{
    self, content_type_for, is_previewable, list_dir, resolve_path, unique_child, validate_folder_id,
    validate_name, DriveConfig, DriveError, DriveService, PublicFolder, MAX_TEXT_PREVIEW_BYTES,
};
use crate::users::UserStore;

pub fn router() -> axum::Router<()> {
    axum::Router::new()
        // ── 我的网盘 / 公有文件夹（已登录用户） ──
        .route("/assist/api/drive", get(summary))
        .route("/assist/api/drive/", get(summary))
        .route("/assist/api/drive/list", get(list))
        .route("/assist/api/drive/mkdir", post(mkdir))
        .route("/assist/api/drive/rename", post(rename))
        .route("/assist/api/drive/move", post(move_entry))
        .route("/assist/api/drive/file", delete(remove_entry))
        .route("/assist/api/drive/download", get(download))
        .route("/assist/api/drive/raw", get(raw))
        .route("/assist/api/drive/text", get(text_preview))
        .route("/assist/api/drive/upload", post(upload))
        // ── 管理员：公有文件夹与配额 ──
        .route("/assist/api/admin/drive/folders", get(admin_list_folders))
        .route("/assist/api/admin/drive/folders", post(admin_create_folder))
        .route("/assist/api/admin/drive/folders/:id", put(admin_update_folder))
        .route("/assist/api/admin/drive/folders/:id", delete(admin_delete_folder))
        .route("/assist/api/admin/drive/quotas", get(admin_list_quotas))
        .route("/assist/api/admin/drive/quotas/:user_id", put(admin_set_quota))
        .route("/assist/api/admin/drive/settings", get(admin_get_settings))
        .route("/assist/api/admin/drive/settings", put(admin_update_settings))
}

// ── 共享工具 ────────────────────────────────────────────

fn service(settings: &Arc<Settings>, cm: &Arc<ConfigManager>) -> DriveService {
    DriveService::new(settings.clone(), cm.clone())
}

/// 解析查询参数（`?scope=private&path=a/b`）
fn query_param(query: Option<&str>, key: &str) -> Option<String> {
    for pair in query?.split('&') {
        let Some((k, v)) = pair.split_once('=') else {
            continue;
        };
        if k == key {
            return Some(percent_decode(v));
        }
    }
    None
}

/// 极简 percent-decode（够用于文件名/路径；非法序列原样保留）
fn percent_decode(raw: &str) -> String {
    let bytes = raw.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or("");
            if let Ok(v) = u8::from_str_radix(hex, 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        if bytes[i] == b'+' {
            out.push(b' ');
        } else {
            out.push(bytes[i]);
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).to_string()
}

async fn read_upload_field(field: &mut axum::extract::multipart::Field<'_>) -> Vec<u8> {
    let mut buf = Vec::new();
    while let Ok(Some(chunk)) = field.chunk().await {
        buf.extend_from_slice(&chunk);
    }
    buf
}

/// 私有作用域额外校验：公有文件夹写权限
fn require_write(scope: &drive::ResolvedScope) -> Result<(), DriveError> {
    if scope.can_write {
        Ok(())
    } else {
        Err(DriveError::Forbidden)
    }
}

// ── 概览 ────────────────────────────────────────────────

/// `GET /assist/api/drive` —— 网盘概览（开关、配额、可用范围）
async fn summary(
    Extension(settings): Extension<Arc<Settings>>,
    Extension(cm): Extension<Arc<ConfigManager>>,
    Extension(principal): Extension<Principal>,
) -> Response {
    let svc = service(&settings, &cm);
    let config = svc.config();
    let mut folders = Vec::new();
    for folder in &config.public_folders {
        if folder.can_read(&principal.role) {
            folders.push(json!({
                "id": folder.id,
                "name": folder.name,
                "read_role": folder.read_role,
                "write_role": folder.write_role,
                "can_write": folder.can_write(&principal.role),
            }));
        }
    }
    let private_used = svc
        .private_base(&principal.user_id)
        .map(|p| svc.used_bytes(&p))
        .unwrap_or(0);
    Json(json!({
        "enabled": config.enabled,
        "max_upload_mb": config.max_upload_mb,
        "quota_bytes": svc.quota_bytes(&config, &principal.user_id),
        "used_bytes": private_used,
        "public_folders": folders,
        "user": { "id": principal.user_id, "username": principal.username, "role": principal.role.as_str() },
    }))
    .into_response()
}

// ── 列表 ────────────────────────────────────────────────

/// `GET /assist/api/drive/list?scope=private|public:<id>&path=a/b`
async fn list(
    Extension(settings): Extension<Arc<Settings>>,
    Extension(cm): Extension<Arc<ConfigManager>>,
    Extension(principal): Extension<Principal>,
    RawQuery(query): RawQuery,
) -> Response {
    let svc = service(&settings, &cm);
    let scope_raw = query_param(query.as_deref(), "scope").unwrap_or_else(|| "private".to_string());
    let rel = query_param(query.as_deref(), "path").unwrap_or_default();

    let scope = match svc.resolve_scope(&principal, &scope_raw) {
        Ok(s) => s,
        Err(e) => return e.into_response(),
    };
    let dir = match resolve_path(&scope.base, &rel) {
        Ok(p) => p,
        Err(e) => return e.into_response(),
    };
    if dir.exists() && !dir.is_dir() {
        return (StatusCode::BAD_REQUEST, Json(json!({"detail": "该路径不是目录"}))).into_response();
    }
    let entries = match list_dir(&dir) {
        Ok(v) => v,
        Err(e) => return e.into_response(),
    };
    let config = svc.config();
    let used = svc.used_bytes(&scope.base);

    // 面包屑（不含根）
    let normalized = drive_prefix(&rel);
    let mut breadcrumb = Vec::new();
    let mut acc = String::new();
    for part in normalized.split('/').filter(|s| !s.is_empty()) {
        acc = if acc.is_empty() {
            part.to_string()
        } else {
            format!("{}/{}", acc, part)
        };
        breadcrumb.push(json!({"name": part, "path": acc}));
    }

    Json(json!({
        "scope": scope.kind.as_str(),
        "label": scope.label,
        "can_write": scope.can_write,
        "path": normalized,
        "breadcrumb": breadcrumb,
        "entries": entries.iter().map(|e| e.to_json()).collect::<Vec<_>>(),
        "used_bytes": used,
        "quota_bytes": svc.quota_bytes(&config, &principal.user_id),
        "max_upload_mb": config.max_upload_mb,
    }))
    .into_response()
}

/// 归一化路径 + 去掉末尾斜杠（仅用于展示/面包屑）
fn drive_prefix(rel: &str) -> String {
    rel.split('/')
        .map(str::trim)
        .filter(|s| !s.is_empty() && *s != ".")
        .collect::<Vec<_>>()
        .join("/")
}

// ── 目录/文件操作 ───────────────────────────────────────

fn default_scope() -> String {
    "private".to_string()
}

#[derive(Debug, Deserialize)]
struct CreateDirBody {
    #[serde(default = "default_scope")]
    scope: String,
    /// 父目录（相对路径，空 = 根）
    #[serde(default)]
    path: String,
    name: String,
}

#[derive(Debug, Deserialize)]
struct RenameBody {
    #[serde(default = "default_scope")]
    scope: String,
    path: String,
    new_name: String,
}

#[derive(Debug, Deserialize)]
struct MoveBody {
    #[serde(default = "default_scope")]
    scope: String,
    /// 源（相对路径）
    from: String,
    /// 目标目录（相对路径，空 = 根）
    #[serde(default)]
    to_dir: String,
}

/// `POST /assist/api/drive/mkdir`
async fn mkdir(
    Extension(settings): Extension<Arc<Settings>>,
    Extension(cm): Extension<Arc<ConfigManager>>,
    Extension(principal): Extension<Principal>,
    Json(body): Json<CreateDirBody>,
) -> Response {
    let svc = service(&settings, &cm);
    let scope = match svc.resolve_scope(&principal, &body.scope) {
        Ok(s) => s,
        Err(e) => return e.into_response(),
    };
    if let Err(e) = require_write(&scope) {
        return e.into_response();
    }
    let name = match validate_name(&body.name) {
        Ok(n) => n,
        Err(e) => return e.into_response(),
    };
    let parent = match resolve_path(&scope.base, &body.path) {
        Ok(p) => p,
        Err(e) => return e.into_response(),
    };
    if !parent.is_dir() {
        return DriveError::NotDir.into_response();
    }
    let target = parent.join(name);
    if target.exists() {
        return DriveError::Exists.into_response();
    }
    match tokio::fs::create_dir(&target).await {
        Ok(()) => Json(json!({
            "ok": true,
            "path": drive_prefix(&format!("{}/{}", drive_prefix(&body.path), name)),
        }))
        .into_response(),
        Err(e) => DriveError::Io(e.to_string()).into_response(),
    }
}

/// `POST /assist/api/drive/rename`
async fn rename(
    Extension(settings): Extension<Arc<Settings>>,
    Extension(cm): Extension<Arc<ConfigManager>>,
    Extension(principal): Extension<Principal>,
    Json(body): Json<RenameBody>,
) -> Response {
    let svc = service(&settings, &cm);
    let scope = match svc.resolve_scope(&principal, &body.scope) {
        Ok(s) => s,
        Err(e) => return e.into_response(),
    };
    if let Err(e) = require_write(&scope) {
        return e.into_response();
    }
    let src = match resolve_path(&scope.base, &body.path) {
        Ok(p) => p,
        Err(e) => return e.into_response(),
    };
    if !src.exists() {
        return DriveError::NotFound.into_response();
    }
    let new_name = match validate_name(&body.new_name) {
        Ok(n) => n,
        Err(e) => return e.into_response(),
    };
    let Some(parent) = src.parent() else {
        return DriveError::InvalidPath.into_response();
    };
    let dst = parent.join(new_name);
    if dst.exists() {
        return DriveError::Exists.into_response();
    }
    match tokio::fs::rename(&src, &dst).await {
        Ok(()) => Json(json!({"ok": true})).into_response(),
        Err(e) => DriveError::Io(e.to_string()).into_response(),
    }
}

/// `POST /assist/api/drive/move`
async fn move_entry(
    Extension(settings): Extension<Arc<Settings>>,
    Extension(cm): Extension<Arc<ConfigManager>>,
    Extension(principal): Extension<Principal>,
    Json(body): Json<MoveBody>,
) -> Response {
    let svc = service(&settings, &cm);
    let scope = match svc.resolve_scope(&principal, &body.scope) {
        Ok(s) => s,
        Err(e) => return e.into_response(),
    };
    if let Err(e) = require_write(&scope) {
        return e.into_response();
    }
    let src = match resolve_path(&scope.base, &body.from) {
        Ok(p) => p,
        Err(e) => return e.into_response(),
    };
    if !src.exists() {
        return DriveError::NotFound.into_response();
    }
    let to_dir = match resolve_path(&scope.base, &body.to_dir) {
        Ok(p) => p,
        Err(e) => return e.into_response(),
    };
    if !to_dir.is_dir() {
        return DriveError::NotDir.into_response();
    }
    // 不允许把目录移进自身
    if src.is_dir() && to_dir.starts_with(&src) {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"detail": "不能把目录移动到它自己里面"})),
        )
            .into_response();
    }
    let name = src.file_name().map(|n| n.to_owned()).unwrap_or_default();
    let dst = unique_child(&to_dir, &name.to_string_lossy());
    match tokio::fs::rename(&src, &dst).await {
        Ok(()) => Json(json!({"ok": true, "name": dst.file_name().map(|n| n.to_string_lossy().to_string())}))
            .into_response(),
        Err(e) => DriveError::Io(e.to_string()).into_response(),
    }
}

/// `DELETE /assist/api/drive/file?scope&path&recursive=1`
async fn remove_entry(
    Extension(settings): Extension<Arc<Settings>>,
    Extension(cm): Extension<Arc<ConfigManager>>,
    Extension(principal): Extension<Principal>,
    RawQuery(query): RawQuery,
) -> Response {
    let svc = service(&settings, &cm);
    let scope_raw = query_param(query.as_deref(), "scope").unwrap_or_else(|| "private".to_string());
    let rel = query_param(query.as_deref(), "path").unwrap_or_default();
    let recursive = matches!(
        query_param(query.as_deref(), "recursive").as_deref(),
        Some("1") | Some("true")
    );

    let scope = match svc.resolve_scope(&principal, &scope_raw) {
        Ok(s) => s,
        Err(e) => return e.into_response(),
    };
    if let Err(e) = require_write(&scope) {
        return e.into_response();
    }
    let target = match resolve_path(&scope.base, &rel) {
        Ok(p) => p,
        Err(e) => return e.into_response(),
    };
    // 禁止删除作用域根目录
    if target == scope.base {
        return DriveError::InvalidPath.into_response();
    }
    if !target.exists() {
        return DriveError::NotFound.into_response();
    }
    let result = if target.is_dir() {
        let empty = std::fs::read_dir(&target)
            .map(|mut it| it.next().is_none())
            .unwrap_or(true);
        if empty {
            tokio::fs::remove_dir(&target).await
        } else if recursive {
            tokio::fs::remove_dir_all(&target).await
        } else {
            return DriveError::DirNotEmpty.into_response();
        }
    } else {
        tokio::fs::remove_file(&target).await
    };
    match result {
        Ok(()) => Json(json!({"ok": true})).into_response(),
        Err(e) => DriveError::Io(e.to_string()).into_response(),
    }
}

// ── 上传 ────────────────────────────────────────────────

/// `POST /assist/api/drive/upload` —— multipart：`scope`、`path` + 一个或多个 `file`
async fn upload(
    Extension(settings): Extension<Arc<Settings>>,
    Extension(cm): Extension<Arc<ConfigManager>>,
    Extension(principal): Extension<Principal>,
    mut multipart: Multipart,
) -> Response {
    let svc = service(&settings, &cm);
    let config = svc.config();

    let mut scope_raw = "private".to_string();
    let mut rel = String::new();
    let mut results: Vec<Value> = Vec::new();
    let mut scope = None;
    let mut quota_used: Option<u64> = None;

    while let Ok(Some(mut field)) = multipart.next_field().await {
        let name = field.name().unwrap_or("").to_string();
        match name.as_str() {
            "scope" => {
                scope_raw = field.text().await.unwrap_or_else(|_| "private".to_string());
            }
            "path" => {
                rel = field.text().await.unwrap_or_default();
            }
            "file" => {
                // 作用域在第一个文件前解析（前端保证字段顺序）
                if scope.is_none() {
                    let resolved = match svc.resolve_scope(&principal, &scope_raw) {
                        Ok(s) => s,
                        Err(e) => return e.into_response(),
                    };
                    if let Err(e) = require_write(&resolved) {
                        return e.into_response();
                    }
                    quota_used = Some(svc.used_bytes(&resolved.base));
                    scope = Some(resolved);
                }
                let scope = scope.as_ref().expect("作用域已解析");

                let filename = field.file_name().unwrap_or("").to_string();
                let filename = match validate_name(&filename) {
                    Ok(n) => n.to_string(),
                    Err(_) => {
                        results.push(json!({"name": filename, "ok": false, "detail": "文件名非法"}));
                        // 丢弃剩余数据
                        let _ = read_upload_field(&mut field).await;
                        continue;
                    }
                };
                let dir = match resolve_path(&scope.base, &rel) {
                    Ok(p) => p,
                    Err(e) => {
                        results.push(json!({"name": filename, "ok": false, "detail": e.to_string()}));
                        let _ = read_upload_field(&mut field).await;
                        continue;
                    }
                };
                if !dir.is_dir() {
                    results.push(json!({"name": filename, "ok": false, "detail": "目标目录不存在"}));
                    let _ = read_upload_field(&mut field).await;
                    continue;
                }

                // 边读边写临时文件，超限即中止
                let target = unique_child(&dir, &filename);
                let tmp = dir.join(format!(
                    ".upload-{}-{}.part",
                    std::process::id(),
                    chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
                ));
                let mut file = match tokio::fs::File::create(&tmp).await {
                    Ok(f) => f,
                    Err(e) => {
                        results.push(json!({"name": filename, "ok": false, "detail": format!("无法写入：{e}")}));
                        continue;
                    }
                };
                let mut written: u64 = 0;
                let mut error: Option<String> = None;
                loop {
                    match field.chunk().await {
                        Ok(Some(chunk)) => {
                            written += chunk.len() as u64;
                            if let Err(e) = svc.check_size(&config, written) {
                                error = Some(e.to_string());
                                break;
                            }
                            if matches!(scope.kind, drive::ScopeKind::Private) {
                                if let Err(e) = svc.check_quota(
                                    &config,
                                    &principal.user_id,
                                    quota_used.unwrap_or(0),
                                    written,
                                ) {
                                    error = Some(e.to_string());
                                    break;
                                }
                            }
                            if let Err(e) = tokio::io::AsyncWriteExt::write_all(&mut file, &chunk).await
                            {
                                error = Some(format!("写入失败：{e}"));
                                break;
                            }
                        }
                        Ok(None) => break,
                        Err(e) => {
                            error = Some(format!("读取上传数据失败：{e}"));
                            break;
                        }
                    }
                }
                let _ = tokio::io::AsyncWriteExt::flush(&mut file).await;
                drop(file);

                if let Some(err) = error {
                    let _ = tokio::fs::remove_file(&tmp).await;
                    results.push(json!({"name": filename, "ok": false, "detail": err}));
                    continue;
                }
                if let Err(e) = tokio::fs::rename(&tmp, &target).await {
                    let _ = tokio::fs::remove_file(&tmp).await;
                    results.push(json!({"name": filename, "ok": false, "detail": format!("重命名失败：{e}")}));
                    continue;
                }
                *quota_used.as_mut().unwrap() += written;
                tracing::info!(
                    audit = "drive_upload",
                    user = %principal.username,
                    scope = %scope.kind.as_str(),
                    size = written,
                    name = %target.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default(),
                    "上传文件到网盘"
                );
                results.push(json!({
                    "name": target.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or(filename),
                    "ok": true,
                    "size": written,
                }));
            }
            _ => {}
        }
    }

    if scope.is_none() {
        return (StatusCode::BAD_REQUEST, Json(json!({"detail": "请求中缺少文件"}))).into_response();
    }
    let failed = results.iter().filter(|r| r["ok"] != json!(true)).count();
    let status = if failed == 0 {
        StatusCode::CREATED
    } else if failed == results.len() {
        StatusCode::BAD_REQUEST
    } else {
        StatusCode::MULTI_STATUS
    };
    (
        status,
        Json(json!({
            "files": results,
            "used_bytes": quota_used.unwrap_or(0),
            "quota_bytes": svc.quota_bytes(&config, &principal.user_id),
        })),
    )
        .into_response()
}

// ── 下载 / 预览 ─────────────────────────────────────────

/// 打开文件并构造响应（支持 Range、Content-Disposition）
async fn file_response(
    path: &Path,
    headers: &HeaderMap,
    attachment: Option<&str>,
) -> Response {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    let mut file = match tokio::fs::File::open(path).await {
        Ok(f) => f,
        Err(_) => return DriveError::NotFound.into_response(),
    };
    let meta = match file.metadata().await {
        Ok(m) => m,
        Err(e) => return DriveError::Io(e.to_string()).into_response(),
    };
    if meta.is_dir() {
        return DriveError::NotDir.into_response();
    }
    let size = meta.len();
    let content_type = content_type_for(&name);

    // 简易 Range 支持（PDF 预览/媒体拖动需要）
    let range = headers
        .get(header::RANGE)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| parse_range(s, size));
    let (status, start, length) = match range {
        Some((s, e)) => (StatusCode::PARTIAL_CONTENT, s, e - s + 1),
        None => (StatusCode::OK, 0, size),
    };
    if start > 0 && file.seek(std::io::SeekFrom::Start(start)).await.is_err() {
        return DriveError::Io("无法定位文件".to_string()).into_response();
    }

    let stream = ReaderStream::new(file.take(length));
    let mut resp = Response::new(Body::from_stream(stream));
    *resp.status_mut() = status;
    let h = resp.headers_mut();
    h.insert(header::CONTENT_TYPE, HeaderValue::from_str(content_type).unwrap());
    h.insert(header::CONTENT_LENGTH, HeaderValue::from_str(&length.to_string()).unwrap());
    h.insert(header::ACCEPT_RANGES, HeaderValue::from_static("bytes"));
    h.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, max-age=0, must-revalidate"),
    );
    if status == StatusCode::PARTIAL_CONTENT {
        h.insert(
            header::CONTENT_RANGE,
            HeaderValue::from_str(&format!("bytes {}-{}/{}", start, start + length - 1, size)).unwrap(),
        );
    }
    if let Some(filename) = attachment {
        // RFC 5987：同时给 ASCII 回退与 UTF-8 文件名
        let ascii: String = filename
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() || "._- ".contains(c) { c } else { '_' })
            .collect();
        h.insert(
            header::CONTENT_DISPOSITION,
            HeaderValue::from_str(&format!(
                "attachment; filename=\"{}\"; filename*=UTF-8''{}",
                ascii,
                percent_encode(filename)
            ))
            .unwrap(),
        );
    }
    resp
}

fn percent_encode(s: &str) -> String {
    let mut out = String::new();
    for b in s.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(*b as char)
            }
            _ => out.push_str(&format!("%{:02X}", b)),
        }
    }
    out
}

/// `bytes=start-end` / `bytes=start-` 解析
fn parse_range(value: &str, size: u64) -> Option<(u64, u64)> {
    let spec = value.strip_prefix("bytes=")?;
    let (start, end) = spec.split_once('-')?;
    let start: u64 = start.trim().parse().ok()?;
    if start >= size {
        return None;
    }
    let end = if end.trim().is_empty() {
        size - 1
    } else {
        end.trim().parse::<u64>().ok()?.min(size - 1)
    };
    if end < start {
        return None;
    }
    Some((start, end))
}

/// `GET /assist/api/drive/download?scope&path` —— 作为附件下载
async fn download(
    Extension(settings): Extension<Arc<Settings>>,
    Extension(cm): Extension<Arc<ConfigManager>>,
    Extension(principal): Extension<Principal>,
    RawQuery(query): RawQuery,
    headers: HeaderMap,
) -> Response {
    let svc = service(&settings, &cm);
    let scope_raw = query_param(query.as_deref(), "scope").unwrap_or_else(|| "private".to_string());
    let rel = query_param(query.as_deref(), "path").unwrap_or_default();
    let scope = match svc.resolve_scope(&principal, &scope_raw) {
        Ok(s) => s,
        Err(e) => return e.into_response(),
    };
    let path = match resolve_path(&scope.base, &rel) {
        Ok(p) => p,
        Err(e) => return e.into_response(),
    };
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "download".to_string());
    file_response(&path, &headers, Some(&name)).await
}

/// `GET /assist/api/drive/raw?scope&path` —— 内联响应（图片/PDF/音频预览用）
async fn raw(
    Extension(settings): Extension<Arc<Settings>>,
    Extension(cm): Extension<Arc<ConfigManager>>,
    Extension(principal): Extension<Principal>,
    RawQuery(query): RawQuery,
    headers: HeaderMap,
) -> Response {
    let svc = service(&settings, &cm);
    let scope_raw = query_param(query.as_deref(), "scope").unwrap_or_else(|| "private".to_string());
    let rel = query_param(query.as_deref(), "path").unwrap_or_default();
    let scope = match svc.resolve_scope(&principal, &scope_raw) {
        Ok(s) => s,
        Err(e) => return e.into_response(),
    };
    let path = match resolve_path(&scope.base, &rel) {
        Ok(p) => p,
        Err(e) => return e.into_response(),
    };
    // 只对可预览类型内联，其余强制下载，避免 SVG/HTML 之类的 XSS 风险
    let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    let attachment = if is_previewable(&name) { None } else { Some(name.as_str()) };
    file_response(&path, &headers, attachment).await
}

/// `GET /assist/api/drive/text?scope&path` —— 文本预览（≤ 512KB）
async fn text_preview(
    Extension(settings): Extension<Arc<Settings>>,
    Extension(cm): Extension<Arc<ConfigManager>>,
    Extension(principal): Extension<Principal>,
    RawQuery(query): RawQuery,
) -> Response {
    let svc = service(&settings, &cm);
    let scope_raw = query_param(query.as_deref(), "scope").unwrap_or_else(|| "private".to_string());
    let rel = query_param(query.as_deref(), "path").unwrap_or_default();
    let scope = match svc.resolve_scope(&principal, &scope_raw) {
        Ok(s) => s,
        Err(e) => return e.into_response(),
    };
    let path = match resolve_path(&scope.base, &rel) {
        Ok(p) => p,
        Err(e) => return e.into_response(),
    };
    let meta = match tokio::fs::metadata(&path).await {
        Ok(m) => m,
        Err(_) => return DriveError::NotFound.into_response(),
    };
    if meta.is_dir() {
        return DriveError::NotDir.into_response();
    }
    let mut reader = match tokio::fs::File::open(&path).await {
        Ok(f) => f,
        Err(e) => return DriveError::Io(e.to_string()).into_response(),
    };
    let limit = meta.len().min(MAX_TEXT_PREVIEW_BYTES);
    let mut buf = vec![0u8; limit as usize];
    if let Err(e) = reader.read_exact(&mut buf).await {
        return DriveError::Io(e.to_string()).into_response();
    }
    Json(json!({
        "content": String::from_utf8_lossy(&buf),
        "truncated": meta.len() > limit,
        "size": meta.len(),
        "name": path.file_name().map(|n| n.to_string_lossy().to_string()),
    }))
    .into_response()
}

// ── 管理员：公有文件夹 ──────────────────────────────────

fn require_admin(role: &Role) -> Result<(), Response> {
    auth::assert_role(role, &["admin"])
}

#[derive(Debug, Deserialize)]
struct CreateFolderBody {
    name: String,
    #[serde(default)]
    read_role: Option<String>,
    #[serde(default)]
    write_role: Option<String>,
}

#[derive(Debug, Deserialize)]
struct UpdateFolderBody {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    read_role: Option<String>,
    #[serde(default)]
    write_role: Option<String>,
}

/// `GET /assist/api/admin/drive/folders`
async fn admin_list_folders(
    Extension(role): Extension<Role>,
    Extension(settings): Extension<Arc<Settings>>,
    Extension(cm): Extension<Arc<ConfigManager>>,
) -> Response {
    if let Err(resp) = require_admin(&role) {
        return resp;
    }
    let svc = service(&settings, &cm);
    let config = svc.config();
    let folders: Vec<Value> = config
        .public_folders
        .iter()
        .map(|f| {
            let base = svc.public_base(&f.id).ok();
            let used = base.as_ref().map(|b| svc.used_bytes(b)).unwrap_or(0);
            json!({
                "id": f.id,
                "name": f.name,
                "read_role": f.read_role,
                "write_role": f.write_role,
                "created_at": f.created_at,
                "used_bytes": used,
            })
        })
        .collect();
    Json(json!({ "folders": folders, "count": folders.len() })).into_response()
}

/// 校验角色阈值字符串
fn validate_role_threshold(role: &str) -> Result<String, Response> {
    match Role::from_str(role.trim()) {
        Some(_) => Ok(role.trim().to_string()),
        None => Err((
            StatusCode::BAD_REQUEST,
            Json(json!({"detail": "角色无效：仅支持 admin / view-only / view-upload"})),
        )
            .into_response()),
    }
}

/// `POST /assist/api/admin/drive/folders`
async fn admin_create_folder(
    Extension(role): Extension<Role>,
    Extension(settings): Extension<Arc<Settings>>,
    Extension(cm): Extension<Arc<ConfigManager>>,
    Json(body): Json<CreateFolderBody>,
) -> Response {
    if let Err(resp) = require_admin(&role) {
        return resp;
    }
    let name = match validate_name(&body.name) {
        Ok(n) => n.to_string(),
        Err(e) => return e.into_response(),
    };
    let read_role = match validate_role_threshold(body.read_role.as_deref().unwrap_or("view-only")) {
        Ok(r) => r,
        Err(resp) => return resp,
    };
    let write_role = match validate_role_threshold(body.write_role.as_deref().unwrap_or("admin")) {
        Ok(r) => r,
        Err(resp) => return resp,
    };

    let svc = service(&settings, &cm);
    let mut config = svc.config();
    if config.public_folders.iter().any(|f| f.name == name) {
        return (
            StatusCode::CONFLICT,
            Json(json!({"detail": "同名公有文件夹已存在"})),
        )
            .into_response();
    }
    // 生成 id（8 位随机 hex）
    let id = {
        use rand::RngCore;
        let mut buf = [0u8; 4];
        rand::thread_rng().fill_bytes(&mut buf);
        format!("pf_{}", hex::encode(buf))
    };
    let folder = PublicFolder {
        id: id.clone(),
        name: name.clone(),
        read_role,
        write_role,
        created_at: chrono::Utc::now().to_rfc3339(),
    };
    config.public_folders.push(folder);

    let base = match svc.public_base(&id) {
        Ok(b) => b,
        Err(e) => return e.into_response(),
    };
    if let Err(e) = tokio::fs::create_dir_all(&base).await {
        return DriveError::Io(e.to_string()).into_response();
    }
    if let Err(e) = save_drive_config(&cm, &config) {
        return e.into_response();
    }
    tracing::info!(audit = "drive_folder_create", folder = %name, %id, "创建公有文件夹");
    Json(json!({"folder": {
        "id": id, "name": name,
        "read_role": config.public_folders.last().map(|f| f.read_role.clone()).unwrap_or_default(),
        "write_role": config.public_folders.last().map(|f| f.write_role.clone()).unwrap_or_default(),
        "created_at": config.public_folders.last().map(|f| f.created_at.clone()).unwrap_or_default(),
    }}))
    .into_response()
}

/// `PUT /assist/api/admin/drive/folders/:id`
async fn admin_update_folder(
    Extension(role): Extension<Role>,
    Extension(settings): Extension<Arc<Settings>>,
    Extension(cm): Extension<Arc<ConfigManager>>,
    axum::extract::Path(id): axum::extract::Path<String>,
    Json(body): Json<UpdateFolderBody>,
) -> Response {
    if let Err(resp) = require_admin(&role) {
        return resp;
    }
    if validate_folder_id(&id).is_err() {
        return DriveError::InvalidPath.into_response();
    }
    let read_role = match body.read_role.as_deref().map(validate_role_threshold) {
        Some(Ok(r)) => Some(r),
        Some(Err(resp)) => return resp,
        None => None,
    };
    let write_role = match body.write_role.as_deref().map(validate_role_threshold) {
        Some(Ok(r)) => Some(r),
        Some(Err(resp)) => return resp,
        None => None,
    };
    let name = match body.name.as_deref().map(validate_name) {
        Some(Ok(n)) => Some(n.to_string()),
        Some(Err(e)) => return e.into_response(),
        None => None,
    };

    let svc = service(&settings, &cm);
    let mut config = svc.config();
    let Some(folder) = config.public_folders.iter_mut().find(|f| f.id == id) else {
        return DriveError::FolderNotFound.into_response();
    };
    if let Some(name) = name {
        folder.name = name;
    }
    if let Some(role) = read_role {
        folder.read_role = role;
    }
    if let Some(role) = write_role {
        folder.write_role = role;
    }
    let folder = folder.clone();
    if let Err(e) = save_drive_config(&cm, &config) {
        return e.into_response();
    }
    tracing::info!(audit = "drive_folder_update", folder = %folder.name, %id, "更新公有文件夹");
    Json(json!({"folder": {
        "id": folder.id, "name": folder.name,
        "read_role": folder.read_role, "write_role": folder.write_role,
        "created_at": folder.created_at,
    }}))
    .into_response()
}

/// `DELETE /assist/api/admin/drive/folders/:id?force=1`
async fn admin_delete_folder(
    Extension(role): Extension<Role>,
    Extension(settings): Extension<Arc<Settings>>,
    Extension(cm): Extension<Arc<ConfigManager>>,
    axum::extract::Path(id): axum::extract::Path<String>,
    RawQuery(query): RawQuery,
) -> Response {
    if let Err(resp) = require_admin(&role) {
        return resp;
    }
    if validate_folder_id(&id).is_err() {
        return DriveError::InvalidPath.into_response();
    }
    let force = matches!(
        query_param(query.as_deref(), "force").as_deref(),
        Some("1") | Some("true")
    );

    let svc = service(&settings, &cm);
    let mut config = svc.config();
    let Some(idx) = config.public_folders.iter().position(|f| f.id == id) else {
        return DriveError::FolderNotFound.into_response();
    };
    let base = match svc.public_base(&id) {
        Ok(b) => b,
        Err(e) => return e.into_response(),
    };
    if base.exists() {
        let non_empty = std::fs::read_dir(&base)
            .map(|mut it| it.next().is_some())
            .unwrap_or(false);
        if non_empty && !force {
            return (
                StatusCode::CONFLICT,
                Json(json!({"detail": "文件夹非空，需 force=1 才能强制删除"})),
            )
                .into_response();
        }
        if let Err(e) = tokio::fs::remove_dir_all(&base).await {
            return DriveError::Io(e.to_string()).into_response();
        }
    }
    let removed = config.public_folders.remove(idx);
    if let Err(e) = save_drive_config(&cm, &config) {
        return e.into_response();
    }
    tracing::warn!(audit = "drive_folder_delete", folder = %removed.name, %id, "删除公有文件夹");
    Json(json!({"ok": true})).into_response()
}

// ── 管理员：配额与设置 ──────────────────────────────────

/// `GET /assist/api/admin/drive/quotas`
async fn admin_list_quotas(
    Extension(role): Extension<Role>,
    Extension(settings): Extension<Arc<Settings>>,
    Extension(cm): Extension<Arc<ConfigManager>>,
    Extension(users): Extension<Arc<UserStore>>,
) -> Response {
    if let Err(resp) = require_admin(&role) {
        return resp;
    }
    let svc = service(&settings, &cm);
    let config = svc.config();
    let mut rows = Vec::new();
    for user in users.list() {
        let used = svc
            .private_base(&user.id)
            .map(|p| svc.used_bytes(&p))
            .unwrap_or(0);
        rows.push(json!({
            "user_id": user.id,
            "username": user.username,
            "display_name": user.display_name,
            "role": user.role,
            "used_bytes": used,
            "quota_bytes": svc.quota_bytes(&config, &user.id),
            "quota_override_mb": config.quota_overrides.get(&user.id).copied(),
        }));
    }
    Json(json!({
        "users": rows,
        "default_quota_mb": config.default_quota_mb,
        "max_upload_mb": config.max_upload_mb,
    }))
    .into_response()
}

#[derive(Debug, Deserialize)]
struct SetQuotaBody {
    /// 配额（MB）；0 = 不限
    quota_mb: u64,
    /// true 时删除该用户的覆盖值，回退到默认配额
    #[serde(default)]
    clear: bool,
}

/// `PUT /assist/api/admin/drive/quotas/:user_id`
async fn admin_set_quota(
    Extension(role): Extension<Role>,
    Extension(cm): Extension<Arc<ConfigManager>>,
    Extension(users): Extension<Arc<UserStore>>,
    axum::extract::Path(user_id): axum::extract::Path<String>,
    Json(body): Json<SetQuotaBody>,
) -> Response {
    if let Err(resp) = require_admin(&role) {
        return resp;
    }
    let Some(user) = users.find_by_id(&user_id) else {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({"detail": "用户不存在"})),
        )
            .into_response();
    };
    let mut config: DriveConfig = serde_json::from_value(cm.get_service_config()["drive"].clone())
        .unwrap_or_default();
    if body.clear {
        config.quota_overrides.remove(&user.id);
    } else {
        config.quota_overrides.insert(user.id.clone(), body.quota_mb);
    }
    if let Err(e) = save_drive_config(&cm, &config) {
        return e.into_response();
    }
    tracing::info!(
        audit = "drive_quota_update",
        user = %user.username,
        quota_mb = body.quota_mb,
        clear = body.clear,
        "更新用户网盘配额"
    );
    Json(json!({"ok": true, "quota_mb": if body.clear { None } else { Some(body.quota_mb) } }))
        .into_response()
}

/// `GET /assist/api/admin/drive/settings`
async fn admin_get_settings(
    Extension(role): Extension<Role>,
    Extension(cm): Extension<Arc<ConfigManager>>,
) -> Response {
    if let Err(resp) = require_admin(&role) {
        return resp;
    }
    let config: DriveConfig =
        serde_json::from_value(cm.get_service_config()["drive"].clone()).unwrap_or_default();
    Json(json!({
        "enabled": config.enabled,
        "max_upload_mb": config.max_upload_mb,
        "default_quota_mb": config.default_quota_mb,
    }))
    .into_response()
}

#[derive(Debug, Deserialize)]
struct UpdateDriveSettingsBody {
    #[serde(default)]
    enabled: Option<bool>,
    #[serde(default)]
    max_upload_mb: Option<u64>,
    #[serde(default)]
    default_quota_mb: Option<u64>,
}

/// `PUT /assist/api/admin/drive/settings`
async fn admin_update_settings(
    Extension(role): Extension<Role>,
    Extension(cm): Extension<Arc<ConfigManager>>,
    Json(body): Json<UpdateDriveSettingsBody>,
) -> Response {
    if let Err(resp) = require_admin(&role) {
        return resp;
    }
    let mut config: DriveConfig =
        serde_json::from_value(cm.get_service_config()["drive"].clone()).unwrap_or_default();
    if let Some(v) = body.enabled {
        config.enabled = v;
    }
    if let Some(v) = body.max_upload_mb {
        config.max_upload_mb = v.clamp(1, 10_240);
    }
    if let Some(v) = body.default_quota_mb {
        config.default_quota_mb = v;
    }
    if let Err(e) = save_drive_config(&cm, &config) {
        return e.into_response();
    }
    tracing::info!(
        audit = "drive_settings_update",
        enabled = config.enabled,
        max_upload_mb = config.max_upload_mb,
        default_quota_mb = config.default_quota_mb,
        "更新网盘设置"
    );
    Json(json!({
        "enabled": config.enabled,
        "max_upload_mb": config.max_upload_mb,
        "default_quota_mb": config.default_quota_mb,
    }))
    .into_response()
}

/// 原子保存 `drive` 段（加写锁、只改这一段，避免与其它配置保存互相覆盖）
fn save_drive_config(cm: &ConfigManager, config: &DriveConfig) -> Result<(), DriveError> {
    let value = serde_json::to_value(config).map_err(|e| DriveError::Io(e.to_string()))?;
    cm.update_section("drive", |section| {
        *section = value.clone();
        Ok(())
    })
    .map_err(|e| DriveError::Io(e.to_string()))?;
    Ok(())
}

// 供 main.rs 计算上传 body 上限（multipart 开销留 1MB 余量）
pub fn body_limit_bytes(cm: &ConfigManager) -> usize {
    let config: DriveConfig = serde_json::from_value(cm.get_service_config()["drive"].clone())
        .unwrap_or_default();
    // 单次请求最多按 10 个文件估算，上限 512MB
    let per_request = config.max_upload_bytes().saturating_mul(10) + 1024 * 1024;
    per_request.min(512 * 1024 * 1024) as usize
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn range_parsing() {
        assert_eq!(parse_range("bytes=0-99", 1000), Some((0, 99)));
        assert_eq!(parse_range("bytes=100-", 1000), Some((100, 999)));
        assert_eq!(parse_range("bytes=0-5000", 1000), Some((0, 999)));
        assert_eq!(parse_range("bytes=1000-", 1000), None);
        assert_eq!(parse_range("bytes=500-100", 1000), None);
        assert_eq!(parse_range("items=0-10", 1000), None);
    }

    #[test]
    fn percent_encode_decode_roundtrip() {
        let name = "论文 2024 (v2).pdf";
        let encoded = percent_encode(name);
        assert!(!encoded.contains(' '));
        assert_eq!(percent_decode(&encoded), name);
        assert_eq!(percent_decode("a+b"), "a b");
        assert_eq!(percent_decode("%zz"), "%zz");
    }

    #[test]
    fn query_param_extracts_and_decodes() {
        let q = Some("scope=public%3Apf_1&path=%E8%AE%BA%E6%96%87%2Fa.pdf");
        assert_eq!(query_param(q, "scope").as_deref(), Some("public:pf_1"));
        assert_eq!(query_param(q, "path").as_deref(), Some("论文/a.pdf"));
        assert_eq!(query_param(q, "path"), Some("论文/a.pdf".to_string()));
        assert_eq!(query_param(None, "path"), None);
    }
}
