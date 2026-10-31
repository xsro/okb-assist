//! 用户存储。
//!
//! 用户数据存放在 `config.json` 的 `users` 段（结构见 [`UsersSection`]），本模块是它
//! 唯一的读写入口：
//!
//! - 密码只存 **Argon2id PHC 哈希**（`$argon2id$v=19$m=19456,t=2,p=1$...`），永不回显；
//! - 长期凭证（API / MCP token）只存 **sha256 哈希**，明文只在签发时返回一次；
//! - 所有写操作走 [`ConfigManager::update_users_section`]：加写锁 → 从磁盘重读 →
//!   只改 `users` 段 → 临时文件 + rename 原子写，避免与 LLM/向量库配置保存互相覆盖；
//! - 维护「至少一个 active admin」不变量，防止把系统锁死。
//!
//! 会话 token 不在这里（见 [`crate::session`]）：会话是内存态，用户是持久化数据。

use std::sync::{Arc, OnceLock};

use argon2::password_hash::{rand_core::OsRng, SaltString};
use argon2::{Argon2, PasswordHash, PasswordHasher, PasswordVerifier};
use chrono::{DateTime, Duration, Utc};
use rand::RngCore;
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::auth::Role;
use crate::config_manager::ConfigManager;

/// 密码长度下限（字符）
pub const MIN_PASSWORD_LEN: usize = 8;
/// 密码长度上限（字符）
pub const MAX_PASSWORD_LEN: usize = 128;
/// API token 前缀（自动生成的 `okb_<32B base64url>`），便于识别与日志脱敏
pub const API_TOKEN_PREFIX: &str = "okb_";
/// 自定义令牌长度下限（字符）
pub const MIN_TOKEN_LEN: usize = 8;
/// 自定义令牌长度上限（字符）
pub const MAX_TOKEN_LEN: usize = 256;

/// 历史「引导期 token」的 label（旧版本会把 `system.json` 的 `token` 接管为该用户的一条
/// api_token）。现已不再产生此类令牌，保留仅为了兼容旧安装：该用户**首次设置密码成功后
/// 会自动吊销**遗留的这类令牌（见 [`UserStore::set_password`]）。
pub const BOOTSTRAP_TOKEN_LABEL: &str = "bootstrap";

pub const STATUS_PENDING: &str = "pending";
pub const STATUS_ACTIVE: &str = "active";
pub const STATUS_DISABLED: &str = "disabled";

/// 连续失败多少次后开始锁定
const LOCK_AFTER_FAILURES: u32 = 5;
/// 锁定时长上限
const MAX_LOCK_SECONDS: i64 = 3600;

// ── 错误 ────────────────────────────────────────────────

/// 用户模块错误（handler 层按变体映射 HTTP 状态码）
#[derive(Debug, thiserror::Error)]
pub enum UserError {
    #[error("用户名无效：仅允许 3-32 位字母、数字、下划线、点、短横线")]
    InvalidUsername,
    #[error("密码长度需在 {}–{} 位之间", MIN_PASSWORD_LEN, MAX_PASSWORD_LEN)]
    WeakPassword,
    #[error("用户名已存在")]
    UsernameTaken,
    #[error("用户不存在")]
    NotFound,
    #[error("角色无效：仅支持 admin / view-only / view-upload")]
    InvalidRole,
    #[error("状态无效：仅支持 pending / active / disabled")]
    InvalidStatus,
    #[error("至少需要保留一个启用中的管理员")]
    LastAdmin,
    #[error("账号已被锁定，请稍后再试")]
    Locked,
    /// 自定义令牌字符串不合法（长度 / 字符集）
    #[error("{0}")]
    InvalidToken(String),
    /// 该令牌字符串已被其他账号占用
    #[error("该令牌已被其他账号使用，请换一个")]
    TokenTaken,
    #[error("配置写入失败：{0}")]
    Config(String),
}

impl UserError {
    /// 建议的 HTTP 状态码
    pub fn status_code(&self) -> axum::http::StatusCode {
        use axum::http::StatusCode;
        match self {
            UserError::NotFound => StatusCode::NOT_FOUND,
            UserError::UsernameTaken => StatusCode::CONFLICT,
            UserError::LastAdmin => StatusCode::CONFLICT,
            UserError::TokenTaken => StatusCode::CONFLICT,
            UserError::Locked => StatusCode::TOO_MANY_REQUESTS,
            UserError::Config(_) => StatusCode::INTERNAL_SERVER_ERROR,
            _ => StatusCode::BAD_REQUEST,
        }
    }

    /// 转为 `{"detail": ...}` 响应（与其他路由的错误风格一致）
    pub fn into_response(self) -> axum::response::Response {
        use axum::response::IntoResponse;
        (
            self.status_code(),
            axum::Json(json!({"detail": self.to_string()})),
        )
            .into_response()
    }
}

// ── 状态 ────────────────────────────────────────────────

/// 用户状态
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UserStatus {
    /// 已注册，等待管理员审核
    Pending,
    Active,
    /// 已禁用：会话与 token 一并失效
    Disabled,
}

impl UserStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            UserStatus::Pending => STATUS_PENDING,
            UserStatus::Active => STATUS_ACTIVE,
            UserStatus::Disabled => STATUS_DISABLED,
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s.trim() {
            STATUS_PENDING => Some(UserStatus::Pending),
            STATUS_ACTIVE => Some(UserStatus::Active),
            STATUS_DISABLED => Some(UserStatus::Disabled),
            _ => None,
        }
    }
}

// ── 数据结构 ────────────────────────────────────────────

/// 个人令牌的权限作用域：**严格二选一**。
///
/// - [`TokenScope::Api`]：可调 `/assist/api/*`，**不能**调 MCP；
/// - [`TokenScope::Mcp`]：可调 `/assist/mcp`，**不能**调 `/assist/api/*`。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TokenScope {
    Api,
    Mcp,
}

impl Default for TokenScope {
    fn default() -> Self {
        // 缺字段时按最小权限：只给 API（MCP 需要用户显式选择）
        Self::Api
    }
}

impl TokenScope {
    pub fn as_str(&self) -> &'static str {
        match self {
            TokenScope::Api => "api",
            TokenScope::Mcp => "mcp",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s.trim().to_lowercase().as_str() {
            "api" => Some(TokenScope::Api),
            "mcp" => Some(TokenScope::Mcp),
            _ => None,
        }
    }

    /// 是否可用于 `/assist/api/*`
    pub fn allows_api(&self) -> bool {
        matches!(self, TokenScope::Api)
    }

    /// 是否可用于 MCP（还需用户本身开启 mcp）
    pub fn allows_mcp(&self) -> bool {
        matches!(self, TokenScope::Mcp)
    }
}

