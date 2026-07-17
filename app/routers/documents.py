import json
import os
import random
import re
import string
import uuid
import zipfile
from pathlib import Path
from typing import Optional
from threading import Lock

from fastapi import APIRouter, Depends, HTTPException, UploadFile, File
from fastapi.responses import Response
from pydantic import BaseModel
from sqlalchemy.orm import Session

from app.config import get_settings
from app.database import get_db
from app.models import Document, DocStatus, DocumentVectorIndex, IndexStatus
from app.utils import calculate_file_hash, to_relative_path, to_absolute_path

router = APIRouter(prefix="/assist/api/documents", tags=["documents"])

settings = get_settings()
UPLOAD_DIR = Path(settings.uploads_folder)
UPLOAD_DIR.mkdir(parents=True, exist_ok=True)

QDRANT_USER_ID = 0  # Default user ID for Qdrant since no auth

# ── 文件别名系统（生成不可猜测的 PDF 访问路径） ─────────────────────────────
_file_aliases: dict[str, int] = {}   # alias -> doc_id
_alias_lock = Lock()


def _make_alias(year: int | None, title: str | None) -> str:
    """生成 {year}_{shorttitle}_{random6}.pdf 格式的别名。"""
    y = str(year) if year else "unknown"
    # 标题取前 30 字符，只保留字母数字和连字符
    if title:
        t = re.sub(r"[^a-zA-Z0-9一-鿿]+", "_", title)[:30].strip("_")
    else:
        t = "untitled"
    rand = "".join(random.choices(string.ascii_lowercase + string.digits, k=6))
    return f"{y}_{t}_{rand}.pdf"


def get_doc_by_alias(alias: str) -> int | None:
    """通过别名查找 doc_id，供外部路由使用。"""
    with _alias_lock:
        return _file_aliases.get(alias)


def _next_available_id(db: Session) -> int | None:
    """查找最小的可用 ID（复用已删除文档的 ID）。"""
    existing_ids = [r[0] for r in db.query(Document.id).order_by(Document.id).all()]
    if not existing_ids:
        return None
    expected = 1
    for eid in existing_ids:
        if eid != expected:
            return expected
        expected += 1
    return None


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
    status_message: Optional[str] = None
    indexed_dbs: Optional[list[str]] = None  # 已索引的数据库列表
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


def _doc_to_out(doc: Document, db: Session = None) -> dict:
    # Handle year field - convert invalid values to None
    year = doc.year
    if year is not None and not isinstance(year, int):
        try:
            year = int(year) if year else None
        except (ValueError, TypeError):
            year = None

    # 查询索引数据库信息
    indexed_dbs = []
    if db and doc.id:
        indexes = db.query(DocumentVectorIndex).filter(
            DocumentVectorIndex.document_id == doc.id,
            DocumentVectorIndex.status == IndexStatus.indexed,
        ).all()
        indexed_dbs = [idx.vector_db_id for idx in indexes]

    return DocumentOut(
        id=doc.id,
        filename=doc.filename,
        file_path=doc.file_path,
        markdown_path=doc.markdown_path,
        file_hash=doc.file_hash,
        title=doc.title,
        authors=doc.authors,
        year=year,
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
        status_message=doc.status_message,
        indexed_dbs=indexed_dbs if indexed_dbs else None,
        created_at=doc.created_at.isoformat() if doc.created_at else None,
        updated_at=doc.updated_at.isoformat() if doc.updated_at else None,
    ).model_dump()


