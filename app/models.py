import enum
from datetime import datetime, timezone
from sqlalchemy import Column, Integer, String, Boolean, DateTime, ForeignKey, Text, Enum
from sqlalchemy.orm import relationship
from app.database import Base


class DocStatus(str, enum.Enum):
    uploaded = "uploaded"
    markdown_done = "markdown_done"
    meta_done = "meta_done"
    indexed = "indexed"


class User(Base):
    __tablename__ = "users"

    id = Column(Integer, primary_key=True, index=True)
    username = Column(String(50), unique=True, nullable=False, index=True)
    hashed_password = Column(String(128), nullable=False)
    is_admin = Column(Boolean, default=False)
    created_at = Column(DateTime, default=lambda: datetime.now(timezone.utc))

    documents = relationship("Document", back_populates="user", cascade="all, delete-orphan")


class Document(Base):
    __tablename__ = "documents"

    id = Column(Integer, primary_key=True, index=True)
    user_id = Column(Integer, ForeignKey("users.id"), nullable=False)
    filename = Column(String(255), nullable=False)
    file_path = Column(String(500), nullable=False)

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

    # Status
    status = Column(Enum(DocStatus), default=DocStatus.uploaded)
    qdrant_collection = Column(String(100), nullable=True)

    created_at = Column(DateTime, default=lambda: datetime.now(timezone.utc))
    updated_at = Column(DateTime, default=lambda: datetime.now(timezone.utc), onupdate=lambda: datetime.now(timezone.utc))

    user = relationship("User", back_populates="documents")
