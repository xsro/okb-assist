import enum
from datetime import datetime, timezone
from sqlalchemy import Column, Integer, String, DateTime, Text, Enum, Float, ForeignKey, UniqueConstraint
from sqlalchemy.orm import relationship
from app.database import Base


class DocStatus(str, enum.Enum):
    uploaded = "uploaded"
    parsing = "parsing"
    markdown_done = "markdown_done"
    extracting = "extracting"
    meta_done = "meta_done"
    indexing = "indexing"
    indexed = "indexed"
    error = "error"


class IndexStatus(str, enum.Enum):
    pending = "pending"
    indexing = "indexing"
    indexed = "indexed"
    error = "error"


class Document(Base):
    __tablename__ = "documents"

    id = Column(Integer, primary_key=True, index=True)
    filename = Column(String(255), nullable=False)
    file_hash = Column(String(64), nullable=True, index=True)  # SHA256 hash

    # Meta fields
    title = Column(String(500), nullable=True)
    authors = Column(Text, nullable=True)  # JSON list
    year = Column(Integer, nullable=True)
    doi = Column(String(200), nullable=True)
    source = Column(String(200), nullable=True)
    journal = Column(String(200), nullable=True)
    keywords = Column(Text, nullable=True)  # JSON list
    abstract = Column(Text, nullable=True)
    category = Column(String(100), nullable=True)
    doc_type = Column(String(50), nullable=True)  # book, article, etc.
    language = Column(String(10), nullable=True)  # en, zh, ja, fr, ru, de, ko, etc.

    # English meta (for non-English documents)
    title_en = Column(String(500), nullable=True)
    authors_en = Column(Text, nullable=True)  # JSON list
    keywords_en = Column(Text, nullable=True)  # JSON list
    abstract_en = Column(Text, nullable=True)
    journal_en = Column(String(200), nullable=True)

    # Paths are derived from system.json templates (see app/paths.py), not stored here.

    # MinerU task tracking
    mineru_task_id = Column(String(100), nullable=True)  # MinerU async task ID for resume after timeout

    # Status (处理状态，不包含索引状态)
    status = Column(Enum(DocStatus), default=DocStatus.uploaded)
    status_message = Column(String(500), nullable=True)  # Error or progress message
    progress = Column(Float, default=0.0)  # 0-100

    # 兼容旧字段（已废弃，使用 vector_indexes 关系）
    qdrant_collection = Column(String(100), nullable=True)
    vector_db_id = Column(String(50), nullable=True)

    created_at = Column(DateTime, default=lambda: datetime.now(timezone.utc))
    updated_at = Column(DateTime, default=lambda: datetime.now(timezone.utc), onupdate=lambda: datetime.now(timezone.utc))

    # 关系：文档在各向量数据库中的索引状态
    vector_indexes = relationship("DocumentVectorIndex", back_populates="document", cascade="all, delete-orphan")


class DocumentVectorIndex(Base):
    """文档在向量数据库中的索引状态记录"""
    __tablename__ = "document_vector_index"

    id = Column(Integer, primary_key=True, index=True)
    document_id = Column(Integer, ForeignKey("documents.id", ondelete="CASCADE"), nullable=False)
    vector_db_id = Column(String(50), nullable=False)  # 对应 config.json 中 vector_dbs 的 id
    collection_name = Column(String(100), nullable=True)  # 向量数据库中的集合名称
    status = Column(Enum(IndexStatus), default=IndexStatus.pending)
    error_message = Column(Text, nullable=True)
    created_at = Column(DateTime, default=lambda: datetime.now(timezone.utc))
    updated_at = Column(DateTime, default=lambda: datetime.now(timezone.utc), onupdate=lambda: datetime.now(timezone.utc))

    # 关系
    document = relationship("Document", back_populates="vector_indexes")

    # 唯一约束：一个文档在同一向量数据库中只有一条记录
    __table_args__ = (
        UniqueConstraint('document_id', 'vector_db_id', name='uq_doc_vector_db'),
    )