/// 用户的一条长期凭证（纯 API 或纯 MCP，二选一）。
///
/// 明文 token 只在签发响应里出现一次，磁盘上只有 `token_sha256`。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ApiToken {
    pub id: String,
    #[serde(default)]
    pub label: String,
    /// `sha256(明文 token)` 的小写 hex
    pub token_sha256: String,
    /// 权限作用域（api / mcp，互斥）
    #[serde(default)]
    pub scope: TokenScope,
    pub created_at: String,
    #[serde(default)]
    pub last_used_at: Option<String>,
}

impl ApiToken {
    /// 对外暴露的元数据（绝不含哈希）
    pub fn to_public_json(&self) -> Value {
        json!({
            "id": self.id,
            "label": self.label,
            "scope": self.scope.as_str(),
            "created_at": self.created_at,
            "last_used_at": self.last_used_at,
        })
    }
}

/// 单个用户
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct User {
    pub id: String,
    pub username: String,
    #[serde(default)]
    pub display_name: String,
    /// `admin` / `view-only` / `view-upload`（字符串存储，便于配置手改）
    pub role: String,
    /// `pending` / `active` / `disabled`
    pub status: String,
    /// 是否允许通过该用户的会话 / token 访问 MCP
    #[serde(default)]
    pub mcp: bool,
    /// Argon2id PHC 字符串；`None` 表示尚未设置密码（只能走 token 登录或等管理员重置）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub password_hash: Option<String>,
    /// 引导流程创建的管理员需要强制改密
    #[serde(default)]
    pub must_set_password: bool,
    #[serde(default)]
    pub api_tokens: Vec<ApiToken>,
    pub created_at: String,
    pub updated_at: String,
    #[serde(default)]
    pub last_login_at: Option<String>,
    /// 连续登录失败次数
    #[serde(default)]
    pub failed_logins: u32,
    /// 锁定截止时间（RFC3339）
    #[serde(default)]
    pub locked_until: Option<String>,
}

impl User {
    pub fn role_enum(&self) -> Option<Role> {
        Role::from_str(&self.role)
    }

    pub fn status_enum(&self) -> Option<UserStatus> {
        UserStatus::from_str(&self.status)
    }

    pub fn is_active(&self) -> bool {
        self.status == STATUS_ACTIVE
    }

    pub fn is_pending(&self) -> bool {
        self.status == STATUS_PENDING
    }

    pub fn is_admin(&self) -> bool {
        self.role == "admin"
    }

    /// 是否处于锁定窗口内
    pub fn locked_until_at(&self) -> Option<DateTime<Utc>> {
        let raw = self.locked_until.as_deref()?;
        let at = DateTime::parse_from_rfc3339(raw).ok()?.with_timezone(&Utc);
        if at > Utc::now() {
            Some(at)
        } else {
            None
        }
    }

    /// 对外暴露的用户信息（不含密码哈希与 token 哈希）
    pub fn to_public_json(&self) -> Value {
        json!({
            "id": self.id,
            "username": self.username,
            "display_name": self.display_name,
            "role": self.role,
            "status": self.status,
            "mcp": self.mcp,
            "has_password": self.password_hash.is_some(),
            "must_set_password": self.must_set_password,
            "created_at": self.created_at,
            "updated_at": self.updated_at,
            "last_login_at": self.last_login_at,
            "api_tokens": self.api_tokens.iter().map(ApiToken::to_public_json).collect::<Vec<_>>(),
        })
    }
}

/// `config.json` 的 `users` 段
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct UsersSection {
    #[serde(default = "default_version")]
    pub version: u32,
    /// 是否允许自助注册
    #[serde(default = "default_true")]
    pub allow_registration: bool,
    /// 注册后是否需要管理员审核（true → `status = pending`）
    #[serde(default = "default_true")]
    pub require_approval: bool,
    /// 自助注册用户的默认角色（不允许 admin）
    #[serde(default = "default_registration_role")]
    pub registration_role: String,
    /// 会话有效期（小时），滑动续期
    #[serde(default = "default_session_ttl_hours")]
    pub session_ttl_hours: i64,
    #[serde(default)]
    pub items: Vec<User>,
}

fn default_version() -> u32 {
    1
}

fn default_true() -> bool {
    true
}

fn default_registration_role() -> String {
    "view-only".to_string()
}

fn default_session_ttl_hours() -> i64 {
    crate::session::DEFAULT_TTL_HOURS
}

impl Default for UsersSection {
    fn default() -> Self {
        Self {
            version: default_version(),
            allow_registration: true,
            require_approval: true,
            registration_role: default_registration_role(),
            session_ttl_hours: default_session_ttl_hours(),
            items: Vec::new(),
        }
    }
}

impl UsersSection {
    pub fn find_by_username(&self, username: &str) -> Option<&User> {
        let needle = username.trim().to_lowercase();
        self.items
            .iter()
            .find(|u| u.username.to_lowercase() == needle)
    }

    pub fn find_by_id(&self, id: &str) -> Option<&User> {
        self.items.iter().find(|u| u.id == id)
    }

    /// 启用中的管理员数量（「至少一个 active admin」不变量的依据）
    pub fn active_admin_count(&self) -> usize {
        self.items
            .iter()
            .filter(|u| u.is_admin() && u.is_active())
            .count()
    }

    pub fn pending_count(&self) -> usize {
        self.items.iter().filter(|u| u.is_pending()).count()
    }
}

/// 创建用户的入参
#[derive(Clone, Debug, Default)]
pub struct NewUser {
    pub username: String,
    /// 留空则创建「无密码」账号（只能 Token 登录 / 等管理员设置密码）
    pub password: Option<String>,
    pub display_name: String,
    /// 留空默认 `view-only`
    pub role: String,
    /// 留空默认 `active`
    pub status: String,
    pub mcp: bool,
    pub must_set_password: bool,
}

/// 更新用户的入参（`None` 表示「不修改」）
#[derive(Clone, Debug, Default)]
pub struct UserPatch {
    pub display_name: Option<String>,
    pub role: Option<String>,
    pub status: Option<String>,
    pub mcp: Option<bool>,
}

// ── 哈希 / 校验工具 ─────────────────────────────────────

fn username_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"^[A-Za-z0-9_.-]{3,32}$").unwrap())
}

/// 校验用户名格式
pub fn validate_username(username: &str) -> Result<(), UserError> {
    if username_regex().is_match(username.trim()) {
        Ok(())
    } else {
        Err(UserError::InvalidUsername)
    }
}

/// 校验密码强度（仅长度，无复杂度要求，避免本地工具过于难用）
pub fn validate_password(password: &str) -> Result<(), UserError> {
    let n = password.chars().count();
    if n < MIN_PASSWORD_LEN || n > MAX_PASSWORD_LEN {
        Err(UserError::WeakPassword)
    } else {
        Ok(())
    }
}

