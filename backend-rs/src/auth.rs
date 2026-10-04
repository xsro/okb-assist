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