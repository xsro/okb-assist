//! PDF 健康检查。
//!
//! 判断库里的 PDF 是否还能用（文件缺失 / 空文件 / 不是 PDF / 被截断 / 结构损坏 / 加密）。
//! 结果写入 `documents.pdf_health`（状态）与 `documents.pdf_health_message`（原因）：
//!
//! | `pdf_health` | 含义 |
//! |---|---|
//! | `ok` | 文件头尾完整、结构可解析，且解析器没有任何诊断 |
//! | `warning` | 能用但不完美（结构损坏已被解析器自动修复 / 缺 `%%EOF` 尾部标记 / 已加密 / 无 mutool 未做结构检查） |
//! | `error` | 不可用（空文件 / 非 PDF / 结构损坏读不出页数） |
//! | `missing` | PDF 文件不在 `pdf_path` 模板指向的位置 |
//!
//! 检查是**轻量**的：只读文件头 1 KB、文件尾 2 KB，再让 mutool / mupdf 读一次页数，
//! 不走 `pdf_meta::extract_pdf_report_from_path`（那会整份抽取正文，批量检查太贵）。
//!
//! ## 为什么必须看解析器的 stderr
//!
//! 损坏的文件（典型：`startxref` 偏移错了、xref 表偏移全错）mutool 会**自动修复**：
//! 它往 stderr 打印 `format error: cannot recognize xref format`、
//! `warning: trying to repair broken xref`、`warning: repairing PDF document`，
//! 然后正常输出页数，**退出码仍然是 0**。只看页数会把这类文件误判成 `ok`
//! （实测 `data/pdfs/1016/1016.pdf`：startxref 指向 xref 表中段，mutool 修复后报 13 页）。
//! 所以这里额外收集 stderr 诊断：有诊断 → `warning`（`pdf_health_message` 写明损坏类型），
//! 无诊断且页数正常才判 `ok`。
//!
//! 注：引擎由 `config.json` 的 `pdf.engine` 在运行时选择（见 [`crate::services::pdf_engine`]）：
//! `mutool` 命令行（默认）或内置 `mupdf`（需 `--features mupdf`）。
//! 内置 mupdf 没有回传修复警告的接口，只有页数，所以选它时
//! 「xref 损坏但能自动修复」检不出（`mutool` 引擎才有）。

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

/// 只取 PDF 页数（异步，可能阻塞，内部走 spawn_blocking）。
///
/// 供分片解析判断是否需要切片使用：只做轻量的结构检查（读页数），
/// 不像 [`check_pdf_health`] 那样连文件头尾与诊断一起判。
/// 解析引擎不可用 / 文件损坏时返回 `None`。
pub async fn page_count(path: String) -> Option<i64> {
    tokio::task::spawn_blocking(move || page_count_sync(Path::new(&path)))
        .await
        .ok()
        .flatten()
}

/// 只取 PDF 页数（同步，可能阻塞，请在 `spawn_blocking` 里调用）
pub fn page_count_sync(path: &Path) -> Option<i64> {
    inspect_structure(path).page_count
}

/// 结构检查结果（由 mutool 或 mupdf 提供）
struct Structure {
    page_count: Option<i64>,
    encrypted: bool,
    /// 解析工具不可用（未安装 mutool），结构检查被跳过
    tool_unavailable: bool,
    /// 工具能跑但读不出结构，说明文件大概率损坏
    error: Option<String>,
    /// 解析器往 stderr 报的诊断（正常文件为空）
    diagnostics: Vec<String>,
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
        return append_diagnostics(
            PdfHealth::new(
                HEALTH_WARNING,
                format!("PDF 已加密（{pages} 页），部分解析功能可能受限"),
                Some(pages),
            ),
            &structure.diagnostics,
        );
    }
    if !trailer_ok {
        return append_diagnostics(
            PdfHealth::new(
                HEALTH_WARNING,
                format!("文件尾缺少 %%EOF 标记（可能被截断），共 {pages} 页"),
                Some(pages),
            ),
            &structure.diagnostics,
        );
    }
    // 5) 解析器诊断：mutool 遇到 xref 偏移错误、数据截断、对象语法错误时会往 stderr
    //    报 `format error` / `warning`，然后**自动修复**并正常输出页数（退出码依旧 0）。
    //    只看页数会把这类「能修但已损坏」的文件误判成 ok，所以有诊断就算 warning。
    if !structure.diagnostics.is_empty() {
        let detail = summarize_diagnostics(&structure.diagnostics);
        return PdfHealth::new(
            HEALTH_WARNING,
            format!(
                "PDF 结构已损坏（{detail}），解析器自动修复后能读出 {pages} 页；\
                 严格的阅读器/工具（部分版本 Acrobat、qpdf、部分解析器）可能直接报错"
            ),
            Some(pages),
        );
    }
    PdfHealth::new(
        HEALTH_OK,
        format!(
            "正常（{pages} 页，引擎：{}）",
            crate::services::pdf_engine::active().as_str()
        ),
        Some(pages),
    )
}

