//! 静态目录挂载（`system.json` 的 `static_dirs` 段）。
//!
//! 把本地一个文件夹挂到任意 URL 前缀下，直接对外提供只读文件服务（下载 PDF、
//! 放前端 demo、把导出的 Markdown 目录给外部工具抓取等）。
//!
//! 配置示例：
//!
//! ```json
//! "static_dirs": [
//!   {
//!     "path": "/static",
//!     "dir": "data/static",
//!     "try_index": true,
//!     "index": "index.html",
//!     "list_dir": false,
//!     "auth": true,
//!     "cache_control": true
//!   }
//! ]
//! ```
//!
//! 字段含义（除 `path` / `dir` 外均可省略）：
//!
//! | 字段 | 默认 | 说明 |
//! |---|---|---|
//! | `path` | — | URL 前缀，必填，以 `/` 开头；`/static/a.txt` 落到 `dir/a.txt` |
//! | `dir` | — | 本地目录，必填；相对路径按 `cwd` 解析，支持 `{system_dir}` / `{env:VAR}` |
//! | `try_index` | `true` | 命中目录且目录下有 `index` 指定的文件时，直接返回该文件 |
//! | `index` | `index.html` | 首页文件名 |
//! | `list_dir` | `false` | 目录下没有首页时返回简单文件列表（默认关闭，避免意外泄露目录内容） |
//! | `auth` | `true` | 是否需要凭证（会话 / 用户令牌 / 受信任子网）；`false` 表示完全公开 |
//! | `cache_control` | `true` | 是否附加 `Cache-Control`（静态资源长缓存，`index.html` 不缓存） |
//!
//! 安全约定：
//! - 请求路径先做 percent-decoding，然后拒绝 `..`、反斜杠与 NUL；
//! - 最终路径必须 `canonicalize()` 且仍在挂载根目录内，因此指向目录外的软链同样被拒；
//! - 只注册 `GET`/`HEAD`（axum 的 `get()` 自动兼顾 HEAD），其他方法返回 405；
//! - 挂载前缀不能与内置路由冲突，也不能互相嵌套（`/a` 与 `/a/b` 会在 matchit 里冲突），
//!   冲突条目在启动时被跳过并打 `warn`；匹配时最长前缀优先；
//! - `system.json` 需重启进程生效（路由在启动时注册）。

use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};

use crate::auth;
use crate::config::Settings;

/// 一条静态目录挂载配置（`dir` 已解析为绝对路径）。
#[derive(Clone, Debug)]
pub struct StaticMount {
    /// URL 前缀：`/` 开头、无结尾斜杠
    pub path: String,
    /// 本地根目录（绝对路径）
    pub dir: String,
    /// 命中目录时是否补首页
    pub try_index: bool,
    /// 首页文件名
    pub index: String,
    /// 目录无首页时是否返回文件列表
    pub list_dir: bool,
    /// 是否需要凭证
    pub auth: bool,
    /// 是否附加 Cache-Control
    pub cache_control: bool,
}

impl StaticMount {
    /// 从 `static_dirs` 的一项解析出挂载配置；`path`/`dir` 缺失或非法时返回 `None`。
    pub fn from_value(value: &serde_json::Value, cwd: &str, system_dir: &str) -> Option<Self> {
        let obj = value.as_object()?;

        // ── path：URL 前缀 ──
        let raw_path = obj.get("path")?.as_str()?.trim();
        if !raw_path.starts_with('/') {
            tracing::warn!(path = %raw_path, "static_dirs 的 path 必须以 / 开头，已忽略该挂载");
            return None;
        }
        let path = raw_path.trim_end_matches('/').to_string();
        if path.is_empty() {
            tracing::warn!("static_dirs 不允许挂载到根路径 /，已忽略该挂载");
            return None;
        }

        // ── dir：本地目录 ──
        let raw_dir = obj.get("dir")?.as_str()?.trim();
        if raw_dir.is_empty() {
            tracing::warn!(path = %path, "static_dirs 的 dir 为空，已忽略该挂载");
            return None;
        }
        let expanded = crate::config_manager::ConfigManager::substitute_path_variables_str(
            &raw_dir.replace("{system_dir}", system_dir),
            "",
        );
        let dir = if Path::new(&expanded).is_absolute() {
            expanded
        } else {
            Path::new(cwd).join(&expanded).to_string_lossy().to_string()
        };

        let flag = |key: &str, default: bool| obj.get(key).and_then(|v| v.as_bool()).unwrap_or(default);
        let index = obj
            .get("index")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or("index.html")
            .to_string();

        Some(Self {
            path,
            dir,
            try_index: flag("try_index", true),
            index,
            list_dir: flag("list_dir", false),
            auth: flag("auth", true),
            cache_control: flag("cache_control", true),
        })
    }
}

