//! 用户管理路由（admin）与「我的 token」自助管理（任意已登录用户）。
//!
//! 用户数据存放在 `config.json` 的 `users` 段（见 [`crate::users`]），这里只做
//! HTTP 层：权限判定、参数校验、会话/凭据的级联失效与审计日志。
//!
//! 约定：
//! - 所有响应都不含 `password_hash` / `token_sha256`（统一走 [`User::to_public_json`]）；
//! - API token 明文**只在签发响应里出现一次**；
//! - 禁用/删除用户、重置密码时，同时撤销该用户的会话（`SessionStore`）。

use std::sync::Arc;

use axum::extract::{Extension, Path};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Json, Response};
use axum::routing::{delete, get, post, put};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::auth::{self, Principal, Role};
use crate::session::SessionStore;
use crate::users::{NewUser, UserPatch, UserStore, STATUS_ACTIVE};

pub fn router() -> axum::Router<()> {
    axum::Router::new()
        // ── 用户管理（admin） ──
        .route("/assist/api/admin/users", get(list_users))
        .route("/assist/api/admin/users/", get(list_users))
        .route("/assist/api/admin/users", post(create_user))
        .route("/assist/api/admin/users/", post(create_user))
        .route("/assist/api/admin/users/settings", get(get_settings))
        .route("/assist/api/admin/users/settings", put(update_settings))
        .route("/assist/api/admin/users/:id", put(update_user))
        .route("/assist/api/admin/users/:id", delete(delete_user))
        .route("/assist/api/admin/users/:id/approve", post(approve_user))
        .route("/assist/api/admin/users/:id/approve/", post(approve_user))
        .route("/assist/api/admin/users/:id/password", post(reset_password))
        .route("/assist/api/admin/users/:id/password/", post(reset_password))
        .route("/assist/api/admin/users/:id/tokens", get(list_user_tokens))
        .route("/assist/api/admin/users/:id/tokens/", get(list_user_tokens))
        .route("/assist/api/admin/users/:id/tokens", post(issue_user_token))
        .route("/assist/api/admin/users/:id/tokens/", post(issue_user_token))
        .route(
            "/assist/api/admin/users/:id/tokens/:token_id",
            delete(revoke_user_token),
        )
        // ── 我的 token（任意已登录用户） ──
        .route("/assist/api/auth/tokens", get(list_my_tokens))
        .route("/assist/api/auth/tokens/", get(list_my_tokens))
        .route("/assist/api/auth/tokens", post(issue_my_token))
        .route("/assist/api/auth/tokens/", post(issue_my_token))
        .route("/assist/api/auth/tokens/:token_id", delete(revoke_my_token))
}

// ── 请求体 ──────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct CreateUserBody {
    username: String,
    #[serde(default)]
    password: Option<String>,
    #[serde(default)]
    display_name: String,
    #[serde(default)]
    role: String,
    #[serde(default)]
    status: String,
    #[serde(default)]
    mcp: bool,
}

#[derive(Debug, Deserialize)]
struct UpdateUserBody {
    #[serde(default)]
    display_name: Option<String>,
    #[serde(default)]
    role: Option<String>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    mcp: Option<bool>,
}

#[derive(Debug, Deserialize)]
struct ResetPasswordBody {
    password: String,
    /// 同时吊销该用户的全部 API token（默认 false：只吊销会话）
    #[serde(default)]
    revoke_tokens: bool,
}

#[derive(Debug, Deserialize)]
struct IssueTokenBody {
    #[serde(default)]
    label: String,
    #[serde(default)]
    mcp: bool,
}

#[derive(Debug, Deserialize)]
struct UpdateSettingsBody {
    #[serde(default)]
    allow_registration: Option<bool>,
    #[serde(default)]
    require_approval: Option<bool>,
    #[serde(default)]
    registration_role: Option<String>,
    #[serde(default)]
    session_ttl_hours: Option<i64>,
}

// ── 工具 ────────────────────────────────────────────────

/// admin 权限门（失败直接返回 403 响应）
fn require_admin(role: &Role) -> Result<(), Response> {
    auth::assert_role(role, &["admin"])
}

fn not_found(detail: &str) -> Response {
    (
        StatusCode::NOT_FOUND,
        Json(json!({"detail": detail})),
    )
        .into_response()
}

