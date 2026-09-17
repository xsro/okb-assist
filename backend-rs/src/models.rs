//! 数据模型。

use serde::{Deserialize, Serialize};
use sqlx::FromRow;

/// 文档状态枚举
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DocStatus {
    Uploaded,
    Parsing,
    MarkdownDone,
    Extracting,
    MetaDone,
    Indexing,
    Indexed,
    Error,
}

impl DocStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            DocStatus::Uploaded => "uploaded",
            DocStatus::Parsing => "parsing",
            DocStatus::MarkdownDone => "markdown_done",
            DocStatus::Extracting => "extracting",
            DocStatus::MetaDone => "meta_done",
            DocStatus::Indexing => "indexing",
            DocStatus::Indexed => "indexed",
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
            "extracting" => Ok(DocStatus::Extracting),
            "meta_done" => Ok(DocStatus::MetaDone),
            "indexing" => Ok(DocStatus::Indexing),
            "indexed" => Ok(DocStatus::Indexed),
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
    pub status: String,
    pub status_message: Option<String>,
    pub progress: f64,
    pub qdrant_collection: Option<String>,
    pub vector_db_id: Option<String>,
    pub created_at: Option<String>,
    pub updated_at: Option<String>,
}

impl Document {
    pub fn doc_status(&self) -> DocStatus {
        self.status.parse().unwrap_or(DocStatus::Uploaded)
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