/// 与既有路由冲突的 URL 前缀（挂到这些前缀下会导致路由重复注册并 panic）。
const RESERVED_PREFIXES: &[&str] = &[
    "/assist/api",
    "/assist/mcp",
    "/assist/uploads",
    "/assist/file",
    "/assist/assets",
    "/assist/openapi",
];

/// 读取并归一化 `system.json` 的 `static_dirs` 段（启动时调用一次）。
///
/// 非法 / 重复 / 与内置路由冲突的条目会被跳过并打警告日志。
pub fn load_mounts(settings: &Settings) -> Vec<StaticMount> {
    let cwd = settings.cwd();
    let system_dir = settings.system_dir();
    let mut mounts: Vec<StaticMount> = Vec::new();

    for raw in settings.static_dirs() {
        let Some(mount) = StaticMount::from_value(&raw, &cwd, &system_dir) else {
            continue;
        };
        if RESERVED_PREFIXES
            .iter()
            .any(|p| mount.path == *p || mount.path.starts_with(&format!("{p}/")))
        {
            tracing::warn!(path = %mount.path, "static_dirs 的 path 与内置路由冲突，已忽略该挂载");
            continue;
        }
        if mounts.iter().any(|m| m.path == mount.path) {
            tracing::warn!(path = %mount.path, "static_dirs 出现重复的 path，已忽略后一项");
            continue;
        }
        if !std::path::Path::new(&mount.dir).is_dir() {
            tracing::warn!(path = %mount.path, dir = %mount.dir, "static_dirs 的 dir 不存在或不是目录（挂载仍启用，请求会返回 404）");
        }
        mounts.push(mount);
    }

    // 最长前缀优先（/static/sub 不能被 /static 抢走）；等长时保持配置顺序
    mounts.sort_by_key(|m| std::cmp::Reverse(m.path.len()));

    // 路由层面不允许前缀嵌套（`/a` 与 `/a/b` 会在 matchit 里冲突并导致启动 panic），
    // 嵌套的多条只保留更长（更具体）的那条。
    let mut kept: Vec<StaticMount> = Vec::with_capacity(mounts.len());
    for mount in mounts {
        if let Some(other) = kept.iter().find(|k| is_nested(&k.path, &mount.path)) {
            tracing::warn!(
                path = %mount.path,
                other = %other.path,
                "static_dirs 的 path 与其他挂载存在前缀嵌套，已忽略该挂载（请只保留一条更长前缀）"
            );
            continue;
        }
        kept.push(mount);
    }
    kept
}

/// 两个前缀是否存在嵌套关系（按路径分段比较，避免 `/a` 与 `/ab` 被误判）。
fn is_nested(a: &str, b: &str) -> bool {
    a == b || a.starts_with(&format!("{b}/")) || b.starts_with(&format!("{a}/"))
}

/// 为一个挂载前缀生成路由模式：`/prefix`、`/prefix/` 与 `/prefix/*rest`。
///
/// 中间那条不能省：axum 的 `/*rest` 通配符不匹配空的剩余路径，
/// 缺了它就访问不了 `/prefix/`（目录首页）。
pub fn route_patterns(prefix: &str) -> [String; 3] {
    [
        prefix.to_string(),
        format!("{prefix}/"),
        format!("{}/*rest", prefix),
    ]
}