/// 校验用户自定义的令牌字符串：
///
/// - 长度（字符）在 [`MIN_TOKEN_LEN`]–[`MAX_TOKEN_LEN`] 之间；
/// - 只允许可见 ASCII（`0x21..=0x7E`），因此不能包含空格、制表符、控制字符或非 ASCII。
///
/// 留空表示「自动生成」，由调用方处理（本函数不接受空串）。
pub fn validate_token_plaintext(token: &str) -> Result<(), UserError> {
    let n = token.chars().count();
    if n < MIN_TOKEN_LEN || n > MAX_TOKEN_LEN {
        return Err(UserError::InvalidToken(format!(
            "令牌长度需在 {}–{} 位之间（当前 {} 位）",
            MIN_TOKEN_LEN, MAX_TOKEN_LEN, n
        )));
    }
    if !token.chars().all(|c| c.is_ascii_graphic()) {
        return Err(UserError::InvalidToken(
            "令牌只能包含可见 ASCII 字符（不能有空格、中文或控制字符）".to_string(),
        ));
    }
    Ok(())
}

/// 生成 Argon2id 哈希（PHC 字符串）
pub fn hash_password(password: &str) -> Result<String, UserError> {
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|h| h.to_string())
        .map_err(|e| UserError::Config(format!("密码哈希失败：{e}")))
}

/// 校验密码。哈希缺失或损坏一律返回 false。
pub fn verify_password(user: &User, password: &str) -> bool {
    let Some(raw) = user.password_hash.as_deref() else {
        return false;
    };
    let Ok(parsed) = PasswordHash::new(raw) else {
        return false;
    };
    Argon2::default()
        .verify_password(password.as_bytes(), &parsed)
        .is_ok()
}

/// 用户名不存在时的「等时校验」：跑一次同样代价的 Argon2 校验，
/// 避免攻击者通过响应时间判断用户名是否存在。总是返回 false。
pub fn verify_dummy(password: &str) -> bool {
    static DUMMY_HASH: OnceLock<String> = OnceLock::new();
    let hash = DUMMY_HASH
        .get_or_init(|| hash_password("okb-assist-dummy-password").unwrap_or_default());
    let Ok(parsed) = PasswordHash::new(hash) else {
        return false;
    };
    Argon2::default()
        .verify_password(password.as_bytes(), &parsed)
        .is_ok()
}

/// 恒定时间字符串比较（用于哈希比对）
fn constant_time_eq(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for i in 0..a.len() {
        diff |= a[i] ^ b[i];
    }
    diff == 0
}

/// `sha256(明文 token)` 的小写 hex（落盘只存它）
pub fn hash_api_token(token: &str) -> String {
    let digest = Sha256::digest(token.as_bytes());
    hex::encode(digest)
}

/// 生成新的 API token 明文（`okb_<32B base64url>`）
pub fn generate_api_token() -> String {
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use base64::Engine;
    let mut buf = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut buf);
    format!("{}{}", API_TOKEN_PREFIX, URL_SAFE_NO_PAD.encode(buf))
}

/// 日志/响应中脱敏 token（`okb_****abcd`）
pub fn mask_api_token(token: &str) -> String {
    if token.len() <= API_TOKEN_PREFIX.len() + 4 {
        return format!("{}****", API_TOKEN_PREFIX);
    }
    format!(
        "{}****{}",
        API_TOKEN_PREFIX,
        &token[token.len() - 4..]
    )
}

fn random_user_id() -> String {
    let mut buf = [0u8; 8];
    rand::thread_rng().fill_bytes(&mut buf);
    format!("u_{}", hex::encode(buf))
}

fn now_rfc3339() -> String {
    Utc::now().to_rfc3339()
}

// ── 用户存储 ────────────────────────────────────────────

/// 用户存储：`config.json` 的 `users` 段的读写门面。
#[derive(Clone)]
pub struct UserStore {
    cm: Arc<ConfigManager>,
}

impl UserStore {
    pub fn new(cm: Arc<ConfigManager>) -> Self {
        Self { cm }
    }

    // ── 读取 ──

    /// 读取完整 `users` 段（反序列化失败时回退默认值，避免配置手误导致进程不可用）
    pub fn section(&self) -> UsersSection {
        serde_json::from_value(self.cm.get_service_config()["users"].clone()).unwrap_or_default()
    }

    pub fn list(&self) -> Vec<User> {
        let mut items = self.section().items;
        items.sort_by(|a, b| a.username.to_lowercase().cmp(&b.username.to_lowercase()));
        items
    }

    pub fn find_by_username(&self, username: &str) -> Option<User> {
        self.section().find_by_username(username).cloned()
    }

    pub fn find_by_id(&self, id: &str) -> Option<User> {
        self.section().find_by_id(id).cloned()
    }

    /// 按明文 API token 查找用户（命中后返回该用户与对应 token 元数据）
    pub fn find_by_api_token(&self, token: &str) -> Option<(User, ApiToken)> {
        let sha = hash_api_token(token);
        for user in self.section().items {
            let hit = user
                .api_tokens
                .iter()
                .find(|t| constant_time_eq(&t.token_sha256, &sha))
                .cloned();
            if let Some(t) = hit {
                return Some((user, t));
            }
        }
        None
    }

    /// 启用中的管理员数量
    pub fn active_admin_count(&self) -> usize {
        self.section().active_admin_count()
    }

    /// 是否已初始化（存在带密码的 active admin）
    pub fn is_initialized(&self) -> bool {
        self.section()
            .items
            .iter()
            .any(|u| u.is_admin() && u.is_active() && u.password_hash.is_some())
    }

    pub fn user_count(&self) -> usize {
        self.section().items.len()
    }

    // ── 写入 ──

    /// 统一的原子改写入：读盘 → 反序列化 → 闭包修改 → 序列化 → 原子写盘
    fn mutate<F>(&self, f: F) -> Result<UsersSection, UserError>
    where
        F: FnOnce(&mut UsersSection) -> Result<(), UserError>,
    {
        let mut captured: Option<UserError> = None;
        let mut result: Option<UsersSection> = None;

        let written = self.cm.update_users_section(|section_value| {
            let mut section: UsersSection =
                serde_json::from_value(section_value.clone()).unwrap_or_default();
            match f(&mut section) {
                Ok(()) => {
                    *section_value = serde_json::to_value(&section)?;
                    result = Some(section);
                    Ok(())
                }
                Err(e) => {
                    captured = Some(e);
                    // 用错误中断写入（文件不会被改写），真实原因从 captured 取回
                    Err(anyhow::anyhow!("users 段更新被拒绝"))
                }
            }
        });

        if let Some(e) = captured {
            return Err(e);
        }
        written.map_err(|e| UserError::Config(e.to_string()))?;
        Ok(result.unwrap_or_default())
    }

