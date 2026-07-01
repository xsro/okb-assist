import json
import os
import asyncio
from typing import Optional

from fastapi import APIRouter, Depends, HTTPException, BackgroundTasks
from sqlalchemy.orm import Session

from app.database import get_db, SessionLocal
from app.models import Document, DocStatus
from app.services.mineru import parse_pdf
from app.services.ollama import extract_metadata, get_embedding, add_yaml_frontmatter
from app.services.qdrant import index_document, delete_document_points

router = APIRouter(prefix="/assist/api/pipeline", tags=["pipeline"])

QDRANT_USER_ID = 0  # Default user ID for Qdrant since no auth


def _update_doc_status(doc_id: int, status: DocStatus, message: str = None, progress: float = None):
    """Update document status in database."""
    db = SessionLocal()
    try:
        doc = db.query(Document).filter(Document.id == doc_id).first()
        if doc:
            doc.status = status
            if message is not None:
                doc.status_message = message
            if progress is not None:
                doc.progress = progress
            db.commit()
    finally:
        db.close()


async def _run_parse(doc_id: int, file_path: str):
    """Background task for parsing PDF."""
    try:
        _update_doc_status(doc_id, DocStatus.parsing, "正在解析 PDF...", 10)

        output_dir = os.path.dirname(file_path)
        md_path = await parse_pdf(file_path, output_dir)

        db = SessionLocal()
        try:
            doc = db.query(Document).filter(Document.id == doc_id).first()
            if doc:
                doc.markdown_path = md_path
                doc.status = DocStatus.markdown_done
                doc.status_message = "PDF 解析完成"
                doc.progress = 100
                db.commit()
        finally:
            db.close()
    except Exception as e:
        _update_doc_status(doc_id, DocStatus.error, f"解析失败: {str(e)}")


async def _run_extract(doc_id: int):
    """Background task for extracting metadata."""
    try:
        _update_doc_status(doc_id, DocStatus.extracting, "正在提取元数据...", 10)

        db = SessionLocal()
        try:
            doc = db.query(Document).filter(Document.id == doc_id).first()
            if not doc or not doc.markdown_path:
                _update_doc_status(doc_id, DocStatus.error, "Markdown 文件不存在")
                return

            with open(doc.markdown_path, "r", encoding="utf-8") as f:
                markdown_content = f.read()

            _update_doc_status(doc_id, DocStatus.extracting, "正在调用 Ollama...", 30)

            metadata = await extract_metadata(markdown_content)

            _update_doc_status(doc_id, DocStatus.extracting, "正在保存元数据...", 80)

            # Update document fields
            doc.title = metadata.get("title", "")
            doc.authors = json.dumps(metadata.get("authors", []), ensure_ascii=False)
            doc.year = metadata.get("year")
            doc.doi = metadata.get("doi", "")
            doc.source = metadata.get("source", "")
            doc.journal = metadata.get("journal", "")
            doc.keywords = json.dumps(metadata.get("keywords", []), ensure_ascii=False)
            doc.abstract = metadata.get("abstract", "")
            doc.category = metadata.get("category", "")
            doc.doc_type = metadata.get("type", "")
            doc.language = metadata.get("language", "en")

            # English fields for non-English documents
            if doc.language and doc.language != "en":
                doc.title_en = metadata.get("title_en", "")
                doc.authors_en = json.dumps(metadata.get("authors_en", []), ensure_ascii=False)
                doc.keywords_en = json.dumps(metadata.get("keywords_en", []), ensure_ascii=False)
                doc.abstract_en = metadata.get("abstract_en", "")
                doc.journal_en = metadata.get("journal_en", "")

            # Add YAML frontmatter to markdown
            updated_markdown = add_yaml_frontmatter(markdown_content, metadata)
            with open(doc.markdown_path, "w", encoding="utf-8") as f:
                f.write(updated_markdown)

            doc.status = DocStatus.meta_done
            doc.status_message = "元数据提取完成"
            doc.progress = 100
            db.commit()
        finally:
            db.close()
    except Exception as e:
        _update_doc_status(doc_id, DocStatus.error, f"元数据提取失败: {str(e)}")


