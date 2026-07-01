import json
import os
import uuid
import hashlib
from pathlib import Path
from typing import Optional

from fastapi import APIRouter, Depends, HTTPException, UploadFile, File, Header
from fastapi.responses import FileResponse, Response
from pydantic import BaseModel
from sqlalchemy.orm import Session

from app.config import get_settings
from app.database import get_db
from app.models import Document, DocStatus

router = APIRouter(prefix="/assist/api/documents", tags=["documents"])

UPLOAD_DIR = Path("uploads")
UPLOAD_DIR.mkdir(exist_ok=True)

settings = get_settings()
QDRANT_USER_ID = 0  # Default user ID for Qdrant since no auth


def calculate_file_hash(file_path: str) -> str:
    """Calculate SHA256 hash of a file."""
    sha256_hash = hashlib.sha256()
    with open(file_path, "rb") as f:
        for byte_block in iter(lambda: f.read(4096), b""):
            sha256_hash.update(byte_block)
    return sha256_hash.hexdigest()


def verify_token(x_token: str = Header(...)):
    """Verify upload token from request header."""
    if x_token != settings.upload_token:
        raise HTTPException(status_code=401, detail="无效的上传令牌")


class DocumentOut(BaseModel):
    id: int
    filename: str
    file_path: Optional[str] = None
    markdown_path: Optional[str] = None
    file_hash: Optional[str] = None
    title: Optional[str] = None
    authors: Optional[str] = None
    year: Optional[int] = None
    doi: Optional[str] = None
    source: Optional[str] = None
    journal: Optional[str] = None
    keywords: Optional[str] = None
    abstract: Optional[str] = None
    category: Optional[str] = None
    doc_type: Optional[str] = None
    language: Optional[str] = None
    title_en: Optional[str] = None
    authors_en: Optional[str] = None
    keywords_en: Optional[str] = None
    abstract_en: Optional[str] = None
    journal_en: Optional[str] = None
    status: str
    created_at: Optional[str] = None
    updated_at: Optional[str] = None

    model_config = {"from_attributes": True}


class DocumentUpdate(BaseModel):
    title: Optional[str] = None
    authors: Optional[str] = None
    year: Optional[int] = None
    doi: Optional[str] = None
    source: Optional[str] = None
    journal: Optional[str] = None
    keywords: Optional[str] = None
    abstract: Optional[str] = None
    category: Optional[str] = None
    doc_type: Optional[str] = None
    language: Optional[str] = None
    title_en: Optional[str] = None
    authors_en: Optional[str] = None
    keywords_en: Optional[str] = None
    abstract_en: Optional[str] = None
    journal_en: Optional[str] = None


class MarkdownUpdate(BaseModel):
    content: str


def _doc_to_out(doc: Document) -> dict:
    return DocumentOut(
        id=doc.id,
        filename=doc.filename,
        title=doc.title,
        authors=doc.authors,
        year=doc.year,
        doi=doc.doi,
        source=doc.source,
        journal=doc.journal,
        keywords=doc.keywords,
        abstract=doc.abstract,
        category=doc.category,
        doc_type=doc.doc_type,
        language=doc.language,
        title_en=doc.title_en,
        authors_en=doc.authors_en,
        keywords_en=doc.keywords_en,
        abstract_en=doc.abstract_en,
        journal_en=doc.journal_en,
        status=doc.status.value if isinstance(doc.status, DocStatus) else doc.status,
        created_at=doc.created_at.isoformat() if doc.created_at else None,
        updated_at=doc.updated_at.isoformat() if doc.updated_at else None,
    ).model_dump()


@router.get("/")
def list_documents(
    q: Optional[str] = None,
    status_filter: Optional[str] = None,
    db: Session = Depends(get_db),
):
    query = db.query(Document)

    if q:
        query = query.filter(
            (Document.title.ilike(f"%{q}%")) |
            (Document.authors.ilike(f"%{q}%")) |
            (Document.filename.ilike(f"%{q}%"))
        )

    if status_filter:
        query = query.filter(Document.status == status_filter)

    docs = query.order_by(Document.created_at.desc()).all()
    return [_doc_to_out(d) for d in docs]


