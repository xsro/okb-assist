//! 认证路由：初始化 / 注册 / 登录 / 登出 / 当前用户 / 改密 / 状态 / 兼容检查。
//!
//! 路径都在 `/assist/api/auth/*` 下。其中 `status`、`check`、`login`、`register`、
//! `bootstrap` 由 `main.rs::token_middleware` 放行（无需凭证），其余需要有效会话或
//! API token。
//!
//! 凭证模型：
//! - 密码登录 → 签发服务端会话 token（`sess_...`，见 [`crate::session`]）；
//! - Token 登录 → 直接用会话 token 或用户的 API token（`okb_...`）访问任意接口；
//! - 首次初始化 → 前端初始化页调 `bootstrap` 创建首个管理员（不再有 `system.json` 全局 token）。

use std::sync::{Arc, OnceLock};
use std::time::Duration as StdDuration;

use axum::extract::{ConnectInfo, Extension};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Json, Response};
use axum::routing::{get, post};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::auth::{self, AttemptLimiter, Principal, Role};
use crate::session::{NewSession, SessionStore};
use crate::users::{self, NewUser, User, UserStore, STATUS_ACTIVE};

/// 登录尝试限流：每 key（IP / 用户名）5 分钟内最多 10 次
fn login_limiter() -> &'static AttemptLimiter {
    static LIMITER: OnceLock<AttemptLimiter> = OnceLock::new();
    LIMITER.get_or_init(|| AttemptLimiter::new(10, StdDuration::from_secs(300)))
}

/// 注册限流：每 IP 10 分钟内最多 5 次
fn register_limiter() -> &'static AttemptLimiter {
    static LIMITER: OnceLock<AttemptLimiter> = OnceLock::new();
    LIMITER.get_or_init(|| AttemptLimiter::new(5, StdDuration::from_secs(600)))
}

pub fn router() -> axum::Router<()> {
    axum::Router::new()
        .route("/assist/api/auth/status", get(status))
        .route("/assist/api/auth/status/", get(status))
        .route("/assist/api/auth/bootstrap", post(bootstrap))
        .route("/assist/api/auth/bootstrap/", post(bootstrap))
        .route("/assist/api/auth/register", post(register))
        .route("/assist/api/auth/register/", post(register))
        .route("/assist/api/auth/login", post(login))
        .route("/assist/api/auth/login/", post(login))
        .route("/assist/api/auth/logout", post(logout))
        .route("/assist/api/auth/logout/", post(logout))
        .route("/assist/api/auth/me", get(me))
        .route("/assist/api/auth/me/", get(me))
        .route("/assist/api/auth/password", post(change_password))
        .route("/assist/api/auth/password/", post(change_password))
        .route("/assist/api/auth/check", get(check))
        .route("/assist/api/auth/check/", get(check))
}

// ── 请求体 ──────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct BootstrapBody {
    #[serde(default)]
    username: Option<String>,
    password: String,
    #[serde(default)]
    display_name: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RegisterBody {
    username: String,
    password: String,
    #[serde(default)]
    display_name: String,
}

#[derive(Debug, Deserialize)]
struct LoginBody {
    username: String,
    password: String,
}

#[derive(Debug, Deserialize)]
struct ChangePasswordBody {
    #[serde(default)]
    old_password: String,
    new_password: String,
}

// ── 工具 ────────────────────────────────────────────────

/// 角色 → 前端权限位（保持与旧 `/auth/check` 完全一致的形状，前端零改造）
fn permissions_for(role: &Role, mcp: bool) -> Value {
    let admin = *role == Role::Admin;
    json!({
        "view_document": true,
        "view_pdf": admin || *role == Role::ViewOnly,
        "view_markdown": admin || *role == Role::ViewOnly || *role == Role::ViewUpload,
        "edit": admin,
        "delete": admin,
        "admin": admin,
        "config": admin,
        "upload": admin || *role == Role::ViewUpload,
        "pipeline": admin,
        "mcp": mcp,
    })
}