@router.get("/similar-titles")
def find_similar_titles(
    min_group_size: int = 2,
    db: Session = Depends(get_db),
):
    """查找标题相似的文档（用于去重）。

    按归一化标题分组，返回每组包含的文档列表。
    """
    import re
    from collections import defaultdict

    docs = db.query(Document).filter(Document.title.isnot(None), Document.title != "").all()

    def normalize(title: str) -> str:
        """归一化标题：小写、去除标点空白。"""
        t = title.lower().strip()
        t = re.sub(r'[^a-z0-9一-鿿]+', '', t)  # 保留字母数字中文
        return t

    groups = defaultdict(list)
    for doc in docs:
        key = normalize(doc.title)
        if key:
            groups[key].append(doc)

    # 过滤出有多个文档的组
    result = []
    for key, group_docs in groups.items():
        if len(group_docs) >= min_group_size:
            result.append({
                "normalized_title": key,
                "count": len(group_docs),
                "documents": [_doc_to_out(d, db) for d in group_docs],
            })

    # 按组大小降序
    result.sort(key=lambda g: g["count"], reverse=True)

    return {
        "groups": result,
        "total_groups": len(result),
        "total_documents": sum(g["count"] for g in result),
    }


@router.get("/vector-dbs")
def list_vector_dbs():
    """列出所有配置的向量数据库（用于搜索时选择）。"""
    from app.config_manager import load_config
    cfg = load_config()
    dbs = []
    for db in cfg.get("vector_dbs", []):
        dbs.append({
            "id": db.get("id"),
            "name": db.get("name", db.get("id")),
            "type": db.get("type"),
            "enabled": db.get("enabled", False),
            "collection": db.get("collection", ""),
        })
    return {"vector_dbs": dbs}


@router.get("/doc-types")
def list_doc_types(db: Session = Depends(get_db)):
    """返回所有已使用的文献类型（去重、排序）。"""
    rows = (
        db.query(Document.doc_type)
        .filter(Document.doc_type.isnot(None), Document.doc_type != "")
        .distinct()
        .all()
    )
    types = sorted([r[0] for r in rows])
    return {"doc_types": types}


@router.get("/grep-search")
async def grep_search(
    q: str = "",
    limit: int = 10,
    context: int = 2,
    db: Session = Depends(get_db),
):
    """基于系统 grep 的轻量全文搜索（无需向量数据库）。"""
    if not q.strip():
        return {"results": [], "query": q}

    from app.services.grep_search import grep_search as do_grep

    results = await do_grep(query=q, context_lines=context, limit=limit)

    # 补充文档元数据
    enriched = []
    for hit in results:
        doc_id = hit.get("document_id")
        doc_info = {}
        if doc_id:
            doc = db.query(Document).filter(Document.id == doc_id).first()
            if doc:
                doc_info = {
                    "title": doc.title,
                    "filename": doc.filename,
                    "status": doc.status.value if isinstance(doc.status, DocStatus) else doc.status,
                    "authors": doc.authors,
                    "year": doc.year,
                    "journal": doc.journal,
                }
        enriched.append({**hit, **doc_info})

    return {"results": enriched, "query": q}


@router.get("/search")
async def semantic_search(
    q: str = "",
    limit: int = 5,
    vector_db_id: Optional[str] = None,
    db: Session = Depends(get_db),
):
    """Semantic search across indexed documents using vector similarity."""
    if not q.strip():
        return {"results": [], "query": q}

    from app.services.qdrant import search_similar
    from app.services.ollama import get_embedding

    try:
        results = await search_similar(
            user_id=QDRANT_USER_ID,
            query=q,
            get_embedding_func=get_embedding,
            limit=limit,
            vector_db_id=vector_db_id,
        )
    except ValueError as e:
        raise HTTPException(status_code=400, detail=str(e))

    # Enrich results with document info from DB
    enriched = []
    for hit in results:
        doc_id = hit.get("document_id")
        doc_info = {}
        if doc_id:
            doc = db.query(Document).filter(Document.id == doc_id).first()
            if doc:
                doc_info = {
                    "filename": doc.filename,
                    "status": doc.status.value if isinstance(doc.status, DocStatus) else doc.status,
                    "authors": doc.authors,
                    "year": doc.year,
                    "journal": doc.journal,
                }
        enriched.append({
            **hit,
            **doc_info,
        })

    return {"results": enriched, "query": q}