@router.post("/upload")
async def upload_document(
    file: UploadFile = File(...),
    db: Session = Depends(get_db),
    x_token: str = Header(...),
):
    # Verify token
    if x_token != settings.upload_token:
        raise HTTPException(status_code=401, detail="无效的上传令牌")

    if not file.filename.lower().endswith(".pdf"):
        raise HTTPException(status_code=400, detail="只支持 PDF 文件")

    # Save file
    doc_uuid = str(uuid.uuid4())
    doc_dir = UPLOAD_DIR / doc_uuid
    doc_dir.mkdir(parents=True, exist_ok=True)
    file_path = doc_dir / file.filename

    content = await file.read()
    with open(file_path, "wb") as f:
        f.write(content)

    # Create DB record
    doc = Document(
        filename=file.filename,
        file_path=str(file_path),
        status=DocStatus.uploaded,
    )
    db.add(doc)
    db.commit()
    db.refresh(doc)

    return _doc_to_out(doc)


class RegisterByPath(BaseModel):
    file_path: str
    force: bool = False  # Force registration even if hash exists


@router.post("/register")
def register_document_by_path(
    data: RegisterByPath,
    db: Session = Depends(get_db),
):
    """Register a PDF file by its absolute path (no copy)."""
    file_path = data.file_path

    # Validate file exists
    if not os.path.exists(file_path):
        raise HTTPException(status_code=400, detail=f"文件不存在: {file_path}")

    if not file_path.lower().endswith(".pdf"):
        raise HTTPException(status_code=400, detail="只支持 PDF 文件")

    # Calculate file hash
    file_hash = calculate_file_hash(file_path)

    # Check if already registered by path
    existing = db.query(Document).filter(Document.file_path == file_path).first()
    if existing:
        return _doc_to_out(existing)

    # Check if hash already exists (duplicate file)
    if not data.force:
        duplicate = db.query(Document).filter(Document.file_hash == file_hash).first()
        if duplicate:
            raise HTTPException(
                status_code=409,
                detail={
                    "error": "duplicate",
                    "message": f"文件已存在 (hash: {file_hash[:16]}...)",
                    "existing_id": duplicate.id,
                    "existing_path": duplicate.file_path,
                }
            )

    # Create DB record
    filename = os.path.basename(file_path)
    doc = Document(
        filename=filename,
        file_path=file_path,
        file_hash=file_hash,
        status=DocStatus.uploaded,
    )
    db.add(doc)
    db.commit()
    db.refresh(doc)

    return _doc_to_out(doc)


@router.get("/{doc_id}")
def get_document(
    doc_id: int,
    db: Session = Depends(get_db),
):
    doc = db.query(Document).filter(Document.id == doc_id).first()
    if not doc:
        raise HTTPException(status_code=404, detail="文献不存在")
    return _doc_to_out(doc)


@router.put("/{doc_id}")
def update_document(
    doc_id: int,
    data: DocumentUpdate,
    db: Session = Depends(get_db),
):
    doc = db.query(Document).filter(Document.id == doc_id).first()
    if not doc:
        raise HTTPException(status_code=404, detail="文献不存在")

    update_data = data.model_dump(exclude_unset=True)
    for key, value in update_data.items():
        setattr(doc, key, value)

    db.commit()
    db.refresh(doc)
    return _doc_to_out(doc)


@router.delete("/{doc_id}")
def delete_document(
    doc_id: int,
    db: Session = Depends(get_db),
):
    doc = db.query(Document).filter(Document.id == doc_id).first()
    if not doc:
        raise HTTPException(status_code=404, detail="文献不存在")

    # Delete generated files in uploads/{doc_id}/
    import shutil
    doc_upload_dir = os.path.join("uploads", str(doc_id))
    if os.path.exists(doc_upload_dir):
        shutil.rmtree(doc_upload_dir, ignore_errors=True)

    # Delete from Qdrant if indexed
    if doc.qdrant_collection:
        try:
            from app.services.qdrant import delete_document_points
            delete_document_points(QDRANT_USER_ID, doc.id)
        except Exception:
            pass

    db.delete(doc)
    db.commit()
    return {"detail": "已删除"}