async def _run_index(doc_id: int):
    """Background task for indexing to Qdrant."""
    try:
        _update_doc_status(doc_id, DocStatus.indexing, "正在索引到 Qdrant...", 10)

        db = SessionLocal()
        try:
            doc = db.query(Document).filter(Document.id == doc_id).first()
            if not doc or not doc.markdown_path:
                _update_doc_status(doc_id, DocStatus.error, "Markdown 文件不存在")
                return

            with open(doc.markdown_path, "r", encoding="utf-8") as f:
                markdown_content = f.read()

            _update_doc_status(doc_id, DocStatus.indexing, "正在生成向量...", 30)

            metadata = {
                "title": doc.title or "",
                "authors": json.loads(doc.authors) if doc.authors else [],
                "year": doc.year,
                "type": doc.doc_type or "",
                "keywords": json.loads(doc.keywords) if doc.keywords else [],
            }

            _update_doc_status(doc_id, DocStatus.indexing, "正在上传到 Qdrant...", 60)

            collection_name = await index_document(
                doc_id=doc.id,
                user_id=QDRANT_USER_ID,
                markdown_content=markdown_content,
                metadata=metadata,
                get_embedding_func=get_embedding,
            )

            doc.qdrant_collection = collection_name
            doc.status = DocStatus.indexed
            doc.status_message = "已索引到向量数据库"
            doc.progress = 100
            db.commit()
        finally:
            db.close()
    except Exception as e:
        _update_doc_status(doc_id, DocStatus.error, f"索引失败: {str(e)}")


async def _run_full_pipeline(doc_id: int):
    """Background task for full pipeline."""
    try:
        db = SessionLocal()
        doc = db.query(Document).filter(Document.id == doc_id).first()
        if not doc:
            db.close()
            return

        file_path = doc.file_path
        db.close()

        # Stage 1: Parse
        _update_doc_status(doc_id, DocStatus.parsing, "阶段 1/3: 正在解析 PDF...", 0)
        await _run_parse(doc_id, file_path)

        # Check if parse succeeded
        db = SessionLocal()
        doc = db.query(Document).filter(Document.id == doc_id).first()
        if not doc or doc.status != DocStatus.markdown_done:
            db.close()
            return
        db.close()

        # Stage 2: Extract
        _update_doc_status(doc_id, DocStatus.extracting, "阶段 2/3: 正在提取元数据...", 33)
        await _run_extract(doc_id)

        # Check if extract succeeded
        db = SessionLocal()
        doc = db.query(Document).filter(Document.id == doc_id).first()
        if not doc or doc.status != DocStatus.meta_done:
            db.close()
            return
        db.close()

        # Stage 3: Index
        _update_doc_status(doc_id, DocStatus.indexing, "阶段 3/3: 正在索引到 Qdrant...", 66)
        await _run_index(doc_id)

    except Exception as e:
        _update_doc_status(doc_id, DocStatus.error, f"处理失败: {str(e)}")


@router.post("/{doc_id}/reset")
def reset_document(
    doc_id: int,
    target_status: str = None,
    db: Session = Depends(get_db),
):
    """Reset document status to a previous state for re-processing."""
    doc = db.query(Document).filter(Document.id == doc_id).first()
    if not doc:
        raise HTTPException(status_code=404, detail="文献不存在")

    # Determine target status based on current status
    # Allow resetting to any previous valid state
    reset_map = {
        DocStatus.parsing: [DocStatus.uploaded],
        DocStatus.markdown_done: [DocStatus.uploaded],
        DocStatus.extracting: [DocStatus.uploaded, DocStatus.markdown_done],
        DocStatus.meta_done: [DocStatus.uploaded, DocStatus.markdown_done],
        DocStatus.indexing: [DocStatus.uploaded, DocStatus.markdown_done, DocStatus.meta_done],
        DocStatus.indexed: [DocStatus.uploaded, DocStatus.markdown_done, DocStatus.meta_done],
        DocStatus.error: [DocStatus.uploaded, DocStatus.markdown_done, DocStatus.meta_done],
    }

    valid_targets = reset_map.get(doc.status, [DocStatus.uploaded])

    # If target_status is provided, use it; otherwise default to first valid target
    if target_status and target_status in [s.value for s in valid_targets]:
        doc.status = DocStatus(target_status)
    else:
        doc.status = valid_targets[0]

    doc.status_message = None
    doc.progress = 0

    # Clear downstream data based on target status
    if doc.status == DocStatus.uploaded:
        doc.markdown_path = None
        doc.qdrant_collection = None
    elif doc.status == DocStatus.markdown_done:
        doc.qdrant_collection = None

    db.commit()

    return {
        "detail": f"状态已重置为 {doc.status.value}",
        "status": doc.status.value,
        "valid_targets": [s.value for s in valid_targets],
    }


