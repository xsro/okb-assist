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
                i18n TEXT,
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
            (
                "i18n",
                "ALTER TABLE documents ADD COLUMN i18n TEXT",
            ),
        ] {
            if !Self::has_column(&self.pool, "documents", column).await? {
                sqlx::query(ddl).execute(&self.pool).await?;
                tracing::info!("数据库迁移：documents 表补列 {}", column);
            }
        }

        // ── 双语字段迁移（`*_en` 五列 → 单列 `i18n` JSON）──────────────
        // 基础列存 PDF 主要语言，`i18n` 只存翻译。旧库里的 `*_en` 列在这里搬进
        // `i18n.en`（英文文献的镜像值跳过），随后删除五列。幂等：列已不存在就跳过。
        // 注意必须在 `migrate_list_columns` **之前**执行（后者已不再引用 `*_en` 列）。
        let migrated_i18n = Self::migrate_i18n(&self.pool).await?;
        if migrated_i18n > 0 {
            tracing::info!(
                "数据库迁移：{} 行的英文元数据迁入 i18n 字段",
                migrated_i18n
            );
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

    /// 把 `authors` / `keywords` 里的历史 JSON 数组字符串（如 `["A","B"]`）改写为 `; ` 分隔的
    /// 纯文本，返回改写行数。幂等：已经是纯文本的行不动。
    ///
    /// `authors_en` / `keywords_en` 已随 i18n 迁移并入 `i18n` 并删除，不再出现在这里。
    async fn migrate_list_columns(pool: &SqlitePool) -> anyhow::Result<usize> {
        const COLUMNS: [&str; 2] = ["authors", "keywords"];
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
        let rows: Vec<(i64, Option<String>, Option<String>)> =
            sqlx::query_as(&sql).fetch_all(pool).await?;

        let mut converted = 0usize;
        for (id, authors, keywords) in rows {
            let mut new_values: [Option<String>; 2] = [authors, keywords];
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
            let [a, k] = new_values;
            sqlx::query("UPDATE documents SET authors = ?, keywords = ? WHERE id = ?")
                .bind(a)
                .bind(k)
                .bind(id)
                .execute(pool)
                .await?;
            converted += 1;
        }
        Ok(converted)
    }

    /// 把五个 `*_en` 列搬进 `i18n` JSON 并删除旧列，返回迁移行数。
    ///
    /// 规则（与 `services::i18n` 一致）：
    /// - 基础列为英文（或无法判定语言）时，`*_en` 是镜像，直接丢弃；
    /// - 基础列非英文时，非空且**不与基础列相同**的值写入 `i18n.en.*`；
    /// - 已有合法 `i18n` 的行只补空，不覆盖。
    ///
    /// 幂等：五列都不存在时立即返回。删除列前把原值备份到 `documents_i18n_legacy`。
    async fn migrate_i18n(pool: &SqlitePool) -> anyhow::Result<usize> {
        const LEGACY_COLUMNS: [&str; 5] = [
            "title_en",
            "authors_en",
            "keywords_en",
            "abstract_en",
            "journal_en",
        ];
        let mut present = Vec::new();
        for column in LEGACY_COLUMNS {
            if Self::has_column(pool, "documents", column).await? {
                present.push(column);
            }
        }
        if present.is_empty() {
            return Ok(0);
        }

        // 删列不可逆：先留一份只读备份（与 `documents_status_legacy` 同样的做法）
        if !Self::has_column(pool, "documents_i18n_legacy", "title_en").await? {
            sqlx::query(
                "CREATE TABLE IF NOT EXISTS documents_i18n_legacy AS \
                 SELECT id, title_en, authors_en, keywords_en, abstract_en, journal_en, \
                        datetime('now') AS archived_at FROM documents",
            )
            .execute(pool)
            .await?;
            tracing::info!("数据库迁移：documents 旧双语字段已备份到 documents_i18n_legacy");
        }

        let rows: Vec<(
            i64,
            Option<String>,
            Option<String>,
            Option<String>,
            Option<String>,
            Option<String>,
            Option<String>,
            Option<String>,
            Option<String>,
            Option<String>,
            Option<String>,
            Option<String>,
            Option<String>,
        )> = sqlx::query_as(
            "SELECT id, language, title, authors, journal, keywords, abstract, title_en, \
             authors_en, keywords_en, abstract_en, journal_en, i18n FROM documents",
        )
        .fetch_all(pool)
        .await?;

        let mut migrated = 0usize;
        for (
            id,
            language,
            title,
            authors,
            journal,
            keywords,
            abstract_text,
            title_en,
            authors_en,
            keywords_en,
            abstract_en,
            journal_en,
            i18n,
        ) in rows
        {
            let mut map = crate::services::i18n::parse(i18n.as_deref());
            let before = map.clone();

            // 只有「基础列非英文且语言可判定」时才把 `*_en` 当翻译保存
            let base = crate::services::i18n::base_language(language.as_deref(), title.as_deref());
            if base.as_deref().map_or(false, |b| b != "en") {
                let fields = crate::services::i18n::I18nFields {
                    title: non_duplicate(title_en.as_deref(), title.as_deref()),
                    authors: non_duplicate(authors_en.as_deref(), authors.as_deref()),
                    journal: non_duplicate(journal_en.as_deref(), journal.as_deref()),
                    keywords: non_duplicate(keywords_en.as_deref(), keywords.as_deref()),
                    abstract_text: non_duplicate(abstract_en.as_deref(), abstract_text.as_deref()),
                };
                let incoming = crate::services::i18n::single_lang_map("en", fields);
                crate::services::i18n::merge_missing(&mut map, &incoming);
            }
            if map == before {
                continue;
            }
            sqlx::query("UPDATE documents SET i18n = ? WHERE id = ?")
                .bind(crate::services::i18n::to_db(&map))
                .bind(id)
                .execute(pool)
                .await?;
            migrated += 1;
        }

        // 备份完成后删除旧列（SQLite ≥ 3.35，代码库已在用 DROP COLUMN）
        for column in present {
            sqlx::query(&format!("ALTER TABLE documents DROP COLUMN {}", column))
                .execute(pool)
                .await?;
            tracing::info!("数据库迁移：documents 表删除列 {}", column);
        }

        Ok(migrated)
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
/// 迁移用：`*_en` 的值若与对应基础列相同（英文文献的镜像）则视为重复，不写入 `i18n`。
fn non_duplicate(value: Option<&str>, base: Option<&str>) -> Option<String> {
    let value = value.unwrap_or("").trim();
    if value.is_empty() || base.map(str::trim) == Some(value) {
        None
    } else {
        Some(value.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 旧库（`*_en` 五列）→ 新库（单列 `i18n` JSON）的迁移回归测试。
    #[tokio::test]
    async fn migrates_en_columns_into_i18n() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("migrate.db");
        let opts = SqliteConnectOptions::new().filename(&path).create_if_missing(true);
        let pool = SqlitePoolOptions::new().max_connections(1).connect_with(opts).await.unwrap();
        let db = Database { pool };

        // 旧表结构（含 *_en 五列）
        sqlx::query(
            "CREATE TABLE documents (
                id INTEGER PRIMARY KEY, filename TEXT NOT NULL, file_hash TEXT,
                title TEXT, authors TEXT, year INTEGER, doi TEXT, source TEXT, journal TEXT,
                keywords TEXT, abstract TEXT, category TEXT, doc_type TEXT, language TEXT,
                title_en TEXT, authors_en TEXT, keywords_en TEXT, abstract_en TEXT, journal_en TEXT,
                mineru_task_id TEXT, fs_synced_at TEXT, qdrant_collection TEXT, vector_db_id TEXT,
                pdf_size INTEGER, md_size INTEGER, zip_size INTEGER, pdf_health TEXT,
                pdf_health_message TEXT,
                created_at TEXT DEFAULT (datetime('now')), updated_at TEXT DEFAULT (datetime('now'))
            )",
        )
        .execute(db.pool())
        .await
        .unwrap();
        // 中文文献（英文译名应进 i18n.en）
        sqlx::query(
            "INSERT INTO documents (id, filename, title, language, title_en, authors, authors_en,
                keywords, keywords_en, abstract, abstract_en, journal, journal_en)
             VALUES (1, 'zh.pdf', '中文题名', 'zh', 'Chinese Title', '张三', 'Zhang San',
                '关键词', 'keywords', '中文摘要', 'English abstract', '中文期刊', 'Chinese Journal')",
        )
        .execute(db.pool())
        .await
        .unwrap();
        // 英文文献（*_en 是与基础列相同的镜像 → 丢弃）
        sqlx::query(
            "INSERT INTO documents (id, filename, title, language, title_en, abstract, abstract_en)
             VALUES (2, 'en.pdf', 'Attention', 'en', 'Attention', 'Same abstract', 'Same abstract')",
        )
        .execute(db.pool())
        .await
        .unwrap();
        // 语言未知但标题是 ASCII（按 en 处理 → 丢弃 *_en）
        sqlx::query(
            "INSERT INTO documents (id, filename, title, title_en) VALUES (3, 'x.pdf', 'Foo', 'Foo EN')",
        )
        .execute(db.pool())
        .await
        .unwrap();

        db.init().await.unwrap();

        // 旧列已删除、新列存在
        let cols: Vec<(String,)> = sqlx::query_as("SELECT name FROM pragma_table_info('documents')")
            .fetch_all(db.pool())
            .await
            .unwrap();
        let names: Vec<String> = cols.into_iter().map(|(n,)| n).collect();
        assert!(names.iter().any(|n| n == "i18n"));
        for old in ["title_en", "authors_en", "keywords_en", "abstract_en", "journal_en"] {
            assert!(!names.iter().any(|n| n == old), "旧列未删除：{}", old);
        }

        // 中文文献：五个字段都进 i18n.en，基础列保持中文
        let raw: Option<String> = sqlx::query_scalar("SELECT i18n FROM documents WHERE id = 1")
            .fetch_one(db.pool())
            .await
            .unwrap();
        let map = crate::services::i18n::parse(raw.as_deref());
        let en = map.get("en").expect("缺少 i18n.en");
        assert_eq!(en.title.as_deref(), Some("Chinese Title"));
        assert_eq!(en.authors.as_deref(), Some("Zhang San"));
        assert_eq!(en.journal.as_deref(), Some("Chinese Journal"));
        assert_eq!(en.keywords.as_deref(), Some("keywords"));
        assert_eq!(en.abstract_text.as_deref(), Some("English abstract"));
        // 搜索路径依赖 SQLite JSON1：`json_extract` 必须可用
        let extracted: Option<String> = sqlx::query_scalar(
            "SELECT json_extract(i18n, '$.en.title') FROM documents WHERE id = 1",
        )
        .fetch_one(db.pool())
        .await
        .unwrap();
        assert_eq!(extracted.as_deref(), Some("Chinese Title"));

        let base_title: Option<String> = sqlx::query_scalar("SELECT title FROM documents WHERE id = 1")
            .fetch_one(db.pool())
            .await
            .unwrap();
        assert_eq!(base_title.as_deref(), Some("中文题名"));

        // 英文 / 语言未知：不存 en 镜像
        for id in [2i64, 3i64] {
            let raw: Option<String> = sqlx::query_scalar("SELECT i18n FROM documents WHERE id = ?")
                .bind(id)
                .fetch_one(db.pool())
                .await
                .unwrap();
            assert!(raw.is_none(), "id={} 不应有 i18n", id);
        }

        // 备份表保留原值
        let legacy: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM documents_i18n_legacy")
            .fetch_one(db.pool())
            .await
            .unwrap();
        assert_eq!(legacy, 3);
    }
}
