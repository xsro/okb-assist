//! 子命令 `dir`：扫描目录中的 PDF，先用 lopdf 从 Info 字典取 DOI 与服务端比对，
//! 打印待上传清单让用户确认，确认后才逐个上传

use std::io::{self, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::time::Instant;

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

    /// 试运行：提取 DOI、与服务器比对并打印待上传清单，但不上传
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
    // 本地已知的「DOI → 哈希」（清单里已经拿到 DOI，成功后记下来供 `zotero --reuse-hash` 复用）
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

    // 1. 扫描 PDF
    let all_pdfs = scan_pdfs(Path::new(&args.dir), args.recursive)?;
    let total = all_pdfs.len();
    if total == 0 {
        println!("目录中没有找到 PDF 文件。");
        return Ok(());
    }
    println!("找到 {} 个 PDF 文件。", total);
    println!();

    // 2. 拉一次服务端全量 DOI / 哈希索引，比对全在本地完成（1 次请求；失败则逐文件回退单条查询）
    let client = http_client()?;
    println!("正在获取服务端索引（全部文献的 DOI / 文件哈希）...");
    let server_index = api_fetch_server_index(&client, &base_url, &token).await;
    match &server_index {
        Some(idx) => println!(
            "  服务端索引：{} 篇文献，{} 个可比对 DOI，{} 个可比对哈希（1 次请求）",
            idx.count,
            idx.by_doi.len(),
            idx.hashes.len()
        ),
        None => println!("  ⚠ 无法获取服务端索引，回退为逐个文件单条比对（较慢）"),
    }
    println!();

    // 3. 比对阶段：逐个「lopdf 读 Info 字典取 DOI → 与服务端比对」，这一步不写服务器任何东西
    println!("第一步：提取 DOI 并与服务器比对（共 {} 个）", total);
    println!("  DOI 取自 PDF 的 Info 字典（不解析正文）；提不到 DOI 的文件才计算 sha256 按哈希比对。");
    println!();
    let analyze_started = Instant::now();
    let mut plans: Vec<FilePlan> = Vec::with_capacity(total);
    for (i, path) in all_pdfs.iter().enumerate() {
        let plan = analyze_file(&client, &base_url, &token, &server_index, path).await;
        println!("  [{}/{}] {}", i + 1, total, plan.summary());
        plans.push(plan);
    }
    println!();
    println!("比对完成，用时 {}。", format_duration(analyze_started.elapsed()));
    println!();

    // 4. 打印待上传清单（编号即后面的上传顺序），等用户确认
    let candidates = print_plan(&plans);
    if candidates.is_empty() {
        println!("没有需要上传的文件。");
        return Ok(());
    }
    if args.dry_run {
        println!("试运行：以上清单不会上传。");
        return Ok(());
    }

    let selected = choose_uploads(&candidates, args.interval.yes);
    if selected.is_empty() {
        println!("已取消，未上传任何文件。");
        return Ok(());
    }

    // 5. 上传阶段
    println!();
    println!("开始上传 {} 个文件 ...", selected.len());
    let mut created = 0usize;
    let mut attached = 0usize;
    let mut duplicated = 0usize;
    let mut failed = 0usize;
    // 已执行的上传次数（用于按 `--upload-interval` 节流，首个不等待）
    let mut upload_seq = 0usize;
    let loop_started = Instant::now();

    for (i, plan) in selected.iter().enumerate() {
        args.interval.throttle(upload_seq).await;
        upload_seq += 1;
        // 行尾先换行，进度行独占一行（见 `UploadProgress`）
        println!(
            "  [{}/{}] {} · {}（{}）",
            i + 1,
            selected.len(),
            plan.upload_label(),
            plan.path.display(),
            format_bytes(plan.size)
        );
        match api_upload(
            &base_url,
            &plan.path,
            plan.doi.as_deref(),
            &token,
            args.interval.timeout_override(),
            args.interval.extract_meta,
        )
        .await
        {
            UploadOutcome::Created(result) => {
                // 比对结论说「补入」但服务端另给了 id / 状态时以服务端为准
                let status = result.status.as_deref().unwrap_or("created");
                match plan.action {
                    Action::Attach(_) => attached += 1,
                    _ => created += 1,
                }
                println!("        成功 -> id={}（{}）", result.id, status);
                // 记下 DOI → 哈希：DOI 以本地 Info 字典提取值兜底，服务端补入时不回传 doi
                let doi = result
                    .doi
                    .as_deref()
                    .filter(|s| !s.trim().is_empty())
                    .or(plan.doi.as_deref());
                if let Some(doi) = doi {
                    let h = result
                        .file_hash
                        .as_deref()
                        .or(plan.hash.as_deref())
                        .unwrap_or_default();
                    hash_store.record(doi, h);
                    hash_store.flush();
                }
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
                // 清单里判定「条目缺 PDF」才上传，服务端却说它已有文件 → 索引/健康状态过期，按已存在处理
                let stale_attach = matches!(plan.action, Action::Attach(_)) && kind == "doi";
                if stale_attach {
                    println!(
                        "        -> 服务端已有该 DOI 记录 id={} 且带 PDF（本地索引已过期），未上传",
                        existing_id
                    );
                    duplicated += 1;
                    continue;
                }
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
                    println!("        处理: 用 zotero 子命令的 --dup-doi-action hash 可改走「比对哈希、不同则覆盖已有 PDF」的路子");
                }
                duplicated += 1;
            }
            UploadOutcome::Failed => {
                println!("        失败");
                failed += 1;
            }
        }
    }

    // 6. 最终统计
    let blocked = plans
        .iter()
        .filter(|p| matches!(&p.action, Action::Blocked(_)))
        .count();
    println!();
    println!("==================================================");
    println!("本地 PDF:     {}", total);
    println!("新建记录:     {}", created);
    println!("补入已有条目: {}", attached);
    println!("服务器已存在: {}", plans.iter().filter(|p| matches!(&p.action, Action::Exists(..))).count());
    println!("服务端去重:   {}", duplicated);
    if blocked > 0 {
        println!("未能比对:   {}", blocked);
    }
    println!("失败:         {}", failed);
    println!("用时:         {}", format_duration(loop_started.elapsed()));
    println!("==================================================");

    Ok(())
}

