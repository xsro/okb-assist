//! 日志模块：三路输出。
//!
//! 1. **stdout**：可读文本（控制台/系统日志）；
//! 2. **JSONL 文件**：结构化历史日志（`system.json` 的 `log_path`），供归档与外部工具采集；
//! 3. **进程内环形缓冲**：最近 [`RING_CAPACITY`] 条记录，供 `/assist/api/admin/logs`
//!    短轮询做「实时日志」用（前端不再读日志文件，见 AGENTS.md 易错点 22）。
//!
//! 文件层与环形缓冲层都直接由 `fmt::layer().json()` 产出，因此缓冲里的每一行
//! 与 `okb-log.jsonl` 里的行**逐字节一致**，前端 `LogEntry` 结构无需区分来源。

use std::collections::VecDeque;
use std::fs::OpenOptions;
use std::io;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};
use tracing::Level;
use tracing_subscriber::fmt::MakeWriter;
use tracing_subscriber::prelude::*;
use tracing_subscriber::{fmt, EnvFilter, Registry};

use serde_json::{json, Value};

/// 初始化日志系统。
///
/// - `log_path`: JSONL 日志文件路径。若为空或不合法，默认使用 `okb-log.jsonl`。
/// - `filter`: tracing 过滤器字符串（如 `"okb_assist=info,tower_http=info"`）
///
/// 返回日志文件的绝对路径。
pub fn init_dual_logging(log_path: &str, filter: &str) -> (PathBuf, impl Drop) {
    let path = resolve_log_path(log_path);
    println!("日志文件: {}", path.display());

    // 打开日志文件
    let _file = Arc::new(Mutex::new(
        OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .expect("无法打开日志文件"),
    ));

    // 环境过滤器（只作用于文件层与 stdout 层）
    let env_filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new(filter));

    // JSONL 层：最大详细度输出到文件
    let json_writer = JsonlFile::new(&path);
    let json_layer = fmt::layer()
        .json()
        .with_writer(json_writer)
        .with_current_span(true)
        .with_span_list(true)
        .with_thread_ids(true)
        .with_thread_names(true)
        .with_target(true)
        .with_file(true)
        .with_line_number(true);

    // 可读层：简洁格式输出到 stdout
    let stdout_layer = fmt::layer()
        .with_writer(io::stdout)
        .with_target(false)
        .with_file(false)
        .with_line_number(false)
        .with_thread_ids(false)
        .with_thread_names(false);

    // 环形缓冲层：**不受 `--log-level` 限制**（自己不加 filter），
    // 这样前端把等级切到 debug/trace 时才有内容可看；代价只是多一次
    // 内存序列化，未打开日志页时也仅此而已。
    let ring_layer = fmt::layer()
        .json()
        .with_writer(RingBufferWriter)
        .with_current_span(true)
        .with_span_list(true)
        .with_thread_ids(true)
        .with_thread_names(true)
        .with_target(true)
        .with_file(true)
        .with_line_number(true);

    // 合并订阅者：前两层按 `--log-level` 过滤（单层 filter），第三层不过滤
    let subscriber = Registry::default()
        .with(json_layer.with_filter(env_filter.clone()))
        .with(stdout_layer.with_filter(env_filter))
        .with(ring_layer);

    tracing::subscriber::set_global_default(subscriber)
        .expect("无法设置全局日志订阅者");

    (path, LogGuard)
}

/// 解析日志文件路径。
fn resolve_log_path(path: &str) -> PathBuf {
    let trimmed = path.trim();
    if trimmed.is_empty() || trimmed == "stdout" {
        PathBuf::from("okb-log.jsonl")
    } else {
        PathBuf::from(trimmed)
    }
}

// ── JSONL 文件写入器 ──────────────────────────────────

/// 使用 `tracing-appender` 风格的简单文件写入器。
/// 无需额外依赖：内部用 `std::sync::Mutex` 保护 `std::fs::File`。
#[derive(Clone)]
struct JsonlFile {
    file: Arc<Mutex<std::fs::File>>,
}

impl JsonlFile {
    fn new(path: &PathBuf) -> Self {
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .expect("无法打开日志文件");
        Self {
            file: Arc::new(Mutex::new(file)),
        }
    }
}

