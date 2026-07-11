"""
OKB-Assist MCP Server

Exposes document management tools via Model Context Protocol (MCP)
for use by AI assistants like Claude Desktop, Cursor, etc.

Usage:
  stdio mode:  python -m app.mcp_server
  SSE mode:    mounted at /assist/mcp in main.py
"""

import os
import json
from typing import Optional

from mcp.server.fastmcp import FastMCP

from app.config import get_settings
from app.database import SessionLocal
from app.models import Document, DocStatus
from app.services.qdrant import search_similar, list_collections
from app.services.ollama import get_embedding
from app.utils import to_absolute_path

settings = get_settings()

mcp = FastMCP("OKB-Assist")


def _get_db():
    """Get a database session."""
    return SessionLocal()


def _format_doc(doc: Document) -> dict:
    """Format a document for display."""
    year = doc.year
    if year is not None and not isinstance(year, int):
        try:
            year = int(year) if year else None
        except (ValueError, TypeError):
            year = None

    # 检查 markdown 文件是否存在
    has_markdown = False
    if doc.markdown_path:
        abs_markdown_path = to_absolute_path(doc.markdown_path)
        has_markdown = os.path.exists(abs_markdown_path)

    return {
        "id": doc.id,
        "filename": doc.filename,
        "title": doc.title or doc.filename,
        "authors": doc.authors,
        "year": year,
        "doi": doc.doi,
        "journal": doc.journal,
        "keywords": doc.keywords,
        "abstract": doc.abstract,
        "category": doc.category,
        "doc_type": doc.doc_type,
        "language": doc.language,
        "status": doc.status.value if isinstance(doc.status, DocStatus) else doc.status,
        "has_markdown": has_markdown,
        "pdf_url": f"/assist/api/documents/{doc.id}/pdf",
        "markdown_url": f"/assist/markdown/{doc.id}",
        "detail_url": f"/assist/detail/{doc.id}",
    }


# ─── Tools ───────────────────────────────────────────────────────────────────


@mcp.tool()
async def search_documents(query: str, limit: int = 5) -> str:
    """
    语义搜索文献内容。使用向量数据库进行相似度搜索，返回最相关的文档片段。

    Args:
        query: 搜索查询（支持中文和英文）
        limit: 返回结果数量，默认5
    """
    if not query.strip():
        return json.dumps({"error": "查询不能为空"}, ensure_ascii=False)

    results = await search_similar(
        user_id=0,
        query=query,
        get_embedding_func=get_embedding,
        limit=limit,
    )

    if not results:
        return json.dumps({"message": "未找到相关结果", "query": query}, ensure_ascii=False)

    # Enrich with document info
    db = _get_db()
    try:
        enriched = []
        for hit in results:
            doc_id = hit.get("document_id")
            doc_info = {}
            if doc_id:
                doc = db.query(Document).filter(Document.id == doc_id).first()
                if doc:
                    doc_info = {
                        "filename": doc.filename,
                        "title": doc.title,
                        "authors": doc.authors,
                        "year": doc.year,
                        "journal": doc.journal,
                        "pdf_url": f"/assist/api/documents/{doc.id}/pdf",
                    }
            enriched.append({
                "score": round(hit.get("score", 0), 4),
                "document_id": doc_id,
                "chunk_text": hit.get("chunk_text", "")[:500],
                **doc_info,
            })
    finally:
        db.close()

    return json.dumps({"query": query, "results": enriched}, ensure_ascii=False, indent=2)


