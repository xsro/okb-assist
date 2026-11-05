//! 文献「文件事实」与运行时状态推导。
//!
//! 文献状态**不再存数据库**，而是读取时由两部分推导：
//!
//! - **持久事实**：`documents` 的 `pdf_size` / `md_size` / `zip_size` + `fs_synced_at`
//!   （由 [`crate::routers::documents::sync_doc_file_sizes`] 在每个写路径上同步）。
//!   `fs_synced_at` 为空表示**从未同步过**（状态未知），非空表示「已同步，size 列为真」。
//! - **瞬时运行态**：进程内的活动任务表（`routers::pipeline::active_tasks_map`）与本模块的
//!   错误表 [`LAST_ERROR`]，可选镜像到 sidecar 文件（`system.json` 的 `runtime_state_path`）。
//!
//! 三态语义（以 Markdown 为例）：
//!
//! | 状态 | 判定 |
//! |---|---|
//! | `unknown` 未同步 | `fs_synced_at IS NULL` |
//! | `absent` 确定不存在 | `fs_synced_at IS NOT NULL AND NOT (md_size > 0)` |
//! | `present` 存在 | `md_size > 0` |

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::RwLock;

use serde_json::json;

use crate::config::Settings;
use crate::models::DocStatus;

/// 三态取值（筛选参数用）
pub const STATE_PRESENT: &str = "present";
pub const STATE_ABSENT: &str = "absent";
pub const STATE_UNKNOWN: &str = "unknown";

/// 派生出的可见状态（`status` / `status_message` / `progress` 的替代）
#[derive(Debug, Clone)]
pub struct DocState {
    pub status: DocStatus,
    pub message: Option<String>,
    pub progress: f64,
    /// 文件事实是否已同步过（false = 状态未知，UI 可标注「未同步」）
    pub synced: bool,
}

/// 进程内活动任务的快照（由 pipeline 组装后传入，避免本模块依赖 router）
#[derive(Debug, Clone, Default)]
pub struct RuntimeTask {
    /// `parse` / `extract` / `index` / `hash` / `pdf-health` …
    pub task_type: String,
    pub message: String,
    pub progress: f64,
}

/// 由「文件事实 + 运行时状态」推导可见状态。纯函数、零 IO。
pub fn derive_state(
    fs_synced_at: Option<&str>,
    pdf_size: Option<i64>,
    md_size: Option<i64>,
    task: Option<&RuntimeTask>,
    last_error: Option<&str>,
    mineru_task_id: Option<&str>,
) -> DocState {
    let has_md = md_size.unwrap_or(0) > 0;
    let has_pdf = pdf_size.unwrap_or(0) > 0;
    let synced = fs_synced_at.map(|s| !s.trim().is_empty()).unwrap_or(false);

    // 1) 正在跑的任务
    if let Some(t) = task {
        return DocState {
            status: DocStatus::Parsing,
            message: Some(t.message.clone()).filter(|m| !m.is_empty()),
            progress: t.progress,
            synced,
        };
    }

    // 2) 本进程内的失败记录
    if let Some(msg) = last_error {
        return DocState {
            status: DocStatus::Error,
            message: Some(msg.to_string()),
            progress: 0.0,
            synced,
        };
    }

    // 3) 有 Markdown = 解析完成
    if has_md {
        return DocState {
            status: DocStatus::MarkdownDone,
            message: Some("PDF 解析完成".to_string()),
            progress: 100.0,
            synced,
        };
    }

    // 4) 还存在外部解析任务（重启后恢复在跑状态）
    if mineru_task_id.map(|s| !s.trim().is_empty()).unwrap_or(false) {
        return DocState {
            status: DocStatus::Parsing,
            message: Some("正在恢复上次解析任务…".to_string()),
            progress: 0.0,
            synced,
        };
    }

    // 5) 从未同步过：状态未知（不能谎称「PDF 缺失」）
    if !synced {
        return DocState {
            status: DocStatus::Uploaded,
            message: Some("文件状态未同步".to_string()),
            progress: 0.0,
            synced,
        };
    }

    // 6) 已同步，有 PDF 待解析
    if has_pdf {
        return DocState {
            status: DocStatus::Uploaded,
            message: Some("待解析".to_string()),
            progress: 0.0,
            synced,
        };
    }

    // 7) 已同步且 PDF 确实不存在（例如 BibTeX 只导入条目）
    DocState {
        status: DocStatus::Uploaded,
        message: Some("PDF 缺失，请先上传 PDF".to_string()),
        progress: 0.0,
        synced,
    }
}

// ── SQL 条件构造（筛选白名单，禁止拼接原始列名） ──

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileKind {
    Pdf,
    Md,
    Zip,
}

impl FileKind {
    pub fn column(self) -> &'static str {
        match self {
            FileKind::Pdf => "pdf_size",
            FileKind::Md => "md_size",
            FileKind::Zip => "zip_size",
        }
    }

    /// 参数前缀：`pdf` / `md` / `zip`
    pub fn key(self) -> &'static str {
        match self {
            FileKind::Pdf => "pdf",
            FileKind::Md => "md",
            FileKind::Zip => "zip",
        }
    }
}

