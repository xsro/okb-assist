//! 权限管理模块。
//!
//! 定义角色类型和权限检查辅助函数。

use axum::http::StatusCode;
use axum::response::{IntoResponse, Json, Response};
use serde_json::Value;
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

/// 权限信息（新格式）
///
/// config.json permissions 中每个条目的格式：
/// ```json
/// {"<token_hash>": {"access": "view-upload", "mcp": true}}
/// ```
#[derive(Clone, Debug)]
pub struct PermissionInfo {
    pub role: Role,
    pub mcp: bool,
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

/// 根据 token 匹配角色和权限。
///
/// 匹配顺序：admin (system.json token) → 逐个匹配 permissions 字典。
/// permissions 字典格式：{ token: { access: role, mcp: bool } }
pub fn match_role(
    token: &str,
    admin_token: &str,
    permission_map: &std::collections::HashMap<String, Value>,
) -> Option<PermissionInfo> {
    if token == admin_token {
        return Some(PermissionInfo {
            role: Role::Admin,
            mcp: true,
        });
    }

    if let Some(entry) = permission_map.get(token) {
        let access = entry
            .get("access")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        if let Some(role) = Role::from_str(access) {
            let mcp = entry
                .get("mcp")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            return Some(PermissionInfo { role, mcp });
        }
    }

    None
}

/// 会话归属标识。
///
/// 由访问 token 派生（sha256 前 32 位 hex），**不回传给前端**，仅用于归属比对：
/// - 同一个 token 的请求共享同一归属（同一人换浏览器仍能看到自己的会话）；
/// - 不同 token 即使角色相同也互不可见（避免同一角色的两个人互相看到对话）；
/// - 免鉴权（`token=change-me`、受信任子网代理、图片 URL 等）的请求归为 `local`。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OwnerKey(String);

impl OwnerKey {
    /// 由 token 派生归属标识
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

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// 检查 token 是否具有 MCP 访问权限（admin 或 permissions 中 mcp: true）
pub fn has_mcp_access(
    token: &str,
    admin_token: &str,
    mcp_token: &str,
    permission_map: &std::collections::HashMap<String, Value>,
) -> bool {
    // mcp_token 本身可以直接访问
    if !mcp_token.is_empty() && token == mcp_token {
        return true;
    }
    // admin 有 MCP 访问权
    if token == admin_token {
        return true;
    }
    // 检查 permission map
    if let Some(entry) = permission_map.get(token) {
        if entry
            .get("mcp")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
        {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn owner_key_is_stable_and_token_specific() {
        let a = OwnerKey::from_token("token-a");
        let b = OwnerKey::from_token("token-a");
        let c = OwnerKey::from_token("token-b");

        // 同一 token 稳定（同一人换浏览器仍能看到自己的会话）
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
}