    /// 创建用户。`must_set_password` 在未提供密码时自动置为 true。
    pub fn create(&self, new: NewUser) -> Result<User, UserError> {
        let username = new.username.trim().to_string();
        validate_username(&username)?;

        let password_hash = match new.password.as_deref() {
            Some(pw) if !pw.is_empty() => {
                validate_password(pw)?;
                Some(hash_password(pw)?)
            }
            _ => None,
        };

        let role = if new.role.trim().is_empty() {
            "view-only".to_string()
        } else {
            new.role.trim().to_string()
        };
        if Role::from_str(&role).is_none() {
            return Err(UserError::InvalidRole);
        }

        let status = if new.status.trim().is_empty() {
            STATUS_ACTIVE.to_string()
        } else {
            new.status.trim().to_string()
        };
        if UserStatus::from_str(&status).is_none() {
            return Err(UserError::InvalidStatus);
        }

        let must_set_password = new.must_set_password || password_hash.is_none();

        let section = self.mutate(|section| {
            if section.find_by_username(&username).is_some() {
                return Err(UserError::UsernameTaken);
            }
            let now = now_rfc3339();
            section.items.push(User {
                id: random_user_id(),
                username: username.clone(),
                display_name: new.display_name.trim().to_string(),
                role: role.clone(),
                status: status.clone(),
                mcp: new.mcp,
                password_hash: password_hash.clone(),
                must_set_password,
                api_tokens: Vec::new(),
                created_at: now.clone(),
                updated_at: now,
                last_login_at: None,
                failed_logins: 0,
                locked_until: None,
            });
            Ok(())
        })?;

        section
            .find_by_username(&username)
            .cloned()
            .ok_or(UserError::NotFound)
    }

    /// 更新用户属性（角色 / 状态 / 备注名 / mcp）。
    ///
    /// 违反「至少一个 active admin」时返回 [`UserError::LastAdmin`]。
    pub fn update(&self, id: &str, patch: &UserPatch) -> Result<User, UserError> {
        if let Some(role) = patch.role.as_deref() {
            if Role::from_str(role.trim()).is_none() {
                return Err(UserError::InvalidRole);
            }
        }
        if let Some(status) = patch.status.as_deref() {
            if UserStatus::from_str(status.trim()).is_none() {
                return Err(UserError::InvalidStatus);
            }
        }

        let section = self.mutate(|section| {
            let active_admins = section.active_admin_count();
            let idx = section
                .items
                .iter()
                .position(|u| u.id == id)
                .ok_or(UserError::NotFound)?;

            // 先把「将要变成什么」算出来，再判不变量
            let current_is_last_admin = {
                let u = &section.items[idx];
                u.is_admin() && u.is_active() && active_admins == 1
            };
            if current_is_last_admin {
                let next_role = patch
                    .role
                    .as_deref()
                    .map(str::trim)
                    .unwrap_or(&section.items[idx].role);
                let next_status = patch
                    .status
                    .as_deref()
                    .map(str::trim)
                    .unwrap_or(&section.items[idx].status);
                if next_role != "admin" || next_status != STATUS_ACTIVE {
                    return Err(UserError::LastAdmin);
                }
            }

            let user = &mut section.items[idx];
            if let Some(v) = patch.display_name.as_deref() {
                user.display_name = v.trim().to_string();
            }
            if let Some(v) = patch.role.as_deref() {
                user.role = v.trim().to_string();
            }
            if let Some(v) = patch.status.as_deref() {
                user.status = v.trim().to_string();
            }
            if let Some(v) = patch.mcp {
                user.mcp = v;
            }
            user.updated_at = now_rfc3339();
            Ok(())
        })?;

        section.find_by_id(id).cloned().ok_or(UserError::NotFound)
    }

    /// 删除用户（最后一个 active admin 不可删）
    pub fn delete(&self, id: &str) -> Result<User, UserError> {
        let mut removed: Option<User> = None;
        self.mutate(|section| {
            let active_admins = section.active_admin_count();
            let idx = section
                .items
                .iter()
                .position(|u| u.id == id)
                .ok_or(UserError::NotFound)?;
            let is_last_admin = {
                let u = &section.items[idx];
                u.is_admin() && u.is_active() && active_admins == 1
            };
            if is_last_admin {
                return Err(UserError::LastAdmin);
            }
            removed = Some(section.items.remove(idx));
            Ok(())
        })?;
        removed.ok_or(UserError::NotFound)
    }

    /// 设置/重置密码（管理员重置与自己改密都走这里）。
    ///
    /// 会清除 `must_set_password`、失败计数与锁定；并**自动吊销遗留的引导期 token**
    /// （`label == "bootstrap"`，旧版本 `system.json.token` 的遗产）。
    /// 撤销已有会话由路由层负责（需要 `SessionStore`）。
    pub fn set_password(&self, id: &str, password: &str) -> Result<User, UserError> {
        validate_password(password)?;
        let hash = hash_password(password)?;
        let section = self.mutate(|section| {
            let user = section
                .items
                .iter_mut()
                .find(|u| u.id == id)
                .ok_or(UserError::NotFound)?;
            user.password_hash = Some(hash.clone());
            user.must_set_password = false;
            user.failed_logins = 0;
            user.locked_until = None;
            user.api_tokens
                .retain(|t| t.label != BOOTSTRAP_TOKEN_LABEL);
            user.updated_at = now_rfc3339();
            Ok(())
        })?;
        section.find_by_id(id).cloned().ok_or(UserError::NotFound)
    }

    /// 记录一次登录结果：成功则清空失败计数并刷新 `last_login_at`；
    /// 失败则累加并按指数退避设置 `locked_until`（http 429 由调用方决定是否返回）。
    pub fn record_login(&self, id: &str, success: bool) -> Result<User, UserError> {
        let section = self.mutate(|section| {
            let user = section
                .items
                .iter_mut()
                .find(|u| u.id == id)
                .ok_or(UserError::NotFound)?;
            let now = Utc::now();
            if success {
                user.failed_logins = 0;
                user.locked_until = None;
                user.last_login_at = Some(now.to_rfc3339());
            } else {
                user.failed_logins = user.failed_logins.saturating_add(1);
                if user.failed_logins >= LOCK_AFTER_FAILURES {
                    let over = (user.failed_logins - LOCK_AFTER_FAILURES) as u32;
                    // 30s * 2^over，上限 1 小时
                    let secs = 30i64
                        .saturating_mul(1i64 << over.min(7))
                        .min(MAX_LOCK_SECONDS);
                    user.locked_until = Some((now + Duration::seconds(secs)).to_rfc3339());
                }
            }
            user.updated_at = now.to_rfc3339();
            Ok(())
        })?;
        section.find_by_id(id).cloned().ok_or(UserError::NotFound)
    }

