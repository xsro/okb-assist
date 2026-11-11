//! PDF 按页切分（外部 `mutool`）。
//!
//! 用途：MinerU 分片解析。官方精准解析 API 限制单文件 ≤ 200 页，超过就得切片。
//! 切片有两种做法，本模块实现**本地切页**：
//!
//! | 做法 | 上传量 | 依赖 |
//! |---|---|---|
//! | 本地切页（本模块） | = 原文件大小（每片只传自己那部分） | 外部 `mutool` |
//! | 整份上传 + 页码范围 | = 文件大小 × 片数 | MinerU 支持页码参数 |
//!
//! 命令：`mutool clean -ggg -D <in.pdf> <out.pdf> <pages>`
//!
//! - `-ggg`：合并重复对象并压缩 xref（体积更小，解析器也更快）；
//! - `-D`：输出**去掉加密**的副本 —— 国内学位论文常见「空用户口令 + AES-128」的加密件
//!   （CNKI / ReaderEx 导出），去掉加密后 MinerU 不会因权限位拒绝；
//! - 末尾的 `pages` 是 1-based 页列表/范围（如 `1-100`、`101-129`）。
//!
//! 切页**始终走外部 `mutool`**（`system.json` 的 `mutool_path`），与 `pdf.engine`
//! （`mutool` / 内置 `mupdf`）无关；mutool 不可用时由调用方退回「整份上传 + 页码范围」。

use std::path::Path;

use tracing::{debug, info};

/// 单个分片切页的超时（秒）。100 页的 PDF 实测 < 1s，给足余量。
const SPLIT_TIMEOUT_SECS: u64 = 300;

/// 把 `src` 按 `ranges`（形如 `["1-100", "101-129"]`）切成多个 PDF，写到 `out_dir`。
///
/// 成功时返回与 `ranges` **一一对应**的文件路径；失败时清掉 `out_dir` 并返回 Err
/// （调用方据此退回「整份上传 + 页码范围」）。
pub async fn split_pdf(
    src: &str,
    ranges: &[String],
    out_dir: &str,
) -> anyhow::Result<Vec<String>> {
    if ranges.is_empty() {
        anyhow::bail!("切页范围为空");
    }
    if !Path::new(src).is_file() {
        anyhow::bail!("源 PDF 不存在: {}", src);
    }

    let out = Path::new(out_dir);
    std::fs::create_dir_all(out)?;
    let mutool = crate::settings::get_settings().mutool_path();

    let mut files: Vec<String> = Vec::with_capacity(ranges.len());
    for (idx, range) in ranges.iter().enumerate() {
        let target = out.join(format!("part_{:03}.pdf", idx + 1));
        let _ = std::fs::remove_file(&target);

        if let Err(e) = split_one(&mutool, src, &target, range).await {
            // 半成品不留：否则下次重试会看到一堆「分片存在」的假象
            let _ = std::fs::remove_dir_all(out);
            anyhow::bail!("{}（第 {}/{} 片，页码 {}）", e, idx + 1, ranges.len(), range);
        }
        files.push(target.to_string_lossy().to_string());
    }

    info!(
        "PDF 已本地切分为 {} 片（mutool clean）：{}",
        files.len(),
        files.join(", ")
    );
    Ok(files)
}

/// 切一片：跑一次 `mutool clean -ggg -D in out pages` 并校验产物非空
async fn split_one(mutool: &str, src: &str, target: &Path, pages: &str) -> anyhow::Result<()> {
    debug!("mutool clean -ggg -D {} {} {}", src, target.display(), pages);

    let mut cmd = tokio::process::Command::new(mutool);
    cmd.arg("clean")
        .arg("-ggg")
        .arg("-D")
        .arg(src)
        .arg(target)
        .arg(pages)
        .kill_on_drop(true);

    let output = match tokio::time::timeout(
        std::time::Duration::from_secs(SPLIT_TIMEOUT_SECS),
        cmd.output(),
    )
    .await
    {
        Err(_) => anyhow::bail!("mutool 切页超时（{}s）", SPLIT_TIMEOUT_SECS),
        Ok(Err(e)) => anyhow::bail!("无法执行 {}：{}", mutool, e),
        Ok(Ok(o)) => o,
    };

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stderr = stderr.trim();
        anyhow::bail!(
            "mutool 切页失败（退出码 {:?}）：{}",
            output.status.code(),
            stderr.chars().take(200).collect::<String>()
        );
    }

    // mutool 对「能写但内容为空」的情况也返回 0，必须自己校验产物
    let size = std::fs::metadata(target).map(|m| m.len()).unwrap_or(0);
    if size == 0 {
        anyhow::bail!("mutool 切页产物为空: {}", target.display());
    }
    Ok(())
}
