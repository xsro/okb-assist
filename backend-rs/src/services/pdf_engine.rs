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

use crate::settings::get_settings;

/// 当前二进制是否编入了内置 mupdf（`--features mupdf`）
pub const MUPDF_AVAILABLE: bool = cfg!(feature = "mupdf");

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
}
