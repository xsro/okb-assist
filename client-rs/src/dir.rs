//! 子命令 `dir`：扫描目录中的 PDF，与服务端比对后交互上传

use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;

use anyhow::{bail, Context, Result};
use clap::Parser;
use reqwest::Client;
use walkdir::WalkDir;

use crate::util::*;

// ==================== CLI 参数 ====================

/// 目录扫描参数
#[derive(Parser, Debug)]
pub(crate) struct DirArgs {
    /// 要扫描的目录路径（包含 PDF 文件）
    dir: String,

    /// OKB-Assist 服务地址（缺省取 `~/.okb/client.json` 的 `url`）
    #[arg(long)]
    base_url: Option<String>,

    /// 访问令牌（缺省取 `~/.okb/client.json` 的 `token`）
    #[arg(long)]
    token: Option<String>,

    /// 试运行，不实际上传
    #[arg(long)]
    dry_run: bool,

    /// 递归搜索子目录
    #[arg(long, short)]
    recursive: bool,

    /// 上传后触发解析流水线（需要显式指定）
    #[arg(long)]
    process: bool,

    #[command(flatten)]
    interval: UploadIntervalArgs,
}

// ==================== 子命令: dir ====================

/// 扫描目录，收集 PDF 文件列表
fn scan_pdfs(dir: &Path, recursive: bool) -> Result<Vec<PathBuf>> {
    let mut pdfs = Vec::new();

    if !dir.is_dir() {
        bail!("目录不存在: {}", dir.display());
    }

    let walker = if recursive {
        WalkDir::new(dir)
            .follow_links(true)
            .into_iter()
            .filter_entry(|e| {
                // 跳过隐藏目录和常见非文献目录
                let name = e.file_name().to_string_lossy();
                if e.depth() > 0 && e.file_type().is_dir() {
                    return !name.starts_with('.')
                        && name != "__pycache__"
                        && name != "node_modules"
                        && name != ".git";
                }
                true
            })
            .collect::<Vec<_>>()
    } else {
        // 非递归: 只读 dir 下直接文件
        let entries = std::fs::read_dir(dir)
            .with_context(|| format!("无法读取目录: {}", dir.display()))?;
        for entry in entries {
            let entry = entry?;
            let path = entry.path();
            if path.is_file() {
                let ext = path
                    .extension()
                    .map(|e| e.to_string_lossy().to_lowercase())
                    .unwrap_or_default();
                if ext == "pdf" {
                    pdfs.push(path);
                }
            }
        }
        pdfs.sort_by_key(|p| p.file_name().map(|n| n.to_string_lossy().to_string()));
        return Ok(pdfs);
    };

    for entry in walker {
        let entry = entry?;
        if !entry.file_type().is_file() {
            continue;
        }
        let path = entry.path().to_path_buf();
        let ext = path
            .extension()
            .map(|e| e.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        if ext == "pdf" {
            pdfs.push(path);
        }
    }

    pdfs.sort_by_key(|p| p.file_name().map(|n| n.to_string_lossy().to_string()));
    Ok(pdfs)
}

pub(crate) async fn run_dir(args: &DirArgs) -> Result<()> {
    let cfg = load_config();
    let token = read_token(args.token.as_deref(), cfg.token.as_deref());
    let base_url = args.base_url.clone().unwrap_or_else(|| cfg.base_url());
    let process = args.process;
    // 上传进度开关（`--no-progress`）
    PROGRESS_ENABLED.store(!args.interval.no_progress, Ordering::Relaxed);
    // `dir` 不知道 DOI（要等上传后服务端返回），所以只做记录：上传成功后写回 `~/.okb/client/doi-hash.json`
    let mut hash_store = HashDoiStore::load();

    println!("扫描目录: {}", args.dir);
    println!("递归搜索: {}", if args.recursive { "是" } else { "否" });
    println!("上传间隔: {}", args.interval.describe());
    println!(
        "PDF 元数据抽取: {}",
        if args.interval.extract_meta {
            "开启"
        } else {
            "跳过（默认，--extract-meta 开启）"
        }
    );
    println!("服务地址: {}", base_url);
    println!("Token: {}", if token.is_some() { "********" } else { "未设置" });
    println!();

    // 1. 扫描 PDF（这里**不**计算哈希：哈希与比对放到上传前逐个做，避免先扫全库再上传）
    let all_pdfs = scan_pdfs(Path::new(&args.dir), args.recursive)?;
    let total = all_pdfs.len();
    if total == 0 {
        println!("目录中没有找到 PDF 文件。");
        return Ok(());
    }
    println!("找到 {} 个 PDF 文件。", total);
    println!();

    // 2. 拉一次服务端全量哈希索引，供上传前逐个比对（1 次请求；失败则逐文件回退 diff-hashes）
    let client = http_client()?;
    println!("正在获取服务端索引（全部文件哈希）...");
    let server_index = api_fetch_server_index(&client, &base_url, &token).await;
    match &server_index {
        Some(idx) => println!(
            "  服务端索引：{} 篇文献，{} 个可比对哈希（1 次请求）",
            idx.count,
            idx.hashes.len()
        ),
        None => println!("  ⚠ 无法获取服务端索引，回退为逐个文件 diff-hashes 比对（较慢）"),
    }
    println!();

    // 3. 列出本地文件（文件太多时只列前 200 个，避免刷屏）
    const LIST_LIMIT: usize = 200;
    for (i, p) in all_pdfs.iter().take(LIST_LIMIT).enumerate() {
        println!("  [{:>3}] {}", i + 1, p.display());
    }
    if all_pdfs.len() > LIST_LIMIT {
        println!("  …（其余 {} 个未列出）", all_pdfs.len() - LIST_LIMIT);
    }
    println!();
    println!("提示：上传前会逐个计算哈希并与服务器比对，服务器已有的文件自动跳过。");
    println!();

    // 4. 交互选择
    print!("请选择 [a] 全部上传  [s] 逐个选择  [q] 退出: ");
    io::stdout().flush().ok();
    let mut choice = String::new();
    io::stdin().read_line(&mut choice).ok();
    let choice = choice.trim().to_lowercase();

    if choice == "q" {
        println!("已退出。");
        return Ok(());
    }

    let selected: Vec<&PathBuf> = if choice == "a" {
        all_pdfs.iter().collect()
    } else if choice == "s" {
        let mut sel = Vec::new();
        for p in &all_pdfs {
            print!("上传 {}？[y/N]: ", p.display());
            io::stdout().flush().ok();
            let mut ans = String::new();
            io::stdin().read_line(&mut ans).ok();
            if ans.trim().to_lowercase() == "y" {
                sel.push(p);
            } else {
                println!("  跳过。");
            }
        }
        sel
    } else {
        println!("无效选择，退出。");
        return Ok(());
    };

    if selected.is_empty() {
        println!("未选择任何文件。");
        return Ok(());
    }

    // 5. 逐个文件：计算哈希 -> 与服务端比对 -> 决定是否上传
    println!();
    println!("开始处理 {} 个文件 ...", selected.len());
    let mut uploaded = 0usize;
    let mut skipped = 0usize;
    let mut hash_failed = 0usize;
    let mut failed = 0usize;
    // 已执行的上传次数（用于按 `--upload-interval` 节流，首个不等待）
    let mut upload_seq = 0usize;
    let loop_started = std::time::Instant::now();

    for (i, path) in selected.iter().enumerate() {
        // 哈希是阻塞 IO：丢到后台线程，不卡住 tokio worker
        print!(
            "  [{}/{}] 计算哈希: {} ... ",
            i + 1,
            selected.len(),
            path.display()
        );
        io::stdout().flush().ok();
        let item_started = std::time::Instant::now();
        let hash = match sha256_file_async((*path).clone()).await {
            Ok(h) => {
                println!(
                    "{} ({})",
                    &h[..16.min(h.len())],
                    format_duration(item_started.elapsed())
                );
                h
            }
            Err(e) => {
                println!("失败: {} ({})", e, format_duration(item_started.elapsed()));
                hash_failed += 1;
                continue;
            }
        };

        // 上传前比对：服务器已有相同哈希则跳过
        match server_has_hash(&client, &base_url, &token, &server_index, &hash).await {
            Some(true) => {
                println!("        -> 服务器已存在该文件，跳过");
                skipped += 1;
                continue;
            }
            Some(false) => {}
            None => {
                println!("        -> ⚠ 哈希比对请求失败，本文件未上传（可重试）");
                failed += 1;
                continue;
            }
        }

        if args.dry_run {
            println!("        -> [dry-run] 服务器没有该文件，跳过上传");
            continue;
        }

        // 两次上传之间按 `--upload-interval` 等待（首个不等待）
        args.interval.throttle(upload_seq).await;
        upload_seq += 1;
        // 行尾先换行，进度行独占一行（见 `UploadProgress`）
        println!("        上传 ...");
        match api_upload(
            &base_url,
            path,
            None,
            &token,
            args.interval.timeout_override(),
            args.interval.extract_meta,
        )
        .await
        {
            UploadOutcome::Created(result) => {
                println!("        成功 -> id={}", result.id);
                // 记下 DOI → 哈希（服务端算的与本地一致，优先用服务端返回值），供 `zotero --reuse-hash` 复用
                if let Some(doi) = result.doi.as_deref().filter(|s| !s.trim().is_empty()) {
                    let h = result.file_hash.as_deref().unwrap_or(hash.as_str());
                    hash_store.record(doi, h);
                    hash_store.flush();
                }
                uploaded += 1;
                // 触发解析流水线
                if process && api_trigger_pipeline(&client, &base_url, result.id, &token).await {
                    println!("        -> 已触发流水线");
                }
            }
            UploadOutcome::Duplicate {
                kind,
                existing_id,
                existing_doi,
                existing_filename,
                same_file,
                message,
            } => {
                println!("        被服务端去重拦下");
                println!(
                    "        已有文档: id={} | DOI={} | 文件名={}",
                    existing_id,
                    existing_doi.as_deref().unwrap_or("（空）"),
                    existing_filename.as_deref().unwrap_or("（未知）")
                );
                if kind == "hash" {
                    println!(
                        "        原因: 这份 PDF 的内容（sha256）已在库中{}，与本次要登记的新文献是两条不同记录",
                        if same_file { "" } else { "（另一条记录）" }
                    );
                    println!(
                        "        处理: 多为库里那条记录的 PDF 装错（或你本地这份文件本来就是另一篇）——先确认 id={} 的真实内容再决定重传 PDF 或删掉它",
                        existing_id
                    );
                } else {
                    println!("        原因: {}", if message.is_empty() { "DOI 已存在" } else { message.as_str() });
                    println!("        处理: 用 --dup-doi-action hash 可改走「比对哈希、不同则覆盖已有 PDF」的路子");
                }
                failed += 1;
            }
            UploadOutcome::Failed => {
                println!("        失败");
                failed += 1;
            }
        }
    }

    // 6. 最终统计
    println!();
    println!("==================================================");
    println!("本地 PDF:     {}", total);
    println!("已上传:       {}", uploaded);
    println!("服务器已存在: {}", skipped);
    if hash_failed > 0 {
        println!("哈希失败:     {}", hash_failed);
    }
    println!("失败:         {}", failed);
    println!("用时:         {}", format_duration(loop_started.elapsed()));
    println!("==================================================");

    Ok(())
}

/// 与服务器比对单个文件哈希：`Some(true)` 已存在 / `Some(false)` 缺失 / `None` 比对失败。
///
/// 优先用一次性拉到的全量索引在本地比对；索引不可用时回退为单条 `diff-hashes` 请求。
async fn server_has_hash(
    client: &Client,
    base_url: &str,
    token: &Option<String>,
    server_index: &Option<ServerIndex>,
    hash: &str,
) -> Option<bool> {
    match server_index {
        Some(idx) => Some(idx.has_hash(hash)),
        None => {
            let diff = api_diff_hashes(client, base_url, &[hash.to_string()], token).await?;
            Some(!diff.missing.iter().any(|h| h.eq_ignore_ascii_case(hash)))
        }
    }
}