/// 取客户端 IP（优先 `X-Forwarded-For`，回退 TCP 对端）
fn client_ip(headers: &HeaderMap, addr: Option<ConnectInfo<std::net::SocketAddr>>) -> String {
    if let Some(ip) = headers
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(',').next())
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        return ip.to_string();
    }
    match addr {
        Some(ConnectInfo(a)) => a.ip().to_string(),
        None => "unknown".to_string(),
    }
}

fn user_agent(headers: &HeaderMap) -> Option<String> {
    headers
        .get(axum::http::header::USER_AGENT)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string())
}

fn err(status: StatusCode, detail: impl Into<String>) -> Response {
    (status, Json(json!({"detail": detail.into()}))).into_response()
}

/// 登录失败统一文案（不区分「用户不存在」与「密码错误」，避免用户名枚举）
fn login_failed() -> Response {
    err(StatusCode::UNAUTHORIZED, "用户名或密码错误")
}

/// 签发会话并返回登录响应
fn issue_login_response(
    users: &UserStore,
    sessions: &SessionStore,
    user: &User,
    ip: String,
    ua: Option<String>,
) -> Response {
    // 会话 TTL 跟随配置（`users.session_ttl_hours`，默认 168 小时）
    sessions.set_ttl_hours(users.section().session_ttl_hours);

    let role = user.role_enum().unwrap_or(Role::ViewOnly);
    let (token, session) = sessions.issue(NewSession {
        user_id: user.id.clone(),
        username: user.username.clone(),
        role: role.clone(),
        mcp: user.mcp,
        client_ip: Some(ip.clone()),
        user_agent: ua,
    });
    tracing::info!(
        audit = "login",
        user = %user.username,
        role = %role.as_str(),
        %ip,
        expires_at = %session.expires_at.to_rfc3339(),
        "登录成功，已签发会话"
    );

    // 浏览器直接导航到 `static_dirs` 挂载的静态目录时无法携带 `X-Token`，
    // 因此登录成功顺带下发一个仅供静态目录使用的会话 cookie（API 不读它）。
    let cookie = crate::static_files::session_cookie_header(
        &token,
        users.section().session_ttl_hours as i64,
    );

    let mut response = Json(json!({
        "token": token,
        "expires_at": session.expires_at.to_rfc3339(),
        "role": role.as_str(),
        "permissions": permissions_for(&role, user.mcp),
        "must_set_password": user.must_set_password,
        "user": user.to_public_json(),
    }))
    .into_response();
    if let Some(cookie) = cookie {
        response
            .headers_mut()
            .insert(axum::http::header::SET_COOKIE, cookie);
    }
    response
}

// ── 状态 ────────────────────────────────────────────────

/// `GET /assist/api/auth/status` —— 无需凭证，供登录页决定展示什么。
async fn status(Extension(users): Extension<Arc<UserStore>>) -> Json<Value> {
    let section = users.section();
    Json(json!({
        "initialized": users.active_admin_count() > 0,
        "user_count": section.items.len(),
        "pending_count": section.pending_count(),
        "allow_registration": section.allow_registration,
        "require_approval": section.require_approval,
        "registration_role": section.registration_role,
        "session_ttl_hours": section.session_ttl_hours,
        "version": section.version,
    }))
}

// ── 初始化（首次启动） ──────────────────────────────────

/// `POST /assist/api/auth/bootstrap` —— 仅在「没有任何管理员」时可用，创建首个 admin。
async fn bootstrap(
    Extension(users): Extension<Arc<UserStore>>,
    Extension(sessions): Extension<Arc<SessionStore>>,
    headers: HeaderMap,
    addr: Option<ConnectInfo<std::net::SocketAddr>>,
    Json(body): Json<BootstrapBody>,
) -> Response {
    if users.active_admin_count() > 0 {
        return err(StatusCode::CONFLICT, "系统已初始化，请直接登录");
    }

    let username = body
        .username
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("admin")
        .to_string();

    let created = users.create(NewUser {
        username,
        password: Some(body.password),
        display_name: body.display_name.unwrap_or_else(|| "管理员".to_string()),
        role: "admin".to_string(),
        status: STATUS_ACTIVE.to_string(),
        mcp: true,
        must_set_password: false,
    });

    match created {
        Ok(user) => {
            tracing::warn!(
                audit = "bootstrap",
                user = %user.username,
                ip = %client_ip(&headers, addr),
                "首次初始化：已创建管理员账号"
            );
            issue_login_response(&users, &sessions, &user, client_ip(&headers, addr), user_agent(&headers))
        }
        Err(e) => e.into_response(),
    }
}

