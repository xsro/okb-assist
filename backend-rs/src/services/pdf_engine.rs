//! PDF 解析引擎选择（运行时）。
//!
//! 同一套 PDF 能力有两份实现：
//!
//! | 引擎 | 实现 | 何时可用 |
//! |---|---|---|
//! | `mutool` | 外部命令行 `mutool`（`system.json` 的 `mutool_path`） | **永远可用**（默认） |
//! | `mupdf` | 内置 `mupdf` 库（`mupdf` feature） | 只有 `cargo build --features mupdf` 编出来的二进制才有 |
//!
//! 引擎由 `config.json` 的 `pdf.engine` 决定（配置页可改，改完即时生效，无需重启）。
//! 选了 `mupdf` 但当前二进制没启用 feature 时**自动回退到 `mutool`**，
//! 配置值本身保留（便于换回带 feature 的二进制后继续用）。
//!
//! 差异提示：只有 `mutool` 会把「xref 损坏但已自动修复」之类的诊断打到 stderr，
//! 内置 mupdf 没有这个接口，所以 `mupdf` 引擎下 PDF 健康检查检不出「能修复的损坏」。

use serde_json::{json, Value};

use crate::settings::get_settings;

/// 当前二进制是否编入了内置 mupdf（`--features mupdf`）
pub const MUPDF_AVAILABLE: bool = cfg!(feature = "mupdf");

/// 内置 mupdf 依赖的版本号（取自 `Cargo.toml` 的 `mupdf` 依赖声明，由 `build.rs` 注入）。
///
/// 只是「编译进来的是哪个 cairo/crate 版本」这个信息，用于日志页展示；
/// 是否真的可用看 [`MUPDF_AVAILABLE`]。
pub const MUPDF_CRATE_VERSION: &str = env!("MUPDF_CRATE_VERSION");

/// PDF 解析引擎
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PdfEngine {
    /// 外部命令行 mutool
    Mutool,
    /// 内置 mupdf 库
    Mupdf,
}

impl PdfEngine {
    /// 配置里使用的字符串
    pub fn as_str(self) -> &'static str {
        match self {
            PdfEngine::Mutool => "mutool",
            PdfEngine::Mupdf => "mupdf",
        }
    }

    /// 界面上显示的名字
    pub fn label(self) -> &'static str {
        match self {
            PdfEngine::Mutool => "mutool 命令行",
            PdfEngine::Mupdf => "内置 mupdf 库",
        }
    }

    /// 解析配置字符串（不区分大小写，未知取值返回 None）
    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "mutool" => Some(PdfEngine::Mutool),
            "mupdf" | "builtin" | "built-in" => Some(PdfEngine::Mupdf),
            _ => None,
        }
    }

    /// 该引擎在当前二进制里是否真的可用
    pub fn available(self) -> bool {
        match self {
            PdfEngine::Mutool => true,
            PdfEngine::Mupdf => MUPDF_AVAILABLE,
        }
    }

    /// 能力兜底：选了不可用的引擎就退回 `mutool`
    pub fn fallback(self) -> Self {
        if self.available() {
            self
        } else {
            PdfEngine::Mutool
        }
    }
}

/// 配置里写的引擎（`config.json` 的 `pdf.engine`，未知/缺失时按 `mutool` 处理）。
///
/// 注意：这里**不做能力兜底**，只反映配置值；要判断实际用哪个请用 [`active`]。
pub fn configured() -> PdfEngine {
    let raw = get_settings().pdf_engine_name();
    PdfEngine::parse(&raw).unwrap_or(PdfEngine::Mutool)
}

/// 实际生效的引擎（配置值 + 编译特性兜底）
pub fn active() -> PdfEngine {
    configured().fallback()
}

// ── 版本探测（供日志页「系统版本」展示） ────────────────

/// 外部 `mutool -v` 的探测结果
#[derive(Debug, Clone, Default)]
pub struct MutoolVersion {
    /// 进程是否成功跑起来并输出了版本行
    pub available: bool,
    /// 版本行（`mutool -v` 的首个非空行，如 `mutool version 1.28.5`）
    pub version: Option<String>,
    /// 不可用的原因（路径错、超时、无输出）
    pub error: Option<String>,
}

