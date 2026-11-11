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
                fs_synced_at TEXT,
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
            (
                "fs_synced_at",
                "ALTER TABLE documents ADD COLUMN fs_synced_at TEXT",
            ),
        ] {
            if !Self::has_column(&self.pool, "documents", column).await? {
                sqlx::query(ddl).execute(&self.pool).await?;
                tracing::info!("数据库迁移：documents 表补列 {}", column);
            }
        }

        // ── 状态列移除 ─────────────────────────────
        // 文献状态改为「由文件事实推导」（见 `services::file_state`），
        // 因此 `status` / `status_message` / `progress` 三列整体删除。
        // 老库先备份一份旧值（`DROP COLUMN` 不可逆），便于回滚排查。
        if Self::has_column(&self.pool, "documents", "status").await? {
            sqlx::query(
                "CREATE TABLE IF NOT EXISTS documents_status_legacy AS \
                 SELECT id, status, status_message, progress, datetime('now') AS archived_at \
                 FROM documents",
            )
            .execute(&self.pool)
            .await?;
            tracing::info!("数据库迁移：documents 旧状态已备份到 documents_status_legacy");
        }
        for column in ["status", "status_message", "progress"] {
            if Self::has_column(&self.pool, "documents", column).await? {
                sqlx::query(&format!("ALTER TABLE documents DROP COLUMN {}", column))
                    .execute(&self.pool)
                    .await?;
                tracing::info!("数据库迁移：documents 表删除列 {}", column);
            }
        }

        // ── 年份列类型修复 ─────────────────────────
        // 历史数据里有把 `year` 写成 `''`（TEXT）的（旧版本/旧库遗留）。sqlx 按 `Option<i64>`
        // 解码 TEXT 会报 `mismatched types`，导致任何 `SELECT ... year` 整条查询失败
        // （BibTeX 导入比对曾因此静默拿到空索引）。这里一次性把这些值修成 INTEGER/NULL，
        // 幂等；CAST 只是兜底，不能代替这里的清洗。
        for ddl in [
            "UPDATE documents SET year = NULL WHERE typeof(year) = 'text' AND trim(year) = ''",
            "UPDATE documents SET year = CAST(trim(year) AS INTEGER) \
             WHERE typeof(year) = 'text' AND trim(year) GLOB '[0-9]*'",
            "UPDATE documents SET year = NULL WHERE typeof(year) = 'text'",
        ] {
            let fixed = sqlx::query(ddl).execute(&self.pool).await?;
            if fixed.rows_affected() > 0 {
                tracing::info!(
                    "数据库迁移：documents.year 修正 {} 行（{}）",
                    fixed.rows_affected(),
                    ddl
                );
            }
        }

        // 常用查找列建索引：`by-hash` / `by-doi` / 上传去重都是这几列上的等值查询，
        // 文档数量上去后没索引就退化成全表扇描（幂等，可重复执行）。
        // 文件事实列（size / fs_synced_at / pdf_health）是列表筛选与排序的主力条件，同样建索引。
        for ddl in [
            "CREATE INDEX IF NOT EXISTS idx_documents_file_hash ON documents(file_hash)",
            "CREATE INDEX IF NOT EXISTS idx_documents_doi ON documents(doi)",
            "CREATE INDEX IF NOT EXISTS idx_documents_md_size ON documents(md_size)",
            "CREATE INDEX IF NOT EXISTS idx_documents_pdf_size ON documents(pdf_size)",
            "CREATE INDEX IF NOT EXISTS idx_documents_fs_synced_at ON documents(fs_synced_at)",
            "CREATE INDEX IF NOT EXISTS idx_documents_pdf_health ON documents(pdf_health)",
        ] {
            sqlx::query(ddl).execute(&self.pool).await?;
        }

        // ── 作者/关键词列格式迁移（JSON 数组字符串 → `; ` 分隔纯文本）─────────
        // 历史：抽取路径与 BibTeX 导入写成 `["A","B"]`，手工编辑 / Crossref 写纯文本，
        // 两种格式混存；`citation::from_document` 按分隔符拆时会把 JSON 碎片当人名。
        // 这里一次性统一为纯文本（幂等，解析失败的行原样保留）。
        let converted = Self::migrate_list_columns(&self.pool).await?;
        if converted > 0 {
            tracing::info!(
                "数据库迁移：{} 行的作者/关键词列由 JSON 数组改为 `; ` 分隔文本",
                converted
            );
        }

        // ── 文档类型归一化（同义词 → Zotero 标准名）────────────────────
        // 历史：Crossref 回填写 `journal`/`conference`、BibTeX 直接落 `inproceedings`、
        // 旧版实现写 `article`、手改过 `book_section`，于是 `type_counts` 被拆成多套词表，
        // 且 `citation.rs` 里标准名 `journalArticle` 匹配不到任何分支（引用类型退成 misc/Z）。
        // 这里把存量行一次性归一到 `services::doc_type::STANDARD`（幂等）。
        let converted_types = Self::migrate_doc_types(&self.pool).await?;
        if converted_types > 0 {
            tracing::info!(
                "数据库迁移：{} 行的 doc_type 归一化为 Zotero 标准类型名",
                converted_types
            );
        }

        Ok(())
    }

    /// 把 `documents.doc_type` 里的历史同义词（`article` / `journal` / `conference` /
    /// `book_section` / `inproceedings` / `misc` …）改写为 Zotero 标准名，返回改写行数。
    /// 幂等：已经是标准值的行不动；无法识别的值落为 `document`。
    async fn migrate_doc_types(pool: &SqlitePool) -> anyhow::Result<usize> {
        let rows: Vec<(i64, String)> = sqlx::query_as(
            "SELECT id, doc_type FROM documents WHERE doc_type IS NOT NULL AND TRIM(doc_type) != ''",
        )
        .fetch_all(pool)
        .await?;

        let mut converted = 0usize;
        for (id, raw) in rows {
            // 快路径：已经是标准名的行直接跳过（绝大多数行不用再跑归一化）
            if crate::services::doc_type::is_standard(&raw) {
                continue;
            }
            let normalized = crate::services::doc_type::normalize(&raw);
            if normalized.is_empty() || normalized == raw {
                continue;
            }
            sqlx::query("UPDATE documents SET doc_type = ? WHERE id = ?")
                .bind(&normalized)
                .bind(id)
                .execute(pool)
                .await?;
            converted += 1;
        }
        Ok(converted)
    }

    /// 把 `authors` / `authors_en` / `keywords` / `keywords_en` 里的历史 JSON 数组字符串
    /// （如 `["A","B"]`）改写为 `; ` 分隔的纯文本，返回改写行数。幂等：已经是纯文本的行不动。
    async fn migrate_list_columns(pool: &SqlitePool) -> anyhow::Result<usize> {
        const COLUMNS: [&str; 4] = ["authors", "authors_en", "keywords", "keywords_en"];
        let where_clause = COLUMNS
            .iter()
            .map(|c| format!("{} LIKE '[%'", c))
            .collect::<Vec<_>>()
            .join(" OR ");
        let sql = format!(
            "SELECT id, {} FROM documents WHERE {}",
            COLUMNS.join(", "),
            where_clause
        );
        let rows: Vec<(i64, Option<String>, Option<String>, Option<String>, Option<String>)> =
            sqlx::query_as(&sql).fetch_all(pool).await?;

        let mut converted = 0usize;
        for (id, authors, authors_en, keywords, keywords_en) in rows {
            let mut new_values: [Option<String>; 4] = [authors, authors_en, keywords, keywords_en];
            let mut changed = false;
            for value in new_values.iter_mut() {
                let Some(raw) = value.as_deref() else { continue };
                if !raw.trim_start().starts_with('[') {
                    continue;
                }
                // 能解析成 JSON 数组才改；否则（如 `[1,2]` 或残缺 JSON）原样保留
                let Ok(serde_json::Value::Array(items)) =
                    serde_json::from_str::<serde_json::Value>(raw.trim())
                else {
                    continue;
                };
                let list: Vec<String> = items
                    .iter()
                    .filter_map(|x| x.as_str())
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(String::from)
                    .collect();
                *value = if list.is_empty() { None } else { Some(list.join("; ")) };
                changed = true;
            }
            if !changed {
                continue;
            }
            let [a, ae, k, ke] = new_values;
            sqlx::query(
                "UPDATE documents SET authors = ?, authors_en = ?, keywords = ?, keywords_en = ? \
                 WHERE id = ?",
            )
            .bind(a)
            .bind(ae)
            .bind(k)
            .bind(ke)
            .bind(id)
            .execute(pool)
            .await?;
            converted += 1;
        }
        Ok(converted)
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