// ── 注册 ────────────────────────────────────────────────

/// `POST /assist/api/auth/register` —— 开放注册（受 `users.allow_registration` 控制）。
///
/// 注册用户角色取 `users.registration_role`（不允许 admin），`require_approval = true`
/// 时状态为 `pending`，需管理员在用户管理页启用后才能登录。
async fn register(
    Extension(users): Extension<Arc<UserStore>>,
    headers: HeaderMap,
    addr: Option<ConnectInfo<std::net::SocketAddr>>,
    Json(body): Json<RegisterBody>,
) -> Response {
    let section = users.section();
    if !section.allow_registration {
        return err(StatusCode::FORBIDDEN, "当前未开放注册");
    }

    let ip = client_ip(&headers, addr);
    let ip_key = format!("ip:{}", ip);
    if !register_limiter().allow(&ip_key) {
        return err(StatusCode::TOO_MANY_REQUESTS, "注册过于频繁，请稍后再试");
    }
    register_limiter().hit(&ip_key);

    let status = if section.require_approval {
        users::STATUS_PENDING.to_string()
    } else {
        STATUS_ACTIVE.to_string()
    };

    let created = users.create(NewUser {
        username: body.username,
        password: Some(body.password),
        display_name: body.display_name,
        role: section.registration_role.clone(),
        status: status.clone(),
        // 新用户默认不给 MCP 权限（管理员可后续开启）
        mcp: false,
        must_set_password: false,
    });

    match created {
        Ok(user) => {
            tracing::info!(
                audit = "register",
                user = %user.username,
                role = %user.role,
                status = %user.status,
                %ip,
                "新用户注册"
            );
            let mut payload = json!({
                "user": user.to_public_json(),
                "pending_approval": user.status == users::STATUS_PENDING,
            });
            payload["detail"] = json!(if user.status == users::STATUS_PENDING {
                "注册成功，请等待管理员审核"
            } else {
                "注册成功，请登录"
            });
            (StatusCode::CREATED, Json(payload)).into_response()
        }
        Err(e) => e.into_response(),
    }
}

// ── 登录 / 登出 ─────────────────────────────────────────

/// `POST /assist/api/auth/login` —— 用户名 + 密码登录，返回会话 token。
async fn login(
    Extension(users): Extension<Arc<UserStore>>,
    Extension(sessions): Extension<Arc<SessionStore>>,
    headers: HeaderMap,
    addr: Option<ConnectInfo<std::net::SocketAddr>>,
    Json(body): Json<LoginBody>,
) -> Response {
    let ip = client_ip(&headers, addr);
    let ip_key = format!("ip:{}", ip);
    let user_key = format!("user:{}", body.username.trim().to_lowercase());

    if !login_limiter().allow(&ip_key) || !login_limiter().allow(&user_key) {
        tracing::warn!(audit = "login_blocked", user = %body.username, %ip, "登录尝试过于频繁");
        return err(
            StatusCode::TOO_MANY_REQUESTS,
            "尝试过于频繁，请稍后再试",
        );
    }

    let Some(user) = users.find_by_username(&body.username) else {
        // 等时校验，避免通过响应时间枚举用户名
        users::verify_dummy(&body.password);
        login_limiter().hit(&ip_key);
        login_limiter().hit(&user_key);
        tracing::warn!(audit = "login_failed", user = %body.username, %ip, reason = "unknown_user", "登录失败");
        return login_failed();
    };

    if let Some(until) = user.locked_until_at() {
        tracing::warn!(audit = "login_locked", user = %user.username, %ip, "账号锁定中，拒绝登录");
        return err(
            StatusCode::TOO_MANY_REQUESTS,
            format!(
                "账号因连续登录失败已锁定，请于 {} 后再试",
                until.to_rfc3339()
            ),
        );
    }

    if !user.is_active() {
        let detail = if user.is_pending() {
            "账号待管理员审核"
        } else {
            "账号已被禁用"
        };
        tracing::warn!(audit = "login_denied", user = %user.username, %ip, status = %user.status, "登录被拒绝");
        return err(StatusCode::FORBIDDEN, detail);
    }

    if !users::verify_password(&user, &body.password) {
        login_limiter().hit(&ip_key);
        login_limiter().hit(&user_key);
        let _ = users.record_login(&user.id, false);
        if let Some(updated) = users.find_by_id(&user.id) {
            if let Some(until) = updated.locked_until_at() {
                tracing::warn!(audit = "login_locked", user = %updated.username, %ip, "连续失败触发锁定");
                return err(
                    StatusCode::TOO_MANY_REQUESTS,
                    format!(
                        "连续登录失败过多，账号已锁定至 {}",
                        until.to_rfc3339()
                    ),
                );
            }
        }
        tracing::warn!(audit = "login_failed", user = %user.username, %ip, reason = "bad_password", "登录失败");
        return login_failed();
    }

    login_limiter().reset(&ip_key);
    login_limiter().reset(&user_key);
    let user = match users.record_login(&user.id, true) {
        Ok(u) => u,
        Err(e) => return e.into_response(),
    };

    issue_login_response(&users, &sessions, &user, ip, user_agent(&headers))
}

