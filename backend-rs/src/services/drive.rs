//! 用户网盘（云盘）服务。
//!
//! 设计要点：
//!
//! - **每个用户一个私有目录**：`{drive_root}/users/{user_id}/`，只有本人可读写；
//! - **公有文件夹由管理员创建**：`{drive_root}/_public/{folder_id}/`，权限按**角色阈值**
//!   配置（`read_role` / `write_role`），不做逐用户 ACL；
//! - **文件系统即事实来源**：文件内容、目录结构直接落在磁盘上，`config.json` 的
//!   `drive` 段只存策略（开关、单文件上限、配额、公有文件夹定义）；
//! - **配额**：默认不限（`default_quota_mb = 0`），可按用户覆盖（`quota_overrides`）；
//!   统计方式是遍历该用户目录累加文件大小（本地工具、文件量小，足够用）；
//! - **路径安全**：所有相对路径逐段校验（拒绝 `..`、绝对路径、控制字符、超长段），
//!   并在拼接后校验 canonicalize 结果仍在根目录内（防符号链接逃逸）。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::auth::{Principal, Role};
use crate::config::Settings;
use crate::config_manager::ConfigManager;

/// 名称最大字节数（文件名 / 目录名）
pub const MAX_NAME_LEN: usize = 200;
/// 相对路径最大字节数
pub const MAX_PATH_LEN: usize = 1024;
/// 文本预览最多读取的字节数
pub const MAX_TEXT_PREVIEW_BYTES: u64 = 512 * 1024;
/// 私有目录与公有目录的区分目录名
pub const PRIVATE_DIR: &str = "users";
pub const PUBLIC_DIR: &str = "_public";

// ── 配置 ────────────────────────────────────────────────

/// `config.json` 的 `drive` 段。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DriveConfig {
    /// 是否启用网盘
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// 单个文件上传上限（MB）
    #[serde(default = "default_max_upload_mb")]
    pub max_upload_mb: u64,
    /// 默认配额（MB，0 = 不限）
    #[serde(default)]
    pub default_quota_mb: u64,
    /// 按用户覆盖配额（MB，0 = 不限）
    #[serde(default)]
    pub quota_overrides: HashMap<String, u64>,
    /// 公有文件夹定义
    #[serde(default)]
    pub public_folders: Vec<PublicFolder>,
}

fn default_true() -> bool {
    true
}

fn default_max_upload_mb() -> u64 {
    200
}

impl Default for DriveConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            max_upload_mb: default_max_upload_mb(),
            default_quota_mb: 0,
            quota_overrides: HashMap::new(),
            public_folders: Vec::new(),
        }
    }
}

impl DriveConfig {
    /// 单文件上限（字节）
    pub fn max_upload_bytes(&self) -> u64 {
        self.max_upload_mb.max(1) * 1024 * 1024
    }

    pub fn find_folder(&self, id: &str) -> Option<&PublicFolder> {
        self.public_folders.iter().find(|f| f.id == id)
    }
}

/// 公有文件夹：`read_role` / `write_role` 是**角色阈值**（角色等级 ≥ 该角色即可）。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PublicFolder {
    pub id: String,
    pub name: String,
    /// 可读取的最低角色（默认 view-only = 全站可读）
    #[serde(default = "default_read_role")]
    pub read_role: String,
    /// 可写入/上传/删除的最低角色（默认 admin）
    #[serde(default = "default_write_role")]
    pub write_role: String,
    #[serde(default)]
    pub created_at: String,
}

fn default_read_role() -> String {
    "view-only".to_string()
}

fn default_write_role() -> String {
    "admin".to_string()
}

// ── 角色阈值 ────────────────────────────────────────────

/// 角色等级：view-only(1) < view-upload(2) < admin(3)
pub fn role_rank(role: &Role) -> u8 {
    match role {
        Role::ViewOnly => 1,
        Role::ViewUpload => 2,
        Role::Admin => 3,
    }
}