/// 静态目录的请求处理器。
///
/// 注册在 `get()` 上，因此只响应 `GET`/`HEAD`（axum 会为 HEAD 自动去掉响应体），
/// 其他方法由 axum 返回 405。
///
/// 鉴权顺序（`auth: true` 时）：会话 cookie → `X-Token`/`Bearer`/`?token=` → 受信任子网。
/// 未通过时：浏览器导航（`Accept` 含 `text/html`）重定向到 SPA 登录页并带上
/// `?redirect=`，登录后回跳；其余请求返回 401 JSON。
pub async fn serve_mount(req: axum::extract::Request) -> Response {
    let path = req.uri().path().to_string();

    let Some(settings) = req.extensions().get::<Arc<Settings>>().cloned() else {
        return json_error(StatusCode::INTERNAL_SERVER_ERROR, "配置未注入");
    };

    // 最长前缀匹配（load_mounts 已按长度倒序）
    let mounts = load_mounts(&settings);
    let mount = mounts
        .into_iter()
        .find(|m| path == m.path || path.starts_with(&format!("{}/", m.path)));
    let Some(mount) = mount else {
        return not_found();
    };

    // 鉴权（默认需要凭证；`auth: false` 才公开）
    let mut issue_cookie: Option<HeaderValue> = None;
    if mount.auth {
        match authorize(&req, &settings) {
            Credential::Cookie => {}
            // 用 header / ?token= 认证成功时顺手下发 cookie，
            // 这样页面里的相对资源（图片/子页面）不必再带 token
            Credential::Other(token) => {
                issue_cookie = session_cookie_header(&token, session_ttl_hours(&req));
            }
            Credential::None => return unauthorized_response(&req),
        }
    }

    let response = serve_located(&path, req.uri().query(), &mount).await;
    match issue_cookie {
        Some(cookie) => {
            let mut response = response;
            response.headers_mut().insert(header::SET_COOKIE, cookie);
            response
        }
        None => response,
    }
}

/// 静态目录专用会话 cookie 名。
///
/// 与 `token_middleware`（`X-Token`/`Bearer`）**完全分开**：API 不读这个 cookie，
/// 只为「浏览器直接导航到静态目录」这一种场景下发，避免扩大 API 的鉴权面。
pub const COOKIE_NAME: &str = "okb_static";

/// 鉴权结果
enum Credential {
    /// 本模块下发的会话 cookie 有效
    Cookie,
    /// 用 `?token=` / 头部认证成功，值为 token（需下发 cookie）
    Other(String),
    /// 未通过
    None,
}

fn authorize(req: &axum::extract::Request, settings: &Settings) -> Credential {
    let stores = (
        req.extensions().get::<Arc<crate::users::UserStore>>(),
        req.extensions().get::<Arc<crate::session::SessionStore>>(),
    );
    if let (Some(store), Some(sessions)) = stores {
        // 1) 本模块下发的 cookie（浏览器直接导航）
        if let Some(token) = cookie_token(req.headers()) {
            if auth::resolve_principal(&token, store, sessions).is_some_and(|p| p.api) {
                return Credential::Cookie;
            }
        }
        // 2) 显式凭证：X-Token / Authorization / ?token=（MCP 专属令牌不能用于静态目录）
        if let Some(token) = auth::extract_token(req) {
            if auth::resolve_principal(&token, store, sessions).is_some_and(|p| p.api) {
                return Credential::Other(token);
            }
        }
    }
    // 3) 受信任子网与 API 同一道门禁：必须显式 trusted_subnet_enabled = true
    if settings.trusted_subnet_enabled() {
        let subnets = settings.trusted_subnets();
        if !subnets.is_empty() {
            let client_ip = crate::extract_client_ip(req);
            if crate::utils::ip_matches_subnets(&client_ip, &subnets) {
                return Credential::Cookie; // 无需 cookie，直接放行
            }
        }
    }
    Credential::None
}

/// 会话 cookie 的 `Max-Age`（取自 `users.session_ttl_hours`，默认 7 天）。
fn session_ttl_hours(req: &axum::extract::Request) -> i64 {
    req.extensions()
        .get::<Arc<crate::users::UserStore>>()
        .map(|store| store.section().session_ttl_hours as i64)
        .filter(|hours| *hours > 0)
        .unwrap_or(168)
}

/// 构造下发 cookie 的 `Set-Cookie` 头值（token 仅允许 URL-safe 字符）。
pub fn session_cookie_header(token: &str, ttl_hours: i64) -> Option<HeaderValue> {
    let token = token.trim();
    if token.is_empty()
        || !token
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        return None;
    }
    let max_age = (ttl_hours.max(1)) * 3600;
    HeaderValue::from_str(&format!(
        "{COOKIE_NAME}={token}; Path=/; HttpOnly; SameSite=Lax; Max-Age={max_age}"
    ))
    .ok()
}

/// 清除 cookie 的 `Set-Cookie` 头值（登出时使用）。
pub fn clear_cookie_header() -> HeaderValue {
    HeaderValue::from_static("okb_static=; Path=/; HttpOnly; SameSite=Lax; Max-Age=0")
}