/// 把解析器诊断追加到已有消息后面（用于「已加密 / 缺 %%EOF」这类已判 warning 的分支，
/// 避免把诊断信息丢掉）
fn append_diagnostics(mut health: PdfHealth, diagnostics: &[String]) -> PdfHealth {
    if diagnostics.is_empty() {
        return health;
    }
    health.message = format!(
        "{}；另：解析器诊断 {}",
        health.message,
        summarize_diagnostics(diagnostics)
    );
    health
}

/// 把 mutool 的 stderr 诊断行压缩成一句中文描述
fn summarize_diagnostics(diagnostics: &[String]) -> String {
    let joined = diagnostics.join(" ").to_lowercase();
    let mut kinds: Vec<&str> = Vec::new();
    let mut push = |k: &'static str| {
        if !kinds.contains(&k) {
            kinds.push(k);
        }
    };
    if joined.contains("xref") || joined.contains("repair") {
        push("交叉引用表(xref)损坏");
    }
    if joined.contains("premature")
        || joined.contains("truncat")
        || joined.contains("end of data")
        || joined.contains("eof")
    {
        push("数据被截断");
    }
    if joined.contains("syntax") || joined.contains("invalid") {
        push("对象语法错误");
    }
    if joined.contains("cannot find")
        || joined.contains("out of range")
        || joined.contains("missing object")
    {
        push("对象引用无效");
    }

    // 类目之外再附上原始首行，方便管理员对照 mutool 输出
    let first = diagnostics
        .first()
        .map(|l| truncate_chars(l, 120))
        .unwrap_or_default();
    if kinds.is_empty() {
        // 未知诊断：原样贴出前两条
        let raw: Vec<String> = diagnostics.iter().take(2).map(|l| truncate_chars(l, 120)).collect();
        return truncate_chars(&raw.join("；"), 240);
    }
    format!("{}；原始诊断：{}", kinds.join("、"), first)
}

/// 按字符（而非字节）截断，避免把中文切碎
fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max).collect();
    out.push('…');
    out
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

// ── 结构检查（按运行时引擎分发）───────────────────────

/// 结构检查入口：按 `config.json` 的 `pdf.engine` 选 `mutool` 或内置 `mupdf`。
///
/// 选了 mupdf 但二进制未启用 `mupdf` feature 时，[`PdfEngine::fallback`] 会退回 mutool。
fn inspect_structure(path: &Path) -> Structure {
    match crate::services::pdf_engine::active() {
        #[cfg(feature = "mupdf")]
        crate::services::pdf_engine::PdfEngine::Mupdf => inspect_structure_mupdf(path),
        _ => inspect_structure_mutool(path),
    }
}

// ── 结构检查：mupdf（内置库）────────────────────────────

#[cfg(feature = "mupdf")]
fn inspect_structure_mupdf(path: &Path) -> Structure {
    use mupdf::Document;

    // 注意：mupdf 0.8 的 `Document` 没有回传修复警告的接口，
    // 「xref 损坏但能自动修复」这类情况在 mupdf 引擎下无法识别（只能拿到页数）。
    match Document::open(path) {
        Ok(doc) => Structure {
            page_count: doc.page_count().ok().map(|n| n as i64),
            encrypted: doc.needs_password().unwrap_or(false),
            tool_unavailable: false,
            error: None,
            diagnostics: Vec::new(),
        },
        Err(e) => Structure {
            page_count: None,
            encrypted: false,
            tool_unavailable: false,
            error: Some(format!("无法打开 PDF：{e}")),
            diagnostics: Vec::new(),
        },
    }
}

// ── 结构检查：外部 mutool ───────────────────────────────