@mcp.tool()
def read_markdown(doc_id: int, page: int = 1, page_size: int = 5000) -> str:
    """
    读取文献的 Markdown 内容（分页）。Markdown 是从 PDF 解析后的文本格式，包含公式、表格等。

    Args:
        doc_id: 文档 ID
        page: 页码，从1开始
        page_size: 每页字符数，默认5000
    """
    db = _get_db()
    try:
        doc = db.query(Document).filter(Document.id == doc_id).first()
        if not doc:
            return json.dumps({"error": f"文档 {doc_id} 不存在"}, ensure_ascii=False)

        # 将相对路径转换为绝对路径
        abs_markdown_path = to_absolute_path(doc.markdown_path) if doc.markdown_path else None
        if not abs_markdown_path or not os.path.exists(abs_markdown_path):
            return json.dumps({"error": "Markdown 文件尚未生成，请先解析 PDF"}, ensure_ascii=False)

        with open(abs_markdown_path, "r", encoding="utf-8") as f:
            content = f.read()

        # Simple pagination
        total_pages = max(1, (len(content) + page_size - 1) // page_size)
        start = (page - 1) * page_size
        end = start + page_size
        page_content = content[start:end]

        return json.dumps({
            "doc_id": doc_id,
            "title": doc.title or doc.filename,
            "page": page,
            "total_pages": total_pages,
            "content": page_content,
        }, ensure_ascii=False, indent=2)
    finally:
        db.close()


@mcp.tool()
def get_document_info(doc_id: int) -> str:
    """
    获取文献的详细信息，包括元数据、处理状态、PDF 和 Markdown 链接。

    Args:
        doc_id: 文档 ID
    """
    db = _get_db()
    try:
        doc = db.query(Document).filter(Document.id == doc_id).first()
        if not doc:
            return json.dumps({"error": f"文档 {doc_id} 不存在"}, ensure_ascii=False)

        return json.dumps(_format_doc(doc), ensure_ascii=False, indent=2)
    finally:
        db.close()


@mcp.tool()
def list_documents(query: str = "", status: str = "", page: int = 1, page_size: int = 20) -> str:
    """
    搜索或列出文献。支持按标题/作者搜索和按状态过滤。

    Args:
        query: 搜索关键词（可选，匹配标题、作者、文件名）
        status: 状态过滤（可选：uploaded/parsing/markdown_done/extracting/meta_done/indexing/indexed/error）
        page: 页码
        page_size: 每页数量
    """
    db = _get_db()
    try:
        q = db.query(Document)

        if query:
            q = q.filter(
                (Document.title.ilike(f"%{query}%")) |
                (Document.authors.ilike(f"%{query}%")) |
                (Document.filename.ilike(f"%{query}%"))
            )

        if status:
            statuses = [s.strip() for s in status.split(",")]
            q = q.filter(Document.status.in_(statuses))

        total = q.count()
        offset = (page - 1) * page_size
        docs = q.order_by(Document.created_at.desc()).offset(offset).limit(page_size).all()

        items = [_format_doc(doc) for doc in docs]

        return json.dumps({
            "total": total,
            "page": page,
            "page_size": page_size,
            "total_pages": max(1, (total + page_size - 1) // page_size),
            "items": items,
        }, ensure_ascii=False, indent=2)
    finally:
        db.close()


@mcp.tool()
def get_pdf_url(doc_id: int) -> str:
    """
    获取文献的 PDF 下载/预览链接。

    Args:
        doc_id: 文档 ID
    """
    db = _get_db()
    try:
        doc = db.query(Document).filter(Document.id == doc_id).first()
        if not doc:
            return json.dumps({"error": f"文档 {doc_id} 不存在"}, ensure_ascii=False)

        # 将相对路径转换为绝对路径
        abs_file_path = to_absolute_path(doc.file_path) if doc.file_path else None
        if not abs_file_path or not os.path.exists(abs_file_path):
            return json.dumps({"error": "PDF 文件不存在"}, ensure_ascii=False)

        base_url = settings.mineru_url.rstrip("/assist")  # Get base URL
        return json.dumps({
            "doc_id": doc_id,
            "filename": doc.filename,
            "pdf_url": f"/assist/api/documents/{doc_id}/pdf",
            "title": doc.title,
        }, ensure_ascii=False, indent=2)
    finally:
        db.close()


@mcp.tool()
def get_document_abstract(doc_id: int) -> str:
    """
    获取文献的摘要信息。如果有多语言摘要，会同时返回。

    Args:
        doc_id: 文档 ID
    """
    db = _get_db()
    try:
        doc = db.query(Document).filter(Document.id == doc_id).first()
        if not doc:
            return json.dumps({"error": f"文档 {doc_id} 不存在"}, ensure_ascii=False)

        result = {
            "doc_id": doc_id,
            "title": doc.title or doc.filename,
            "abstract": doc.abstract,
        }
        if doc.abstract_en:
            result["abstract_en"] = doc.abstract_en
        if doc.language and doc.language != "en":
            result["language"] = doc.language

        return json.dumps(result, ensure_ascii=False, indent=2)
    finally:
        db.close()


# ─── Resources ───────────────────────────────────────────────────────────────


@mcp.resource("okb://documents/{doc_id}")
def get_document_resource(doc_id: int) -> str:
    """获取文献详情资源"""
    db = _get_db()
    try:
        doc = db.query(Document).filter(Document.id == doc_id).first()
        if not doc:
            return json.dumps({"error": f"文档 {doc_id} 不存在"}, ensure_ascii=False)
        return json.dumps(_format_doc(doc), ensure_ascii=False, indent=2)
    finally:
        db.close()


@mcp.resource("okb://documents/{doc_id}/markdown")
def get_markdown_resource(doc_id: int) -> str:
    """获取文献 Markdown 内容资源"""
    db = _get_db()
    try:
        doc = db.query(Document).filter(Document.id == doc_id).first()
        if not doc:
            return f"Error: 文档 {doc_id} 不存在"

        # 将相对路径转换为绝对路径
        abs_markdown_path = to_absolute_path(doc.markdown_path) if doc.markdown_path else None
        if not abs_markdown_path or not os.path.exists(abs_markdown_path):
            return "Error: Markdown 文件尚未生成"

        with open(abs_markdown_path, "r", encoding="utf-8") as f:
            return f.read()
    finally:
        db.close()


# ─── Run ─────────────────────────────────────────────────────────────────────

if __name__ == "__main__":
    mcp.run(transport="stdio")
