use std::process::Command;

fn main() {
    // 构建时间戳（Unix 秒）
    let build_time = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    println!("cargo:rustc-env=BUILD_TIME={}", build_time);

    // Rust 编译器版本
    if let Ok(output) = Command::new("rustc").arg("--version").output() {
        let version = String::from_utf8_lossy(&output.stdout).trim().to_string();
        println!("cargo:rustc-env=RUSTC_VERSION={}", version);
    } else {
        println!("cargo:rustc-env=RUSTC_VERSION=unknown");
    }

    // 构译 Profile
    let profile = std::env::var("PROFILE").unwrap_or("unknown".to_string());
    println!("cargo:rustc-env=BUILD_PROFILE={}", profile);

    // 构建目标
    let host = std::env::var("HOST").unwrap_or("unknown".to_string());
    println!("cargo:rustc-env=BUILD_HOST={}", host);

    // 内置 mupdf 依赖版本（取自 Cargo.toml 的 `mupdf` 依赖声明）：
    // 日志页「系统版本」用它展示内置引擎版本；未编入 feature 时也照样注入，
    // 是否真正可用由 `cfg!(feature = "mupdf")` 决定。
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap_or_default();
    let mupdf_version = std::fs::read_to_string(
        std::path::Path::new(&manifest_dir).join("Cargo.toml"),
    )
    .ok()
    .and_then(|text| parse_mupdf_version(&text))
    .unwrap_or_else(|| "unknown".to_string());
    println!("cargo:rustc-env=MUPDF_CRATE_VERSION={}", mupdf_version);
    println!("cargo:rerun-if-changed=Cargo.toml");

    // Git 提交（可选，失败时留空）
    if let Ok(output) = Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
    {
        let git_commit = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if !git_commit.is_empty() {
            println!("cargo:rustc-env=GIT_COMMIT={}", git_commit);
        }
    }
}

/// 从 Cargo.toml 里读 `mupdf` 依赖声明的版本号（如 `mupdf = { version = "0.8" }`）。
///
/// 同时兼容表格写法：
/// ```toml
/// [dependencies.mupdf]
/// version = "0.8"
/// ```
/// 只做「按行找键」的宽松解析，够用且不引入 TOML 依赖。
fn parse_mupdf_version(text: &str) -> Option<String> {
    let mut in_mupdf_table = false;
    for raw in text.lines() {
        let line = raw.trim();
        if line.starts_with('[') {
            in_mupdf_table = line == "[dependencies.mupdf]";
            continue;
        }
        if in_mupdf_table {
            if let Some(rest) = line.strip_prefix("version") {
                if let Some(v) = first_quoted(rest) {
                    return Some(v);
                }
            }
            continue;
        }
        if let Some(rest) = line.strip_prefix("mupdf") {
            // `mupdf-sys` 之类不会被误判：去掉名字后必须以 `=` 开头
            if let Some(rest) = rest.trim_start().strip_prefix('=') {
                if let Some(v) = first_quoted(rest) {
                    return Some(v);
                }
            }
        }
    }
    None
}

/// 取一段文本里第一个双引号包裹的非空内容
fn first_quoted(text: &str) -> Option<String> {
    let start = text.find('"')? + 1;
    let end = text[start..].find('"')? + start;
    let value = text[start..end].trim().to_string();
    if value.is_empty() {
        None
    } else {
        Some(value)
    }
}