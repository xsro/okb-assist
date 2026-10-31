//! 问答会话的文件存储。
//!
//! **一个会话一个文件**（默认 `data/chat_sessions/{id}.json`，模板取自
//! `system.json` 的 `chat_session_path`），不使用数据库：
//! - 只有用户在前端显式点击「保存」才会落盘（后端不在对话过程中写任何东西）；
//! - 文件内同时保存展示条目（`items`）与发给模型的完整 transcript
//!   （含 `assistant.tool_calls` 与 `tool` 结果），以便重新打开后能继续对话；
//! - `owner_key` 由**用户 id** 派生（`u:<user_id>`，见 `auth::OwnerKey::from_user`），
//!   免鉴权请求为 `local`；旧文件可能是 `t:<sha256(token) 前 32 位>` 的旧格式。只在服务端用于归属比对，
//!   API 响应里只回传 `mine` 与归属角色；
//! - 写入为原子写（先写 `*.tmp` 再 rename），避免半截文件。
//!
//! 会话 ID 形如 `20261015T155512123-4f2a9c`（UTC 时间 + 随机后缀），字典序即时间序；
//! 读取时对 ID 做白名单校验（仅 `[A-Za-z0-9_-]`），防止路径穿越。

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::config::Settings;
use crate::paths;

/// 会话文件格式版本（后续字段变更时用于兼容处理）
pub const SESSION_FORMAT_VERSION: u32 = 1;
/// 会话 ID 最大长度
const MAX_ID_LEN: usize = 64;

/// 一个会话文件的内容
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredSession {
    /// 文件格式版本
    #[serde(default)]
    pub version: u32,
    /// 会话 ID（与文件名一致）
    #[serde(default)]
    pub id: String,
    /// 标题（默认取首条用户消息）
    #[serde(default)]
    pub title: String,
    /// 归属人标识（`u:<user_id>`，旧格式为 `t:<sha256>`，免鉴权为 `local`）；**不对外回传**
    #[serde(default)]
    pub owner_key: String,
    /// 保存时的角色（admin / view-only / view-upload）
    #[serde(default)]
    pub role: String,
    /// 保存时使用的模型池条目名
    #[serde(default)]
    pub model: String,
    /// 保存时使用的工具协议
    #[serde(default)]
    pub tool_mode: String,
    /// 展示条目数
    #[serde(default)]
    pub message_count: i64,
    #[serde(default)]
    pub created_at: String,
    #[serde(default)]
    pub updated_at: String,
    /// 前端展示条目（role/content/steps/mode/notes，结构由前端定义）
    #[serde(default)]
    pub items: Value,
    /// 发给模型的完整 transcript
    #[serde(default)]
    pub transcript: Value,
}

impl StoredSession {
    /// 列表/响应用的元信息（**不含 owner_key**，只给 `mine`）
    pub fn meta(&self, owner: &str) -> Value {
        serde_json::json!({
            "id": self.id,
            "title": self.title,
            "role": self.role,
            "model": self.model,
            "tool_mode": self.tool_mode,
            "message_count": self.message_count,
            "created_at": self.created_at,
            "updated_at": self.updated_at,
            "mine": self.owner_key == owner,
        })
    }
}

/// 会话 ID 是否合法（白名单字符，长度受限）——用于拼接文件名前的防穿越校验
pub fn is_valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= MAX_ID_LEN
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// 生成新会话 ID：UTC 时间（到毫秒）+ 随机短后缀，字典序即时间序
pub fn new_id() -> String {
    let uuid = uuid::Uuid::new_v4().simple().to_string();
    format!(
        "{}-{}",
        chrono::Utc::now().format("%Y%m%dT%H%M%S%3f"),
        &uuid[..6]
    )
}

/// 当前 UTC 时间（与项目其他时间字段一致：秒级 + Z）
pub fn now_iso() -> String {
    crate::utils::now_iso()
}

/// 写入会话文件（原子写：先写临时文件再 rename）
pub fn save(settings: &Settings, session: &StoredSession) -> std::io::Result<()> {
    let path = paths::get_chat_session_path_buf(settings, &session.id);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let body = serde_json::to_string_pretty(session).unwrap_or_else(|_| "{}".to_string());
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, body)?;
    std::fs::rename(&tmp, &path)?;
    Ok(())
}

