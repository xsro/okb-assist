import enum
from datetime import datetime, timezone
from sqlalchemy import Column, Integer, String, DateTime, Text, Enum, Float
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


class Document(Base):
    __tablename__ = "documents"

    id = Column(Integer, primary_key=True, index=True)
    filename = Column(String(255), nullable=False)
    file_path = Column(String(500), nullable=False)
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

    # Paths
    markdown_path = Column(String(500), nullable=True)

    # MinerU task tracking
    mineru_task_id = Column(String(100), nullable=True)  # MinerU async task ID for resume after timeout

    # Status
    status = Column(Enum(DocStatus), default=DocStatus.uploaded)
    status_message = Column(String(500), nullable=True)  # Error or progress message
    progress = Column(Float, default=0.0)  # 0-100
    qdrant_collection = Column(String(100), nullable=True)

    created_at = Column(DateTime, default=lambda: datetime.now(timezone.utc))
    updated_at = Column(DateTime, default=lambda: datetime.now(timezone.utc), onupdate=lambda: datetime.now(timezone.utc))
