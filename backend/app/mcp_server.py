"""
OKB-Assist MCP Server

Exposes document management tools via Model Context Protocol (MCP)
for use by AI assistants like Claude Desktop, Cursor, etc.

Usage:
  stdio mode:  python -m app.mcp_server
  SSE mode:    mounted at /assist/mcp in main.py  → /assist/mcp/sse
  HTTP mode:   served at /assist/mcp/stream in main.py (exact Route)
"""

import os
import json
import ipaddress
from contextlib import asynccontextmanager
from typing import Optional

from mcp.server.fastmcp import FastMCP
from mcp.server.transport_security import TransportSecuritySettings
from mcp.server.auth.settings import AuthSettings
from mcp.server.auth.provider import AccessToken
from pydantic import AnyHttpUrl
from starlette.applications import Starlette
from starlette.routing import Mount, Route
from starlette.responses import Response

from app.config import get_settings
from app.database import SessionLocal
from app.models import Document, DocStatus
from app.paths import get_markdown_path, get_pdf_path

settings = get_settings()
MCP_MOUNT_PATH = "/assist/mcp"


# ── Bearer Token Authentication ──────────────────────────────────────────────

class StaticTokenVerifier:
    """Verifies Bearer Token using mcp_token from system.json."""

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


# ── LAN Whitelist: 192.168.1.0/24 Token-Free Access ────────────────────────────
# When FastMCP builds the Streamable HTTP / SSE app, it uses the module-level
# symbol `mcp.server.fastmcp.server.BearerAuthBackend` to verify Bearer Token.
# We prepend a subnet check before its `authenticate` (where Token verification
# happens): requests from 192.168.1.0/24 are treated as authenticated,
# bypassing Token verification; other IPs follow the original logic.
# By replacing this module-level symbol, FastMCP automatically uses the
# whitelisted backend when building the app.

from mcp.server.auth.middleware.bearer_auth import (
    BearerAuthBackend,
    AuthenticatedUser,
)
from starlette.authentication import AuthCredentials

_LAN_NETWORK = ipaddress.ip_network("192.168.1.0/24")


def _ip_in_lan(client_ip: str | None) -> bool:
    """Check if the client IP is in 192.168.1.0/24; invalid IPs fall back to Token verification."""
    if not client_ip:
        return False
    try:
        return ipaddress.ip_address(client_ip) in _LAN_NETWORK
    except ValueError:
        return False


class LanBypassBearerAuthBackend(BearerAuthBackend):
    """Allow 192.168.1.0/24 LAN requests before Bearer Token verification."""

    async def authenticate(self, conn):
        client_ip = conn.client.host if conn.client else None
        if _ip_in_lan(client_ip):
            # Treated as authenticated: reuse the configured token to generate
            # a valid AccessToken, keeping the subsequent auth path identical to
            # carrying a valid Token.
            access_token = await self.token_verifier.verify_token(self.token_verifier.token)
            return AuthCredentials(access_token.scopes), AuthenticatedUser(access_token)
        return await super().authenticate(conn)


# Make FastMCP use the whitelisted backend when building the app
# (only affects FastMCP instances in this process).
import mcp.server.fastmcp.server as _fastmcp_server

_fastmcp_server.BearerAuthBackend = LanBypassBearerAuthBackend


_token = settings.mcp_token
_auth_enabled = bool(_token) and _token != "change-me"