/// 角色字符串 → 等级（非法值按最高门槛处理，避免误放行）
fn rank_of(role: &str) -> u8 {
    Role::from_str(role).map(|r| role_rank(&r)).unwrap_or(3)
}

impl PublicFolder {
    pub fn can_read(&self, role: &Role) -> bool {
        role_rank(role) >= rank_of(&self.read_role)
    }

    pub fn can_write(&self, role: &Role) -> bool {
        role_rank(role) >= rank_of(&self.write_role)
    }
}

// ── 错误 ────────────────────────────────────────────────

#[derive(Debug, thiserror::Error)]
pub enum DriveError {
    #[error("网盘未启用")]
    Disabled,
    #[error("路径非法")]
    InvalidPath,
    #[error("名称非法：不能包含 / \\\\ 或控制字符，且不超过 {MAX_NAME_LEN} 字节")]
    InvalidName,
    #[error("文件或目录不存在")]
    NotFound,
    #[error("目标目录不存在或不是目录")]
    NotDir,
    #[error("同名文件或目录已存在")]
    Exists,
    #[error("目录非空，需确认递归删除")]
    DirNotEmpty,
    #[error("超出租期配额：已用 {used} MB / 配额 {quota} MB")]
    QuotaExceeded { used: u64, quota: u64 },
    #[error("单个文件超过上限 {0} MB")]
    TooLarge(u64),
    #[error("无权访问该公有文件夹")]
    Forbidden,
    #[error("公有文件夹不存在")]
    FolderNotFound,
    #[error("文件操作失败：{0}")]
    Io(String),
}

impl DriveError {
    pub fn status_code(&self) -> axum::http::StatusCode {
        use axum::http::StatusCode;
        match self {
            DriveError::Disabled | DriveError::Forbidden => StatusCode::FORBIDDEN,
            DriveError::NotFound | DriveError::FolderNotFound => StatusCode::NOT_FOUND,
            DriveError::NotDir => StatusCode::BAD_REQUEST,
            DriveError::Exists | DriveError::DirNotEmpty => StatusCode::CONFLICT,
            DriveError::QuotaExceeded { .. } => StatusCode::PAYLOAD_TOO_LARGE,
            DriveError::TooLarge(_) => StatusCode::PAYLOAD_TOO_LARGE,
            DriveError::Io(_) => StatusCode::INTERNAL_SERVER_ERROR,
            _ => StatusCode::BAD_REQUEST,
        }
    }

    pub fn into_response(self) -> axum::response::Response {
        use axum::response::IntoResponse;
        (
            self.status_code(),
            axum::Json(json!({"detail": self.to_string()})),
        )
            .into_response()
    }
}

// ── 路径安全 ────────────────────────────────────────────

/// 校验单个文件名/目录名（不允许路径分隔符）
pub fn validate_name(name: &str) -> Result<&str, DriveError> {
    let name = name.trim();
    if name.is_empty()
        || name == "."
        || name == ".."
        || name.len() > MAX_NAME_LEN
        || name.contains('/')
        || name.contains('\\')
        || name.contains('\0')
        || name.chars().any(|c| c.is_control())
    {
        return Err(DriveError::InvalidName);
    }
    Ok(name)
}

/// 归一化相对路径：逐段校验，丢弃空段与 `.`，拒绝 `..`/绝对路径/控制字符。
pub fn sanitize_rel_path(rel: &str) -> Result<String, DriveError> {
    let rel = rel.trim();
    if rel.len() > MAX_PATH_LEN {
        return Err(DriveError::InvalidPath);
    }
    if rel.starts_with('/') || rel.starts_with('\\') {
        return Err(DriveError::InvalidPath);
    }
    let mut parts: Vec<&str> = Vec::new();
    for part in rel.split(['/', '\\']) {
        let part = part.trim();
        if part.is_empty() || part == "." {
            continue;
        }
        if part == ".." || part.len() > MAX_NAME_LEN || part.chars().any(|c| c.is_control()) {
            return Err(DriveError::InvalidPath);
        }
        parts.push(part);
    }
    Ok(parts.join("/"))
}