// ==================== 比对计划 ====================

/// 比对阶段对单个 PDF 的结论
struct FilePlan {
    path: PathBuf,
    size: u64,
    /// 从 PDF Info 字典提取并归一化后的 DOI
    doi: Option<String>,
    /// 提不到 DOI 时才计算，用来与服务端比文件内容
    hash: Option<String>,
    action: Action,
    /// 附注（如 Info 字典读不出来），只在比对阶段那一行尾部显示
    note: Option<String>,
}

/// 单个 PDF 的处置结论
enum Action {
    /// 库里没有这个 DOI（没有 DOI 时 = 没有这份文件）→ 新建记录
    Create,
    /// 库里有同 DOI 的条目但没有 PDF → 把这份文件补进该条目
    Attach(u64),
    /// 库里已经有了，不上传：`(归类, 详情)`
    Exists(&'static str, String),
    /// 比对没做成（文件读不了 / 请求失败），本次不动它
    Blocked(String),
}

/// 清单分组（决定编号顺序）
#[derive(PartialEq, Clone, Copy)]
enum Category {
    NewByDoi,
    Attach,
    NewByHash,
}

impl FilePlan {
    fn is_upload(&self) -> bool {
        matches!(self.action, Action::Create | Action::Attach(_))
    }

    fn category(&self) -> Category {
        match &self.action {
            Action::Attach(_) => Category::Attach,
            _ if self.doi.is_some() => Category::NewByDoi,
            _ => Category::NewByHash,
        }
    }

    /// 比对阶段的一行结论
    fn summary(&self) -> String {
        let line = match (&self.doi, &self.hash) {
            (Some(doi), _) => doi.clone(),
            (None, Some(hash)) => format!("无 DOI · sha256 {}", &hash[..12.min(hash.len())]),
            (None, None) => "无 DOI · 未取哈希".to_string(),
        };
        let verdict = match &self.action {
            Action::Create => "需上传（库中没有）".to_string(),
            Action::Attach(id) => format!("需上传（补入条目 id={}）", id),
            Action::Exists(kind, detail) if detail.is_empty() => {
                format!("跳过（{}）", kind)
            }
            Action::Exists(kind, detail) => format!("跳过（{}：{}）", kind, detail),
            Action::Blocked(why) => format!("未比对：{}", why),
        };
        match &self.note {
            Some(note) => format!("{} → {} · {}", line, verdict, note),
            None => format!("{} → {}", line, verdict),
        }
    }