/// 实现 MakeWriter — 每次返回一个持有文件锁的写入器。
impl<'a> MakeWriter<'a> for JsonlFile {
    type Writer = JsonlGuard;

    fn make_writer(&'a self) -> Self::Writer {
        JsonlGuard(self.file.clone())
    }
}

/// 临时持有文件锁的写入器。
struct JsonlGuard(Arc<Mutex<std::fs::File>>);

impl io::Write for JsonlGuard {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.lock().map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))?.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.0.lock().map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))?.flush()
    }
}

/// 确保日志文件在程序退出前关闭
struct LogGuard;

impl Drop for LogGuard {
    fn drop(&mut self) {
        // tracing subscriber 在 guard 释放时自动冲刷全部层
    }
}

// ── 环形缓冲写入器（实时日志流的数据源） ─────────────────

/// `MakeWriter`：把 fmt 层写出的**一整条** JSON 记录追加进环形缓冲。
///
/// 复用 `fmt::layer().json()`（而不是自己实现 `Layer` + `Visit`），
/// 保证缓冲里的行与 `okb-log.jsonl` 里的行格式完全一致。
#[derive(Clone, Copy)]
struct RingBufferWriter;

impl<'a> MakeWriter<'a> for RingBufferWriter {
    type Writer = EventBuf;

    fn make_writer(&'a self) -> Self::Writer {
        // fmt 层对每条事件取一次 writer，写完即 drop，因此 Drop 里就是「一条完整记录」
        EventBuf { buf: Vec::with_capacity(256) }
    }
}

/// 单条事件的写入缓冲：fmt 层写完（Drop）时整行入队。
struct EventBuf {
    buf: Vec<u8>,
}

impl io::Write for EventBuf {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.buf.extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Drop for EventBuf {
    fn drop(&mut self) {
        if self.buf.is_empty() {
            return;
        }
        let line = String::from_utf8_lossy(&self.buf);
        push_line(line.trim());
    }
}

/// 单条日志进入环形缓冲的字节上限。
///
/// 缓冲按「条数」限量（[`RING_CAPACITY`] 条），若某处把大 JSON / 整段 SQL 打进来，
/// 2000 条就能吃掉几十 MB。超长的那条压成「纯文本预览 + truncated 标记」。
const MAX_RECORD_BYTES: usize = 8 * 1024;

/// 把一行 JSONL 解析后入环形缓冲（解析失败时按纯文本条目收下）。
fn push_line(line: &str) {
    if line.is_empty() {
        return;
    }
    match serde_json::from_str::<Value>(line) {
        Ok(entry) => {
            let level = entry
                .get("level")
                .and_then(Value::as_str)
                .and_then(level_from_name)
                .unwrap_or(Level::INFO);
            let entry = if line.len() > MAX_RECORD_BYTES {
                shrink(&entry, line)
            } else {
                entry
            };
            store().push(level, entry);
        }
        // fmt JSON 层的输出总是合法 JSON；这里只兜底（例如未来换成非 JSON writer）
        Err(_) => store().push(
            Level::INFO,
            json!({
                "timestamp": "",
                "level": "INFO",
                "target": "",
                "fields": { "message": line },
            }),
        ),
    }
}

/// 超长记录压成纯文本预览（保留时间/级别/target，正文截断）。
fn shrink(entry: &Value, raw: &str) -> Value {
    let preview: String = raw.chars().take(MAX_RECORD_BYTES / 2).collect();
    json!({
        "timestamp": entry.get("timestamp").cloned().unwrap_or(Value::String(String::new())),
        "level": entry.get("level").cloned().unwrap_or(Value::String("INFO".to_string())),
        "target": entry.get("target").cloned().unwrap_or(Value::String(String::new())),
        "truncated": true,
        "fields": {
            "message": format!("{}……（本条日志过长，已截断）", preview),
        },
    })
}

// ── 日志环形缓冲 ────────────────────────────────────────

/// 环形缓冲容量（条）。2000 条足够覆盖「页面停了很久再回来」的跨度，
/// 内存占用上限在个位数 MB。
pub const RING_CAPACITY: usize = 2000;

/// 一条日志记录。
#[derive(Clone)]
pub struct LogRecord {
    /// 单调递增序号（从 1 开始）。前端把它当增量游标：只要 `seq > since` 的记录。
    pub seq: u64,
    /// 解析后的级别，用于「该等级及以上」的阈值过滤。
    pub level: Level,
    /// 与 `okb-log.jsonl` 里一行完全一致的 JSON 对象。
    pub entry: Value,
}

struct RingInner {
    entries: VecDeque<LogRecord>,
    /// 下一条记录将使用的序号（= 最新序号 + 1）
    next_seq: u64,
}

/// 进程内日志环形缓冲（纯内存，进程重启即清空）。
pub struct LogStore {
    inner: Mutex<RingInner>,
}

impl LogStore {
    fn new() -> Self {
        Self {
            inner: Mutex::new(RingInner {
                entries: VecDeque::with_capacity(RING_CAPACITY),
                next_seq: 1,
            }),
        }
    }

