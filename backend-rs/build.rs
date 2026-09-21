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