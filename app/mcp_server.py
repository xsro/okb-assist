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
from mcp.server.transport_security import TransportSecuritySettings
from mcp.server.auth.settings import AuthSettings
from mcp.server.auth.provider import AccessToken
from pydantic import AnyHttpUrl
from starlette.applications import Starlette

from app.config import get_settings
from app.database import SessionLocal
from app.models import Document, DocStatus
from app.services.qdrant import search_similar, list_collections
from app.services.ollama import get_embedding
from app.utils import to_absolute_path

settings = get_settings()
MCP_MOUNT_PATH = "/assist/mcp"


# ── Bearer Token 认证 ──────────────────────────────────────────────────────

class StaticTokenVerifier:
    """使用 system.json 中 mcp_token 验证 Bearer Token。"""

    def __init__(self, token: str):
        self.token = token

    async def verify_token(self, raw_token: str) -> AccessToken | None:
        if raw_token == self.token:
            return AccessToken(
                token=raw_token,
                client_id="okb-client",
                scopes=["read", "write"],
            )
        return None


_token = settings.mcp_token
_auth_enabled = bool(_token) and _token != "change-me"

_mcp_kwargs: dict = {
    "instructions": (
        "OKB-Assist provides MCP tools for a local academic document library. "
        "Use search_documents for semantic search, grep_search for full-text "
        "keyword/regex search, list_documents to browse records, read_markdown "
        "to read parsed document text, and get_document_info/get_stats for "
        "metadata and library status."
    ),
    "streamable_http_path": "/",
    "transport_security": TransportSecuritySettings(enable_dns_rebinding_protection=False),
}

if _auth_enabled:
    _base_url = settings.public_url.rstrip("/")
    _mcp_kwargs["auth"] = AuthSettings(
        issuer_url=AnyHttpUrl(_base_url),
        resource_server_url=AnyHttpUrl(f"{_base_url}/assist/mcp"),
    )
    _mcp_kwargs["token_verifier"] = StaticTokenVerifier(_token)

mcp = FastMCP("OKB-Assist", **_mcp_kwargs)


def create_mcp_app() -> Starlette:
    """Create an MCP app that supports Streamable HTTP and legacy SSE."""
    streamable_http_app = mcp.streamable_http_app()
    sse_app = mcp.sse_app(mount_path="")

    return Starlette(
        debug=mcp.settings.debug,
        routes=[
            *streamable_http_app.routes,
            *sse_app.routes,
        ],
        middleware=[
            *streamable_http_app.user_middleware,
            *sse_app.user_middleware,
        ],
        lifespan=streamable_http_app.router.lifespan_context,
    )


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
async def grep_search(query: str, limit: int = 10, context: int = 2, doc_ids: str = "") -> str:
    """
    全文搜索文献内容（基于 grep，轻量快速）。无需向量数据库，支持正则表达式。

    Args:
        query: 搜索关键词（支持正则表达式）
        limit: 返回结果数量，默认10
        context: 匹配行前后的上下文行数，默认2
        doc_ids: 逗号分隔的文档 ID 列表，限定搜索范围（如 "1,2,3"），留空搜索全部
    """
    if not query.strip():
        return json.dumps({"error": "查询不能为空"}, ensure_ascii=False)

    from app.services.grep_search import grep_search as do_grep

    # 解析 doc_ids
    id_list = None
    if doc_ids and doc_ids.strip():
        try:
            id_list = [int(x.strip()) for x in doc_ids.split(",") if x.strip()]
        except ValueError:
            return json.dumps({"error": "doc_ids 格式无效，需为逗号分隔的数字"}, ensure_ascii=False)

    results = await do_grep(query=query, context_lines=context, limit=limit, doc_ids=id_list)

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
                        "title": doc.title or "",
                        "authors": doc.authors or "",
                        "year": doc.year,
                        "journal": doc.journal or "",
                    }
            enriched.append({
                "document_id": doc_id,
                "content": hit.get("content", ""),
                **doc_info,
            })

        return json.dumps({
            "query": query,
            "total": len(enriched),
            "results": enriched,
        }, ensure_ascii=False, indent=2)
    finally:
        db.close()