fn ok_users(users: &UserStore) -> Value {
    let list = users.list();
    json!({
        "count": list.len(),
        "pending": list.iter().filter(|u| u.is_pending()).count(),
        "users": list.iter().map(|u| u.to_public_json()).collect::<Vec<_>>(),
    })
}

// ── 用户管理（admin） ───────────────────────────────────

/// `GET /assist/api/admin/users` —— 用户列表（可按 `?status=pending` 过滤）
async fn list_users(
    Extension(role): Extension<Role>,
    Extension(users): Extension<Arc<UserStore>>,
    axum::extract::RawQuery(query): axum::extract::RawQuery,
) -> Response {
    if let Err(resp) = require_admin(&role) {
        return resp;
    }
    let mut payload = ok_users(&users);
    if let Some(status) = query.as_deref().and_then(|q| {
        q.split('&').find_map(|kv| {
            let (k, v) = kv.split_once('=')?;
            (k == "status").then(|| v.to_string())
        })
    }) {
        let filtered: Vec<Value> = payload["users"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .filter(|u| u["status"] == json!(status))
            .collect();
        payload["users"] = Value::Array(filtered);
    }
    Json(payload).into_response()
}

/// `POST /assist/api/admin/users` —— 直接创建用户（可指定角色与状态）
async fn create_user(
    Extension(role): Extension<Role>,
    Extension(users): Extension<Arc<UserStore>>,
    Json(body): Json<CreateUserBody>,
) -> Response {
    if let Err(resp) = require_admin(&role) {
        return resp;
    }

    let status = if body.status.trim().is_empty() {
        STATUS_ACTIVE.to_string()
    } else {
        body.status.trim().to_string()
    };

    match users.create(NewUser {
        username: body.username,
        password: body.password.filter(|p| !p.is_empty()),
        display_name: body.display_name,
        role: body.role,
        status,
        mcp: body.mcp,
        must_set_password: false,
    }) {
        Ok(user) => {
            tracing::info!(
                audit = "user_create",
                user = %user.username,
                role = %user.role,
                status = %user.status,
                "管理员创建用户"
            );
            (StatusCode::CREATED, Json(json!({"user": user.to_public_json()}))).into_response()
        }
        Err(e) => e.into_response(),
    }
}

/// `GET /assist/api/admin/users/settings` —— 注册策略 / 会话 TTL
async fn get_settings(
    Extension(role): Extension<Role>,
    Extension(users): Extension<Arc<UserStore>>,
) -> Response {
    if let Err(resp) = require_admin(&role) {
        return resp;
    }
    let section = users.section();
    Json(json!({
        "allow_registration": section.allow_registration,
        "require_approval": section.require_approval,
        "registration_role": section.registration_role,
        "session_ttl_hours": section.session_ttl_hours,
    }))
    .into_response()
}

/// `PUT /assist/api/admin/users/settings` —— 修改注册策略 / 会话 TTL
async fn update_settings(
    Extension(role): Extension<Role>,
    Extension(users): Extension<Arc<UserStore>>,
    Extension(sessions): Extension<Arc<SessionStore>>,
    Json(body): Json<UpdateSettingsBody>,
) -> Response {
    if let Err(resp) = require_admin(&role) {
        return resp;
    }
    match users.update_settings(
        body.allow_registration,
        body.require_approval,
        body.registration_role.as_deref(),
        body.session_ttl_hours,
    ) {
        Ok(section) => {
            // TTL 立即作用于后续签发的会话与续期
            sessions.set_ttl_hours(section.session_ttl_hours);
            tracing::info!(
                audit = "user_settings_update",
                allow_registration = section.allow_registration,
                require_approval = section.require_approval,
                registration_role = %section.registration_role,
                session_ttl_hours = section.session_ttl_hours,
                "用户策略已更新"
            );
            Json(json!({
                "allow_registration": section.allow_registration,
                "require_approval": section.require_approval,
                "registration_role": section.registration_role,
                "session_ttl_hours": section.session_ttl_hours,
            }))
            .into_response()
        }
        Err(e) => e.into_response(),
    }
}

/// `PUT /assist/api/admin/users/:id` —— 改角色 / 状态 / 备注名 / mcp
async fn update_user(
    Extension(role): Extension<Role>,
    Extension(users): Extension<Arc<UserStore>>,
    Extension(sessions): Extension<Arc<SessionStore>>,
    Path(id): Path<String>,
    Json(body): Json<UpdateUserBody>,
) -> Response {
    if let Err(resp) = require_admin(&role) {
        return resp;
    }

    let patch = UserPatch {
        display_name: body.display_name,
        role: body.role,
        status: body.status,
        mcp: body.mcp,
    };

    match users.update(&id, &patch) {
        Ok(user) => {
            // 禁用 → 立即踢掉该用户所有会话
            if !user.is_active() {
                let n = sessions.revoke_user(&user.id);
                tracing::info!(
                    audit = "user_disable",
                    user = %user.username,
                    revoked_sessions = n,
                    "用户已被禁用，会话已撤销"
                );
            }
            tracing::info!(
                audit = "user_update",
                user = %user.username,
                role = %user.role,
                status = %user.status,
                mcp = user.mcp,
                "用户信息已更新"
            );
            Json(json!({"user": user.to_public_json()})).into_response()
        }
        Err(e) => e.into_response(),
    }
}

/// `POST /assist/api/admin/users/:id/approve` —— 审核通过（pending → active）
async fn approve_user(
    Extension(role): Extension<Role>,
    Extension(users): Extension<Arc<UserStore>>,
    Path(id): Path<String>,
) -> Response {
    if let Err(resp) = require_admin(&role) {
        return resp;
    }
    let patch = UserPatch {
        status: Some(STATUS_ACTIVE.to_string()),
        ..Default::default()
    };
    match users.update(&id, &patch) {
        Ok(user) => {
            tracing::info!(audit = "user_approve", user = %user.username, "用户审核通过");
            Json(json!({"user": user.to_public_json()})).into_response()
        }
        Err(e) => e.into_response(),
    }
}

/// `POST /assist/api/admin/users/:id/password` —— 管理员重置密码
async fn reset_password(
    Extension(role): Extension<Role>,
    Extension(users): Extension<Arc<UserStore>>,
    Extension(sessions): Extension<Arc<SessionStore>>,
    Path(id): Path<String>,
    Json(body): Json<ResetPasswordBody>,
) -> Response {
    if let Err(resp) = require_admin(&role) {
        return resp;
    }
    match users.set_password(&id, &body.password) {
        Ok(user) => {
            let revoked = sessions.revoke_user(&user.id);
            let revoked_tokens = if body.revoke_tokens {
                users.revoke_all_api_tokens(&user.id).unwrap_or(0)
            } else {
                0
            };
            tracing::info!(
                audit = "password_reset",
                user = %user.username,
                revoked_sessions = revoked,
                revoked_tokens,
                "管理员重置了用户密码"
            );
            Json(json!({
                "ok": true,
                "user": user.to_public_json(),
                "revoked_sessions": revoked,
                "revoked_tokens": revoked_tokens,
            }))
            .into_response()
        }
        Err(e) => e.into_response(),
    }
}

/// `DELETE /assist/api/admin/users/:id` —— 删除用户（级联撤销会话与 token）
async fn delete_user(
    Extension(role): Extension<Role>,
    Extension(users): Extension<Arc<UserStore>>,
    Extension(sessions): Extension<Arc<SessionStore>>,
    Path(id): Path<String>,
) -> Response {
    if let Err(resp) = require_admin(&role) {
        return resp;
    }
    match users.delete(&id) {
        Ok(user) => {
            let revoked = sessions.revoke_user(&user.id);
            tracing::warn!(
                audit = "user_delete",
                user = %user.username,
                revoked_sessions = revoked,
                "用户已删除"
            );
            Json(json!({"ok": true, "username": user.username})).into_response()
        }
        Err(e) => e.into_response(),
    }
}

// ── 用户的 API / MCP token（admin 视角） ─────────────────

/// `GET /assist/api/admin/users/:id/tokens`
async fn list_user_tokens(
    Extension(role): Extension<Role>,
    Extension(users): Extension<Arc<UserStore>>,
    Path(id): Path<String>,
) -> Response {
    if let Err(resp) = require_admin(&role) {
        return resp;
    }
    match users.find_by_id(&id) {
        Some(user) => Json(json!({
            "tokens": user.api_tokens.iter().map(|t| t.to_public_json()).collect::<Vec<_>>(),
        }))
        .into_response(),
        None => not_found("用户不存在"),
    }
}

/// `POST /assist/api/admin/users/:id/tokens` —— 签发 token（明文只返回一次）
async fn issue_user_token(
    Extension(role): Extension<Role>,
    Extension(users): Extension<Arc<UserStore>>,
    Path(id): Path<String>,
    Json(body): Json<IssueTokenBody>,
) -> Response {
    if let Err(resp) = require_admin(&role) {
        return resp;
    }
    // 用户被禁用时不允许签发新凭据
    let user = match users.find_by_id(&id) {
        Some(u) => u,
        None => return not_found("用户不存在"),
    };
    if body.mcp && !user.mcp {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"detail": format!(
                "用户 {} 未被授予 MCP 权限，请先在用户信息里开启 mcp",
                user.username
            )})),
        )
            .into_response();
    }
    match users.issue_api_token(&id, &body.label, body.mcp) {
        Ok((plain, token)) => {
            tracing::info!(
                audit = "token_issue",
                user_id = %id,
                token_id = %token.id,
                mcp = token.mcp,
                "管理员为用户签发 token"
            );
            (
                StatusCode::CREATED,
                Json(json!({
                    "token": plain,
                    "meta": token.to_public_json(),
                    "detail": "请立即保存：该 token 只显示这一次",
                })),
            )
                .into_response()
        }
        Err(e) => e.into_response(),
    }
}

