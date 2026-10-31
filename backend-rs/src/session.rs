//! 服务端会话存储。
//!
//! 密码登录成功后签发 `sess_<32B base64url>` 形式的 opaque token，服务端在本模块的
//! 内存表中维护 `token → Session` 映射。会话**不持久化**：进程重启后全部失效，用户需
//! 重新登录（前端 401 拦截会跳转登录页）。
//!
//! 有效期为**滑动窗口**（默认 7 天，见 [`DEFAULT_TTL_HOURS`]）：每次成功校验都会把
//! `expires_at` 推迟到 `now + ttl`，因此只有「空闲超过 TTL」才失效。
//!
//! 会话 token 只存在于内存中，磁盘上不会出现明文；相对地，用户长期凭证（API token）
//! 存在 `config.json` 的 `users[].api_tokens[].token_sha256` 里（只存哈希，见 `users` 模块）。

use std::collections::HashMap;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Mutex;

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use chrono::{DateTime, Duration, Utc};
use rand::RngCore;

use crate::auth::Role;

/// 会话 token 前缀（便于日志脱敏与快速识别）
pub const SESSION_PREFIX: &str = "sess_";

/// 默认会话有效期（小时）：7 天（滑动续期）
pub const DEFAULT_TTL_HOURS: i64 = 168;

/// 单个会话
#[derive(Clone, Debug)]
pub struct Session {
    pub user_id: String,
    pub username: String,
    pub role: Role,
    /// 是否允许通过该会话访问 MCP
    pub mcp: bool,
    pub issued_at: DateTime<Utc>,
    /// 最近一次成功校验时间（滑动续期的依据）
    pub last_seen: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub client_ip: Option<String>,
    pub user_agent: Option<String>,
}

/// 签发会话的入参（避免长参数列表）
#[derive(Clone, Debug)]
pub struct NewSession {
    pub user_id: String,
    pub username: String,
    pub role: Role,
    pub mcp: bool,
    pub client_ip: Option<String>,
    pub user_agent: Option<String>,
}

/// 进程内会话表。
///
/// 用 `Mutex<HashMap>` 而非 `RwLock`：读操作（滑动续期）本身就要写 `last_seen`，
/// 且会话数量很小（本地工具，通常个位数），无需读并发优化。
pub struct SessionStore {
    /// 会话有效期（小时），可在运行时按 `users.session_ttl_hours` 调整
    ttl_hours: AtomicI64,
    sessions: Mutex<HashMap<String, Session>>,
}

impl SessionStore {
    /// 按小时创建（`ttl_hours` 小于 1 时取 1，避免 0/负值导致会话立即失效）
    pub fn new(ttl_hours: i64) -> Self {
        Self {
            ttl_hours: AtomicI64::new(ttl_hours.max(1)),
            sessions: Mutex::new(HashMap::new()),
        }
    }

    /// 运行时调整 TTL（配置里改了 `users.session_ttl_hours` 后调用；已签发的会话
    /// 会在下一次校验续期时套用新值）
    pub fn set_ttl_hours(&self, ttl_hours: i64) {
        self.ttl_hours.store(ttl_hours.max(1), Ordering::Relaxed);
    }

    fn ttl(&self) -> Duration {
        Duration::hours(self.ttl_hours().max(1))
    }

    /// 使用默认 TTL（7 天）
    pub fn with_default_ttl() -> Self {
        Self::new(DEFAULT_TTL_HOURS)
    }

    /// 当前 TTL（小时）
    pub fn ttl_hours(&self) -> i64 {
        self.ttl_hours.load(Ordering::Relaxed)
    }

    /// 签发新会话，返回 `(明文 token, 会话快照)`。
    ///
    /// 永远不会复用调用方带来的旧 token（防会话固定）。
    pub fn issue(&self, init: NewSession) -> (String, Session) {
        let now = Utc::now();
        let session = Session {
            user_id: init.user_id,
            username: init.username,
            role: init.role,
            mcp: init.mcp,
            issued_at: now,
            last_seen: now,
            expires_at: now + self.ttl(),
            client_ip: init.client_ip,
            user_agent: init.user_agent,
        };
        let token = random_session_token();
        self.sessions
            .lock()
            .unwrap()
            .insert(token.clone(), session.clone());
        (token, session)
    }

    /// 校验会话 token；有效则**滑动续期**并返回会话快照。
    ///
    /// 顺带惰性清理已过期会话（每次校验只清理一次全表，量级很小）。
    pub fn validate(&self, token: &str) -> Option<Session> {
        if !token.starts_with(SESSION_PREFIX) {
            return None;
        }
        let now = Utc::now();
        let mut guard = self.sessions.lock().unwrap();
        guard.retain(|_, s| s.expires_at > now);
        let session = guard.get_mut(token)?;
        session.last_seen = now;
        session.expires_at = now + self.ttl();
        Some(session.clone())
    }

    /// 只读校验（不续期），用于状态展示
    pub fn peek(&self, token: &str) -> Option<Session> {
        if !token.starts_with(SESSION_PREFIX) {
            return None;
        }
        let now = Utc::now();
        let guard = self.sessions.lock().unwrap();
        guard.get(token).filter(|s| s.expires_at > now).cloned()
    }

    /// 撤销单个会话，返回是否命中
    pub fn revoke(&self, token: &str) -> bool {
        self.sessions.lock().unwrap().remove(token).is_some()
    }

    /// 撤销某用户的全部会话（改密 / 禁用 / 删除用户时调用）
    pub fn revoke_user(&self, user_id: &str) -> usize {
        let mut guard = self.sessions.lock().unwrap();
        let before = guard.len();
        guard.retain(|_, s| s.user_id != user_id);
        before - guard.len()
    }

