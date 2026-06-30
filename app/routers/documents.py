import json
import os
import uuid
from pathlib import Path
from typing import Optional

from fastapi import APIRouter, Depends, HTTPException, UploadFile, File, status
from fastapi.responses import FileResponse
from pydantic import BaseModel
from sqlalchemy.orm import Session

from app.auth import get_current_user
from app.database import get_db
from app.models import Document, DocStatus, User

router = APIRouter(prefix="/assist/api/documents", tags=["documents"])

UPLOAD_DIR = Path("uploads")
UPLOAD_DIR.mkdir(exist_ok=True)


class DocumentOut(BaseModel):
    id: int
    filename: str
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
    current_user: User = Depends(get_current_user),
):
    query = db.query(Document).filter(Document.user_id == current_user.id)

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
    current_user: User = Depends(get_current_user),
):
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
        user_id=current_user.id,
        filename=file.filename,
        file_path=str(file_path),
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
    current_user: User = Depends(get_current_user),
):
    doc = db.query(Document).filter(
        Document.id == doc_id,
        Document.user_id == current_user.id,
    ).first()
    if not doc:
        raise HTTPException(status_code=404, detail="文献不存在")
    return _doc_to_out(doc)


@router.put("/{doc_id}")
def update_document(
    doc_id: int,
    data: DocumentUpdate,
    db: Session = Depends(get_db),
    current_user: User = Depends(get_current_user),
):
    doc = db.query(Document).filter(
        Document.id == doc_id,
        Document.user_id == current_user.id,
    ).first()
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
    current_user: User = Depends(get_current_user),
):
    doc = db.query(Document).filter(
        Document.id == doc_id,
        Document.user_id == current_user.id,
    ).first()
    if not doc:
        raise HTTPException(status_code=404, detail="文献不存在")

    # Delete files
    if doc.file_path and os.path.exists(doc.file_path):
        doc_dir = os.path.dirname(doc.file_path)
        import shutil
        shutil.rmtree(doc_dir, ignore_errors=True)

    db.delete(doc)
    db.commit()
    return {"detail": "已删除"}


@router.get("/{doc_id}/pdf")
def get_pdf(
    doc_id: int,
    db: Session = Depends(get_db),
    current_user: User = Depends(get_current_user),
):
    doc = db.query(Document).filter(
        Document.id == doc_id,
        Document.user_id == current_user.id,
    ).first()
    if not doc or not doc.file_path or not os.path.exists(doc.file_path):
        raise HTTPException(status_code=404, detail="PDF 文件不存在")

    return FileResponse(doc.file_path, media_type="application/pdf", filename=doc.filename)


@router.get("/{doc_id}/markdown")
def get_markdown(
    doc_id: int,
    db: Session = Depends(get_db),
    current_user: User = Depends(get_current_user),
):
    doc = db.query(Document).filter(
        Document.id == doc_id,
        Document.user_id == current_user.id,
    ).first()
    if not doc:
        raise HTTPException(status_code=404, detail="文献不存在")
    if not doc.markdown_path or not os.path.exists(doc.markdown_path):
        raise HTTPException(status_code=404, detail="Markdown 文件尚未生成")

    with open(doc.markdown_path, "r", encoding="utf-8") as f:
        content = f.read()
    return {"content": content}


@router.put("/{doc_id}/markdown")
def update_markdown(
    doc_id: int,
    data: MarkdownUpdate,
    db: Session = Depends(get_db),
    current_user: User = Depends(get_current_user),
):
    doc = db.query(Document).filter(
        Document.id == doc_id,
        Document.user_id == current_user.id,
    ).first()
    if not doc:
        raise HTTPException(status_code=404, detail="文献不存在")
    if not doc.markdown_path:
        raise HTTPException(status_code=400, detail="Markdown 文件尚未生成")

    with open(doc.markdown_path, "w", encoding="utf-8") as f:
        f.write(data.content)
    return {"detail": "Markdown 已更新"}