/// `POST /assist/api/auth/logout` —— 撤销当前会话 token。
async fn logout(
    Extension(sessions): Extension<Arc<SessionStore>>,
    headers: HeaderMap,
) -> Response {
    // 头部凭证优先；同时兼容浏览器静态目录下发的 cookie（否则登出后静态目录仍能打开）
    let token = auth::extract_token_from_headers(&headers)
        .or_else(|| crate::static_files::cookie_token(&headers));
    let revoked = token.map(|t| sessions.revoke(&t)).unwrap_or(false);
    tracing::info!(audit = "logout", revoked, "登出");
    let mut response = Json(json!({"ok": true, "revoked": revoked})).into_response();
    response.headers_mut().insert(
        axum::http::header::SET_COOKIE,
        crate::static_files::clear_cookie_header(),
    );
    response
}

// ── 当前用户 ────────────────────────────────────────────

/// `GET /assist/api/auth/me` —— 当前主体信息（含 token 元数据，不含任何哈希）。
async fn me(
    Extension(users): Extension<Arc<UserStore>>,
    Extension(principal): Extension<Principal>,
) -> Response {
    let role = principal.role.clone();
    let body = match users.find_by_id(&principal.user_id) {
        Some(user) => json!({
            "user": user.to_public_json(),
            "role": role.as_str(),
            "mcp": principal.mcp,
            "via": principal.via.as_str(),
            "permissions": permissions_for(&role, principal.mcp),
        }),
        // 受信任子网等「非用户」主体
        None if principal.user_id == "local" => json!({
            "user": {
                "id": "local",
                "username": "local",
                "display_name": "本地网络",
                "role": role.as_str(),
                "status": "active",
                "mcp": principal.mcp,
                "has_password": false,
                "must_set_password": false,
                "api_tokens": [],
            },
            "role": role.as_str(),
            "mcp": principal.mcp,
            "via": principal.via.as_str(),
            "permissions": permissions_for(&role, principal.mcp),
        }),
        None => {
            return err(StatusCode::UNAUTHORIZED, "用户不存在或已被删除");
        }
    };
    Json(body).into_response()
}

/// `POST /assist/api/auth/password` —— 修改自己的密码。
///
/// 已有密码时需提供 `old_password`；无密码账号（管理员创建但尚未设密）可直设。
/// 成功后撤销该用户的全部会话（遗留的 `bootstrap` token 由 `UserStore::set_password` 一并吊销）。
async fn change_password(
    Extension(users): Extension<Arc<UserStore>>,
    Extension(sessions): Extension<Arc<SessionStore>>,
    Extension(principal): Extension<Principal>,
    Json(body): Json<ChangePasswordBody>,
) -> Response {
    let Some(user) = users.find_by_id(&principal.user_id) else {
        return err(StatusCode::UNAUTHORIZED, "用户不存在或已被删除");
    };

    if user.password_hash.is_some() && !users::verify_password(&user, &body.old_password) {
        tracing::warn!(audit = "password_change_failed", user = %user.username, "原密码错误");
        return err(StatusCode::BAD_REQUEST, "原密码错误");
    }

    match users.set_password(&user.id, &body.new_password) {
        Ok(updated) => {
            let revoked = sessions.revoke_user(&user.id);
            tracing::info!(
                audit = "password_change",
                user = %updated.username,
                revoked_sessions = revoked,
                "密码已更新"
            );
            Json(json!({
                "ok": true,
                "revoked_sessions": revoked,
                "user": updated.to_public_json(),
            }))
            .into_response()
        }
        Err(e) => e.into_response(),
    }
}

