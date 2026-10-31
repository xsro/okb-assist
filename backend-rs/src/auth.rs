//! 认证与授权模块。
//!
//! 提供三部分能力：
//!
//! 1. **角色判定**（历史接口）：[`Role`] + [`assert_role`]，40+ 处处理器继续用
//!    `Extension<Role>` 判定权限，升级用户体系后签名与语义均未变化。
//! 2. **凭证解析**（新模型）：[`extract_token`] 取凭证，[`resolve_principal`] 解析成
//!    [`Principal`]（服务端会话 `sess_...` → 用户 API token `okb_...`）。
//! 3. **初始化判定**：[`is_initialized`]（是否存在启用中的管理员）。
//!
//! 首个管理员由前端初始化页调用 `POST /assist/api/auth/bootstrap` 创建；
//! 不再从 `system.json` 接管任何全局 token。
//!
//! 用户数据与会话分别由 [`crate::users`] 与 [`crate::session`] 管理。

use axum::http::HeaderMap;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Json, Response};
use serde_json::json;

/// 用户角色
#[derive(Clone, Debug, PartialEq)]
pub enum Role {
    Admin,
    ViewOnly,
    ViewUpload,
}

impl Role {
    pub fn as_str(&self) -> &'static str {
        match self {
            Role::Admin => "admin",
            Role::ViewOnly => "view-only",
            Role::ViewUpload => "view-upload",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "admin" => Some(Role::Admin),
            "view-only" => Some(Role::ViewOnly),
            "view-upload" => Some(Role::ViewUpload),
            _ => None,
        }
    }
}

/// 检查当前角色是否有权限访问。
/// `required` 为允许访问的角色列表。
pub fn assert_role(role: &Role, required: &[&str]) -> Result<(), Response> {
    if required.contains(&role.as_str()) {
        Ok(())
    } else {
        Err((
            StatusCode::FORBIDDEN,
            Json(json!({"detail": "权限不足"})),
        )
            .into_response())
    }
}

// ── 会话归属 ────────────────────────────────────────────

/// 会话归属标识。
///
/// **不回传给前端**，仅用于归属比对：
/// - `u:<user_id>`：登录用户（同一人换浏览器仍能看到自己的会话历史）；
/// - `local`：免鉴权来源（受信任子网、图片 URL 等）；
/// - `t:<sha256 前 32 位>`：旧模型（由 token 派生），仅用于兼容历史会话文件。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OwnerKey(String);

impl OwnerKey {
    /// 由 token 派生归属标识（旧模型，保留用于读取历史会话文件）
    pub fn from_token(token: &str) -> Self {
        use sha2::{Digest, Sha256};
        let digest = Sha256::digest(token.as_bytes());
        let hex = hex::encode(digest);
        Self(format!("t:{}", &hex[..32]))
    }

    /// 免鉴权请求的归属标识
    pub fn local() -> Self {
        Self("local".to_string())
    }

    /// 由**用户 id** 派生归属标识（新模型：会话历史按人归属）
    ///
    /// 同一用户换浏览器 / 换 token 仍能看到自己的会话；不同用户互不可见。
    pub fn from_user(user_id: &str) -> Self {
        Self(format!("u:{}", user_id))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

// ── 认证主体（新模型） ──────────────────────────────────

/// 凭证来源，便于审计与排障
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CredentialKind {
    /// 密码登录签发的服务端会话 token（`sess_...`）
    Session,
    /// 用户长期凭证（`okb_...`，在「账户」页签发）
    ApiToken,
    /// 受信任子网这类「非用户」主体
    LocalSubnet,
}

impl CredentialKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            CredentialKind::Session => "session",
            CredentialKind::ApiToken => "api_token",
            CredentialKind::LocalSubnet => "local_subnet",
        }
    }
}

/// 已认证的主体。
///
/// 同一个请求既会注入 [`Principal`]（需要用户身份时使用），也会注入旧的 [`Role`]
/// （兼容既有 `Extension<Role>` 处理器，保持 `assert_role` 判定不变）。
#[derive(Clone, Debug)]
pub struct Principal {
    pub user_id: String,
    pub username: String,
    pub role: Role,
    /// 是否允许访问 MCP（已按「用户 mcp 布尔 × 令牌作用域」算好的最终值）
    pub mcp: bool,
    /// 是否允许访问 `/assist/api/*`（MCP 专属令牌为 false）
    pub api: bool,
    pub via: CredentialKind,
    /// 通过 API token 认证时的 token id
    pub token_id: Option<String>,
}

impl Principal {
    /// 受信任子网这类「非用户」主体（同时允许 API 与 MCP）
    pub fn local(role: Role, mcp: bool) -> Self {
        Self {
            user_id: "local".to_string(),
            username: "local".to_string(),
            role,
            mcp,
            api: true,
            via: CredentialKind::LocalSubnet,
            token_id: None,
        }
    }