_mcp_kwargs: dict = {
    "instructions": (
        "OKB-Assist provides MCP tools for a local academic document library. "
        "Use grep_search for full-text keyword/regex search, list_documents "
        "to browse records, read_markdown to read parsed document text, "
        "and get_document_info/get_stats for metadata and library status."
    ),
    # The Streamable HTTP endpoint is mounted at the EXACT path
    # /assist/mcp/stream (no trailing slash) in okb_assist_main.py via an exact
    # Route. We set streamable_http_path to that same full path so the internal
    # route matches the scope path the parent app hands over (a Route does not
    # strip the mount prefix the way Mount would). This lets MCP clients connect
    # to /assist/mcp/stream directly without a 307 redirect.
    "streamable_http_path": "/assist/mcp/stream",
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


def create_streamable_app() -> Starlette:
    """Standalone Streamable HTTP app, served at the exact path /assist/mcp/stream.

    In okb_assist_main.py this sub-app is registered with
    `app.add_route("/assist/mcp/stream", create_streamable_app(), ...)` — an
    EXACT Route, not a Mount. A Mount would require a trailing slash
    (/assist/mcp/stream/) and, worse, the legacy SSE Mount at /assist/mcp would
    capture /assist/mcp/stream before the Streamable endpoint could. Registering
    it as an exact Route lets MCP clients connect to /assist/mcp/stream directly
    without a 307 redirect (which a POST `initialize` cannot follow).

    This sub-app already carries its own auth middleware stack
    (AuthenticationMiddleware + AuthContextMiddleware), so Bearer-token
    validation works without touching the parent app's middleware.

    The session manager's lifespan is removed here on purpose: it is started
    exactly once by the parent app's lifespan (okb_assist_main.py ->
    mcp_session_manager.run()). Leaving it on this sub-app would call
    session_manager.run() a second time, which raises RuntimeError
    ("can only be called once per instance").
    """
    app = mcp.streamable_http_app()

    @asynccontextmanager
    async def _noop(_app):
        yield

    app.router.lifespan_context = _noop
    return app


def create_sse_app() -> Starlette:
    """Standalone legacy SSE app, mounted at /assist/mcp.

    This yields the SAME URLs as before the split:
      - GET  /assist/mcp/sse          (SSE stream)
      - POST /assist/mcp/messages/    (client message POST; the exact URL is
                                      sent to the client via the `endpoint`
                                      SSE event, so clients derive it
                                      automatically and need no config change)

    Keeping the two transports in separate mounts (instead of merging their
    routes into one Starlette) prevents a client that talks to both endpoints
    from assuming the Streamable HTTP and SSE sessions are shared.
    """
    return mcp.sse_app(mount_path="")


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

    # Check if the markdown file exists (path derived from system.json)
    abs_markdown_path = get_markdown_path(doc.id)
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
async def grep_search(
    query: str,
    limit: int = 10,
    context: int = 2,
    doc_ids: str = "",
    algorithm: str = "full",
    regex: bool = True,
    journal: str = "",
    year_start: int = 0,
    year_end: int = 0,
) -> str:
    """
    Full-text search of document content (grep-based, lightweight and fast).
    No vector database required, supports regex.

    Metadata pre-filtering by journal name and year range, then grep search
    on candidate documents.

    Args:
        query: Search keywords (supports regex)
        limit: Number of results to return, default 10
        context: Number of context lines before and after the match, default 2
        doc_ids: Document ID scope, supports commas and ranges (e.g., "1,2,5-100,4"), empty means search all
        algorithm: Search algorithm, "full"=full scan (original), "fast"=metadata pre-filter candidates (doc_ids must be unspecified)
        regex: Whether to match as regex, True for regex (default), False for literal match
        journal: Fuzzy match on journal name (optional, empty means no restriction)
        year_start: Starting year inclusive (optional, 0 means no restriction)
        year_end: Ending year inclusive (optional, 0 means no restriction)
    """
    if not query.strip():
        return json.dumps({"error": "查询不能为空"}, ensure_ascii=False)

    from app.services.grep_search import grep_search as do_grep
    from app.services.grep_search import parse_doc_ids

    # Parse doc_ids (supports commas and ranges, e.g., 1,2,5-100)
    id_list = None
    if doc_ids and doc_ids.strip():
        try:
            id_list = parse_doc_ids(doc_ids)
        except ValueError:
            return json.dumps({"error": "doc_ids 格式无效，支持逗号与区间，如 1,2,5-100"}, ensure_ascii=False)

    # year_start/year_end of 0 means unspecified, convert to None
    ys = year_start if year_start > 0 else None
    ye = year_end if year_end > 0 else None

    # Pass through algorithm / regex / journal / year params: default full + regex, consistent with original behavior
    db = _get_db()
    results = await do_grep(
        query=query,
        context_lines=context,
        limit=limit,
        doc_ids=id_list,
        algorithm=algorithm,
        regex=regex,
        db=db,
        journal=journal or None,
        year_start=ys,
        year_end=ye,
    )

    # Reuse the db session obtained above (avoid duplicate creation causing session leak in fast mode)
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
    Search document metadata (title, authors, journal, keywords, abstract, DOI, etc.).
    Returns complete info for matching documents, suitable for finding documents
    by author, title, journal, etc.

    Args:
        query: Search keywords (fuzzy match on title, authors, journal, keywords, abstract, DOI, etc.)
        limit: Number of results to return, default 10
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
def read_markdown(doc_id: int, page: int = 1, page_size: int = 5000) -> str:
    """
    Read document Markdown content (paginated). Markdown is the text format parsed
    from PDF, containing formulas, tables, etc.

    Args:
        doc_id: Document ID
        page: Page number, starting from 1
        page_size: Characters per page, default 5000
    """
    db = _get_db()
    try:
        doc = db.query(Document).filter(Document.id == doc_id).first()
        if not doc:
            return json.dumps({"error": f"文档 {doc_id} 不存在"}, ensure_ascii=False)

        # Markdown path is derived from system.json
        abs_markdown_path = get_markdown_path(doc_id)
        if not os.path.exists(abs_markdown_path):
            return json.dumps({"error": "Markdown file not yet generated, please parse the PDF first"}, ensure_ascii=False)

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
    Get detailed document information, including metadata, processing status,
    PDF and Markdown links.

    Args:
        doc_id: Document ID
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
    Search or list documents. Supports search by title/author, filter by status
    and document type.

    Args:
        query: Search keywords (optional, matches title, authors, filename)
        status: Status filter (optional, comma-separated: uploaded/parsing/markdown_done/extracting/meta_done/indexing/indexed/error)
        doc_type: Document type filter (optional, comma-separated, Zotero types: journalArticle/book/conferencePaper/thesis/report/preprint/bookSection etc.)
        page: Page number
        page_size: Items per page
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
    Get the PDF download/preview link for a document.

    Args:
        doc_id: Document ID
    """
    db = _get_db()
    try:
        doc = db.query(Document).filter(Document.id == doc_id).first()
        if not doc:
            return json.dumps({"error": f"文档 {doc_id} 不存在"}, ensure_ascii=False)

        # Source PDF path is derived from system.json
        abs_file_path = get_pdf_path(doc_id)
        if not os.path.exists(abs_file_path):
            return json.dumps({"error": "PDF file does not exist"}, ensure_ascii=False)

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
    Get document abstract information. If there are multi-language abstracts,
    both will be returned.

    Args:
        doc_id: Document ID
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
    Get knowledge base statistics, including total document count, counts by
    status, and counts by type.
    """
    db = _get_db()
    try:
        total = db.query(Document).count()

        # Count by status
        status_counts = {}
        for s in DocStatus:
            count = db.query(Document).filter(Document.status == s).count()
            if count > 0:
                status_counts[s.value] = count

        # Count by type
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
    List all document types used in the knowledge base (Zotero standard types).
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
    """Get document detail resource"""
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
    """Get document Markdown content resource"""
    db = _get_db()
    try:
        doc = db.query(Document).filter(Document.id == doc_id).first()
        if not doc:
            return f"Error: Document {doc_id} does not exist"

        # Markdown path is derived from system.json
        abs_markdown_path = get_markdown_path(doc_id)
        if not os.path.exists(abs_markdown_path):
            return "Error: Markdown file not yet generated"

        with open(abs_markdown_path, "r", encoding="utf-8") as f:
            return f.read()
    finally:
        db.close()


# ─── Run ─────────────────────────────────────────────────────────────────────

if __name__ == "__main__":
    mcp.run(transport="stdio")