    /// 撤销全部会话（重置等运维操作）
    pub fn revoke_all(&self) -> usize {
        let mut guard = self.sessions.lock().unwrap();
        let n = guard.len();
        guard.clear();
        n
    }

    /// 清理过期会话，返回清理条数
    pub fn purge_expired(&self) -> usize {
        let now = Utc::now();
        let mut guard = self.sessions.lock().unwrap();
        let before = guard.len();
        guard.retain(|_, s| s.expires_at > now);
        before - guard.len()
    }

    /// 当前存活会话数
    pub fn len(&self) -> usize {
        self.sessions.lock().unwrap().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl Default for SessionStore {
    fn default() -> Self {
        Self::with_default_ttl()
    }
}

/// 生成 `sess_<32B base64url>` 形式的随机会话 token
fn random_session_token() -> String {
    let mut buf = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut buf);
    format!("{}{}", SESSION_PREFIX, URL_SAFE_NO_PAD.encode(buf))
}

/// 日志/响应中脱敏会话 token（只保留前缀与末 4 位）
pub fn mask_session_token(token: &str) -> String {
    if token.len() <= SESSION_PREFIX.len() + 4 {
        return format!("{}****", SESSION_PREFIX);
    }
    format!(
        "{}****{}",
        SESSION_PREFIX,
        &token[token.len() - 4..]
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn new_session(user: &str) -> NewSession {
        NewSession {
            user_id: format!("u_{}", user),
            username: user.to_string(),
            role: Role::ViewOnly,
            mcp: false,
            client_ip: Some("127.0.0.1".to_string()),
            user_agent: Some("test".to_string()),
        }
    }

    #[test]
    fn issue_and_validate_roundtrip() {
        let store = SessionStore::with_default_ttl();
        let (token, issued) = store.issue(new_session("alice"));
        assert!(token.starts_with(SESSION_PREFIX));
        assert_eq!(store.len(), 1);

        let got = store.validate(&token).expect("会话应有效");
        assert_eq!(got.user_id, issued.user_id);
        assert_eq!(got.username, "alice");
        assert_eq!(got.role, Role::ViewOnly);
        // 滑动续期：校验后过期时间应被推后
        assert!(got.expires_at >= issued.expires_at);
        assert_eq!(store.ttl_hours(), DEFAULT_TTL_HOURS);
    }

    #[test]
    fn rejects_unknown_and_foreign_tokens() {
        let store = SessionStore::with_default_ttl();
        // 未签发过
        assert!(store.validate("sess_nope").is_none());
        // 非会话前缀的 token（API token / 旧 admin token）不应被当成会话
        assert!(store.validate("okb_abcdef").is_none());
        assert!(store.validate("60b8be6583ac41ab").is_none());
    }

    #[test]
    fn expired_session_is_purged() {
        // TTL 被钳制为至少 1 小时；用 0 模拟“立即过期”不可行，
        // 因此直接构造一个已过期的会话插入表中。
        let store = SessionStore::new(1);
        let (token, _) = store.issue(new_session("bob"));
        {
            let mut guard = store.sessions.lock().unwrap();
            let s = guard.get_mut(&token).unwrap();
            s.expires_at = Utc::now() - Duration::seconds(1);
        }
        assert!(store.validate(&token).is_none());
        assert_eq!(store.len(), 0, "过期会话应被清理");
        assert_eq!(store.purge_expired(), 0);
    }

    #[test]
    fn revoke_and_revoke_user() {
        let store = SessionStore::with_default_ttl();
        let (t1, _) = store.issue(new_session("alice"));
        let (t2, _) = store.issue(new_session("alice"));
        let (t3, _) = store.issue(new_session("bob"));

        assert!(store.revoke(&t1));
        assert!(!store.revoke(&t1));
        assert_eq!(store.revoke_user("u_alice"), 1);
        assert_eq!(store.len(), 1);
        assert!(store.validate(&t2).is_none());
        assert!(store.validate(&t3).is_some());

        assert_eq!(store.revoke_all(), 1);
        assert!(store.is_empty());
    }

    #[test]
    fn tokens_are_unique() {
        let store = SessionStore::with_default_ttl();
        let (a, _) = store.issue(new_session("a"));
        let (b, _) = store.issue(new_session("a"));
        assert_ne!(a, b);
    }

    #[test]
    fn mask_hides_token_body() {
        let masked = mask_session_token("sess_abcdefghijklmnop");
        assert!(masked.starts_with("sess_****"));
        assert!(masked.ends_with("mnop"));
        assert!(!masked.contains("abcdefghij"));
    }

    #[test]
    fn ttl_is_clamped_to_at_least_one_hour() {
        assert_eq!(SessionStore::new(0).ttl_hours(), 1);
        assert_eq!(SessionStore::new(-5).ttl_hours(), 1);
        assert_eq!(SessionStore::new(168).ttl_hours(), 168);
    }

    #[test]
    fn ttl_can_be_changed_at_runtime() {
        let store = SessionStore::new(DEFAULT_TTL_HOURS);
        store.set_ttl_hours(24);
        assert_eq!(store.ttl_hours(), 24);
        let (_, session) = store.issue(new_session("carol"));
        let span = session.expires_at - session.issued_at;
        assert_eq!(span.num_hours(), 24);
        // 非法值仍被钳制
        store.set_ttl_hours(0);
        assert_eq!(store.ttl_hours(), 1);
    }
}