    /// 会话归属标识（会话历史按人隔离）
    pub fn owner_key(&self) -> OwnerKey {
        if self.user_id == "local" {
            OwnerKey::local()
        } else {
            OwnerKey::from_user(&self.user_id)
        }
    }
}

// ── 凭证提取 ────────────────────────────────────────────

/// 从请求头提取凭证：`X-Token` 优先，其次 `Authorization: Bearer <token>`。
pub fn extract_token_from_headers(headers: &HeaderMap) -> Option<String> {
    if let Some(raw) = headers.get("x-token").and_then(|v| v.to_str().ok()) {
        let token = raw.trim();
        if !token.is_empty() {
            return Some(token.to_string());
        }
    }
    if let Some(raw) = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
    {
        let raw = raw.trim();
        let rest = raw
            .strip_prefix("Bearer ")
            .or_else(|| raw.strip_prefix("bearer "))?;
        let token = rest.trim();
        if !token.is_empty() {
            return Some(token.to_string());
        }
    }
    None
}

/// 从查询串提取 `?token=...`（兼容旧前端的 iframe/img URL 传参方式）
pub fn extract_token_from_query(query: Option<&str>) -> Option<String> {
    for pair in query?.split('&') {
        let Some((k, v)) = pair.split_once('=') else {
            continue;
        };
        if k == "token" {
            let token = v.trim();
            if !token.is_empty() {
                return Some(token.to_string());
            }
        }
    }
    None
}

/// 从请求中提取凭证（头优先，其次 `?token=`）
pub fn extract_token<B>(req: &axum::extract::Request<B>) -> Option<String> {
    extract_token_from_headers(req.headers())
        .or_else(|| extract_token_from_query(req.uri().query()))
}

// ── 凭证解析 ────────────────────────────────────────────

/// 解析访问凭证 → 认证主体。
///
/// 顺序：服务端会话（`sess_`） → 用户 API token（按 sha256 匹配）。
/// 无论哪条路径，都以**用户表当前状态**为准：被禁用/删除的用户即使持有有效会话也
/// 立即失效（并顺手清掉该会话）。
pub fn resolve_principal(
    token: &str,
    users: &crate::users::UserStore,
    sessions: &crate::session::SessionStore,
) -> Option<Principal> {
    let token = token.trim();
    if token.is_empty() {
        return None;
    }

    // 1) 服务端会话
    if token.starts_with(crate::session::SESSION_PREFIX) {
        let session = sessions.validate(token)?;
        return match users.find_by_id(&session.user_id) {
            Some(user) if user.is_active() => {
                let role = user.role_enum()?;
                Some(Principal {
                    user_id: user.id,
                    username: user.username,
                    role,
                    // 会话（密码登录）等同于完整权限：API 可用，MCP 看用户开关
                    mcp: user.mcp,
                    api: true,
                    via: CredentialKind::Session,
                    token_id: None,
                })
            }
            // 用户已禁用/删除：会话立即作废
            _ => {
                sessions.revoke(token);
                None
            }
        };
    }

    // 2) 用户 API token
    let (user, api_token) = users.find_by_api_token(token)?;
    if !user.is_active() {
        return None;
    }
    let role = user.role_enum()?;
    Some(Principal {
        user_id: user.id,
        username: user.username,
        role,
        // 令牌作用域严格互斥：`api` 只能调 API，`mcp` 只能调 MCP；
        // MCP 还需用户本人开启 mcp。
        mcp: user.mcp && api_token.scope.allows_mcp(),
        api: api_token.scope.allows_api(),
        via: CredentialKind::ApiToken,
        token_id: Some(api_token.id),
    })
}

/// 系统是否已完成初始化：存在至少一个启用中的管理员。
///
/// 未初始化时除 `/assist/api/auth/{status,check,bootstrap}` 以外的接口一律拒绝。
pub fn is_initialized(users: &crate::users::UserStore) -> bool {
    users.active_admin_count() > 0
}

// ── 限流 ────────────────────────────────────────────────

/// 内存滑动窗口限流器（按 key 记录最近尝试时间）。
///
/// 用于登录与注册防暴破：key 可以是 `ip:<addr>`、`user:<name>` 等。
/// 不持久化，进程重启即清空（可接受：重启本身就是运维动作）。
pub struct AttemptLimiter {
    limit: usize,
    window: std::time::Duration,
    hits: std::sync::Mutex<std::collections::HashMap<String, Vec<std::time::Instant>>>,
}

impl AttemptLimiter {
    pub fn new(limit: usize, window: std::time::Duration) -> Self {
        Self {
            limit,
            window,
            hits: std::sync::Mutex::new(std::collections::HashMap::new()),
        }
    }