    /// 清单里的一行
    fn plan_line(&self) -> String {
        let head = match (&self.doi, &self.hash) {
            (Some(doi), _) => doi.clone(),
            (None, Some(hash)) => format!("sha256 {}…", &hash[..16.min(hash.len())]),
            (None, None) => "（无 DOI / 无哈希）".to_string(),
        };
        let target = match &self.action {
            Action::Attach(id) => format!("  → 补入 id={}", id),
            _ => String::new(),
        };
        format!(
            "{} · {} · {}{}",
            head,
            format_bytes(self.size),
            self.path.display(),
            target
        )
    }

    fn upload_label(&self) -> String {
        match &self.action {
            Action::Attach(id) => format!("补入条目 id={}", id),
            _ if self.doi.is_some() => "新建（库中无此 DOI）".to_string(),
            _ => "新建（库中无此哈希）".to_string(),
        }
    }
}

/// 单个文件的「取 DOI → 比对」：只读本地文件与查服务端，不写服务器
async fn analyze_file(
    client: &Client,
    base_url: &str,
    token: &Option<String>,
    index: &Option<ServerIndex>,
    path: &Path,
) -> FilePlan {
    let size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    let (doi, note) = match extract_doi_async(path.to_path_buf()).await {
        Ok(Some(doi)) => (Some(doi), None),
        Ok(None) => (None, None),
        Err(e) => (
            None,
            Some(format!("Info 字典读不出（按哈希比对）: {:#}", e)),
        ),
    };

    // 有 DOI 就以 DOI 为准：库里没有该 DOI，或有该 DOI 但条目没有 PDF，才需要传
    if let Some(doi) = doi.as_ref() {
        let action = match lookup_server_entry(client, base_url, token, index, doi).await {
            None => Action::Create,
            Some(entry) => {
                // 「没有文件」= 服务端没算出 file_hash，或健康检查已判定 missing
                if entry.hash().is_empty() || entry.health() == "missing" {
                    Action::Attach(entry.id)
                } else {
                    let h = entry.hash();
                    Action::Exists(
                        "同 DOI 条目已有 PDF",
                        format!("id={} · 服务端哈希 {}", entry.id, &h[..12.min(h.len())]),
                    )
                }
            }
        };
        return FilePlan {
            path: path.to_path_buf(),
            size,
            doi: Some(doi.clone()),
            hash: None,
            action,
            note,
        };
    }

    // 提不到 DOI：只能比文件内容
    let hash = match sha256_file_async(path.to_path_buf()).await {
        Ok(h) => h,
        Err(e) => {
            return FilePlan {
                path: path.to_path_buf(),
                size,
                doi: None,
                hash: None,
                action: Action::Blocked(format!("计算哈希失败: {:#}", e)),
                note,
            }
        }
    };
    let action = match server_has_hash(client, base_url, token, index, &hash).await {
        Some(true) => Action::Exists("哈希已在库中", String::new()),
        Some(false) => Action::Create,
        None => Action::Blocked("哈希比对请求失败（可重试）".to_string()),
    };
    FilePlan {
        path: path.to_path_buf(),
        size,
        doi: None,
        hash: Some(hash),
        action,
        note,
    }
}

/// 按 DOI 取服务端记录：`None` = 库里没有这个 DOI。
///
/// 全量索引可用时在本地查；不可用时回退 `by-doi` 单条查询。回退路径分不清
/// 「查不到」与「请求失败」，一律当「库里没有」处理 —— 真重复时服务端的 DOI 去重
/// 会返回 409，不会造出第二条记录。
async fn lookup_server_entry(
    client: &Client,
    base_url: &str,
    token: &Option<String>,
    index: &Option<ServerIndex>,
    doi: &str,
) -> Option<ServerEntry> {
    match index {
        Some(idx) => idx.lookup(doi).cloned(),
        None => match api_lookup_by_doi(client, base_url, doi, token).await {
            (true, Some(doc)) => Some(ServerEntry::from_doc_json(&doc)),
            _ => None,
        },
    }
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

// ==================== 清单确认 ====================

/// 打印待上传清单（按处置方式分组）与不上传的部分，返回带编号的候选项
fn print_plan(plans: &[FilePlan]) -> Vec<&FilePlan> {
    /// 每组最多列出的条数（未列出的仍然会被上传）
    const LIST_LIMIT: usize = 200;

    let upload: Vec<&FilePlan> = plans.iter().filter(|p| p.is_upload()).collect();
    let exists: Vec<(&'static str, usize)> = {
        let mut buckets: Vec<(&'static str, usize)> = Vec::new();
        for p in plans.iter().filter_map(|p| match &p.action {
            Action::Exists(kind, _) => Some(*kind),
            _ => None,
        }) {
            match buckets.iter_mut().find(|(k, _)| *k == p) {
                Some((_, n)) => *n += 1,
                None => buckets.push((p, 1)),
            }
        }
        buckets
    };
    let blocked: Vec<&FilePlan> = plans
        .iter()
        .filter(|p| matches!(&p.action, Action::Blocked(_)))
        .collect();
    let no_doi = plans.iter().filter(|p| p.doi.is_none()).count();

    println!("==================================================");
    println!(
        "比对结果：{} 个文件，{} 个需要上传（{} 个提取不到 DOI，改按文件哈希比对）",
        plans.len(),
        upload.len(),
        no_doi
    );
    println!();

    let groups = [
        (Category::NewByDoi, "新建 —— 库中没有这个 DOI"),
        (Category::Attach, "补入 —— 同 DOI 的条目已存在但缺 PDF"),
        (Category::NewByHash, "新建 —— 提取不到 DOI，按文件哈希比对"),
    ];
    let mut candidates: Vec<&FilePlan> = Vec::new();
    for (category, title) in groups {
        let items: Vec<&FilePlan> = upload
            .iter()
            .copied()
            .filter(|p| p.category() == category)
            .collect();
        if items.is_empty() {
            continue;
        }
        let bytes: u64 = items.iter().map(|p| p.size).sum();
        println!("── {}（{} 个 · {}）", title, items.len(), format_bytes(bytes));
        for (i, item) in items.iter().enumerate() {
            if i < LIST_LIMIT {
                println!("  [{:>3}] {}", candidates.len() + 1, item.plan_line());
            }
            candidates.push(item);
        }
        if items.len() > LIST_LIMIT {
            println!(
                "  …（其余 {} 个未列出，确认后同样会上传）",
                items.len() - LIST_LIMIT
            );
        }
        println!();
    }

    if !exists.is_empty() {
        let detail: Vec<String> = exists
            .iter()
            .map(|(kind, n)| format!("{} {} 个", kind, n))
            .collect();
        println!(
            "不需要上传（{}）：{}",
            exists.iter().map(|(_, n)| n).sum::<usize>(),
            detail.join(" / ")
        );
    }
    if !blocked.is_empty() {
        println!("未能比对（{}，本次不会上传）：", blocked.len());
        for p in blocked.iter().take(20) {
            if let Action::Blocked(why) = &p.action {
                println!("  · {} —— {}", p.path.display(), why);
            }
        }
        if blocked.len() > 20 {
            println!("  …（其余 {} 个未列出）", blocked.len() - 20);
        }
    }
    println!();
    candidates
}

/// 让用户确认清单：`y` 全部上传、`s` 逐个挑选、其他 = 取消；带 `-y` 时不询问，直接全部上传
fn choose_uploads<'a>(candidates: &[&'a FilePlan], assume_yes: bool) -> Vec<&'a FilePlan> {
    if assume_yes {
        println!("已用 -y 自动确认：{} 个文件全部上传。", candidates.len());
        return candidates.to_vec();
    }
    // 提示末尾带换行：半截行在部分终端（重定向 / ConPTY）里会一直留在缓冲里，看不出程序其实在等输入
    println!(
        "确认上传这 {} 个文件？[y] 全部上传  [s] 逐个挑选  [n] 取消: ",
        candidates.len()
    );
    if !io::stdin().is_terminal() {
        println!("标准输入不是终端（管道 / 重定向 / 后台执行），不会等待输入；要直接上传请加 -y。");
        return Vec::new();
    }
    io::stdout().flush().ok();
    let mut answer = String::new();
    io::stdin().read_line(&mut answer).ok();
    match answer.trim().to_lowercase().as_str() {
        "y" | "yes" => candidates.to_vec(),
        "s" => {
            let mut selected: Vec<&FilePlan> = Vec::new();
            for plan in candidates {
                println!("  上传 {}？[y/N]: ", plan.path.display());
                io::stdout().flush().ok();
                let mut one = String::new();
                io::stdin().read_line(&mut one).ok();
                if one.trim().eq_ignore_ascii_case("y") {
                    selected.push(*plan);
                } else {
                    println!("    跳过。");
                }
            }
            selected
        }
        _ => Vec::new(),
    }
}