// ── 兼容检查端点 ────────────────────────────────────────

/// `GET /assist/api/auth/check` —— 旧前端使用的角色/权限探测端点。
///
/// 无需中间件放行之外的处理：自己解析凭证，返回与旧版一致的 `{role, valid, permissions}`
/// 结构，并额外附带 `user` / `via`。
async fn check(
    Extension(users): Extension<Arc<UserStore>>,
    Extension(sessions): Extension<Arc<SessionStore>>,
    req: axum::extract::Request,
) -> Response {
    let token = auth::extract_token(&req);

    let principal = token
        .as_deref()
        .and_then(|t| auth::resolve_principal(t, &users, &sessions));

    let Some(principal) = principal else {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({
                "role": null,
                "valid": false,
                "detail": "无效的 token"
            })),
        )
            .into_response();
    };

    let user = users.find_by_id(&principal.user_id);
    let must_set_password = user.as_ref().map(|u| u.must_set_password).unwrap_or(false);

    // MCP 专属令牌不能用于 Web 登录（Token 登录）：前端会把它当会话用，
    // 后续所有 `/assist/api/*` 都会 401，所以直接在这里判无效。
    if !principal.api {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({
                "role": null,
                "valid": false,
                "code": "mcp_only_token",
                "detail": "该令牌仅用于 MCP，不能用于 Web 登录；请改用权限为 API 的令牌或账号密码登录",
            })),
        )
            .into_response();
    }

    // 刷新页面 / Token 登录也顺带下发静态目录 cookie（与登录一致）
    let cookie = token.as_deref().and_then(|t| {
        crate::static_files::session_cookie_header(t, users.section().session_ttl_hours as i64)
    });

    let mut response = Json(json!({
        "role": principal.role.as_str(),
        "valid": true,
        "permissions": permissions_for(&principal.role, principal.mcp),
        "mcp": principal.mcp,
        "via": principal.via.as_str(),
        "must_set_password": must_set_password,
        "user": user.map(|u| u.to_public_json()),
    }))
    .into_response();
    if let Some(cookie) = cookie {
        response
            .headers_mut()
            .insert(axum::http::header::SET_COOKIE, cookie);
    }
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn permissions_match_legacy_shape() {
        let admin = permissions_for(&Role::Admin, true);
        assert_eq!(admin["admin"], true);
        assert_eq!(admin["upload"], true);
        assert_eq!(admin["mcp"], true);

        let viewer = permissions_for(&Role::ViewOnly, false);
        assert_eq!(viewer["admin"], false);
        assert_eq!(viewer["upload"], false);
        assert_eq!(viewer["view_pdf"], true);
        assert_eq!(viewer["view_markdown"], true);
        assert_eq!(viewer["mcp"], false);

        let uploader = permissions_for(&Role::ViewUpload, true);
        assert_eq!(uploader["upload"], true);
        assert_eq!(uploader["view_pdf"], false);
        assert_eq!(uploader["view_markdown"], true);
        assert_eq!(uploader["edit"], false);
    }

    #[test]
    fn client_ip_prefers_forwarded_header() {
        let mut headers = HeaderMap::new();
        headers.insert(
            "x-forwarded-for",
            "203.0.113.9, 10.0.0.1".parse().unwrap(),
        );
        assert_eq!(client_ip(&headers, None), "203.0.113.9");
        // 无头信息且无 ConnectInfo 时回退 unknown
        assert_eq!(client_ip(&HeaderMap::new(), None), "unknown");
    }
}