    /// 是否允许再尝试一次（不记次）
    pub fn allow(&self, key: &str) -> bool {
        let now = std::time::Instant::now();
        let mut guard = self.hits.lock().unwrap();
        let entry = guard.entry(key.to_string()).or_default();
        entry.retain(|t| now.duration_since(*t) < self.window);
        entry.len() < self.limit
    }

    /// 记录一次尝试
    pub fn hit(&self, key: &str) {
        let now = std::time::Instant::now();
        let mut guard = self.hits.lock().unwrap();
        let entry = guard.entry(key.to_string()).or_default();
        entry.retain(|t| now.duration_since(*t) < self.window);
        entry.push(now);
    }

    /// 清空某个 key 的记录（登录成功后调用）
    pub fn reset(&self, key: &str) {
        self.hits.lock().unwrap().remove(key);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn owner_key_is_stable_and_token_specific() {
        let a = OwnerKey::from_token("token-a");
        let b = OwnerKey::from_token("token-a");
        let c = OwnerKey::from_token("token-b");

        // 同一 token 稳定
        assert_eq!(a, b);
        // 不同 token 即使角色相同也不同归属
        assert_ne!(a, c);
        // 免鉴权请求单独一档
        assert_ne!(a, OwnerKey::local());
        assert_eq!(OwnerKey::local().as_str(), "local");

        // t: + 32 位 hex，不包含原始 token
        assert!(a.as_str().starts_with("t:"));
        assert_eq!(a.as_str().len(), 34);
        assert!(!a.as_str().contains("token-a"));
    }

    // ── 新模型：Principal / resolve_principal / 限流 ─────────

    use std::sync::Arc;

    use crate::config_manager::ConfigManager;
    use crate::session::{NewSession, SessionStore};
    use crate::users::{NewUser, UserStore};

    fn stores() -> (tempfile::TempDir, Arc<UserStore>, Arc<SessionStore>) {
        let dir = tempfile::tempdir().unwrap();
        let cm = Arc::new(ConfigManager::for_tests(
            dir.path().join("config.json"),
            dir.path().join("system.json"),
        ));
        (
            dir,
            Arc::new(UserStore::new(cm)),
            Arc::new(SessionStore::with_default_ttl()),
        )
    }

    fn make_user(users: &UserStore, name: &str, role: &str, mcp: bool) -> crate::users::User {
        users
            .create(NewUser {
                username: name.to_string(),
                password: Some("password-123".to_string()),
                role: role.to_string(),
                mcp,
                ..Default::default()
            })
            .unwrap()
    }

    fn issue_for(sessions: &SessionStore, user: &crate::users::User) -> String {
        sessions
            .issue(NewSession {
                user_id: user.id.clone(),
                username: user.username.clone(),
                role: user.role_enum().unwrap(),
                mcp: user.mcp,
                client_ip: None,
                user_agent: None,
            })
            .0
    }

    #[test]
    fn owner_key_from_user_is_distinct() {
        let a = OwnerKey::from_user("u_1");
        assert_eq!(a.as_str(), "u:u_1");
        assert_ne!(a, OwnerKey::from_user("u_2"));
        assert_ne!(a, OwnerKey::local());
        assert_ne!(a, OwnerKey::from_token("u_1"));
    }

    #[test]
    fn resolve_principal_via_session() {
        let (_dir, users, sessions) = stores();
        let user = make_user(&users, "alice", "view-upload", true);
        let token = issue_for(&sessions, &user);

        let principal = resolve_principal(&token, &users, &sessions).expect("会话应能解析");
        assert_eq!(principal.user_id, user.id);
        assert_eq!(principal.username, "alice");
        assert_eq!(principal.role, Role::ViewUpload);
        assert!(principal.mcp);
        // 会话（密码登录）等同完整权限
        assert!(principal.api);
        assert_eq!(principal.via, CredentialKind::Session);
        assert_eq!(principal.owner_key().as_str(), format!("u:{}", user.id));
    }

    #[test]
    fn resolve_principal_respects_token_scope() {
        let (_dir, users, sessions) = stores();
        // 作用域严格互斥：mcp 令牌不能调 API，api 令牌不能调 MCP
        let user = make_user(&users, "bob", "view-only", true);
        let (api_plain, _) = users
            .issue_api_token(&user.id, "zotero", crate::users::TokenScope::Api)
            .unwrap();
        let api_principal = resolve_principal(&api_plain, &users, &sessions).expect("token 应能解析");
        assert_eq!(api_principal.via, CredentialKind::ApiToken);
        assert!(api_principal.token_id.is_some());
        assert!(api_principal.api, "api 令牌可用于 /assist/api/*");
        assert!(!api_principal.mcp, "api 令牌不能用于 MCP");

        let (mcp_plain, _) = users
            .issue_api_token(&user.id, "mcp", crate::users::TokenScope::Mcp)
            .unwrap();
        let mcp_principal = resolve_principal(&mcp_plain, &users, &sessions).unwrap();
        assert!(!mcp_principal.api, "mcp 令牌不能用于 /assist/api/*");
        assert!(mcp_principal.mcp);

        // mcp 令牌仍受用户级开关约束：用户 mcp = false → 拒绝访问 MCP
        let user2 = make_user(&users, "carol", "view-only", false);
        let (plain2, _) = users
            .issue_api_token(&user2.id, "agent", crate::users::TokenScope::Mcp)
            .unwrap();
        let p2 = resolve_principal(&plain2, &users, &sessions).unwrap();
        assert!(!p2.mcp);
        assert!(!p2.api);
    }

    #[test]
    fn disabled_user_invalidates_session_and_token() {
        let (_dir, users, sessions) = stores();
        let user = make_user(&users, "dave", "view-only", true);
        let token = issue_for(&sessions, &user);
        let (plain, _) = users
            .issue_api_token(&user.id, "t", crate::users::TokenScope::Mcp)
            .unwrap();

        users
            .update(
                &user.id,
                &crate::users::UserPatch {
                    status: Some(crate::users::STATUS_DISABLED.to_string()),
                    ..Default::default()
                },
            )
            .unwrap();

        assert!(resolve_principal(&token, &users, &sessions).is_none());
        assert!(resolve_principal(&plain, &users, &sessions).is_none());
        // 失效会话被顺手清掉
        assert_eq!(sessions.len(), 0);
    }

    #[test]
    fn unknown_and_empty_tokens_rejected() {
        let (_dir, users, sessions) = stores();
        assert!(resolve_principal("", &users, &sessions).is_none());
        assert!(resolve_principal("   ", &users, &sessions).is_none());
        assert!(resolve_principal("sess_notissued", &users, &sessions).is_none());
        assert!(resolve_principal("okb_notissued", &users, &sessions).is_none());
        assert!(resolve_principal("change-me", &users, &sessions).is_none());
    }

    #[test]
    fn is_initialized_tracks_active_admins() {
        let (_dir, users, _sessions) = stores();
        assert!(!is_initialized(&users));
        let reader = make_user(&users, "reader", "view-only", false);
        assert!(!is_initialized(&users), "仅普通用户不算已初始化");
        let _admin = make_user(&users, "root", "admin", true);
        assert!(is_initialized(&users));
        users
            .update(
                &reader.id,
                &crate::users::UserPatch {
                    status: Some(crate::users::STATUS_DISABLED.to_string()),
                    ..Default::default()
                },
            )
            .unwrap();
        assert!(is_initialized(&users));
    }

    #[test]
    fn local_principal_uses_local_owner_key() {
        let p = Principal::local(Role::Admin, true);
        assert_eq!(p.owner_key(), OwnerKey::local());
        assert_eq!(p.via, CredentialKind::LocalSubnet);
    }

    #[test]
    fn extract_token_sources() {
        let mut headers = HeaderMap::new();
        headers.insert("x-token", "from-header".parse().unwrap());
        assert_eq!(
            extract_token_from_headers(&headers),
            Some("from-header".to_string())
        );

        let mut headers = HeaderMap::new();
        headers.insert("authorization", "Bearer tok-123".parse().unwrap());
        assert_eq!(
            extract_token_from_headers(&headers),
            Some("tok-123".to_string())
        );

        let mut headers = HeaderMap::new();
        headers.insert("authorization", "Basic abc".parse().unwrap());
        assert_eq!(extract_token_from_headers(&headers), None);

        assert_eq!(
            extract_token_from_query(Some("a=1&token=xyz&b=2")),
            Some("xyz".to_string())
        );
        assert_eq!(extract_token_from_query(Some("a=1")), None);
        assert_eq!(extract_token_from_query(None), None);
    }

    #[test]
    fn attempt_limiter_blocks_after_limit() {
        let limiter = AttemptLimiter::new(2, std::time::Duration::from_secs(60));
        assert!(limiter.allow("ip:1.2.3.4"));
        limiter.hit("ip:1.2.3.4");
        assert!(limiter.allow("ip:1.2.3.4"));
        limiter.hit("ip:1.2.3.4");
        assert!(!limiter.allow("ip:1.2.3.4"));
        // 不同 key 互不影响
        assert!(limiter.allow("ip:5.6.7.8"));
        // 重置后恢复
        limiter.reset("ip:1.2.3.4");
        assert!(limiter.allow("ip:1.2.3.4"));
    }
}