// ==================== PDF Info → DOI（lopdf） ====================

/// 用 lopdf 只读文件结构与 Info 字典（不解析正文）拿 DOI
///
/// 与服务端 `extract_meta=light` 同源：出版商把 DOI 写进自定义 Info 键（`/doi`）很常见，
/// 也可能混在 `/Subject`、`/Keywords` 里，所以整本字典的字符串值都扫一遍，
/// 排序规则见 `best_doi_from_entries`。
/// `Err` = 连结构都没读出来（损坏 / 加密），`Ok(None)` = 读到了但里面没有 DOI，
/// 两种情况都退化成按文件哈希比对。
fn doi_from_pdf_info(path: &Path) -> Result<Option<String>> {
    let meta = lopdf::Document::load_metadata(path)
        .with_context(|| format!("无法解析 PDF: {}", path.display()))?;

    // 标准字段已在 PdfMetadata 上拍平成 String，自定义键（`/doi` 这类）还在 custom 里
    let mut entries: Vec<(String, String)> = Vec::new();
    for (key, value) in [
        ("Title", meta.title),
        ("Author", meta.author),
        ("Subject", meta.subject),
        ("Keywords", meta.keywords),
        ("Creator", meta.creator),
        ("Producer", meta.producer),
    ] {
        if let Some(value) = value.filter(|v| !v.trim().is_empty()) {
            entries.push((key.to_string(), value));
        }
    }
    for (key, value) in &meta.custom {
        if let Some(text) = info_value_text(value) {
            if !text.trim().is_empty() {
                entries.push((String::from_utf8_lossy(key).to_string(), text));
            }
        }
    }

    Ok(best_doi_from_entries(entries))
}

