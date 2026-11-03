//! SQLite 数据库连接管理。

use sqlx::sqlite::{
    SqliteConnectOptions, SqliteJournalMode, SqlitePool, SqlitePoolOptions, SqliteSynchronous,
};
use std::path::Path;
use std::str::FromStr;

pub struct Database {
    pub pool: SqlitePool,
}

impl Database {
    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    /// 将 `sqlite://` 形式的连接串转换为 SQLx 可用的连接串。
    /// 输入：`sqlite:///rel.db`（相对）、`sqlite:////abs.db`（绝对）
    /// 输出：`sqlite:rel.db`（相对）、`sqlite:/abs.db`（绝对）
    fn normalize_sqlite_url(database_url: &str) -> String {
        if let Some(rest) = database_url.strip_prefix("sqlite://") {
            if rest.is_empty() {
                "sqlite::memory:".to_string()
            } else if let Some(abs) = rest.strip_prefix('/') {
                // sqlite:///rel.db → sqlite:rel.db；sqlite:////abs.db → sqlite:/abs.db
                format!("sqlite:{}", abs)
            } else {
                // sqlite://foo.db → sqlite:foo.db
                format!("sqlite:{}", rest)
            }
        } else if database_url.starts_with("sqlite:") {
            database_url.to_string()
        } else {
            format!("sqlite:{}", database_url)
        }
    }

    /// 从连接串中提取数据库文件路径。
    fn extract_file_path(database_url: &str) -> String {
        let url = Self::normalize_sqlite_url(database_url);
        let path = url
            .trim_start_matches("sqlite:")
            .splitn(2, '?')
            .next()
            .unwrap_or("")
            .to_string();
        path
    }

    pub async fn new(database_url: &str) -> anyhow::Result<Self> {
        let url = Self::normalize_sqlite_url(database_url);

        // 创建父目录（跳过内存数据库）
        let file_path = Self::extract_file_path(database_url);
        if !file_path.is_empty() && file_path != ":memory:" {
            if let Some(parent) = Path::new(&file_path).parent() {
                if !parent.as_os_str().is_empty() {
                    std::fs::create_dir_all(parent)?;
                }
            }
        }

        // WAL 模式：读不阻塞写、写不阻塞读，适合流水线后台写 + 前台读并发的场景；
        // synchronous=NORMAL 是 WAL 下的推荐组合（兼顾性能与安全性，不损坏数据库）。
        let opts = SqliteConnectOptions::from_str(&url)?
            .create_if_missing(true)
            .journal_mode(SqliteJournalMode::Wal)
            .synchronous(SqliteSynchronous::Normal);

        let pool = SqlitePoolOptions::new()
            .max_connections(5)
            .connect_with(opts)
            .await?;

        Ok(Self { pool })
    }

    pub async fn init(&self) -> anyhow::Result<()> {
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS documents (
                id INTEGER PRIMARY KEY,
                filename TEXT NOT NULL,
                file_hash TEXT,
                title TEXT,
                authors TEXT,
                year INTEGER,
                doi TEXT,
                source TEXT,
                journal TEXT,
                keywords TEXT,
                abstract TEXT,
                category TEXT,
                doc_type TEXT,
                language TEXT,
                title_en TEXT,
                authors_en TEXT,
                keywords_en TEXT,
                abstract_en TEXT,
                journal_en TEXT,
                mineru_task_id TEXT,
                status TEXT DEFAULT 'uploaded',
                status_message TEXT,
                progress REAL DEFAULT 0.0,
                qdrant_collection TEXT,
                vector_db_id TEXT,
                pdf_size INTEGER,
                md_size INTEGER,
                zip_size INTEGER,
                pdf_health TEXT,
                pdf_health_message TEXT,
                created_at TEXT DEFAULT (datetime('now')),
                updated_at TEXT DEFAULT (datetime('now'))
            )
            "#,
        )
        .execute(&self.pool)
        .await?;

        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS document_vector_index (
                id INTEGER PRIMARY KEY,
                document_id INTEGER NOT NULL,
                vector_db_id TEXT NOT NULL,
                collection_name TEXT,
                status TEXT DEFAULT 'pending',
                error_message TEXT,
                created_at TEXT DEFAULT (datetime('now')),
                updated_at TEXT DEFAULT (datetime('now')),
                UNIQUE(document_id, vector_db_id),
                FOREIGN KEY (document_id) REFERENCES documents(id) ON DELETE CASCADE
            )
            "#,
        )
        .execute(&self.pool)
        .await?;

        // ── 轻量迁移 ─────────────────────────────────
        // `CREATE TABLE IF NOT EXISTS` 不会给已有库补列，后加的列在这里逐个补齐。
        // 新增列时：同时改上面的建表语句，并在此追加一行。
        for (column, ddl) in [
            (
                "pdf_health",
                "ALTER TABLE documents ADD COLUMN pdf_health TEXT",
            ),
            (
                "pdf_health_message",
                "ALTER TABLE documents ADD COLUMN pdf_health_message TEXT",
            ),
        ] {
            if !Self::has_column(&self.pool, "documents", column).await? {
                sqlx::query(ddl).execute(&self.pool).await?;
                tracing::info!("数据库迁移：documents 表补列 {}", column);
            }
        }

        // 常用查找列建索引：`by-hash` / `by-doi` / 上传去重都是这几列上的等值查询，
        // 文档数量上去后没索引就退化成全表扇描（幂等，可重复执行）。
        for ddl in [
            "CREATE INDEX IF NOT EXISTS idx_documents_file_hash ON documents(file_hash)",
            "CREATE INDEX IF NOT EXISTS idx_documents_doi ON documents(doi)",
        ] {
            sqlx::query(ddl).execute(&self.pool).await?;
        }

        Ok(())
    }

    /// 判断某张表是否已有某列（用于轻量迁移）
    async fn has_column(pool: &SqlitePool, table: &str, column: &str) -> anyhow::Result<bool> {
        let rows: Vec<(String,)> =
            sqlx::query_as(&format!("SELECT name FROM pragma_table_info('{}')", table))
                .fetch_all(pool)
                .await?;
        Ok(rows.iter().any(|(name,)| name == column))
    }
}