    /// 签发 API / MCP token（明文自动生成，`okb_<32B base64url>`）。
    ///
    /// 返回 `(明文 token, token 元数据)`，**明文只在此刻返回一次**，磁盘上只有 sha256。
    pub fn issue_api_token(
        &self,
        id: &str,
        label: &str,
        scope: TokenScope,
    ) -> Result<(String, ApiToken), UserError> {
        let plain = generate_api_token();
        let token = self.register_api_token(id, &plain, label, scope)?;
        Ok((plain, token))
    }

    /// 把一把**已存在的明文 token** 登记到用户名下（只存 sha256）。
    ///
    /// 用于「用户自定义令牌」（如把外部系统已有的 token 收敛为个人凭证）。
    /// 校验：至少 [`MIN_TOKEN_LEN`] 位、仅可见 ASCII（不含空格）、全局不重复。
    /// 同一个用户的同一 token 重复登记是幂等的。
    pub fn register_api_token(
        &self,
        id: &str,
        plaintext: &str,
        label: &str,
        scope: TokenScope,
    ) -> Result<ApiToken, UserError> {
        validate_token_plaintext(plaintext)?;
        let sha = hash_api_token(plaintext);
        let token = ApiToken {
            id: format!("at_{}", &sha[..12]),
            label: label.trim().to_string(),
            token_sha256: sha.clone(),
            scope,
            created_at: now_rfc3339(),
            last_used_at: None,
        };
        let token_for_section = token.clone();
        self.mutate(|section| {
            // 全局唯一：不允许两个账号登记同一个明文令牌（否则解析主体会有歧义）
            let taken_by_other = section.items.iter().any(|u| {
                u.id != id && u.api_tokens.iter().any(|t| t.token_sha256 == sha)
            });
            if taken_by_other {
                return Err(UserError::TokenTaken);
            }
            let user = section
                .items
                .iter_mut()
                .find(|u| u.id == id)
                .ok_or(UserError::NotFound)?;
            // 同一个 token 不重复登记（幂等）
            if !user
                .api_tokens
                .iter()
                .any(|t| t.token_sha256 == token_for_section.token_sha256)
            {
                user.api_tokens.push(token_for_section.clone());
            }
            user.updated_at = now_rfc3339();
            Ok(())
        })?;
        Ok(token)
    }

    /// 撤销某个 API token，返回是否命中
    pub fn revoke_api_token(&self, id: &str, token_id: &str) -> Result<bool, UserError> {
        let mut removed = false;
        self.mutate(|section| {
            let user = section
                .items
                .iter_mut()
                .find(|u| u.id == id)
                .ok_or(UserError::NotFound)?;
            let before = user.api_tokens.len();
            user.api_tokens.retain(|t| t.id != token_id);
            removed = user.api_tokens.len() != before;
            user.updated_at = now_rfc3339();
            Ok(())
        })?;
        Ok(removed)
    }

    /// 撤销用户全部 API token（改密 / 禁用 / 删除时的配套操作）
    pub fn revoke_all_api_tokens(&self, id: &str) -> Result<usize, UserError> {
        let mut n = 0;
        self.mutate(|section| {
            let user = section
                .items
                .iter_mut()
                .find(|u| u.id == id)
                .ok_or(UserError::NotFound)?;
            n = user.api_tokens.len();
            user.api_tokens.clear();
            user.updated_at = now_rfc3339();
            Ok(())
        })?;
        Ok(n)
    }

    /// 旧 `config.json.permissions` 的一次性迁移（幂等）。
    ///
    /// **方案 B（全部失效重发）**：旧 token 一个都不接管（不入库、不落盘），只为每个
    /// 条目建一个「无凭据占位用户」：
    ///
    /// - `username = "legacy-<sha256(token) 前 6 位>"`（重名时追加序号）
    /// - `role` / `mcp` 沿用原条目（旧字符串格式 `"view-only"` 也支持）
    /// - `status = pending`、`password_hash = None`、`api_tokens = []`
    ///
    /// 迁移后从磁盘删除 `permissions` 键，并打印迁移清单（不含明文 token）。
    /// 管理员后续可为其重置密码转正，或直接删除。返回迁移数量。
    pub fn migrate_legacy_permissions(&self) -> usize {
        let config = self.cm.get_service_config();
        let Some(perms) = config.get("permissions").and_then(|v| v.as_object()) else {
            return 0;
        };
        if perms.is_empty() {
            // 空字典也要清掉，避免每次启动都走迁移
            let _ = self.cm.take_config_key("permissions");
            return 0;
        }

        // (token, role, mcp) —— 兼容旧版 `"token": "view-only"` 与新格式
        let entries: Vec<(String, String, bool)> = perms
            .iter()
            .map(|(token, entry)| {
                let role = if entry.is_string() {
                    entry.as_str().unwrap_or("").to_string()
                } else {
                    entry
                        .get("access")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string()
                };
                let mcp = entry
                    .get("mcp")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                (token.clone(), role, mcp)
            })
            .collect();

        let mut created = 0usize;
        let result = self.mutate(|section| {
            for (token, role, mcp) in &entries {
                let role = if Role::from_str(role).is_some() {
                    role.clone()
                } else {
                    "view-only".to_string()
                };
                let suffix = &hash_api_token(token)[..6];
                let mut username = format!("legacy-{}", suffix);
                let mut n = 0usize;
                while section.find_by_username(&username).is_some() {
                    n += 1;
                    username = format!("legacy-{}-{}", suffix, n);
                }
                let now = now_rfc3339();
                section.items.push(User {
                    id: random_user_id(),
                    username,
                    display_name: "旧 token 迁移".to_string(),
                    role,
                    status: STATUS_PENDING.to_string(),
                    mcp: *mcp,
                    password_hash: None,
                    must_set_password: true,
                    api_tokens: Vec::new(),
                    created_at: now.clone(),
                    updated_at: now,
                    last_login_at: None,
                    failed_logins: 0,
                    locked_until: None,
                });
                created += 1;
            }
            Ok(())
        });

        match result {
            Ok(_) => {
                let _ = self.cm.take_config_key("permissions");
                let names: Vec<String> = self
                    .section()
                    .items
                    .into_iter()
                    .filter(|u| u.username.starts_with("legacy-"))
                    .map(|u| format!("{}({})", u.username, u.role))
                    .collect();
                tracing::warn!(
                    count = created,
                    users = ?names,
                    "旧 permissions 已迁移为无凭据占位用户（pending）：旧 token 全部失效，请为其重置密码或删除"
                );
                created
            }
            Err(e) => {
                tracing::error!(error = %e, "旧 permissions 迁移失败，已跳过（原配置保留）");
                0
            }
        }
    }