/// 按「键名可信度 → 键名」排序后逐个扫描，取第一个像 DOI 的值
///
/// `lopdf` 的 custom 是 HashMap，遍历顺序每次都不一样，直接扫会让同一个文件在两轮比对里
/// 报出不同的 DOI；排序后结果与遍历顺序无关。
fn best_doi_from_entries(entries: Vec<(String, String)>) -> Option<String> {
    let mut entries = entries;
    entries.sort_by(|a, b| {
        doi_key_rank(&a.0)
            .cmp(&doi_key_rank(&b.0))
            .then_with(|| a.0.cmp(&b.0))
    });
    entries.iter().find_map(|(_, value)| find_doi_in(value))
}

/// Info 键名的 DOI 可信度（越小越先扫）
///
/// WPS 导出的 PDF 会把刊级 DOI 和文章 DOI 并存：`/WPS-ARTICLEDOI` = `10.1049/cth2.12206`、
/// `/WPS-JOURNALDOI` = `10.1049/(ISSN)1751-8652`。后者标识的是**刊物**，拿它登记会把同刊文章
/// 并成同一条记录，所以带 `article` 的键排最前，其次才是单纯的 `/doi`。
fn doi_key_rank(key: &str) -> u8 {
    let k = key.to_ascii_lowercase();
    let plain: String = k.chars().filter(|c| c.is_ascii_alphanumeric()).collect();
    if k.contains("doi") && k.contains("article") {
        0
    } else if plain == "doi" {
        1
    } else if k.contains("doi") {
        2
    } else {
        3
    }
}