    /// 追加一条记录；超出容量时丢最旧的。
    ///
    /// 锁中毒时继续用里面的数据 —— 日志采集不能因为某次格式化 panic 就永久停摆。
    fn push(&self, level: Level, entry: Value) {
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let seq = guard.next_seq;
        guard.next_seq = seq + 1;
        if guard.entries.len() >= RING_CAPACITY {
            guard.entries.pop_front();
        }
        guard.entries.push_back(LogRecord { seq, level, entry });
    }

    /// 取 `seq > since` 且 `level` 不低于 `min_level` 的记录（按时间正序）。
    ///
    /// - `limit`：本次最多返回多少条。命中数超过 `limit` 时**返回最旧的那批**
    ///   并置 `truncated = true`，前端据此立即再拉一次把落后的部分追平（不静默丢日志）。
    /// - `lost`：`since` 之后、但已经被挤出缓冲区的记录条数（前端提示"中间有日志被丢弃"）。
    /// - `keep`：附加过滤谓词（如「隐藏本接口自身的请求日志」）。`matched` / `truncated`
    ///   都按过滤后的结果统计，因此游标语义不受影响。
    ///
    /// 返回的每个条目都会补一个 `seq` 字段（前端用它做稳定的渲染 key 与展开状态标识）。
    pub fn snapshot<F>(&self, since: u64, min_level: Level, limit: usize, keep: F) -> Snapshot
    where
        F: Fn(&Value) -> bool,
    {
        let limit = limit.max(1);
        let guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());

        let next_seq = guard.next_seq;
        let latest_seq = next_seq.saturating_sub(1);
        let oldest_seq = guard.entries.front().map(|r| r.seq);

        // 已经被挤出缓冲区的部分（since 太旧）
        let lost = match oldest_seq {
            Some(oldest) if since > 0 && since + 1 < oldest => oldest - since - 1,
            _ => 0,
        };

        let threshold = level_rank(min_level);
        let mut entries: Vec<Value> = Vec::new();
        let mut matched = 0usize;
        // 最后一条「已装进本批」的记录序号：截断时游标必须停在这里，
        // 否则前端拿着全局最新序号会跳过剩下那部分日志。
        let mut last_seq = 0u64;
        for record in guard.entries.iter() {
            if record.seq <= since || level_rank(record.level) < threshold || !keep(&record.entry) {
                continue;
            }
            matched += 1;
            if entries.len() < limit {
                let mut entry = record.entry.clone();
                // 补序号：前端的 v-for key 与「展开哪一条」都用它，追加新日志时才不会错位/折叠
                entry["seq"] = json!(record.seq);
                entries.push(entry);
                last_seq = record.seq;
            }
        }
        let truncated = matched > entries.len();
        // `since` 是**排他**下界（只返回 seq > since）。没截断 = 已追平，游标推到
        // 「最新序号 + 1」；截断 = 只消费到本批最后一条，游标就停在它的序号上，
        // 下一轮正好从它的下一条继续（既不跳过也不重复）。
        let cursor = if truncated { last_seq } else { next_seq };

        Snapshot {
            entries,
            matched,
            next_seq: cursor,
            latest_seq,
            oldest_seq,
            lost,
            truncated,
        }
    }
}