    /// 修改注册策略 / 会话 TTL 等段级标志
    pub fn update_settings(
        &self,
        allow_registration: Option<bool>,
        require_approval: Option<bool>,
        registration_role: Option<&str>,
        session_ttl_hours: Option<i64>,
    ) -> Result<UsersSection, UserError> {
        if let Some(role) = registration_role {
            let role = role.trim();
            if Role::from_str(role).is_none() || role == "admin" {
                return Err(UserError::InvalidRole);
            }
        }
        self.mutate(|section| {
            if let Some(v) = allow_registration {
                section.allow_registration = v;
            }
            if let Some(v) = require_approval {
                section.require_approval = v;
            }
            if let Some(v) = registration_role {
                section.registration_role = v.trim().to_string();
            }
            if let Some(v) = session_ttl_hours {
                section.session_ttl_hours = v.max(1);
            }
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// 建一个只在临时目录里读写的 store（不污染仓库里的 config.json）
    fn test_store() -> (tempfile::TempDir, UserStore, Arc<ConfigManager>) {
        let dir = tempfile::tempdir().unwrap();
        let config: PathBuf = dir.path().join("config.json");
        let system: PathBuf = dir.path().join("system.json");
        let cm = Arc::new(ConfigManager::for_tests(config, system));
        let store = UserStore::new(cm.clone());
        (dir, store, cm)
    }

    fn admin() -> NewUser {
        NewUser {
            username: "admin".to_string(),
            password: Some("admin-pass-123".to_string()),
            role: "admin".to_string(),
            mcp: true,
            ..Default::default()
        }
    }

    #[test]
    fn default_section_flags() {
        let (_dir, store, _cm) = test_store();
        let section = store.section();
        assert_eq!(section.version, 1);
        assert!(section.allow_registration);
        assert!(section.require_approval);
        assert_eq!(section.registration_role, "view-only");
        assert_eq!(section.session_ttl_hours, 168);
        assert!(section.items.is_empty());
        assert!(!store.is_initialized());
    }

    #[test]
    fn create_persists_hashed_password_only() {
        let (dir, store, _cm) = test_store();
        let user = store.create(admin()).unwrap();
        assert!(user.id.starts_with("u_"));
        assert_eq!(user.role, "admin");
        assert_eq!(user.status, STATUS_ACTIVE);
        assert!(!user.must_set_password);
        let raw = std::fs::read_to_string(dir.path().join("config.json")).unwrap();
        assert!(raw.contains("$argon2id$"), "应存 Argon2id PHC 哈希");
        assert!(!raw.contains("admin-pass-123"), "不应出现明文密码");
        assert!(store.is_initialized());
        assert_eq!(store.active_admin_count(), 1);
        assert_eq!(store.user_count(), 1);
    }

    #[test]
    fn password_verification() {
        let (_dir, store, _cm) = test_store();
        let user = store.create(admin()).unwrap();
        assert!(verify_password(&user, "admin-pass-123"));
        assert!(!verify_password(&user, "wrong-pass"));
        // 无密码账号永远校验失败
        let token_only = store
            .create(NewUser {
                username: "reader".to_string(),
                ..Default::default()
            })
            .unwrap();
        assert!(!verify_password(&token_only, ""));
        assert!(token_only.must_set_password);
    }

    #[test]
    fn username_rules_and_uniqueness() {
        let (_dir, store, _cm) = test_store();
        store.create(admin()).unwrap();
        // 大小写不敏感重复
        assert!(matches!(
            store.create(NewUser {
                username: "ADMIN".to_string(),
                ..Default::default()
            }),
            Err(UserError::UsernameTaken)
        ));
        // 非法用户名
        assert!(matches!(
            store.create(NewUser {
                username: "ab".to_string(),
                ..Default::default()
            }),
            Err(UserError::InvalidUsername)
        ));
        assert!(matches!(
            store.create(NewUser {
                username: "bad name!".to_string(),
                ..Default::default()
            }),
            Err(UserError::InvalidUsername)
        ));
    }

    #[test]
    fn weak_password_and_invalid_role_rejected() {
        let (_dir, store, _cm) = test_store();
        let mut short = admin();
        short.password = Some("1234567".to_string());
        assert!(matches!(
            store.create(short),
            Err(UserError::WeakPassword)
        ));

        let mut bad_role = admin();
        bad_role.username = "who".to_string();
        bad_role.role = "superuser".to_string();
        assert!(matches!(store.create(bad_role), Err(UserError::InvalidRole)));
    }

    #[test]
    fn last_admin_cannot_be_removed_or_demoted() {
        let (_dir, store, _cm) = test_store();
        let solo = store.create(admin()).unwrap();
        assert!(matches!(
            store.delete(&solo.id),
            Err(UserError::LastAdmin)
        ));
        assert!(matches!(
            store.update(
                &solo.id,
                &UserPatch {
                    role: Some("view-only".to_string()),
                    ..Default::default()
                }
            ),
            Err(UserError::LastAdmin)
        ));
        assert!(matches!(
            store.update(
                &solo.id,
                &UserPatch {
                    status: Some(STATUS_DISABLED.to_string()),
                    ..Default::default()
                }
            ),
            Err(UserError::LastAdmin)
        ));

        // 第二个管理员存在后即可操作
        let second = store
            .create(NewUser {
                username: "admin2".to_string(),
                password: Some("admin2-pass-123".to_string()),
                role: "admin".to_string(),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(store.active_admin_count(), 2);
        store.delete(&second.id).unwrap();
        assert_eq!(store.active_admin_count(), 1);
    }

    #[test]
    fn update_user_fields() {
        let (_dir, store, _cm) = test_store();
        let u = store
            .create(NewUser {
                username: "reader".to_string(),
                password: Some("reader-pass-1".to_string()),
                ..Default::default()
            })
            .unwrap();
        let updated = store
            .update(
                &u.id,
                &UserPatch {
                    display_name: Some("  Reader One  ".to_string()),
                    role: Some("view-upload".to_string()),
                    mcp: Some(true),
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(updated.display_name, "Reader One");
        assert_eq!(updated.role, "view-upload");
        assert!(updated.mcp);
        assert!(matches!(
            store.update(
                &u.id,
                &UserPatch {
                    status: Some("bogus".to_string()),
                    ..Default::default()
                }
            ),
            Err(UserError::InvalidStatus)
        ));
        assert!(matches!(
            store.update("u_missing", &UserPatch::default()),
            Err(UserError::NotFound)
        ));
    }

    #[test]
    fn api_tokens_are_hashed_and_revocable() {
        let (dir, store, _cm) = test_store();
        let u = store
            .create(NewUser {
                username: "client".to_string(),
                password: Some("client-pass-1".to_string()),
                mcp: true,
                ..Default::default()
            })
            .unwrap();

        let (plain, token) = store.issue_api_token(&u.id, "zotero", TokenScope::Mcp).unwrap();
        assert!(plain.starts_with(API_TOKEN_PREFIX));
        assert!(token.id.starts_with("at_"));

        let raw = std::fs::read_to_string(dir.path().join("config.json")).unwrap();
        assert!(!raw.contains(&plain), "明文 token 不得落盘");
        assert!(raw.contains(&token.token_sha256));

        let (found, found_token) = store.find_by_api_token(&plain).unwrap();
        assert_eq!(found.id, u.id);
        assert_eq!(found_token.label, "zotero");
        assert!(store.find_by_api_token("okb_wrong").is_none());

        assert!(store.revoke_api_token(&u.id, &token.id).unwrap());
        assert!(!store.revoke_api_token(&u.id, &token.id).unwrap());
        assert!(store.find_by_api_token(&plain).is_none());

        store.issue_api_token(&u.id, "a", TokenScope::Api).unwrap();
        store.issue_api_token(&u.id, "b", TokenScope::Api).unwrap();
        assert_eq!(store.revoke_all_api_tokens(&u.id).unwrap(), 2);
        assert_eq!(store.find_by_id(&u.id).unwrap().api_tokens.len(), 0);
    }

    #[test]
    fn public_json_hides_secrets() {
        let (_dir, store, _cm) = test_store();
        let u = store.create(admin()).unwrap();
        let (plain, _) = store.issue_api_token(&u.id, "l", TokenScope::Mcp).unwrap();
        let json = store.find_by_id(&u.id).unwrap().to_public_json().to_string();
        assert!(!json.contains("password_hash"));
        assert!(!json.contains("token_sha256"));
        assert!(!json.contains(&plain));
        assert!(json.contains("\"has_password\":true"));
    }

    #[test]
    fn set_password_clears_flags() {
        let (_dir, store, _cm) = test_store();
        let u = store
            .create(NewUser {
                username: "newbie".to_string(),
                ..Default::default()
            })
            .unwrap();
        assert!(u.must_set_password);
        // 制造一次失败锁定
        for _ in 0..LOCK_AFTER_FAILURES {
            store.record_login(&u.id, false).unwrap();
        }
        let locked = store.find_by_id(&u.id).unwrap();
        assert!(locked.locked_until_at().is_some());
        assert_eq!(locked.failed_logins, LOCK_AFTER_FAILURES);

        let updated = store.set_password(&u.id, "brand-new-pass").unwrap();
        assert!(!updated.must_set_password);
        assert_eq!(updated.failed_logins, 0);
        assert!(updated.locked_until.is_none());
        assert!(verify_password(&updated, "brand-new-pass"));
    }

    #[test]
    fn record_login_sets_timestamps() {
        let (_dir, store, _cm) = test_store();
        let u = store.create(admin()).unwrap();
        assert!(u.last_login_at.is_none());
        let after = store.record_login(&u.id, true).unwrap();
        assert!(after.last_login_at.is_some());
        assert_eq!(after.failed_logins, 0);
        let failed = store.record_login(&u.id, false).unwrap();
        assert_eq!(failed.failed_logins, 1);
        // 未到阈值不锁定
        assert!(failed.locked_until_at().is_none());
    }

    #[test]
    fn update_settings_rejects_admin_registration_role() {
        let (_dir, store, _cm) = test_store();
        assert!(matches!(
            store.update_settings(None, None, Some("admin"), None),
            Err(UserError::InvalidRole)
        ));
        let section = store
            .update_settings(Some(false), Some(false), Some("view-upload"), Some(24))
            .unwrap();
        assert!(!section.allow_registration);
        assert!(!section.require_approval);
        assert_eq!(section.registration_role, "view-upload");
        assert_eq!(section.session_ttl_hours, 24);
    }

    #[test]
    fn changes_survive_reload_without_touching_other_sections() {
        let (_dir, store, cm) = test_store();
        // 先写一份「服务配置」
        let mut svc = cm.get_service_config();
        svc["base_url"] = json!("http://example.test/");
        cm.save_config(&svc).unwrap();

        store.create(admin()).unwrap();
        cm.reload_config();

        assert_eq!(store.user_count(), 1);
        assert_eq!(
            cm.get_service_config()["base_url"],
            json!("http://example.test/"),
            "改 user 不应冲掉服务配置"
        );
        // 反过来：保存服务配置不应覆盖 users
        let mut svc2 = cm.get_service_config();
        svc2["base_url"] = json!("http://other.test/");
        cm.save_config(&svc2).unwrap();
        cm.reload_config();
        assert_eq!(store.user_count(), 1, "保存服务配置不得清空 users");
    }

    #[test]
    fn register_api_token_is_idempotent_and_revocable_by_label() {
        let (_dir, store, _cm) = test_store();
        let u = store.create(admin()).unwrap();
        // 引导场景：接管 system.json 的 token
        let t1 = store
            .register_api_token(&u.id, "legacy-admin-token", BOOTSTRAP_TOKEN_LABEL, TokenScope::Mcp)
            .unwrap();
        let t2 = store
            .register_api_token(&u.id, "legacy-admin-token", BOOTSTRAP_TOKEN_LABEL, TokenScope::Mcp)
            .unwrap();
        assert_eq!(t1.token_sha256, t2.token_sha256);
        assert_eq!(store.find_by_id(&u.id).unwrap().api_tokens.len(), 1, "重复登记应幂等");
        assert!(store.find_by_api_token("legacy-admin-token").is_some());
        // 空 token 拒绝
        assert!(store.register_api_token(&u.id, "  ", "x", TokenScope::Api).is_err());
    }

    #[test]
    fn custom_token_validation_and_global_uniqueness() {
        let (_dir, store, _cm) = test_store();
        let a = store.create(admin()).unwrap();
        let b = store
            .create(NewUser {
                username: "bob".to_string(),
                password: Some("bob-pass-123".to_string()),
                mcp: true,
                ..Default::default()
            })
            .unwrap();

        // 自定义令牌：≥ 8 位、可见 ASCII 即可
        let ok = store
            .register_api_token(&a.id, "we-network-control", "agent", TokenScope::Mcp)
            .unwrap();
        assert_eq!(ok.scope, TokenScope::Mcp);
        assert_eq!(ok.to_public_json()["scope"], json!("mcp"));
        assert!(!ok.to_public_json().to_string().contains("token_sha256"));

        // 太短 / 含空格 / 非 ASCII 一律拒绝
        assert!(matches!(
            store.register_api_token(&a.id, "short", "x", TokenScope::Api),
            Err(UserError::InvalidToken(_))
        ));
        assert!(matches!(
            store.register_api_token(&a.id, "has space", "x", TokenScope::Api),
            Err(UserError::InvalidToken(_))
        ));
        assert!(matches!(
            store.register_api_token(&a.id, "中文令牌中文令牌", "x", TokenScope::Api),
            Err(UserError::InvalidToken(_))
        ));

        // 全局唯一：别的账号不能用同一串明文
        assert!(matches!(
            store.register_api_token(&b.id, "we-network-control", "dup", TokenScope::Mcp),
            Err(UserError::TokenTaken)
        ));
        // 同一用户重复登记仍幂等（不新增条目）
        store
            .register_api_token(&a.id, "we-network-control", "agent", TokenScope::Mcp)
            .unwrap();
        assert_eq!(store.find_by_id(&a.id).unwrap().api_tokens.len(), 1);
    }

    #[test]
    fn token_scope_defaults_to_api_and_round_trips() {
        // 缺字段 → api（最小权限）
        let parsed: ApiToken = serde_json::from_value(json!({
            "id": "at_x",
            "label": "zotero",
            "token_sha256": "00",
            "created_at": "2026-01-01T00:00:00Z"
        }))
        .unwrap();
        assert_eq!(parsed.scope, TokenScope::Api);

        assert_eq!(TokenScope::from_str("MCP"), Some(TokenScope::Mcp));
        assert_eq!(TokenScope::from_str(" api "), Some(TokenScope::Api));
        assert_eq!(TokenScope::from_str("both"), None);
        assert!(!TokenScope::Mcp.allows_api());
        assert!(!TokenScope::Api.allows_mcp());
    }

    #[test]
    fn set_password_revokes_bootstrap_token() {
        let (_dir, store, _cm) = test_store();
        // 模拟引导：创建无密码 admin + bootstrap token + 一把普通 token
        let u = store
            .create(NewUser {
                username: "admin".to_string(),
                role: "admin".to_string(),
                ..Default::default()
            })
            .unwrap();
        assert!(u.must_set_password);
        store
            .register_api_token(&u.id, "system-json-token", BOOTSTRAP_TOKEN_LABEL, TokenScope::Mcp)
            .unwrap();
        let (keep, _) = store.issue_api_token(&u.id, "zotero", TokenScope::Api).unwrap();

        store.set_password(&u.id, "new-admin-pass").unwrap();

        assert!(
            store.find_by_api_token("system-json-token").is_none(),
            "设置密码后引导 token 必须失效"
        );
        assert!(store.find_by_api_token(&keep).is_some(), "普通 token 不受影响");
    }

    /// 直接把 `permissions` 写进磁盘上的 config.json（模拟已部署实例的旧配置）。
    ///
    /// 不能用 `save_config()`：新版本已把 `permissions` 从默认配置里移除，
    /// 保存服务配置时本来就会把它丢掉（这是给配置页的防护，不是迁移路径）。
    fn write_legacy_permissions(
        dir: &tempfile::TempDir,
        cm: &ConfigManager,
        value: serde_json::Value,
    ) {
        let path = dir.path().join("config.json");
        let mut raw: serde_json::Value = std::fs::read_to_string(&path)
            .ok()
            .and_then(|c| serde_json::from_str(&c).ok())
            .unwrap_or_else(|| json!({}));
        raw["permissions"] = value;
        std::fs::write(&path, serde_json::to_string_pretty(&raw).unwrap()).unwrap();
        cm.reload_config();
    }

    #[test]
    fn migrate_legacy_permissions_creates_placeholder_users() {
        let (dir, store, cm) = test_store();
        write_legacy_permissions(
            &dir,
            &cm,
            json!({
                "old-token-a": {"access": "view-upload", "mcp": true},
                "old-token-b": "view-only"
            }),
        );
        assert!(cm.get_service_config().get("permissions").is_some());

        let n = store.migrate_legacy_permissions();
        assert_eq!(n, 2);

        // 占位用户：pending、无密码、无 token，但保留角色与 mcp
        let users = store.list();
        assert_eq!(users.len(), 2);
        assert!(users.iter().all(|u| u.username.starts_with("legacy-")));
        assert!(users.iter().all(|u| u.status == STATUS_PENDING));
        assert!(users.iter().all(|u| u.password_hash.is_none()));
        assert!(users.iter().all(|u| u.api_tokens.is_empty()));

        let mut roles: Vec<(String, bool)> =
            users.iter().map(|u| (u.role.clone(), u.mcp)).collect();
        roles.sort_by(|a, b| a.0.cmp(&b.0));
        assert!(roles.contains(&("view-only".to_string(), false)));
        assert!(roles.contains(&("view-upload".to_string(), true)));

        // 旧 token 已彻底失效
        assert!(store.find_by_api_token("old-token-a").is_none());
        assert!(store.find_by_api_token("old-token-b").is_none());

        // permissions 已从磁盘移除，二次迁移无操作
        cm.reload_config();
        assert!(cm.get_service_config().get("permissions").is_none());
        assert_eq!(store.migrate_legacy_permissions(), 0);
        assert_eq!(store.list().len(), 2);
        // 磁盘上也不应残留旧 token 明文
        let raw = std::fs::read_to_string(dir.path().join("config.json")).unwrap();
        assert!(!raw.contains("old-token-a"));
    }

    #[test]
    fn migrate_legacy_permissions_handles_duplicate_and_empty() {
        let (dir, store, cm) = test_store();
        // 空字典：直接清场，不建用户
        write_legacy_permissions(&dir, &cm, json!({}));
        assert_eq!(store.migrate_legacy_permissions(), 0);
        assert_eq!(store.user_count(), 0);
        cm.reload_config();
        assert!(cm.get_service_config().get("permissions").is_none());

        // 非法角色降级为 view-only
        write_legacy_permissions(&dir, &cm, json!({"t": {"access": "root", "mcp": false}}));
        assert_eq!(store.migrate_legacy_permissions(), 1);
        let users = store.list();
        assert_eq!(users.len(), 1);
        assert_eq!(users[0].role, "view-only");
    }

    #[test]
    fn defaults_match_config_manager_section() {
        // 防止 config_manager::default_users_section() 与 UsersSection::default() 失衡
        let expected = serde_json::to_value(UsersSection::default()).unwrap();
        assert_eq!(crate::config_manager::default_users_section(), expected);
    }

    #[test]
    fn validation_helpers() {
        assert!(validate_username("alice").is_ok());
        assert!(validate_username("a.b-c_1").is_ok());
        assert!(validate_username("ab").is_err());
        assert!(validate_username("alice space").is_err());
        assert!(validate_username(&"x".repeat(33)).is_err());
        assert!(validate_password("12345678").is_ok());
        assert!(validate_password("1234567").is_err());
        assert!(validate_password(&"x".repeat(129)).is_err());
    }
}