/// 把相对路径拼到 `base` 下，并校验结果不会逃出 `base`（含符号链接场景）。
///
/// `base` 不存在时会先创建（网盘根目录/用户目录懒创建）。
pub fn resolve_path(base: &Path, rel: &str) -> Result<PathBuf, DriveError> {
    std::fs::create_dir_all(base).map_err(|e| DriveError::Io(e.to_string()))?;
    let sanitized = sanitize_rel_path(rel)?;
    let mut path = base.to_path_buf();
    for part in sanitized.split('/').filter(|s| !s.is_empty()) {
        path.push(part);
    }
    let base_canon = base
        .canonicalize()
        .map_err(|e| DriveError::Io(e.to_string()))?;

    // 对「已存在」或「最近的已存在祖先」做 canonicalize 校验
    let mut probe = path.clone();
    loop {
        if probe.exists() {
            let canon = probe
                .canonicalize()
                .map_err(|e| DriveError::Io(e.to_string()))?;
            if !canon.starts_with(&base_canon) {
                return Err(DriveError::InvalidPath);
            }
            break;
        }
        match probe.parent() {
            Some(parent) if parent.starts_with(base) => probe = parent.to_path_buf(),
            _ => break,
        }
        if probe == base {
            break;
        }
    }

    Ok(path)
}

/// 公有文件夹 id 校验（同时防路径穿越）
pub fn validate_folder_id(id: &str) -> Result<&str, DriveError> {
    if id.is_empty()
        || id.len() > 32
        || !id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        return Err(DriveError::InvalidPath);
    }
    Ok(id)
}

// ── 作用域 ──────────────────────────────────────────────

/// 网盘作用域：私有目录 或 某个公有文件夹
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ScopeKind {
    Private,
    Public(String),
}

impl ScopeKind {
    pub fn as_str(&self) -> String {
        match self {
            ScopeKind::Private => "private".to_string(),
            ScopeKind::Public(id) => format!("public:{}", id),
        }
    }
}

/// 解析后的作用域（含物理根目录与写权限）
#[derive(Clone, Debug)]
pub struct ResolvedScope {
    pub kind: ScopeKind,
    pub base: PathBuf,
    pub can_write: bool,
    pub label: String,
}

// ── 目录项 ──────────────────────────────────────────────

#[derive(Clone, Debug)]
pub struct DriveEntry {
    pub name: String,
    pub is_dir: bool,
    pub size: u64,
    pub modified: Option<String>,
}

impl DriveEntry {
    pub fn to_json(&self) -> serde_json::Value {
        json!({
            "name": self.name,
            "is_dir": self.is_dir,
            "size": self.size,
            "modified": self.modified,
            "ext": if self.is_dir { "".to_string() } else {
                self.name.rsplit_once('.').map(|(_, e)| e.to_lowercase()).unwrap_or_default()
            },
        })
    }
}

// ── 服务 ────────────────────────────────────────────────

/// 网盘操作门面（配置读取 + 路径解析 + 配额计算）
pub struct DriveService {
    settings: Arc<Settings>,
    cm: Arc<ConfigManager>,
}

impl DriveService {
    pub fn new(settings: Arc<Settings>, cm: Arc<ConfigManager>) -> Self {
        Self { settings, cm }
    }

    /// 读取 `config.json` 的 `drive` 段（解析失败时回退默认值）
    pub fn config(&self) -> DriveConfig {
        serde_json::from_value(self.cm.get_service_config()["drive"].clone()).unwrap_or_default()
    }

    /// 网盘根目录（相对 `cwd`，进程启动时已 chdir）
    pub fn root(&self) -> PathBuf {
        PathBuf::from(self.settings.drive_root())
    }

