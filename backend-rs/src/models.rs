//! 数据模型。

use serde::{Deserialize, Serialize};
use sqlx::FromRow;

/// 文档状态枚举（仅主流程串行状态）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DocStatus {
    Uploaded,
    Parsing,
    MarkdownDone,
    Error,
}

impl DocStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            DocStatus::Uploaded => "uploaded",
            DocStatus::Parsing => "parsing",
            DocStatus::MarkdownDone => "markdown_done",
            DocStatus::Error => "error",
        }
    }
}

impl std::fmt::Display for DocStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

impl std::str::FromStr for DocStatus {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "uploaded" => Ok(DocStatus::Uploaded),
            "parsing" => Ok(DocStatus::Parsing),
            "markdown_done" => Ok(DocStatus::MarkdownDone),
            "error" => Ok(DocStatus::Error),
            _ => Err(format!("Unknown status: {}", s)),
        }
    }
}

/// 向量索引状态枚举
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum IndexStatus {
    Pending,
    Indexing,
    Indexed,
    Error,
}

impl IndexStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            IndexStatus::Pending => "pending",
            IndexStatus::Indexing => "indexing",
            IndexStatus::Indexed => "indexed",
            IndexStatus::Error => "error",
        }
    }
}

impl std::fmt::Display for IndexStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

impl std::str::FromStr for IndexStatus {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "pending" => Ok(IndexStatus::Pending),
            "indexing" => Ok(IndexStatus::Indexing),
            "indexed" => Ok(IndexStatus::Indexed),
            "error" => Ok(IndexStatus::Error),
            _ => Err(format!("Unknown index status: {}", s)),
        }
    }
}

/// Document model
#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct Document {
    pub id: i64,
    pub filename: String,
    pub file_hash: Option<String>,
    pub title: Option<String>,
    pub authors: Option<String>,
    pub year: Option<i64>,
    pub doi: Option<String>,
    pub source: Option<String>,
    pub journal: Option<String>,
    pub keywords: Option<String>,
    #[sqlx(rename = "abstract")]
    pub abstract_text: Option<String>,
    pub category: Option<String>,
    pub doc_type: Option<String>,
    pub language: Option<String>,
    pub title_en: Option<String>,
    pub authors_en: Option<String>,
    pub keywords_en: Option<String>,
    pub abstract_en: Option<String>,
    pub journal_en: Option<String>,
    pub mineru_task_id: Option<String>,
    /// 文件事实（size）上次从文件系统同步的时间；`None` = 从未同步过（状态未知）
    pub fs_synced_at: Option<String>,
    pub qdrant_collection: Option<String>,
    pub vector_db_id: Option<String>,
    pub created_at: Option<String>,
    pub updated_at: Option<String>,
    pub pdf_size: Option<i64>,
    pub md_size: Option<i64>,
    pub zip_size: Option<i64>,
    /// PDF 健康检查结果：`ok` / `warning` / `error` / `missing`（`NULL` = 未检查）
    pub pdf_health: Option<String>,
    /// 健康检查原因
    pub pdf_health_message: Option<String>,
}

impl Document {
    pub fn has_pdf(&self) -> bool {
        self.pdf_size.unwrap_or(0) > 0
    }

    pub fn has_md(&self) -> bool {
        self.md_size.unwrap_or(0) > 0
    }

    pub fn has_zip(&self) -> bool {
        self.zip_size.unwrap_or(0) > 0
    }
}

/// DocumentVectorIndex model
#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct DocumentVectorIndex {
    pub id: Option<i64>,
    pub document_id: i64,
    pub vector_db_id: String,
    pub collection_name: Option<String>,
    pub status: String,
    pub error_message: Option<String>,
    pub created_at: Option<String>,
    pub updated_at: Option<String>,
}

impl DocumentVectorIndex {
    pub fn index_status(&self) -> IndexStatus {
        self.status.parse().unwrap_or(IndexStatus::Pending)
    }
}