/// 外部 `mutool` 结构检查。**始终编译**：即使编入了 mupdf，配置也可能选 `mutool`。
fn inspect_structure_mutool(path: &Path) -> Structure {
    use std::process::Command;

    let mutool = crate::settings::get_settings().mutool_path();
    let mut diagnostics: Vec<String> = Vec::new();

    // 先探一次：连进程都起不来就是没装 mutool，不算文件有问题
    let probe = Command::new(&mutool)
        .arg("show")
        .arg(path)
        .arg("trailer/Root")
        .output();
    match probe {
        Err(_) => {
            return Structure {
                page_count: None,
                encrypted: false,
                tool_unavailable: true,
                error: None,
                diagnostics: Vec::new(),
            }
        }
        Ok(out) => push_diagnostics(&mut diagnostics, &out.stderr),
    }

    let encrypted = mutool_show(&mutool, path, &["trailer/Encrypt"], &mut diagnostics)
        .map(|t| {
            let t = t.trim();
            !t.is_empty() && t != "null"
        })
        .unwrap_or(false);

    // 页数：优先 Page tree 的 /Count，回退到 `mutool pages` 计数
    let page_count = mutool_show(
        &mutool,
        path,
        &["trailer/Root/Pages/Count"],
        &mut diagnostics,
    )
    .and_then(|out| out.trim().parse::<i64>().ok())
    .filter(|n| *n > 0)
    .or_else(|| mutool_pages_count(&mutool, path, &mut diagnostics));

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
        diagnostics,
    }
}

/// 跑一次 `mutool show <path> <args>`，顺带把 stderr 诊断收进 `diagnostics`。
///
/// 修复告警（`format error: cannot recognize xref format` 等）不会让退出码变非 0，
/// 所以必须靠 stderr 判断，不能只看 `status.success()`。
fn mutool_show(
    mutool: &str,
    path: &Path,
    args: &[&str],
    diagnostics: &mut Vec<String>,
) -> Option<String> {
    use std::process::Command;

    let out = Command::new(mutool)
        .arg("show")
        .arg(path)
        .args(args)
        .output()
        .ok()?;
    push_diagnostics(diagnostics, &out.stderr);
    if !out.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).to_string())
}

/// 把 stderr 里的诊断逐行收集（去重、忽略空行）
fn push_diagnostics(sink: &mut Vec<String>, stderr: &[u8]) {
    for line in String::from_utf8_lossy(stderr).lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if !sink.iter().any(|l| l == line) {
            sink.push(line.to_string());
        }
    }
}

/// `mutool pages` 的输出行数统计（回退方案）
fn mutool_pages_count(mutool: &str, path: &Path, diagnostics: &mut Vec<String>) -> Option<i64> {
    use std::process::Command;

    let out = Command::new(mutool).arg("pages").arg(path).output().ok()?;
    push_diagnostics(diagnostics, &out.stderr);
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

#[cfg(test)]
mod tests {
    use super::*;

    /// 实测 1016.pdf 的 stderr（xref 偏移错误，mutool 自动修复后仍退出 0）
    const BROKEN_XREF: &[&str] = &[
        "format error: cannot recognize xref format",
        "warning: trying to repair broken xref",
        "warning: repairing PDF document",
    ];

    #[test]
    fn summarize_broken_xref() {
        let lines: Vec<String> = BROKEN_XREF.iter().map(|s| s.to_string()).collect();
        let text = summarize_diagnostics(&lines);
        assert!(text.contains("交叉引用表"), "got: {text}");
        assert!(text.contains("cannot recognize xref format"), "got: {text}");
    }

    #[test]
    fn summarize_truncated() {
        let lines = vec!["error: premature end of data".to_string()];
        let text = summarize_diagnostics(&lines);
        assert!(text.contains("截断"), "got: {text}");
    }

    #[test]
    fn summarize_unknown_keeps_raw() {
        let lines = vec!["some weird diagnostic".to_string()];
        let text = summarize_diagnostics(&lines);
        assert!(text.contains("some weird diagnostic"), "got: {text}");
    }

    #[test]
    fn push_diagnostics_dedups() {
        let mut sink = Vec::new();
        push_diagnostics(&mut sink, b"warning: a\n\nwarning: a\nwarning: b\n");
        assert_eq!(sink, vec!["warning: a".to_string(), "warning: b".to_string()]);
    }

    /// 真实文件探针：`OKB_PDF_HEALTH_PROBE=<path> cargo test -- --nocapture health_probe`
    #[test]
    fn health_probe() {
        let Ok(path) = std::env::var("OKB_PDF_HEALTH_PROBE") else {
            return;
        };
        let h = check_pdf_health_sync(Path::new(&path));
        println!("status={} pages={:?} msg={}", h.status, h.page_count, h.message);
        assert!(!h.message.is_empty());
    }
}