/// 读取会话文件；不存在返回 `Ok(None)`，坏 JSON 返回 `Err`
pub fn load(settings: &Settings, id: &str) -> std::io::Result<Option<StoredSession>> {
    if !is_valid_id(id) {
        return Ok(None);
    }
    let path = paths::get_chat_session_path_buf(settings, id);
    match std::fs::read_to_string(&path) {
        Ok(text) => match serde_json::from_str::<StoredSession>(&text) {
            Ok(mut session) => {
                if session.id.is_empty() {
                    session.id = id.to_string();
                }
                Ok(Some(session))
            }
            Err(e) => Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("会话文件 {} 解析失败: {}", path.display(), e),
            )),
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

/// 列出所有会话（按 `updated_at` 倒序；`updated_at` 缺失时回退用 ID 排序）
///
/// 目录中无法解析的文件会被跳过并记日志，不影响其余会话。
pub fn list(settings: &Settings) -> std::io::Result<Vec<StoredSession>> {
    let Some(dir) = paths::get_chat_session_dir(settings) else {
        return Ok(Vec::new());
    };
    if !dir.exists() {
        return Ok(Vec::new());
    }

    let mut sessions = Vec::new();
    for entry in std::fs::read_dir(&dir)? {
        let entry = entry?;
        let path = entry.path();
        if !path.is_file() || path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let Some(id) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        if !is_valid_id(id) {
            continue;
        }
        match load(settings, id) {
            Ok(Some(session)) => sessions.push(session),
            Ok(None) => {}
            Err(e) => tracing::warn!("跳过无法解析的会话文件: {}", e),
        }
    }

    sessions.sort_by(|a, b| {
        let ka = if a.updated_at.is_empty() { &a.id } else { &a.updated_at };
        let kb = if b.updated_at.is_empty() { &b.id } else { &b.updated_at };
        kb.cmp(ka)
    });
    Ok(sessions)
}

/// 删除会话文件；返回是否真的删掉了
pub fn delete(settings: &Settings, id: &str) -> std::io::Result<bool> {
    if !is_valid_id(id) {
        return Ok(false);
    }
    let path = paths::get_chat_session_path_buf(settings, id);
    if !path.exists() {
        return Ok(false);
    }
    std::fs::remove_file(&path)?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn id_validation_blocks_path_traversal() {
        assert!(is_valid_id("20261015T155512123-4f2a9c"));
        assert!(is_valid_id("abc_DEF-123"));
        assert!(!is_valid_id("../etc/passwd"));
        assert!(!is_valid_id("a/b"));
        assert!(!is_valid_id("a\\b"));
        assert!(!is_valid_id(".."));
        assert!(!is_valid_id(""));
        assert!(!is_valid_id("会话"));
        assert!(!is_valid_id(&"x".repeat(MAX_ID_LEN + 1)));
    }

    #[test]
    fn new_id_is_sortable_and_unique() {
        let a = new_id();
        let b = new_id();
        assert!(is_valid_id(&a));
        assert_ne!(a, b);
        // 字典序即时间序（同一毫秒内用随机后缀区分，不保证顺序，但格式一致）
        assert_eq!(a.len(), 18 + 1 + 6);
    }

    #[test]
    fn meta_hides_owner_key_and_marks_mine() {
        let session = StoredSession {
            version: SESSION_FORMAT_VERSION,
            id: "s1".to_string(),
            title: "t".to_string(),
            owner_key: "t:abc".to_string(),
            role: "view-only".to_string(),
            model: "m".to_string(),
            tool_mode: "auto".to_string(),
            message_count: 2,
            created_at: "2026-01-01T00:00:00Z".to_string(),
            updated_at: "2026-01-01T00:00:00Z".to_string(),
            items: serde_json::json!([]),
            transcript: serde_json::json!([]),
        };
        let mine = session.meta("t:abc");
        assert_eq!(mine["mine"], true);
        assert!(mine.get("owner_key").is_none());
        assert_eq!(session.meta("t:other")["mine"], false);
    }
}