/// 跑一次 `mutool -v` 取外部命令行版本。
///
/// `mutool` 把版本打到 **stderr**（1.28 实测），所以两个流都看；
/// 超时 5 秒、`kill_on_drop`，不会拖住请求。
pub async fn mutool_version() -> MutoolVersion {
    probe_mutool_version(&get_settings().mutool_path()).await
}

async fn probe_mutool_version(mutool: &str) -> MutoolVersion {
    use tokio::process::Command;
    use tokio::time::{timeout, Duration};

    let output = timeout(
        Duration::from_secs(5),
        Command::new(mutool).arg("-v").kill_on_drop(true).output(),
    )
    .await;

    let output = match output {
        Err(_) => {
            return MutoolVersion {
                available: false,
                version: None,
                error: Some("执行 mutool -v 超时（5s）".to_string()),
            }
        }
        Ok(Err(e)) => {
            return MutoolVersion {
                available: false,
                version: None,
                error: Some(format!("无法执行 {mutool} -v：{e}")),
            }
        }
        Ok(Ok(output)) => output,
    };

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    match pick_version_line(&stdout).or_else(|| pick_version_line(&stderr)) {
        Some(version) => MutoolVersion {
            available: true,
            version: Some(version),
            error: None,
        },
        None => MutoolVersion {
            available: false,
            version: None,
            error: Some(format!(
                "{} -v 没有输出可识别的版本（退出码 {:?}）",
                mutool,
                output.status.code()
            )),
        },
    }
}

/// 从命令输出里挑一行版本：优先含数字的非空行，否则第一个非空行
fn pick_version_line(raw: &str) -> Option<String> {
    let mut fallback: Option<String> = None;
    for line in raw.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        // 一行一条：`mutool version 1.28.5`
        if line.chars().any(|c| c.is_ascii_digit()) {
            return Some(line.to_string());
        }
        if fallback.is_none() {
            fallback = Some(line.to_string());
        }
    }
    fallback
}

/// 供 `GET /assist/api/admin/`（日志页「系统版本」）使用的 PDF 引擎版本报告。
///
/// 结构：`{engine: {configured, active, label}, mutool: {path, available, version, error},
/// builtin_mupdf: {available, version}}`。
pub async fn version_report() -> Value {
    let configured_engine = configured();
    let active_engine = configured_engine.fallback();
    let mutool = mutool_version().await;

    json!({
        "engine": {
            "configured": configured_engine.as_str(),
            "active": active_engine.as_str(),
            "label": active_engine.label(),
        },
        "mutool": {
            "path": get_settings().mutool_path(),
            "available": mutool.available,
            "version": mutool.version,
            "error": mutool.error,
        },
        "builtin_mupdf": {
            "available": MUPDF_AVAILABLE,
            "version": MUPDF_CRATE_VERSION,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_known_values() {
        assert_eq!(PdfEngine::parse("mutool"), Some(PdfEngine::Mutool));
        assert_eq!(PdfEngine::parse(" MuPDF "), Some(PdfEngine::Mupdf));
        assert_eq!(PdfEngine::parse("builtin"), Some(PdfEngine::Mupdf));
        assert_eq!(PdfEngine::parse("acrobat"), None);
    }

    #[test]
    fn fallback_respects_feature() {
        assert_eq!(PdfEngine::Mutool.fallback(), PdfEngine::Mutool);
        if MUPDF_AVAILABLE {
            assert_eq!(PdfEngine::Mupdf.fallback(), PdfEngine::Mupdf);
        } else {
            assert_eq!(PdfEngine::Mupdf.fallback(), PdfEngine::Mutool);
        }
    }

    #[test]
    fn pick_version_line_handles_both_streams() {
        // mutool 实际把版本行打到 stderr，stdout 常常是空的
        assert_eq!(pick_version_line(""), None);
        assert_eq!(
            pick_version_line("\nmutool version 1.28.5\n"),
            Some("mutool version 1.28.5".to_string())
        );
        // 没有数字时保留首个非空行
        assert_eq!(
            pick_version_line("usage: mutool\n"),
            Some("usage: mutool".to_string())
        );
    }
}
