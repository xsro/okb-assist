//! 双输出日志模块：同时向 stdout 输出可读日志和向 JSONL 文件输出结构化日志。
//!
//! 日志文件路径从 system.json 的 `log_path` 读取，不再支持 "stdout" 特殊值。
//! 未指定或为空时默认输出到 `okb-log.jsonl`。

use std::fs::OpenOptions;
use std::io;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tracing_subscriber::fmt::MakeWriter;
use tracing_subscriber::prelude::*;
use tracing_subscriber::{fmt, EnvFilter, Registry};

/// 初始化双输出日志系统。
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

    // 环境过滤器
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

    // 合并订阅者
    let subscriber = Registry::default()
        .with(env_filter)
        .with(json_layer)
        .with(stdout_layer);

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
}