/// `doi_from_pdf_info` 的异步版（读整个文件 + 解析结构都是阻塞操作，丢到后台线程）
async fn extract_doi_async(path: PathBuf) -> Result<Option<String>> {
    tokio::task::spawn_blocking(move || doi_from_pdf_info(&path))
        .await
        .map_err(|e| anyhow::anyhow!("DOI 提取任务崩溃: {}", e))?
}

/// Info 条目取可读文本：字符串按 PDF 编码（PDFDocEncoding / UTF-16BE）解码，名字直接按字节取
fn info_value_text(value: &lopdf::Object) -> Option<String> {
    match value {
        lopdf::Object::String(_, _) => lopdf::decode_text_string(value).ok(),
        lopdf::Object::Name(bytes) => Some(String::from_utf8_lossy(bytes).to_string()),
        _ => None,
    }
}

/// DOI 允许的字符集（与服务端 `pdf_meta::is_doi_char` 一致）
fn is_doi_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || "-._;()/:".contains(c)
}

/// 是否停在 DOI 常见的「折断点」上（`.` `/` `-` `_`）
fn ends_at_doi_break(s: &str) -> bool {
    let t = s.trim_end();
    t.ends_with('.') || t.ends_with('/') || t.ends_with('-') || t.ends_with('_')
}

/// 拼回「同一串里被空格拆开的 DOI 续写」，判据与服务端 `extend_doi_over_space` 相同
///
/// 中文期刊的 `/doi` 值常见 `10.3969/j. issn.1007-130X.2024.12.001`，不接续就只能拿到
/// 半截 `10.3969/j`（与服务端存的 DOI 指纹对不上，还会把残值写进库里）。
/// 只在「已匹配到折断点」且「空格后的片段明显像 DOI 续写（≥2 个点，或含 `-` / `/`）」时接续，
/// 句末句号后面跟英文单词不会被粘进来。
fn extend_doi_over_space(matched: &str, rest: &str) -> String {
    let mut doi = matched.to_string();
    let mut rest = rest;
    while ends_at_doi_break(&doi) {
        let trimmed = rest.trim_start();
        if trimmed.len() == rest.len() {
            break; // 后面没有空白 ⇒ 已经到头
        }
        let token: String = trimmed.chars().take_while(|c| is_doi_char(*c)).collect();
        if token.is_empty() {
            break;
        }
        let doi_ish =
            token.matches('.').count() >= 2 || token.contains('-') || token.contains('/');
        if !doi_ish {
            break;
        }
        doi.push_str(&token);
        rest = &trimmed[token.len()..];
    }
    doi
}

/// 在任意文本里找第一个 `10.NNNN/后缀` 形态的 DOI（对齐服务端 `scan_doi` 的 plain 分支）
///
/// 前缀（`doi:`、`https://doi.org/`）不用管：只认 `10.` 起始的核心，尾部标点交给 `normalize_doi`。
fn find_doi_in(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut from = 0usize;
    while let Some(offset) = text[from..].find("10.") {
        let start = from + offset;
        from = start + 3;
        // 前面贴着数字或点号的不是 DOI 起点（`110.1234/x` 是版本号/长号，不是注册机构号）；
        // `https://doi.org/10.x` 这种带前缀的写法很常见，分隔符一律放行
        if start > 0 && (bytes[start - 1].is_ascii_digit() || bytes[start - 1] == b'.') {
            continue;
        }
        // `10.` 后必须是 4~9 位注册机构号，再跟一个 `/`
        let mut idx = start + 3;
        let digits_start = idx;
        while idx < bytes.len() && bytes[idx].is_ascii_digit() {
            idx += 1;
        }
        let digits = idx - digits_start;
        if !(4..=9).contains(&digits) || idx >= bytes.len() || bytes[idx] != b'/' {
            continue;
        }
        let suffix_start = idx + 1;
        let mut end = suffix_start;
        while end < bytes.len() && is_doi_char(bytes[end] as char) {
            end += 1;
        }
        if end == suffix_start {
            continue;
        }
        // 后缀字符集是 ASCII，切片两端必然落在字符边界上
        let matched = &text[start..end];
        let extended = extend_doi_over_space(matched, &text[end..]);
        // 刊级/书级标识不是这篇文献的 DOI（`10.1049/(ISSN)1751-8652`），继续找下一个候选
        let lower = extended[3..].to_ascii_lowercase();
        if lower.contains("(issn") || lower.contains("(isbn") {
            continue;
        }
        if let Some(doi) = normalize_doi(&extended) {
            return Some(doi);
        }
    }
    None
}