    /// 私有目录：`{drive_root}/users/{user_id}`
    pub fn private_base(&self, user_id: &str) -> Result<PathBuf, DriveError> {
        validate_folder_id(user_id)?;
        Ok(self.root().join(PRIVATE_DIR).join(user_id))
    }

    /// 公有目录：`{drive_root}/_public/{folder_id}`
    pub fn public_base(&self, folder_id: &str) -> Result<PathBuf, DriveError> {
        validate_folder_id(folder_id)?;
        Ok(self.root().join(PUBLIC_DIR).join(folder_id))
    }

    /// 解析 `scope` 参数（`private` / `public:<id>`），并判定当前主体的读写权限
    pub fn resolve_scope(
        &self,
        principal: &Principal,
        scope: &str,
    ) -> Result<ResolvedScope, DriveError> {
        let config = self.config();
        if !config.enabled {
            return Err(DriveError::Disabled);
        }
        let scope = scope.trim();
        if scope.is_empty() || scope == "private" {
            let base = self.private_base(&principal.user_id)?;
            return Ok(ResolvedScope {
                kind: ScopeKind::Private,
                base,
                can_write: true,
                label: "我的网盘".to_string(),
            });
        }

        let Some(folder_id) = scope.strip_prefix("public:") else {
            return Err(DriveError::InvalidPath);
        };
        validate_folder_id(folder_id)?;
        let folder = config.find_folder(folder_id).ok_or(DriveError::FolderNotFound)?;
        if !folder.can_read(&principal.role) {
            return Err(DriveError::Forbidden);
        }
        Ok(ResolvedScope {
            kind: ScopeKind::Public(folder_id.to_string()),
            base: self.public_base(folder_id)?,
            can_write: folder.can_write(&principal.role),
            label: folder.name.clone(),
        })
    }

    /// 校验「再加 `extra` 字节」是否会超过配额（返回带上下文的错误，便于上层直接响应）
    pub fn check_quota(
        &self,
        config: &DriveConfig,
        user_id: &str,
        used: u64,
        extra: u64,
    ) -> Result<(), DriveError> {
        if let Some(quota) = self.quota_bytes(config, user_id) {
            if used.saturating_add(extra) > quota {
                return Err(DriveError::QuotaExceeded {
                    used: (used + extra) / (1024 * 1024),
                    quota: quota / (1024 * 1024),
                });
            }
        }
        Ok(())
    }

    /// 校验单文件大小是否超过上限
    pub fn check_size(&self, config: &DriveConfig, size: u64) -> Result<(), DriveError> {
        if size > config.max_upload_bytes() {
            return Err(DriveError::TooLarge(config.max_upload_mb));
        }
        Ok(())
    }

    /// 当前用户在某作用域下的配额（字节）；`None` 表示不限
    pub fn quota_bytes(&self, config: &DriveConfig, user_id: &str) -> Option<u64> {
        let mb = config
            .quota_overrides
            .get(user_id)
            .copied()
            .unwrap_or(config.default_quota_mb);
        if mb == 0 {
            None
        } else {
            Some(mb * 1024 * 1024)
        }
    }

    /// 统计目录占用（字节）
    pub fn used_bytes(&self, dir: &Path) -> u64 {
        if !dir.exists() {
            return 0;
        }
        let mut total = 0u64;
        for entry in walkdir::WalkDir::new(dir)
            .follow_links(false)
            .into_iter()
            .filter_map(|e| e.ok())
        {
            if entry.file_type().is_file() {
                total += entry.metadata().map(|m| m.len()).unwrap_or(0);
            }
        }
        total
    }
}