@router.get("/")
def list_documents(
    q: Optional[str] = None,
    status_filter: Optional[str] = None,
    doc_type_filter: Optional[str] = None,
    page: int = 1,
    page_size: int = 50,
    sort_by: Optional[str] = None,
    sort_order: Optional[str] = "asc",
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
        # Support multiple status filters separated by comma
        statuses = status_filter.split(",")
        query = query.filter(Document.status.in_(statuses))

    if doc_type_filter:
        # Support multiple doc types separated by comma
        doc_types = doc_type_filter.split(",")
        query = query.filter(Document.doc_type.in_(doc_types))

    # Get total count
    total = query.count()

    # Apply sorting
    _SORTABLE_COLUMNS = {
        "id": Document.id,
        "title": Document.title,
        "authors": Document.authors,
        "year": Document.year,
        "doc_type": Document.doc_type,
        "status": Document.status,
        "journal": Document.journal,
        "language": Document.language,
        "doi": Document.doi,
        "category": Document.category,
        "created_at": Document.created_at,
        "updated_at": Document.updated_at,
    }
    sort_col = _SORTABLE_COLUMNS.get(sort_by, Document.created_at)
    if sort_order == "asc":
        order_clause = sort_col.asc().nullslast()
    else:
        order_clause = sort_col.desc().nullsfirst()

    # Apply pagination
    offset = (page - 1) * page_size
    docs = query.order_by(order_clause).offset(offset).limit(page_size).all()

    return {
        "items": [_doc_to_out(d, db) for d in docs],
        "total": total,
        "page": page,
        "page_size": page_size,
        "total_pages": (total + page_size - 1) // page_size,
    }


@router.post("/upload")
async def upload_document(
    file: UploadFile = File(...),
    db: Session = Depends(get_db),
):
    if not file.filename.lower().endswith(".pdf"):
        raise HTTPException(status_code=400, detail="只支持 PDF 文件")

    content = await file.read()

    # Calculate hash from the bytes already in memory
    import hashlib
    file_hash = hashlib.sha256(content).hexdigest()

    # Create DB record, reuse smallest available ID
    doc = Document(
        filename=file.filename,
        file_path="",
        file_hash=file_hash,
        status=DocStatus.uploaded,
    )
    available_id = _next_available_id(db)
    if available_id is not None:
        doc.id = available_id
    db.add(doc)
    db.commit()
    db.refresh(doc)

    # Save file as uploads/{id}/{id}.pdf
    doc_dir = UPLOAD_DIR / str(doc.id)
    doc_dir.mkdir(parents=True, exist_ok=True)
    file_path = doc_dir / f"{doc.id}.pdf"

    with open(file_path, "wb") as f:
        f.write(content)

    # 存储相对路径
    doc.file_path = to_relative_path(str(file_path))
    db.commit()
    db.refresh(doc)

    return _doc_to_out(doc, db)


class RegisterByPath(BaseModel):
    file_path: str
    force: bool = False  # Force registration even if hash exists


@router.post("/register")
def register_document_by_path(
    data: RegisterByPath,
    db: Session = Depends(get_db),
):
    """通过绝对路径注册 PDF 文件（复制到 uploads 目录）。"""
    file_path = data.file_path

    # Validate file exists
    if not os.path.exists(file_path):
        raise HTTPException(status_code=400, detail=f"文件不存在: {file_path}")

    if not file_path.lower().endswith(".pdf"):
        raise HTTPException(status_code=400, detail="只支持 PDF 文件")

    # Calculate file hash
    file_hash = calculate_file_hash(file_path)

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

    # Create DB record, reuse smallest available ID
    filename = os.path.basename(file_path)
    doc = Document(
        filename=filename,
        file_path="",
        file_hash=file_hash,
        status=DocStatus.uploaded,
    )
    available_id = _next_available_id(db)
    if available_id is not None:
        doc.id = available_id
    db.add(doc)
    db.commit()
    db.refresh(doc)

    # Copy file to uploads/{id}/{id}.pdf
    import shutil
    doc_dir = UPLOAD_DIR / str(doc.id)
    doc_dir.mkdir(parents=True, exist_ok=True)
    dest_path = doc_dir / f"{doc.id}.pdf"
    shutil.copy2(file_path, dest_path)

    # 存储相对路径
    doc.file_path = to_relative_path(str(dest_path))
    db.commit()
    db.refresh(doc)

    return _doc_to_out(doc, db)


@router.get("/by-hash/{file_hash}")
def get_document_by_hash(
    file_hash: str,
    db: Session = Depends(get_db),
):
    """根据文件 SHA256 哈希查找已注册的文档。"""
    if len(file_hash) != 64:
        raise HTTPException(status_code=400, detail="哈希格式无效，需要 64 位 SHA256")

    doc = db.query(Document).filter(Document.file_hash == file_hash).first()
    if not doc:
        raise HTTPException(status_code=404, detail="未找到匹配的文档")

    return _doc_to_out(doc, db)


@router.get("/by-doi/{doi:path}")
def get_document_by_doi(
    doi: str,
    db: Session = Depends(get_db),
):
    """根据 DOI 查找已注册的文档。"""
    if not doi.strip():
        raise HTTPException(status_code=400, detail="DOI 不能为空")

    doc = db.query(Document).filter(Document.doi == doi).first()
    if not doc:
        raise HTTPException(status_code=404, detail="未找到匹配的文档")

    return _doc_to_out(doc, db)


@router.get("/{doc_id}")
def get_document(
    doc_id: int,
    db: Session = Depends(get_db),
):
    doc = db.query(Document).filter(Document.id == doc_id).first()
    if not doc:
        raise HTTPException(status_code=404, detail="文献不存在")
    return _doc_to_out(doc, db)


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
    return _doc_to_out(doc, db)


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
    # 使用绝对路径删除文件
    doc_upload_dir = to_absolute_path(str(doc_id))
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


@router.head("/{doc_id}/pdf")
def check_pdf_exists(
    doc_id: int,
    db: Session = Depends(get_db),
):
    """检查 PDF 文件是否存在（HEAD 请求，不返回文件内容）。"""
    doc = db.query(Document).filter(Document.id == doc_id).first()
    if not doc or not doc.file_path:
        raise HTTPException(status_code=404, detail="PDF 文件不存在")

    abs_file_path = to_absolute_path(doc.file_path)
    if not os.path.exists(abs_file_path):
        raise HTTPException(status_code=404, detail="PDF 文件不存在")

    return Response(status_code=200)


@router.get("/{doc_id}/pdf")
def get_pdf(
    doc_id: int,
    db: Session = Depends(get_db),
):
    doc = db.query(Document).filter(Document.id == doc_id).first()
    if not doc or not doc.file_path:
        raise HTTPException(status_code=404, detail="PDF 文件不存在")

    # 将相对路径转换为绝对路径
    abs_file_path = to_absolute_path(doc.file_path)
    if not os.path.exists(abs_file_path):
        raise HTTPException(status_code=404, detail="PDF 文件不存在")

    with open(abs_file_path, "rb") as f:
        pdf_content = f.read()

    return Response(
        content=pdf_content,
        media_type="application/pdf",
        headers={"Content-Disposition": "inline"},
    )


# 图片 MIME 类型映射
_IMAGE_MIME = {
    ".jpg": "image/jpeg",
    ".jpeg": "image/jpeg",
    ".png": "image/png",
    ".gif": "image/gif",
    ".svg": "image/svg+xml",
}


@router.get("/{doc_id}/image/{filename}")
def get_image_from_zip(
    doc_id: int,
    filename: str,
    db: Session = Depends(get_db),
):
    """从 images.zip 中读取图片并返回。"""
    doc = db.query(Document).filter(Document.id == doc_id).first()
    if not doc:
        raise HTTPException(status_code=404, detail="文档不存在")

    doc_dir = os.path.join(settings.uploads_folder, str(doc_id))
    zip_path = os.path.join(doc_dir, "images.zip")

    if not os.path.exists(zip_path):
        raise HTTPException(status_code=404, detail="图片包不存在")

    try:
        with zipfile.ZipFile(zip_path, 'r') as zf:
            # 在 zip 中查找匹配的文件（basename 匹配）
            target = None
            for name in zf.namelist():
                if os.path.basename(name) == filename:
                    target = name
                    break
            if not target:
                raise HTTPException(status_code=404, detail="图片不存在")

            img_data = zf.read(target)
    except zipfile.BadZipFile:
        raise HTTPException(status_code=500, detail="图片包损坏")

    ext = os.path.splitext(filename)[1].lower()
    mime = _IMAGE_MIME.get(ext, "application/octet-stream")

    return Response(
        content=img_data,
        media_type=mime,
        headers={"Cache-Control": "public, max-age=86400"},
    )


@router.post("/{doc_id}/pdf")
async def replace_pdf(
    doc_id: int,
    file: UploadFile = File(...),
    db: Session = Depends(get_db),
):
    """替换已有文档的 PDF 文件。"""
    doc = db.query(Document).filter(Document.id == doc_id).first()
    if not doc:
        raise HTTPException(status_code=404, detail="文档不存在")

    if not file.filename.lower().endswith(".pdf"):
        raise HTTPException(status_code=400, detail="只支持 PDF 文件")

    content = await file.read()

    # 计算新文件哈希
    import hashlib
    file_hash = hashlib.sha256(content).hexdigest()

    # 保存文件到 uploads/{id}/{id}.pdf
    doc_dir = UPLOAD_DIR / str(doc.id)
    doc_dir.mkdir(parents=True, exist_ok=True)
    file_path = doc_dir / f"{doc.id}.pdf"

    with open(file_path, "wb") as f:
        f.write(content)

    # 更新数据库记录
    doc.file_path = to_relative_path(str(file_path))
    doc.file_hash = file_hash
    doc.status = DocStatus.uploaded
    db.commit()
    db.refresh(doc)

    return _doc_to_out(doc, db)


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

    # 将相对路径转换为绝对路径
    abs_markdown_path = to_absolute_path(doc.markdown_path) if doc.markdown_path else None
    if not abs_markdown_path or not os.path.exists(abs_markdown_path):
        raise HTTPException(status_code=404, detail="Markdown 文件尚未生成")

    with open(abs_markdown_path, "r", encoding="utf-8") as f:
        content = f.read()

    # Rewrite image paths to use zip-based image API
    import re

    # Replace relative image paths: images/xxx.png -> /assist/api/documents/{id}/image/xxx.png
    def rewrite_image(match):
        img_path = match.group(1)
        if not img_path.startswith(('http://', 'https://', '/assist/')):
            # 提取文件名（去掉 images/ 前缀）
            filename = os.path.basename(img_path)
            return f'](/assist/api/documents/{doc_id}/image/{filename})'
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

    # 将相对路径转换为绝对路径
    abs_markdown_path = to_absolute_path(doc.markdown_path)
    with open(abs_markdown_path, "w", encoding="utf-8") as f:
        f.write(data.content)
    return {"detail": "Markdown 已更新"}


@router.get("/{doc_id}/file-alias")
def generate_file_alias(
    doc_id: int,
    db: Session = Depends(get_db),
):
    """为 PDF 生成一个不可猜测的临时访问路径。"""
    doc = db.query(Document).filter(Document.id == doc_id).first()
    if not doc:
        raise HTTPException(status_code=404, detail="文献不存在")
    if not doc.file_path:
        raise HTTPException(status_code=404, detail="PDF 文件不存在")

    # 检查是否已有别名，没有则创建（原子操作）
    with _alias_lock:
        for alias, did in _file_aliases.items():
            if did == doc_id:
                return {"url": f"/assist/file/{alias}"}
        alias = _make_alias(doc.year, doc.title)
        _file_aliases[alias] = doc_id
    return {"url": f"/assist/file/{alias}"}