@mcp.tool()
async def search_info(query: str, limit: int = 10) -> str:
    """
    搜索文献元数据信息（标题、作者、期刊、关键词、摘要、DOI 等）。
    返回匹配文献的完整信息，适合按作者、标题、期刊等条件查找文献。

    Args:
        query: 搜索关键词（模糊匹配标题、作者、期刊、关键词、摘要、DOI 等字段）
        limit: 返回结果数量，默认10
    """
    if not query.strip():
        return json.dumps({"error": "查询不能为空"}, ensure_ascii=False)

    db = _get_db()
    try:
        like = f"%{query}%"
        q = db.query(Document).filter(
            (Document.title.ilike(like)) |
            (Document.title_en.ilike(like)) |
            (Document.authors.ilike(like)) |
            (Document.authors_en.ilike(like)) |
            (Document.journal.ilike(like)) |
            (Document.journal_en.ilike(like)) |
            (Document.keywords.ilike(like)) |
            (Document.keywords_en.ilike(like)) |
            (Document.abstract.ilike(like)) |
            (Document.abstract_en.ilike(like)) |
            (Document.doi.ilike(like)) |
            (Document.category.ilike(like)) |
            (Document.source.ilike(like))
        )

        docs = q.order_by(Document.updated_at.desc()).limit(limit).all()

        results = []
        for doc in docs:
            results.append({
                "id": doc.id,
                "title": doc.title or "",
                "title_en": doc.title_en or "",
                "authors": doc.authors or "",
                "authors_en": doc.authors_en or "",
                "year": doc.year,
                "doi": doc.doi or "",
                "journal": doc.journal or "",
                "journal_en": doc.journal_en or "",
                "keywords": doc.keywords or "",
                "abstract": (doc.abstract or "")[:300],
                "doc_type": doc.doc_type or "",
                "language": doc.language or "",
                "category": doc.category or "",
                "status": doc.status.value if hasattr(doc.status, 'value') else doc.status,
            })

        return json.dumps({
            "query": query,
            "total": len(results),
            "results": results,
        }, ensure_ascii=False, indent=2)
    finally:
        db.close()


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
def list_documents(query: str = "", status: str = "", doc_type: str = "", page: int = 1, page_size: int = 20) -> str:
    """
    搜索或列出文献。支持按标题/作者搜索、按状态和文献类型过滤。

    Args:
        query: 搜索关键词（可选，匹配标题、作者、文件名）
        status: 状态过滤（可选，逗号分隔：uploaded/parsing/markdown_done/extracting/meta_done/indexing/indexed/error）
        doc_type: 文献类型过滤（可选，逗号分隔，Zotero 类型：journalArticle/book/conferencePaper/thesis/report/preprint/bookSection 等）
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

        if doc_type:
            types = [t.strip() for t in doc_type.split(",")]
            q = q.filter(Document.doc_type.in_(types))

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


@mcp.tool()
def get_stats() -> str:
    """
    获取知识库统计信息，包括文献总数、各状态数量、各类型数量。
    """
    db = _get_db()
    try:
        total = db.query(Document).count()

        # 按状态统计
        status_counts = {}
        for s in DocStatus:
            count = db.query(Document).filter(Document.status == s).count()
            if count > 0:
                status_counts[s.value] = count

        # 按类型统计
        type_rows = (
            db.query(Document.doc_type)
            .filter(Document.doc_type.isnot(None), Document.doc_type != "")
            .all()
        )
        type_counts = {}
        for (dt,) in type_rows:
            type_counts[dt] = type_counts.get(dt, 0) + 1

        return json.dumps({
            "total_documents": total,
            "status_counts": status_counts,
            "type_counts": dict(sorted(type_counts.items(), key=lambda x: -x[1])),
            "indexed_count": status_counts.get("indexed", 0),
        }, ensure_ascii=False, indent=2)
    finally:
        db.close()


@mcp.tool()
def list_doc_types() -> str:
    """
    列出知识库中所有已使用的文献类型（Zotero 标准类型）。
    """
    db = _get_db()
    try:
        rows = (
            db.query(Document.doc_type)
            .filter(Document.doc_type.isnot(None), Document.doc_type != "")
            .distinct()
            .all()
        )
        types = sorted([r[0] for r in rows])
        return json.dumps({"doc_types": types}, ensure_ascii=False, indent=2)
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