/// 从 `Cookie` 头读取静态目录凭证。
pub fn cookie_token(headers: &axum::http::HeaderMap) -> Option<String> {
    for value in headers.get_all(header::COOKIE) {
        let Ok(raw) = value.to_str() else { continue };
        for pair in raw.split(';') {
            let Some((name, val)) = pair.split_once('=') else {
                continue;
            };
            if name.trim() == COOKIE_NAME {
                let val = val.trim().trim_matches('"');
                if !val.is_empty() {
                    return Some(val.to_string());
                }
            }
        }
    }
    None
}

/// 是否为浏览器页面导航（只有这类请求才值得重定向到登录页）。
fn wants_html(req: &axum::extract::Request) -> bool {
    req.headers()
        .get(header::ACCEPT)
        .and_then(|v| v.to_str().ok())
        .map(|v| v.contains("text/html"))
        .unwrap_or(false)
}

/// 未认证响应：浏览器导航 → 302 到登录页（带 `?redirect=` 回跳）；其余 → 401 JSON。
fn unauthorized_response(req: &axum::extract::Request) -> Response {
    if wants_html(req) {
        let target = req
            .uri()
            .path_and_query()
            .map(|pq| pq.as_str())
            .unwrap_or_else(|| req.uri().path());
        let location = format!("/assist/login?redirect={}", encode_redirect(target));
        tracing::debug!(target = %target, "静态目录需登录，重定向到登录页");
        return (StatusCode::FOUND, [(header::LOCATION, location)]).into_response();
    }
    json_error(StatusCode::UNAUTHORIZED, "未授权：该静态目录需要凭证")
}

/// 把目标 URL 编码为 query 值（保留 `/` 以便可读）。
fn encode_redirect(target: &str) -> String {
    let mut out = String::with_capacity(target.len());
    for byte in target.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => {
                out.push(*byte as char)
            }
            _ => out.push_str(&format!("%{:02X}", byte)),
        }
    }
    out
}

/// 已通过鉴权后的路径解析与响应。
///
/// 只接受 `&str` / `&StaticMount` 而不是 `&Request`：`axum::body::Body` 不是 `Sync`，
/// 把 `&Request` 跨 `await` 持有会让整个 handler 的 future 不再 `Send`，
/// axum 就不会把 `serve_mount` 当成 `Handler`。
async fn serve_located(path: &str, query: Option<&str>, mount: &StaticMount) -> Response {
    let path = path.to_string();

    // 请求子路径 → 本地相对路径（percent-decoding 后做穿越校验）
    let rest_raw = &path[mount.path.len()..];
    let Some(rest) = percent_decode(rest_raw) else {
        return not_found();
    };
    let root = Path::new(&mount.dir);
    let Some(target) = safe_resolve(root, &rest) else {
        return not_found();
    };

    if target.is_dir() {
        // 目录统一补结尾斜杠，保证相对链接正确
        if !path.ends_with('/') {
            let mut location = format!("{}/", path);
            if let Some(query) = query {
                location.push('?');
                location.push_str(query);
            }
            return (StatusCode::MOVED_PERMANENTLY, [(header::LOCATION, location)]).into_response();
        }
        if mount.try_index && !mount.index.is_empty() {
            let index = target.join(&mount.index);
            if index.is_file() {
                return serve_file(&index, mount.cache_control).await;
            }
        }
        if mount.list_dir {
            return render_listing(mount, &rest, &target);
        }
        return not_found();
    }

    serve_file(&target, mount.cache_control).await
}

/// 把请求子路径安全地解析到挂载根目录内的真实路径。
///
/// 逐段拒绝 `..` / 反斜杠 / NUL，最终 `canonicalize` 后仍须位于根目录内
/// （软链指到目录外同样被拒）。目标不存在时返回 `None`。
fn safe_resolve(root: &Path, rest: &str) -> Option<PathBuf> {
    let root_canon = root.canonicalize().ok()?;
    if !root_canon.is_dir() {
        return None;
    }
    let mut target = root_canon.clone();
    for segment in rest.split('/') {
        if segment.is_empty() || segment == "." {
            continue;
        }
        if segment == ".." || segment.contains('\0') || segment.contains('\\') {
            return None;
        }
        target.push(segment);
    }
    let target = target.canonicalize().ok()?;
    if !target.starts_with(&root_canon) {
        return None;
    }
    Some(target)
}