@router.post("/{doc_id}/parse")
async def parse_document(
    doc_id: int,
    background_tasks: BackgroundTasks,
    db: Session = Depends(get_db),
):
    """Stage 1: Parse PDF to Markdown using MinerU (async)."""
    doc = db.query(Document).filter(Document.id == doc_id).first()
    if not doc:
        raise HTTPException(status_code=404, detail="文献不存在")

    if doc.status != DocStatus.uploaded:
        raise HTTPException(status_code=400, detail=f"当前状态 {doc.status.value} 不允许解析")

    doc.status = DocStatus.parsing
    doc.status_message = "任务已提交，等待处理..."
    doc.progress = 0
    db.commit()

    background_tasks.add_task(_run_parse, doc_id, doc.file_path)

    return {"detail": "解析任务已提交", "status": "parsing"}


@router.post("/{doc_id}/extract")
async def extract_document_meta(
    doc_id: int,
    background_tasks: BackgroundTasks,
    db: Session = Depends(get_db),
):
    """Stage 2: Extract metadata using Ollama (async)."""
    doc = db.query(Document).filter(Document.id == doc_id).first()
    if not doc:
        raise HTTPException(status_code=404, detail="文献不存在")

    if doc.status != DocStatus.markdown_done:
        raise HTTPException(status_code=400, detail=f"当前状态 {doc.status.value} 不允许提取元数据")

    doc.status = DocStatus.extracting
    doc.status_message = "任务已提交，等待处理..."
    doc.progress = 0
    db.commit()

    background_tasks.add_task(_run_extract, doc_id)

    return {"detail": "元数据提取任务已提交", "status": "extracting"}


@router.post("/{doc_id}/index")
async def index_document_to_qdrant(
    doc_id: int,
    background_tasks: BackgroundTasks,
    db: Session = Depends(get_db),
):
    """Stage 3: Index document into Qdrant (async)."""
    doc = db.query(Document).filter(Document.id == doc_id).first()
    if not doc:
        raise HTTPException(status_code=404, detail="文献不存在")

    if doc.status != DocStatus.meta_done:
        raise HTTPException(status_code=400, detail=f"当前状态 {doc.status.value} 不允许索引")

    doc.status = DocStatus.indexing
    doc.status_message = "任务已提交，等待处理..."
    doc.progress = 0
    db.commit()

    background_tasks.add_task(_run_index, doc_id)

    return {"detail": "索引任务已提交", "status": "indexing"}


@router.post("/{doc_id}/process")
async def process_full_pipeline(
    doc_id: int,
    background_tasks: BackgroundTasks,
    db: Session = Depends(get_db),
):
    """Run all three stages in sequence (async)."""
    doc = db.query(Document).filter(Document.id == doc_id).first()
    if not doc:
        raise HTTPException(status_code=404, detail="文献不存在")

    if doc.status not in [DocStatus.uploaded, DocStatus.error]:
        raise HTTPException(status_code=400, detail=f"当前状态 {doc.status.value} 不允许处理")

    doc.status = DocStatus.parsing
    doc.status_message = "全流程处理已提交..."
    doc.progress = 0
    db.commit()

    background_tasks.add_task(_run_full_pipeline, doc_id)

    return {"detail": "全流程处理任务已提交", "status": "processing"}


@router.get("/{doc_id}/status")
def get_pipeline_status(
    doc_id: int,
    db: Session = Depends(get_db),
):
    """Get current processing status of a document."""
    doc = db.query(Document).filter(Document.id == doc_id).first()
    if not doc:
        raise HTTPException(status_code=404, detail="文献不存在")

    return {
        "status": doc.status.value if isinstance(doc.status, DocStatus) else doc.status,
        "status_message": doc.status_message,
        "progress": doc.progress,
        "has_markdown": doc.markdown_path is not None and os.path.exists(doc.markdown_path or ""),
        "has_meta": doc.title is not None,
        "is_indexed": doc.qdrant_collection is not None,
    }
