//! PDF 健康检查。
//!
//! 判断库里的 PDF 是否还能用（文件缺失 / 空文件 / 不是 PDF / 被截断 / 结构损坏 / 加密）。
//! 结果写入 `documents.pdf_health`（状态）与 `documents.pdf_health_message`（原因）：
//!
//! | `pdf_health` | 含义 |
//! |---|---|
//! | `ok` | 文件头尾完整、结构可解析 |
//! | `warning` | 能用但不完美（缺 `%%EOF` 尾部标记 / 已加密 / 无 mutool 未做结构检查） |
//! | `error` | 不可用（空文件 / 非 PDF / 结构损坏读不出页数） |
//! | `missing` | PDF 文件不在 `pdf_path` 模板指向的位置 |
//!
//! 检查是**轻量**的：只读文件头 1 KB、文件尾 2 KB，再让 mutool / mupdf 读一次页数，
//! 不走 `pdf_meta::extract_pdf_report_from_path`（那会整份抽取正文，批量检查太贵）。

use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

/// 健康状态取值（与数据库 `documents.pdf_health` 一致）
pub const HEALTH_OK: &str = "ok";
pub const HEALTH_WARNING: &str = "warning";
pub const HEALTH_ERROR: &str = "error";
pub const HEALTH_MISSING: &str = "missing";

/// 单份 PDF 的健康检查结果
#[derive(Debug, Clone)]
pub struct PdfHealth {
    /// `ok` / `warning` / `error` / `missing`
    pub status: &'static str,
    /// 人可读的原因（写入 `pdf_health_message`）
    pub message: String,
    /// 解析出的页数（尽力而为，失败为 None）
    pub page_count: Option<i64>,
}

impl PdfHealth {
    fn new(status: &'static str, message: impl Into<String>, page_count: Option<i64>) -> Self {
        Self {
            status,
            message: message.into(),
            page_count,
        }
    }
}

/// 异步检查（在后台线程执行，不阻塞 async 运行时）
pub async fn check_pdf_health(path: String) -> PdfHealth {
    tokio::task::spawn_blocking(move || check_pdf_health_sync(Path::new(&path)))
        .await
        .unwrap_or_else(|e| {
            PdfHealth::new(
                HEALTH_ERROR,
                format!("健康检查任务崩溃: {e}"),
                None,
            )
        })
}

/// 结构检查结果（由 mutool 或 mupdf 提供）
struct Structure {
    page_count: Option<i64>,
    encrypted: bool,
    /// 解析工具不可用（未安装 mutool），结构检查被跳过
    tool_unavailable: bool,
    /// 工具能跑但读不出结构，说明文件大概率损坏
    error: Option<String>,
}

/// 同步检查一份 PDF（可能阻塞，请在 `spawn_blocking` 里调用）
pub fn check_pdf_health_sync(path: &Path) -> PdfHealth {
    // 1) 文件是否存在、是否为空
    let meta = match std::fs::metadata(path) {
        Ok(m) => m,
        Err(e) => {
            return PdfHealth::new(HEALTH_MISSING, format!("PDF 文件缺失或无法访问: {e}"), None)
        }
    };
    if !meta.is_file() {
        return PdfHealth::new(HEALTH_MISSING, "PDF 路径不是普通文件", None);
    }
    if meta.len() == 0 {
        return PdfHealth::new(HEALTH_ERROR, "PDF 文件大小为 0（空文件）", None);
    }

    // 2) 文件头：前 1 KB 必须出现 %PDF-（PDF 规范允许前面有少量垃圾字节）
    let header_ok = match read_head(path, 1024) {
        Ok(head) => contains(&head, b"%PDF-"),
        Err(e) => {
            return PdfHealth::new(HEALTH_ERROR, format!("读取文件头失败: {e}"), None)
        }
    };
    if !header_ok {
        return PdfHealth::new(
            HEALTH_ERROR,
            "文件头缺少 %PDF- 标记，不是有效的 PDF",
            None,
        );
    }

    // 3) 文件尾：最后 2 KB 应有 %%EOF（没有通常意味着下载/传输被截断）
    let trailer_ok = match read_tail(path, 2048) {
        Ok(tail) => contains(&tail, b"%%EOF"),
        Err(_) => false,
    };

    // 4) 结构检查：页数 + 是否加密
    let structure = inspect_structure(path);

    if structure.tool_unavailable {
        return PdfHealth::new(
            HEALTH_WARNING,
            "文件头检查通过，但未做结构检查（mutool 不可用）",
            None,
        );
    }
    if let Some(err) = structure.error {
        return PdfHealth::new(HEALTH_ERROR, err, structure.page_count);
    }
    if structure.encrypted && structure.page_count.is_none() {
        return PdfHealth::new(
            HEALTH_WARNING,
            "PDF 已加密，无法读取结构（可能需要密码）",
            None,
        );
    }
    let pages = structure.page_count.unwrap_or(0);
    if pages <= 0 {
        return PdfHealth::new(HEALTH_ERROR, "PDF 结构异常：页数为 0", None);
    }
    if structure.encrypted {
        return PdfHealth::new(
            HEALTH_WARNING,
            format!("PDF 已加密（{pages} 页），部分解析功能可能受限"),
            Some(pages),
        );
    }
    if !trailer_ok {
        return PdfHealth::new(
            HEALTH_WARNING,
            format!("文件尾缺少 %%EOF 标记（可能被截断），共 {pages} 页"),
            Some(pages),
        );
    }
    PdfHealth::new(HEALTH_OK, format!("正常（{pages} 页）"), Some(pages))
}