#[cfg(test)]
mod doi_tests {
    use super::*;

    #[test]
    fn finds_plain_doi() {
        assert_eq!(
            find_doi_in("10.1007/s11118-024-10109-8").as_deref(),
            Some("10.1007/s11118-024-10109-8")
        );
        assert_eq!(
            find_doi_in("doi: 10.1360/SST-2023-0410.").as_deref(),
            Some("10.1360/SST-2023-0410")
        );
        assert_eq!(
            find_doi_in("https://doi.org/10.1109/TWC.2024.1234567").as_deref(),
            Some("10.1109/TWC.2024.1234567")
        );
        // Info 字典里 `/doi` 常整条写成 URL
        assert_eq!(
            find_doi_in("http://dx.doi.org/10.1016/j.ins.2024.123456").as_deref(),
            Some("10.1016/j.ins.2024.123456")
        );
    }

    #[test]
    fn rejoins_space_split_doi() {
        // 中文期刊 /doi 值的常见写法
        assert_eq!(
            find_doi_in("10.3969/j. issn.1007-130X.2024.12.001").as_deref(),
            Some("10.3969/j.issn.1007-130X.2024.12.001")
        );
    }

    #[test]
    fn does_not_swallow_prose_after_the_doi() {
        assert_eq!(
            find_doi_in("see 10.1234/abc. In this paper we study").as_deref(),
            Some("10.1234/abc")
        );
    }

    #[test]
    fn rejects_numbers_that_are_not_doi_prefixes() {
        // 注册机构号不足 4 位 / 前面还贴着一串数字 ⇒ 不是 DOI
        assert_eq!(find_doi_in("version 10.1 fixed"), None);
        assert_eq!(find_doi_in("build 110.1234/abc"), None);
    }

    #[test]
    fn skips_journal_level_issn_doi() {
        assert_eq!(find_doi_in("10.1049/(ISSN)1751-8652"), None);
        // 同一串里先刊级后文章级：跳过前者
        assert_eq!(
            find_doi_in("10.1049/(ISSN)1751-8652; 10.1049/cth2.12206").as_deref(),
            Some("10.1049/cth2.12206")
        );
        // 中文期刊 `j.issn.` 的常规写法不受影响
        assert_eq!(
            find_doi_in("10.3969/j.issn.1007-130X.2024.12.001").as_deref(),
            Some("10.3969/j.issn.1007-130X.2024.12.001")
        );
    }

    #[test]
    fn prefers_article_doi_over_journal_doi() {
        // 实测样本：WPS 导出的 PDF 里 /WPS-ARTICLEDOI 与 /WPS-JOURNALDOI 并存
        let hit = best_doi_from_entries(vec![
            (
                "Subject".to_string(),
                "IET Control Theory & Appl 2022.16:79-93".to_string(),
            ),
            (
                "WPS-JOURNALDOI".to_string(),
                "10.1049/(ISSN)1751-8652".to_string(),
            ),
            ("Producer".to_string(), "Acrobat 10.0".to_string()),
            ("WPS-ARTICLEDOI".to_string(), "10.1049/cth2.12206".to_string()),
        ]);
        assert_eq!(hit.as_deref(), Some("10.1049/cth2.12206"));
    }

    #[test]
    fn entry_order_does_not_change_the_result() {
        // custom 是 HashMap，遍历顺序每次不同；排序后同一个文件必须每次给出同一个 DOI
        let entries = vec![
            ("doi".to_string(), "10.1000/real-doi".to_string()),
            ("Producer".to_string(), "Acrobat 10.0 /10.9999/typo".to_string()),
        ];
        assert_eq!(
            best_doi_from_entries(entries.clone()).as_deref(),
            Some("10.1000/real-doi")
        );
        assert_eq!(
            best_doi_from_entries(entries.into_iter().rev().collect()).as_deref(),
            Some("10.1000/real-doi")
        );
    }
}