/// 读取文件并返回响应（带 MIME 与可选缓存头）。
async fn serve_file(path: &Path, cache_control: bool) -> Response {
    let bytes = match tokio::fs::read(path).await {
        Ok(bytes) => bytes,
        Err(_) => return not_found(),
    };
    let length = bytes.len();

    let mut response = (StatusCode::OK, bytes).into_response();
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static(crate::mime_for_path(path)),
    );
    if cache_control {
        headers.insert(
            header::CACHE_CONTROL,
            HeaderValue::from_static(crate::cache_control_for(path)),
        );
    } else {
        headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    }
    if let Ok(value) = HeaderValue::from_str(&length.to_string()) {
        headers.insert(header::CONTENT_LENGTH, value);
    }
    response
}

/// 目录列表页（仅在 `list_dir: true` 时使用）。
fn render_listing(mount: &StaticMount, rest: &str, dir: &Path) -> Response {
    let mut entries: Vec<(String, bool)> = Vec::new();
    if let Ok(iter) = std::fs::read_dir(dir) {
        for entry in iter.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with('.') {
                continue;
            }
            let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
            entries.push((name, is_dir));
        }
    }
    // 目录优先，其次按名字排序
    entries.sort_by(|a, b| {
        b.1.cmp(&a.1)
            .then_with(|| a.0.to_lowercase().cmp(&b.0.to_lowercase()))
    });

    let rel = rest.trim().trim_matches('/');
    let title = if rel.is_empty() {
        mount.path.clone()
    } else {
        format!("{}/{}", mount.path, rel)
    };

    let mut html = String::with_capacity(512 + entries.len() * 64);
    html.push_str("<!doctype html><html><head><meta charset=\"utf-8\">");
    html.push_str("<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">");
    html.push_str(&format!("<title>{}</title>", html_escape(&title)));
    html.push_str(
        "<style>body{font:14px/1.6 system-ui,sans-serif;margin:2rem;color:#222}\
         h1{font-size:1.1rem;word-break:break-all}a{color:#0a58ca;text-decoration:none}\
         a:hover{text-decoration:underline}ul{list-style:none;padding:0}\
         li{padding:.15rem 0}small{color:#888}</style></head><body>",
    );
    html.push_str(&format!("<h1>{}/</h1><ul>", html_escape(&title)));
    if !rel.is_empty() {
        html.push_str("<li><a href=\"../\">../</a></li>");
    }
    for (name, is_dir) in &entries {
        let href = if *is_dir {
            format!("{}/", url_encode_path(name))
        } else {
            url_encode_path(name)
        };
        let label = if *is_dir {
            format!("{}/", html_escape(name))
        } else {
            html_escape(name)
        };
        html.push_str(&format!("<li><a href=\"{}\">{}</a></li>", html_escape(&href), label));
    }
    html.push_str("</ul><hr><small>OKB-Assist static_dirs</small></body></html>");

    let mut response = (StatusCode::OK, html).into_response();
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/html; charset=utf-8"),
    );
    headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("no-cache, must-revalidate"),
    );
    response
}

/// 最小 HTML 转义（目录名可能含 `<`、`&` 等字符）。
fn html_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(ch),
        }
    }
    out
}

/// 编码链接中的文件名（保留 `/`），避免空格、`#`、`?` 破坏 URL。
fn url_encode_path(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for byte in name.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => {
                out.push(*byte as char)
            }
            _ => out.push_str(&format!("%{:02X}", byte)),
        }
    }
    out
}

/// 简易 percent-decoding（不做 `+` → 空格转换，仅处理 `%XX`）。
fn percent_decode(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).ok()?;
                out.push(u8::from_str_radix(hex, 16).ok()?);
                i += 3;
            }
            b'%' => return None,
            byte => {
                out.push(byte);
                i += 1;
            }
        }
    }
    String::from_utf8(out).ok()
}

fn not_found() -> Response {
    json_error(StatusCode::NOT_FOUND, "Not Found")
}