/// 读取文件开头 `len` 字节
fn read_head(path: &Path, len: usize) -> std::io::Result<Vec<u8>> {
    let mut file = std::fs::File::open(path)?;
    let mut buf = vec![0u8; len];
    let n = file.read(&mut buf)?;
    buf.truncate(n);
    Ok(buf)
}

/// 读取文件末尾 `len` 字节
fn read_tail(path: &Path, len: u64) -> std::io::Result<Vec<u8>> {
    let mut file = std::fs::File::open(path)?;
    let size = file.metadata()?.len();
    let start = size.saturating_sub(len);
    file.seek(SeekFrom::Start(start))?;
    let mut buf = Vec::with_capacity((size - start) as usize);
    file.read_to_end(&mut buf)?;
    Ok(buf)
}

/// 字节切片是否包含指定子序列
fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    if needle.is_empty() || haystack.len() < needle.len() {
        return false;
    }
    haystack.windows(needle.len()).any(|w| w == needle)
}

// ── 结构检查：mupdf（内置库）────────────────────────────

#[cfg(feature = "mupdf")]
fn inspect_structure(path: &Path) -> Structure {
    use mupdf::Document;

    match Document::open(path) {
        Ok(doc) => Structure {
            page_count: doc.page_count().ok().map(|n| n as i64),
            encrypted: doc.needs_password().unwrap_or(false),
            tool_unavailable: false,
            error: None,
        },
        Err(e) => Structure {
            page_count: None,
            encrypted: false,
            tool_unavailable: false,
            error: Some(format!("无法打开 PDF：{e}")),
        },
    }
}

// ── 结构检查：外部 mutool ───────────────────────────────

#[cfg(not(feature = "mupdf"))]
fn inspect_structure(path: &Path) -> Structure {
    use std::process::Command;

    let mutool = crate::settings::get_settings().mutool_path();

    // 先探一次：连进程都起不来就是没装 mutool，不算文件有问题
    if Command::new(&mutool)
        .arg("show")
        .arg(path)
        .arg("trailer/Root")
        .output()
        .is_err()
    {
        return Structure {
            page_count: None,
            encrypted: false,
            tool_unavailable: true,
            error: None,
        };
    }

    let show = |args: &[&str]| -> Option<String> {
        let out = Command::new(&mutool)
            .arg("show")
            .arg(path)
            .args(args)
            .output()
            .ok()?;
        if !out.status.success() {
            return None;
        }
        Some(String::from_utf8_lossy(&out.stdout).to_string())
    };

    let encrypted = show(&["trailer/Encrypt"])
        .map(|t| {
            let t = t.trim();
            !t.is_empty() && t != "null"
        })
        .unwrap_or(false);

    // 页数：优先 Page tree 的 /Count，回退到 `mutool pages` 计数
    let page_count = show(&["trailer/Root/Pages/Count"])
        .and_then(|out| out.trim().parse::<i64>().ok())
        .filter(|n| *n > 0)
        .or_else(|| mutool_pages_count(&mutool, path));

    let error = if page_count.is_some() {
        None
    } else if encrypted {
        // 加密文件读不到结构是正常的，交给上层判成 warning
        None
    } else {
        Some("无法从 PDF 结构里读出页数（文件可能损坏）".to_string())
    };

    Structure {
        page_count,
        encrypted,
        tool_unavailable: false,
        error,
    }
}

/// `mutool pages` 的输出行数统计（回退方案）
#[cfg(not(feature = "mupdf"))]
fn mutool_pages_count(mutool: &str, path: &Path) -> Option<i64> {
    use std::process::Command;

    let out = Command::new(mutool).arg("pages").arg(path).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let n = text
        .lines()
        .filter(|l| l.trim_start().starts_with("<page "))
        .count() as i64;
    (n > 0).then_some(n)
}