/// [`LogStore::snapshot`] 的结果。
pub struct Snapshot {
    pub entries: Vec<Value>,
    /// 满足过滤条件的总条数（可能大于 `entries.len()`，见 `truncated`）
    pub matched: usize,
    /// 下次请求应作为 `since` 传入的游标（`since` 为排他下界）：未截断时等于
    /// 「最新序号 + 1」，截断时等于「本批最后一条的序号」（保证不跳过、不重复）。
    pub next_seq: u64,
    pub latest_seq: u64,
    /// 缓冲里最旧记录的序号（空缓冲区为 `None`）
    pub oldest_seq: Option<u64>,
    /// `since` 之后被挤出缓冲区的条数
    pub lost: u64,
    /// 命中条数超过 `limit`（前端应立刻再拉）
    pub truncated: bool,
}

static LOG_STORE: OnceLock<LogStore> = OnceLock::new();

/// 取全局日志缓冲（懒初始化，任何线程可调用）。
pub fn store() -> &'static LogStore {
    LOG_STORE.get_or_init(LogStore::new)
}

// ── 级别工具 ────────────────────────────────────────────

/// 级别严重度：trace=0、debug=1、info=2、warn=3、error=4（越大越严重）。
pub fn level_rank(level: Level) -> u8 {
    match level {
        Level::TRACE => 0,
        Level::DEBUG => 1,
        Level::INFO => 2,
        Level::WARN => 3,
        Level::ERROR => 4,
    }
}

/// 解析级别名（大小写不敏感）。
pub fn level_from_name(name: &str) -> Option<Level> {
    match name.trim().to_ascii_lowercase().as_str() {
        "trace" => Some(Level::TRACE),
        "debug" => Some(Level::DEBUG),
        "info" => Some(Level::INFO),
        "warn" | "warning" => Some(Level::WARN),
        "error" => Some(Level::ERROR),
        _ => None,
    }
}

