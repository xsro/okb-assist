//! 权限管理模块。
//!
//! 定义角色类型和权限检查辅助函数。

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

/// 根据 token 匹配角色。
///
/// 匹配顺序：admin (system.json token) → 逐个匹配 permissions 字典 { token: role }
pub fn match_role(
    token: &str,
    admin_token: &str,
    permission_map: &std::collections::HashMap<String, String>,
) -> Option<Role> {
    if token == admin_token {
        return Some(Role::Admin);
    }

    if let Some(role_str) = permission_map.get(token) {
        return Role::from_str(role_str);
    }

    None
}