fn json_error(status: StatusCode, detail: &str) -> Response {
    (
        status,
        axum::Json(serde_json::json!({ "detail": detail })),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mount(value: serde_json::Value) -> Option<StaticMount> {
        StaticMount::from_value(&value, "/tmp", "/tmp")
    }

    #[test]
    fn 解析默认值() {
        let m = mount(serde_json::json!({"path": "/files/", "dir": "data/files"})).unwrap();
        assert_eq!(m.path, "/files");
        assert_eq!(m.dir, "/tmp/data/files");
        assert!(m.try_index);
        assert_eq!(m.index, "index.html");
        assert!(!m.list_dir);
        assert!(m.auth);
        assert!(m.cache_control);
    }

    #[test]
    fn 拒绝非法_path_与_dir() {
        assert!(mount(serde_json::json!({"path": "files", "dir": "d"})).is_none());
        assert!(mount(serde_json::json!({"path": "/", "dir": "d"})).is_none());
        assert!(mount(serde_json::json!({"path": "/a"})).is_none());
        assert!(mount(serde_json::json!({"path": "/a", "dir": "  "})).is_none());
    }

    #[test]
    fn 绝对路径_dir_不拼_cwd() {
        let m = mount(serde_json::json!({"path": "/a", "dir": "/srv/files"})).unwrap();
        assert_eq!(m.dir, "/srv/files");
    }

    #[test]
    fn 路径穿越被拒() {
        let dir = std::env::temp_dir().join(format!("okb-static-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::write(dir.join("a.txt"), b"hi").unwrap();

        assert!(safe_resolve(&dir, "/a.txt").is_some());
        assert!(safe_resolve(&dir, "/sub").is_some());
        assert!(safe_resolve(&dir, "/../etc/passwd").is_none());
        assert!(safe_resolve(&dir, "/sub/../../etc/passwd").is_none());
        assert!(safe_resolve(&dir, "/./a.txt").is_some());
        assert!(safe_resolve(&dir, "/missing.txt").is_none());

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn percent_decoding() {
        assert_eq!(percent_decode("/a%20b").unwrap(), "/a b");
        assert_eq!(percent_decode("/%E4%B8%AD").unwrap(), "/中");
        assert!(percent_decode("/%2").is_none());
        assert!(percent_decode("/%ZZ").is_none());
    }

    #[test]
    fn 静态目录_cookie_下发与解析() {
        let header = session_cookie_header("sess_abc-123", 2).unwrap();
        let text = header.to_str().unwrap();
        assert!(text.starts_with("okb_static=sess_abc-123;"));
        assert!(text.contains("Path=/"));
        assert!(text.contains("HttpOnly"));
        assert!(text.contains("SameSite=Lax"));
        assert!(text.contains("Max-Age=7200"));

        // 非法 token（含空格/分号/中文）不下发
        assert!(session_cookie_header("a b", 1).is_none());
        assert!(session_cookie_header("a;b", 1).is_none());
        assert!(session_cookie_header("  ", 1).is_none());
        assert!(session_cookie_header("中文", 1).is_none());

        // TTL 非正数时至少 1 小时
        assert!(session_cookie_header("okb_x", 0)
            .unwrap()
            .to_str()
            .unwrap()
            .contains("Max-Age=3600"));

        assert!(clear_cookie_header()
            .to_str()
            .unwrap()
            .contains("Max-Age=0"));
    }

    #[test]
    fn cookie_名称精确匹配() {
        let mut headers = axum::http::HeaderMap::new();
        headers.insert(
            header::COOKIE,
            "a=1; okb_static_x=no; okb_static=sess_ok; b=2".parse().unwrap(),
        );
        assert_eq!(cookie_token(&headers).as_deref(), Some("sess_ok"));

        let mut none = axum::http::HeaderMap::new();
        none.insert(header::COOKIE, "a=1".parse().unwrap());
        assert!(cookie_token(&none).is_none());
        assert!(cookie_token(&axum::http::HeaderMap::new()).is_none());
    }

    #[test]
    fn 重定向参数编码() {
        assert_eq!(encode_redirect("/static/a.pdf"), "/static/a.pdf");
        assert_eq!(
            encode_redirect("/static/a b.pdf?x=1&y=2"),
            "/static/a%20b.pdf%3Fx%3D1%26y%3D2"
        );
    }

    #[test]
    fn 路径嵌套判定() {
        assert!(is_nested("/a", "/a/b"));
        assert!(is_nested("/a/b", "/a"));
        assert!(!is_nested("/a", "/ab"));
        assert!(!is_nested("/a", "/b"));
    }

    #[test]
    fn 最长前缀优先() {
        // load_mounts 排序后 /a/b 在 /a 之前
        let mut mounts = vec![
            StaticMount {
                path: "/a/b".into(),
                dir: "/tmp".into(),
                try_index: true,
                index: "index.html".into(),
                list_dir: false,
                auth: true,
                cache_control: true,
            },
            StaticMount {
                path: "/a".into(),
                dir: "/tmp".into(),
                try_index: true,
                index: "index.html".into(),
                list_dir: false,
                auth: true,
                cache_control: true,
            },
        ];
        mounts.sort_by_key(|m| std::cmp::Reverse(m.path.len()));
        assert_eq!(mounts[0].path, "/a/b");
    }
}