/// 级别名（小写），用于 API 回显。
pub fn level_name(level: Level) -> &'static str {
    match level {
        Level::TRACE => "trace",
        Level::DEBUG => "debug",
        Level::INFO => "info",
        Level::WARN => "warn",
        Level::ERROR => "error",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resolve_log_path() {
        assert_eq!(resolve_log_path("stdout"), PathBuf::from("okb-log.jsonl"));
        assert_eq!(resolve_log_path(""), PathBuf::from("okb-log.jsonl"));
        assert_eq!(resolve_log_path(" "), PathBuf::from("okb-log.jsonl"));
        assert_eq!(
            resolve_log_path("data/logs/app.jsonl"),
            PathBuf::from("data/logs/app.jsonl")
        );
    }

    #[test]
    fn level_names_and_ranks() {
        assert_eq!(level_from_name("WARN"), Some(Level::WARN));
        assert_eq!(level_from_name(" warning "), Some(Level::WARN));
        assert_eq!(level_from_name("nope"), None);
        assert_eq!(level_name(Level::ERROR), "error");
        assert!(level_rank(Level::ERROR) > level_rank(Level::WARN));
    }

    /// 游标语义：since 之后的记录才返回；超过 limit 时保留最旧的并标记 truncated；
    /// 被挤出缓冲区的部分计入 lost。
    #[test]
    fn snapshot_cursor_semantics() {
        let store = LogStore::new();
        for i in 0..5 {
            store.push(Level::INFO, json!({ "fields": { "message": i.to_string() } }));
        }
        // 首次（since = 0）拿全部
        let snap = store.snapshot(0, Level::INFO, 100, |_| true);
        assert_eq!(snap.entries.len(), 5);
        assert_eq!(snap.next_seq, 6);
        assert_eq!(snap.lost, 0);
        assert!(!snap.truncated);

        // 增量：只要 seq > 3 的
        let snap = store.snapshot(3, Level::INFO, 100, |_| true);
        assert_eq!(snap.entries.len(), 2);

        // limit 生效：返回最旧的一批 + truncated
        let snap = store.snapshot(0, Level::INFO, 2, |_| true);
        assert_eq!(snap.entries.len(), 2);
        assert_eq!(snap.matched, 5);
        assert!(snap.truncated);
        assert_eq!(snap.entries[0]["fields"]["message"], json!("0"));
        // 截断时游标停在最后一条已返回记录的序号上（不跳到全局最新）
        assert_eq!(snap.next_seq, 2);
        // 顺着游标继续拉，能拿到剩下的全部（不多不少）
        let snap = store.snapshot(2, Level::INFO, 100, |_| true);
        assert_eq!(snap.entries.len(), 3);
        assert_eq!(snap.next_seq, 6);

        // 等级阈值：warn 及以上
        store.push(Level::WARN, json!({ "fields": { "message": "w" } }));
        let snap = store.snapshot(0, Level::WARN, 100, |_| true);
        assert_eq!(snap.entries.len(), 1);
        assert_eq!(snap.entries[0]["fields"]["message"], json!("w"));

        // 游标太旧（记录已被挤出）：lost 计入
        let snap = store.snapshot(1, Level::INFO, 100, |_| true);
        assert_eq!(snap.lost, 0);
    }

    /// 附加过滤谓词：被过滤掉的不计入 matched、不影响游标；返回的条目带 seq
    #[test]
    fn snapshot_filter_and_seq_injection() {
        let store = LogStore::new();
        for i in 0..4 {
            store.push(Level::INFO, json!({ "fields": { "message": i.to_string() } }));
        }
        // 只保留 message != "1"
        let snap = store.snapshot(0, Level::INFO, 100, |entry| {
            entry["fields"]["message"] != json!("1")
        });
        assert_eq!(snap.entries.len(), 3);
        assert_eq!(snap.matched, 3);
        assert_eq!(snap.next_seq, 5);
        // seq 原样带出（用于前端稳定 key）
        assert_eq!(snap.entries[0]["seq"], json!(1));
        assert_eq!(snap.entries[1]["seq"], json!(3));
    }

    /// 超长记录压成预览，避免单条日志占掉大量内存
    #[test]
    fn oversized_record_is_shrunk() {
        let long = "x".repeat(MAX_RECORD_BYTES * 2);
        push_line(&format!(
            "{{\"timestamp\":\"t\",\"level\":\"WARN\",\"target\":\"t\",\"fields\":{{\"message\":\"{}\"}}}}",
            long
        ));
        let snapshot = store().snapshot(0, Level::TRACE, RING_CAPACITY, |_| true);
        let found = snapshot
            .entries
            .iter()
            .find(|e| e["truncated"] == json!(true))
            .expect("应有被截断的记录");
        let message = found["fields"]["message"].as_str().unwrap();
        assert!(message.len() < MAX_RECORD_BYTES * 2);
        assert!(message.contains("已截断"));
        assert_eq!(found["level"], json!("WARN"));
    }

    /// 端到端：环形缓冲层真的能收到「一行合法 JSON」，且 `fields.message` 与入参一致
    #[test]
    fn ring_layer_captures_formatted_events() {
        let subscriber = Registry::default().with(
            fmt::layer()
                .json()
                .with_writer(RingBufferWriter)
                .with_target(true)
                .with_current_span(false)
                .with_span_list(false),
        );
        tracing::subscriber::with_default(subscriber, || {
            tracing::info!(answer = 42, "hello {}", "ring");
        });

        let snapshot = store().snapshot(0, Level::TRACE, RING_CAPACITY, |_| true);
        let found = snapshot
            .entries
            .iter()
            .find(|e| e["fields"]["message"] == json!("hello ring"))
            .expect("环形缓冲里应出现刚写入的事件");
        // 与 JSONL 文件层同源，因此这些键都在
        assert!(found["timestamp"].is_string());
        assert_eq!(found["level"], json!("INFO"));
        assert_eq!(found["fields"]["answer"], json!(42));
    }

    /// 超过容量后丢最旧、`lost` 如实反映
    #[test]
    fn ring_drops_oldest_but_reports_lost() {
        let store = LogStore::new();
        for i in 0..(RING_CAPACITY + 10) {
            store.push(Level::INFO, json!({ "fields": { "message": i.to_string() } }));
        }
        let snap = store.snapshot(0, Level::INFO, RING_CAPACITY * 2, |_| true);
        assert_eq!(snap.entries.len(), RING_CAPACITY);
        assert_eq!(snap.oldest_seq, Some(11));
        assert_eq!(snap.latest_seq, (RING_CAPACITY + 10) as u64);
        assert!(!snap.truncated);

        // 游标停在 1：2..=10 已被挤出
        let snap = store.snapshot(1, Level::INFO, 10, |_| true);
        assert_eq!(snap.lost, 9);
    }
}
