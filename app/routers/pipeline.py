import json
import os

from fastapi import APIRouter, Depends, HTTPException
from sqlalchemy.orm import Session

from app.database import get_db
from app.models import Document, DocStatus
from app.services.mineru import parse_pdf
from app.services.ollama import extract_metadata, get_embedding, add_yaml_frontmatter
from app.services.qdrant import index_document, delete_document_points

router = APIRouter(prefix="/assist/api/pipeline", tags=["pipeline"])

QDRANT_USER_ID = 0  # Default user ID for Qdrant since no auth


@router.post("/{doc_id}/reset")
def reset_document(
    doc_id: int,
    db: Session = Depends(get_db),
):
    """Reset document status to uploaded for re-processing."""
    doc = db.query(Document).filter(Document.id == doc_id).first()
    if not doc:
        raise HTTPException(status_code=404, detail="文献不存在")

    doc.status = DocStatus.uploaded
    doc.markdown_path = None
    doc.qdrant_collection = None
    db.commit()

    return {"detail": "状态已重置", "status": doc.status.value}


@router.post("/{doc_id}/parse")
async def parse_document(
    doc_id: int,
    db: Session = Depends(get_db),
):
    """Stage 1: Parse PDF to Markdown using MinerU."""
    doc = db.query(Document).filter(Document.id == doc_id).first()
    if not doc:
        raise HTTPException(status_code=404, detail="文献不存在")

    if doc.status != DocStatus.uploaded:
        raise HTTPException(status_code=400, detail=f"当前状态 {doc.status.value} 不允许解析")

    try:
        output_dir = os.path.dirname(doc.file_path)
        md_path = await parse_pdf(doc.file_path, output_dir)

        doc.markdown_path = md_path
        doc.status = DocStatus.markdown_done
        db.commit()

        return {"detail": "PDF 解析完成", "status": doc.status.value}
    except Exception as e:
        raise HTTPException(status_code=500, detail=f"解析失败: {str(e)}")


@router.post("/{doc_id}/extract")
async def extract_document_meta(
    doc_id: int,
    db: Session = Depends(get_db),
):
    """Stage 2: Extract metadata using Ollama."""
    doc = db.query(Document).filter(Document.id == doc_id).first()
    if not doc:
        raise HTTPException(status_code=404, detail="文献不存在")

    if doc.status != DocStatus.markdown_done:
        raise HTTPException(status_code=400, detail=f"当前状态 {doc.status.value} 不允许提取元数据")

    if not doc.markdown_path or not os.path.exists(doc.markdown_path):
        raise HTTPException(status_code=400, detail="Markdown 文件不存在")

    try:
        # Read markdown
        with open(doc.markdown_path, "r", encoding="utf-8") as f:
            markdown_content = f.read()

        # Extract metadata
        metadata = await extract_metadata(markdown_content)

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
        db.commit()

        return {"detail": "元数据提取完成", "status": doc.status.value, "metadata": metadata}
    except Exception as e:
        raise HTTPException(status_code=500, detail=f"元数据提取失败: {str(e)}")


@router.post("/{doc_id}/index")
async def index_document_to_qdrant(
    doc_id: int,
    db: Session = Depends(get_db),
):
    """Stage 3: Index document into Qdrant."""
    doc = db.query(Document).filter(Document.id == doc_id).first()
    if not doc:
        raise HTTPException(status_code=404, detail="文献不存在")

    if doc.status != DocStatus.meta_done:
        raise HTTPException(status_code=400, detail=f"当前状态 {doc.status.value} 不允许索引")

    if not doc.markdown_path or not os.path.exists(doc.markdown_path):
        raise HTTPException(status_code=400, detail="Markdown 文件不存在")

    try:
        # Read markdown
        with open(doc.markdown_path, "r", encoding="utf-8") as f:
            markdown_content = f.read()

        # Build metadata dict
        metadata = {
            "title": doc.title or "",
            "authors": json.loads(doc.authors) if doc.authors else [],
            "year": doc.year,
            "type": doc.doc_type or "",
            "keywords": json.loads(doc.keywords) if doc.keywords else [],
        }

        # Index to Qdrant
        collection_name = await index_document(
            doc_id=doc.id,
            user_id=QDRANT_USER_ID,
            markdown_content=markdown_content,
            metadata=metadata,
            get_embedding_func=get_embedding,
        )

        doc.qdrant_collection = collection_name
        doc.status = DocStatus.indexed
        db.commit()

        return {"detail": "已索引到向量数据库", "status": doc.status.value, "collection": collection_name}
    except Exception as e:
        raise HTTPException(status_code=500, detail=f"索引失败: {str(e)}")


@router.post("/{doc_id}/process")
async def process_full_pipeline(
    doc_id: int,
    db: Session = Depends(get_db),
):
    """Run all three stages in sequence."""
    doc = db.query(Document).filter(Document.id == doc_id).first()
    if not doc:
        raise HTTPException(status_code=404, detail="文献不存在")

    results = []

    # Stage 1: Parse
    if doc.status == DocStatus.uploaded:
        try:
            output_dir = os.path.dirname(doc.file_path)
            md_path = await parse_pdf(doc.file_path, output_dir)
            doc.markdown_path = md_path
            doc.status = DocStatus.markdown_done
            db.commit()
            results.append({"stage": "parse", "status": "success"})
        except Exception as e:
            results.append({"stage": "parse", "status": "error", "detail": str(e)})
            return {"detail": "处理失败", "results": results, "status": doc.status.value}

    # Stage 2: Extract
    if doc.status == DocStatus.markdown_done:
        try:
            with open(doc.markdown_path, "r", encoding="utf-8") as f:
                markdown_content = f.read()

            metadata = await extract_metadata(markdown_content)

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

            updated_markdown = add_yaml_frontmatter(markdown_content, metadata)
            with open(doc.markdown_path, "w", encoding="utf-8") as f:
                f.write(updated_markdown)

            doc.status = DocStatus.meta_done
            db.commit()
            results.append({"stage": "extract", "status": "success", "metadata": metadata})
        except Exception as e:
            results.append({"stage": "extract", "status": "error", "detail": str(e)})
            return {"detail": "处理失败", "results": results, "status": doc.status.value}

    # Stage 3: Index
    if doc.status == DocStatus.meta_done:
        try:
            with open(doc.markdown_path, "r", encoding="utf-8") as f:
                markdown_content = f.read()

            metadata = {
                "title": doc.title or "",
                "authors": json.loads(doc.authors) if doc.authors else [],
                "year": doc.year,
                "type": doc.doc_type or "",
                "keywords": json.loads(doc.keywords) if doc.keywords else [],
            }

            collection_name = await index_document(
                doc_id=doc.id,
                user_id=QDRANT_USER_ID,
                markdown_content=markdown_content,
                metadata=metadata,
                get_embedding_func=get_embedding,
            )

            doc.qdrant_collection = collection_name
            doc.status = DocStatus.indexed
            db.commit()
            results.append({"stage": "index", "status": "success", "collection": collection_name})
        except Exception as e:
            results.append({"stage": "index", "status": "error", "detail": str(e)})
            return {"detail": "处理失败", "results": results, "status": doc.status.value}

    return {"detail": "处理完成", "results": results, "status": doc.status.value}


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
        "has_markdown": doc.markdown_path is not None and os.path.exists(doc.markdown_path or ""),
        "has_meta": doc.title is not None,
        "is_indexed": doc.qdrant_collection is not None,
    }