@router.get("/{doc_id}/pdf")
def get_pdf(
    doc_id: int,
    db: Session = Depends(get_db),
):
    doc = db.query(Document).filter(Document.id == doc_id).first()
    if not doc or not doc.file_path or not os.path.exists(doc.file_path):
        raise HTTPException(status_code=404, detail="PDF 文件不存在")

    with open(doc.file_path, "rb") as f:
        pdf_content = f.read()

    return Response(
        content=pdf_content,
        media_type="application/pdf",
        headers={"Content-Disposition": "inline"},
    )


@router.get("/{doc_id}/markdown")
def get_markdown(
    doc_id: int,
    page: int = 1,
    page_size: int = 5000,
    db: Session = Depends(get_db),
):
    """Get markdown content with pagination. page_size is in characters."""
    doc = db.query(Document).filter(Document.id == doc_id).first()
    if not doc:
        raise HTTPException(status_code=404, detail="文献不存在")
    if not doc.markdown_path or not os.path.exists(doc.markdown_path):
        raise HTTPException(status_code=404, detail="Markdown 文件尚未生成")

    with open(doc.markdown_path, "r", encoding="utf-8") as f:
        content = f.read()

    # Rewrite image paths to absolute URLs
    import re
    # Get the directory containing the markdown file (doc_dir)
    doc_dir = os.path.dirname(doc.markdown_path)
    # Get the relative path from uploads/
    rel_path = os.path.relpath(doc_dir, "uploads")

    # Replace relative image paths: images/xxx.png -> /assist/uploads/{rel_path}/images/xxx.png
    def rewrite_image(match):
        img_path = match.group(1)
        if not img_path.startswith(('http://', 'https://', '/assist/')):
            # Relative path, rewrite to absolute
            return f'](/assist/uploads/{rel_path}/{img_path})'
        return match.group(0)

    content = re.sub(r'\]\(([^)]+)\)', rewrite_image, content)

    # Split into pages by double newline to preserve paragraph structure
    pages = _split_into_pages(content, page_size)
    total_pages = len(pages)

    if page < 1 or page > total_pages:
        page = 1

    return {
        "content": pages[page - 1] if pages else "",
        "page": page,
        "total_pages": total_pages,
        "total_length": len(content),
    }


def _split_into_pages(content: str, max_chars: int = 5000) -> list[str]:
    """Split markdown content into pages, trying to break at paragraph boundaries."""
    if len(content) <= max_chars:
        return [content]

    pages = []
    current_pos = 0

    while current_pos < len(content):
        if current_pos + max_chars >= len(content):
            pages.append(content[current_pos:])
            break

        # Find a good break point (double newline)
        search_start = current_pos + max_chars - 500  # Look back a bit
        search_end = min(current_pos + max_chars, len(content))
        chunk = content[search_start:search_end]

        # Try to find paragraph break
        break_pos = chunk.rfind("\n\n")
        if break_pos != -1:
            pages.append(content[current_pos:search_start + break_pos])
            current_pos = search_start + break_pos + 2
        else:
            # Try single newline
            break_pos = chunk.rfind("\n")
            if break_pos != -1:
                pages.append(content[current_pos:search_start + break_pos])
                current_pos = search_start + break_pos + 1
            else:
                # Hard break
                pages.append(content[current_pos:current_pos + max_chars])
                current_pos += max_chars

    return pages


@router.put("/{doc_id}/markdown")
def update_markdown(
    doc_id: int,
    data: MarkdownUpdate,
    db: Session = Depends(get_db),
):
    doc = db.query(Document).filter(Document.id == doc_id).first()
    if not doc:
        raise HTTPException(status_code=404, detail="文献不存在")
    if not doc.markdown_path:
        raise HTTPException(status_code=400, detail="Markdown 文件尚未生成")

    with open(doc.markdown_path, "w", encoding="utf-8") as f:
        f.write(data.content)
    return {"detail": "Markdown 已更新"}