/// `DELETE /assist/api/admin/users/:id/tokens/:token_id`
async fn revoke_user_token(
    Extension(role): Extension<Role>,
    Extension(users): Extension<Arc<UserStore>>,
    Path((id, token_id)): Path<(String, String)>,
) -> Response {
    if let Err(resp) = require_admin(&role) {
        return resp;
    }
    match users.revoke_api_token(&id, &token_id) {
        Ok(true) => {
            tracing::info!(audit = "token_revoke", user_id = %id, %token_id, "token 已撤销");
            Json(json!({"ok": true})).into_response()
        }
        Ok(false) => not_found("token 不存在"),
        Err(e) => e.into_response(),
    }
}

// ── 我的 token（自助） ──────────────────────────────────

/// `GET /assist/api/auth/tokens` —— 查看自己的 token 元数据
async fn list_my_tokens(
    Extension(users): Extension<Arc<UserStore>>,
    Extension(principal): Extension<Principal>,
) -> Response {
    match users.find_by_id(&principal.user_id) {
        Some(user) => Json(json!({
            "tokens": user.api_tokens.iter().map(|t| t.to_public_json()).collect::<Vec<_>>(),
            "mcp_allowed": user.mcp,
        }))
        .into_response(),
        None => not_found("用户不存在"),
    }
}