/// 三态 → SQL 条件；非法取值返回 `None`（调用方应答 400）。
pub fn state_condition(kind: FileKind, state: &str) -> Option<String> {
    match state.trim().to_ascii_lowercase().as_str() {
        STATE_PRESENT => Some(format!("COALESCE({}, 0) > 0", kind.column())),
        STATE_ABSENT => Some(format!(
            "(fs_synced_at IS NOT NULL AND NOT (COALESCE({0}, 0) > 0))",
            kind.column()
        )),
        STATE_UNKNOWN => Some("fs_synced_at IS NULL".to_string()),
        _ => None,
    }
}

/// `has_*` 布尔条件：`true` = 文件存在；`false` = 不存在（**含未同步**）。
pub fn has_condition(kind: FileKind, want: bool) -> String {
    if want {
        format!("COALESCE({}, 0) > 0", kind.column())
    } else {
        format!("NOT (COALESCE({}, 0) > 0)", kind.column())
    }
}

// ── 运行时错误表（内存 + 可选 sidecar） ──

fn error_map() -> &'static RwLock<HashMap<i64, String>> {
    static MAP: std::sync::OnceLock<RwLock<HashMap<i64, String>>> = std::sync::OnceLock::new();
    MAP.get_or_init(|| RwLock::new(HashMap::new()))
}

/// 取某篇文档的运行时错误（进程内）
pub fn last_error(doc_id: i64) -> Option<String> {
    error_map().read().unwrap().get(&doc_id).cloned()
}

/// 全量错误快照（用于 `error` 计数 / 筛选 / 批量重试）
pub fn error_snapshot() -> HashMap<i64, String> {
    error_map().read().unwrap().clone()
}

/// 记录一次失败：写内存 + （若配置了模板）原子写 sidecar
pub fn set_error(settings: &Settings, doc_id: i64, message: &str) {
    error_map()
        .write()
        .unwrap()
        .insert(doc_id, message.to_string());
    write_sidecar(settings, doc_id, message);
}

/// 清除某篇的失败记录
pub fn clear_error(settings: &Settings, doc_id: i64) {
    error_map().write().unwrap().remove(&doc_id);
    remove_sidecar(settings, doc_id);
}

/// 清除全部失败记录（批量「重置错误」用）
pub fn clear_all_errors(settings: &Settings) -> usize {
    let mut map = error_map().write().unwrap();
    let ids: Vec<i64> = map.keys().copied().collect();
    map.clear();
    drop(map);
    for id in &ids {
        remove_sidecar(settings, *id);
    }
    ids.len()
}

/// 启动时从 sidecar 目录恢复错误记录（进程重启后仍能看到「上次失败原因」）
pub fn load_all(settings: &Settings) {
    let Some(dir) = sidecar_dir(settings) else { return };
    let Ok(entries) = std::fs::read_dir(&dir) else { return };
    let mut map = error_map().write().unwrap();
    let mut loaded = 0usize;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else { continue };
        let Ok(id) = stem.parse::<i64>() else { continue };
        let Ok(raw) = std::fs::read_to_string(&path) else { continue };
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&raw) else { continue };
        if let Some(msg) = v.get("last_error").and_then(|x| x.as_str()) {
            if !msg.trim().is_empty() {
                map.insert(id, msg.to_string());
                loaded += 1;
            }
        }
    }
    if loaded > 0 {
        tracing::info!("运行时状态：从 sidecar 恢复 {} 条失败记录", loaded);
    }
}

/// sidecar 文件所在目录（由 `runtime_state_path` 模板推导：把 `{id}` 换成占位名后取父目录）
pub fn sidecar_dir(settings: &Settings) -> Option<PathBuf> {
    let template = settings.runtime_state_path_template();
    if template.trim().is_empty() {
        return None;
    }
    let resolved = crate::config_manager::ConfigManager::substitute_path_variables_str(&template, "id");
    PathBuf::from(resolved).parent().map(|p| p.to_path_buf())
}

fn sidecar_path(settings: &Settings, doc_id: i64) -> Option<PathBuf> {
    if settings.runtime_state_path_template().trim().is_empty() {
        return None;
    }
    Some(PathBuf::from(crate::paths::get_runtime_state_path(settings, doc_id)))
}

fn write_sidecar(settings: &Settings, doc_id: i64, message: &str) {
    let Some(path) = sidecar_path(settings, doc_id) else { return };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let body = json!({
        "id": doc_id,
        "last_error": message,
        "updated_at": crate::utils::now_iso(),
    });
    // 原子写：先写临时文件再 rename（同 `chat_store::save` 的做法）
    let tmp = path.with_extension("json.tmp");
    let text = serde_json::to_string_pretty(&body).unwrap_or_else(|_| "{}".to_string());
    if std::fs::write(&tmp, text).is_ok() {
        let _ = std::fs::rename(&tmp, &path);
    }
}

fn remove_sidecar(settings: &Settings, doc_id: i64) {
    let Some(path) = sidecar_path(settings, doc_id) else { return };
    let _ = std::fs::remove_file(path);
}