/// 列出目录项（目录在前，名称不区分大小写排序）
pub fn list_dir(dir: &Path) -> Result<Vec<DriveEntry>, DriveError> {
    if !dir.exists() {
        // 空目录视为空列表（首次进入用户目录时尚未创建）
        return Ok(Vec::new());
    }
    if !dir.is_dir() {
        return Err(DriveError::NotDir);
    }
    let mut entries = Vec::new();
    let meta_ok = |p: &Path| std::fs::symlink_metadata(p).ok();
    let read = std::fs::read_dir(dir).map_err(|e| DriveError::Io(e.to_string()))?;
    for item in read.flatten() {
        let name = item.file_name().to_string_lossy().to_string();
        let path = item.path();
        let Some(meta) = meta_ok(&path) else {
            continue;
        };
        // 跳过符号链接（避免逃逸与循环）
        if meta.file_type().is_symlink() {
            continue;
        }
        let is_dir = meta.is_dir();
        let modified = meta.modified().ok().map(|t| {
            let dt: chrono::DateTime<chrono::Utc> = t.into();
            dt.to_rfc3339()
        });
        entries.push(DriveEntry {
            name,
            is_dir,
            size: if is_dir { 0 } else { meta.len() },
            modified,
        });
    }
    entries.sort_by(|a, b| {
        b.is_dir
            .cmp(&a.is_dir)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    Ok(entries)
}

/// 以「自动加后缀」的方式在目录内挑一个不冲突的名字（如 `a.txt` → `a (1).txt`）
pub fn unique_child(dir: &Path, name: &str) -> PathBuf {
    let candidate = dir.join(name);
    if !candidate.exists() {
        return candidate;
    }
    let (stem, ext) = match name.rsplit_once('.') {
        Some((s, e)) if !s.is_empty() => (s.to_string(), Some(e.to_string())),
        _ => (name.to_string(), None),
    };
    for i in 1..1000 {
        let filename = match &ext {
            Some(ext) => format!("{} ({}).{}", stem, i, ext),
            None => format!("{} ({})", stem, i),
        };
        let path = dir.join(&filename);
        if !path.exists() {
            return path;
        }
    }
    dir.join(format!("{}-{}", stem, chrono::Utc::now().timestamp()))
}

/// 根据扩展名推断 Content-Type（网盘预览/下载用，覆盖常见类型即可）
pub fn content_type_for(name: &str) -> &'static str {
    let ext = name
        .rsplit_once('.')
        .map(|(_, e)| e.to_ascii_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "bmp" => "image/bmp",
        "svg" => "image/svg+xml",
        "pdf" => "application/pdf",
        "txt" | "log" => "text/plain; charset=utf-8",
        "md" | "markdown" => "text/markdown; charset=utf-8",
        "json" => "application/json; charset=utf-8",
        "csv" => "text/csv; charset=utf-8",
        "html" | "htm" => "text/plain; charset=utf-8",
        "xml" => "application/xml; charset=utf-8",
        "yml" | "yaml" => "text/yaml; charset=utf-8",
        "rs" | "ts" | "js" | "py" | "sh" | "c" | "h" | "cpp" | "go" | "java" | "toml"
        | "ini" | "conf" | "css" | "sql" => "text/plain; charset=utf-8",
        "zip" => "application/zip",
        "gz" => "application/gzip",
        "tar" => "application/x-tar",
        "mp3" => "audio/mpeg",
        "wav" => "audio/wav",
        "mp4" => "video/mp4",
        "webm" => "video/webm",
        _ => "application/octet-stream",
    }
}

/// 是否属于「可在浏览器里直接预览」的类型（图片 / PDF / 纯文本）
pub fn is_previewable(name: &str) -> bool {
    let ct = content_type_for(name);
    ct.starts_with("image/")
        || ct == "application/pdf"
        || ct.starts_with("text/")
        || ct.starts_with("application/json")
        || ct == "application/xml; charset=utf-8"
        || ct == "text/yaml; charset=utf-8"
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::SessionStore;
    use crate::users::NewUser;

    fn temp_service() -> (tempfile::TempDir, DriveService) {
        let dir = tempfile::tempdir().unwrap();
        let system = dir.path().join("system.json");
        std::fs::write(
            &system,
            serde_json::json!({ "drive_root": dir.path().join("drive") }).to_string(),
        )
        .unwrap();
        let cm = Arc::new(ConfigManager::for_tests(
            dir.path().join("config.json"),
            system,
        ));
        // 写入 drive 段（`drive` 是受保护段，save_config 不会写它，必须走 update_section）
        cm.update_section("drive", |section| {
            *section = serde_json::json!({
                "enabled": true,
                "max_upload_mb": 5,
                "default_quota_mb": 10,
                "quota_overrides": {"u_big": 100},
                "public_folders": [
                    {"id": "pf_team", "name": "团队资料", "read_role": "view-only", "write_role": "admin"},
                    {"id": "pf_vip", "name": "仅管理员", "read_role": "admin", "write_role": "admin"}
                ]
            });
            Ok(())
        })
        .unwrap();
        let settings = Arc::new(Settings::new(cm.clone()));
        (dir, DriveService::new(settings, cm))
    }

    fn principal(user: &str, role: Role) -> Principal {
        Principal {
            user_id: user.to_string(),
            username: user.to_string(),
            role,
            mcp: false,
            via: crate::auth::CredentialKind::Session,
            token_id: None,
        }
    }

    #[test]
    fn rejects_dangerous_paths() {
        assert!(sanitize_rel_path("../etc/passwd").is_err());
        assert!(sanitize_rel_path("a/../../b").is_err());
        assert!(sanitize_rel_path("/abs/path").is_err());
        assert!(sanitize_rel_path("a/\0b").is_err());
        assert_eq!(sanitize_rel_path("a//b/./c").unwrap(), "a/b/c");
        assert_eq!(sanitize_rel_path("  ").unwrap(), "");
        assert!(sanitize_rel_path(&"x".repeat(MAX_PATH_LEN + 1)).is_err());

        assert!(validate_name("..").is_err());
        assert!(validate_name("a/b").is_err());
        assert!(validate_name("normal.md").is_ok());
        assert!(validate_folder_id("../../etc").is_err());
        assert!(validate_folder_id("pf_1-2").is_ok());
    }

    #[test]
    fn resolve_path_stays_inside_base() {
        let (dir, svc) = temp_service();
        let base = svc.private_base("u_test").unwrap();
        std::fs::create_dir_all(&base).unwrap();
        let p = resolve_path(&base, "a/b/c.txt").unwrap();
        assert!(p.starts_with(&base));
        assert_eq!(p.file_name().unwrap(), "c.txt");

        // 符号链接指向外部 → 必须被拒绝
        #[cfg(unix)]
        {
            let outside = dir.path().join("outside");
            std::fs::create_dir_all(&outside).unwrap();
            std::os::unix::fs::symlink(&outside, base.join("escape")).unwrap();
            assert!(resolve_path(&base, "escape/secret.txt").is_err());
        }
    }

    #[test]
    fn role_thresholds_gate_public_folders() {
        let (_dir, svc) = temp_service();
        let viewer = principal("u_v", Role::ViewOnly);
        let uploader = principal("u_u", Role::ViewUpload);
        let admin = principal("u_a", Role::Admin);

        // 团队资料：view-only 可读、只有 admin 可写
        let s = svc.resolve_scope(&viewer, "public:pf_team").unwrap();
        assert!(s.can_write == false);
        assert!(s.base.ends_with("_public/pf_team"));
        assert!(svc.resolve_scope(&uploader, "public:pf_team").unwrap().can_write == false);
        assert!(svc.resolve_scope(&admin, "public:pf_team").unwrap().can_write);

        // 仅管理员可见的文件夹
        assert!(matches!(
            svc.resolve_scope(&viewer, "public:pf_vip"),
            Err(DriveError::Forbidden)
        ));
        assert!(svc.resolve_scope(&admin, "public:pf_vip").is_ok());

        // 未知文件夹 / 非法 scope
        assert!(matches!(
            svc.resolve_scope(&admin, "public:nope"),
            Err(DriveError::FolderNotFound)
        ));
        assert!(matches!(
            svc.resolve_scope(&admin, "weird"),
            Err(DriveError::InvalidPath)
        ));
    }

    #[test]
    fn private_scope_is_per_user() {
        let (_dir, svc) = temp_service();
        let a = svc.resolve_scope(&principal("u_a", Role::Admin), "private").unwrap();
        let b = svc.resolve_scope(&principal("u_b", Role::Admin), "private").unwrap();
        assert_ne!(a.base, b.base);
        assert!(a.can_write && b.can_write);
        assert_eq!(a.label, "我的网盘");
    }

    #[test]
    fn quota_uses_override_then_default() {
        let (_dir, svc) = temp_service();
        let cfg = svc.config();
        assert_eq!(cfg.max_upload_bytes(), 5 * 1024 * 1024);
        assert_eq!(svc.quota_bytes(&cfg, "u_big"), Some(100 * 1024 * 1024));
        assert_eq!(svc.quota_bytes(&cfg, "u_normal"), Some(10 * 1024 * 1024));
        // 0 = 不限
        let mut cfg2 = cfg.clone();
        cfg2.default_quota_mb = 0;
        cfg2.quota_overrides.clear();
        assert_eq!(svc.quota_bytes(&cfg2, "u_normal"), None);
    }

    #[test]
    fn disabled_drive_is_rejected() {
        let (_dir, svc) = temp_service();
        let mut cfg = svc.config();
        cfg.enabled = false;
        let value = serde_json::to_value(&cfg).unwrap();
        svc.cm
            .update_section("drive", |section| {
                *section = value.clone();
                Ok(())
            })
            .unwrap();
        assert!(matches!(
            svc.resolve_scope(&principal("u_a", Role::Admin), "private"),
            Err(DriveError::Disabled)
        ));
    }

    #[test]
    fn list_and_unique_names() {
        let (dir, svc) = temp_service();
        let base = svc.private_base("u_list").unwrap();
        resolve_path(&base, "docs").unwrap();
        std::fs::create_dir_all(base.join("docs")).unwrap();
        std::fs::write(base.join("b.txt"), b"hello").unwrap();
        std::fs::write(base.join("a.txt"), b"hi").unwrap();
        std::fs::write(base.join("docs").join("inner.md"), b"# x").unwrap();

        let entries = list_dir(&base).unwrap();
        let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, vec!["docs", "a.txt", "b.txt"], "目录在前、名称排序");
        assert_eq!(entries[0].size, 0);
        assert_eq!(entries[1].size, 2);

        // 自动改名
        let p = unique_child(&base, "a.txt");
        assert_eq!(p.file_name().unwrap(), "a (1).txt");
        std::fs::write(&p, b"x").unwrap();
        assert_eq!(
            unique_child(&base, "a.txt").file_name().unwrap(),
            "a (2).txt"
        );

        // 统计占用
        assert_eq!(svc.used_bytes(&base), 2 + 5 + 3 + 1);
        assert_eq!(svc.used_bytes(&dir.path().join("nope")), 0);
    }

    #[test]
    fn content_types_and_preview() {
        assert_eq!(content_type_for("a.PNG"), "image/png");
        assert_eq!(content_type_for("a.pdf"), "application/pdf");
        assert_eq!(content_type_for("a.md"), "text/markdown; charset=utf-8");
        assert_eq!(content_type_for("noext"), "application/octet-stream");
        assert!(is_previewable("a.png"));
        assert!(is_previewable("a.pdf"));
        assert!(is_previewable("a.txt"));
        assert!(!is_previewable("a.zip"));
        assert!(!is_previewable("a.exe"));
    }

    // 让未使用的导入（用户存储/会话）不告警：仅测试编译期检查
    #[allow(dead_code)]
    fn _touch(users: &crate::users::UserStore, sessions: &SessionStore) {
        let _ = users.create(NewUser::default());
        let _ = sessions.len();
    }
}