/// `POST /assist/api/auth/tokens` —— 为自己签发 token（用于 Zotero / MCP 客户端）
async fn issue_my_token(
    Extension(users): Extension<Arc<UserStore>>,
    Extension(principal): Extension<Principal>,
    Json(body): Json<IssueTokenBody>,
) -> Response {
    let Some(user) = users.find_by_id(&principal.user_id) else {
        return not_found("用户不存在");
    };
    if body.mcp && !user.mcp {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({"detail": "当前账号未被授予 MCP 权限，请先联系管理员开启"})),
        )
            .into_response();
    }

    let label = if body.label.trim().is_empty() {
        "personal".to_string()
    } else {
        body.label
    };
    match users.issue_api_token(&user.id, &label, body.mcp && user.mcp) {
        Ok((plain, token)) => {
            tracing::info!(
                audit = "token_issue_self",
                user = %user.username,
                token_id = %token.id,
                mcp = token.mcp,
                "用户自助签发 token"
            );
            (
                StatusCode::CREATED,
                Json(json!({
                    "token": plain,
                    "meta": token.to_public_json(),
                    "detail": "请立即保存：该 token 只显示这一次",
                })),
            )
                .into_response()
        }
        Err(e) => e.into_response(),
    }
}

/// `DELETE /assist/api/auth/tokens/:token_id` —— 撤销自己的 token
async fn revoke_my_token(
    Extension(users): Extension<Arc<UserStore>>,
    Extension(principal): Extension<Principal>,
    Path(token_id): Path<String>,
) -> Response {
    match users.revoke_api_token(&principal.user_id, &token_id) {
        Ok(true) => {
            tracing::info!(
                audit = "token_revoke_self",
                user = %principal.username,
                %token_id,
                "用户撤销了自己的 token"
            );
            Json(json!({"ok": true})).into_response()
        }
        Ok(false) => not_found("token 不存在"),
        Err(e) => e.into_response(),
    }
}
