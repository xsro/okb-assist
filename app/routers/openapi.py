"""
OpenAPI Tool Server for OpenWebUI

提供标准化的工具接口，供 OpenWebUI 通过 OpenAPI 协议连接和调用。

OpenWebUI 配置:
- URL: http://your-server:8000/openapi.json (主服务)
- 或使用独立服务器: http://your-server:8001/openapi.json
"""

import json
from typing import Optional, List
from fastapi import APIRouter, Depends, HTTPException, Query
from pydantic import BaseModel, Field
from sqlalchemy.orm import Session

from app.database import get_db
from app.models import Document, DocStatus

# 使用空前缀，让路由直接挂在根路径下
# 这样 OpenWebUI 可以直接通过 /openapi.json 访问
router = APIRouter(prefix="/assist/openapi", tags=["OpenAPI Tools"])


# -------------------------------
# Pydantic Models
# -------------------------------

class SearchResult(BaseModel):
    """搜索结果项"""
    document_id: int = Field(..., description="文献ID")
    title: str = Field("", description="文献标题")
    authors: str = Field("", description="作者列表")
    year: Optional[int] = Field(None, description="发表年份")
    journal: str = Field("", description="期刊名称")
    content: str = Field("", description="匹配的文本内容")
    score: float = Field(0.0, description="相似度分数 (0-1)")


class SearchResponse(BaseModel):
    """搜索响应"""
    query: str = Field(..., description="搜索查询")
    results: List[SearchResult] = Field(default_factory=list, description="搜索结果列表")
    total: int = Field(0, description="结果总数")


class DocumentInfo(BaseModel):
    """文献基本信息"""
    id: int = Field(..., description="文献ID")
    title: str = Field("", description="文献标题")
    title_en: str = Field("", description="英文标题")
    authors: str = Field("", description="作者列表")
    authors_en: str = Field("", description="英文作者列表")
    year: Optional[int] = Field(None, description="发表年份")
    doc_type: str = Field("", description="文献类型 (article, thesis, etc.)")
    language: str = Field("", description="语言 (en, zh, etc.)")
    journal: str = Field("", description="期刊名称")
    doi: str = Field("", description="DOI")
    abstract: str = Field("", description="摘要")
    keywords: str = Field("", description="关键词")
    status: str = Field("", description="文档状态")


class DocumentListResponse(BaseModel):
    """文献列表响应"""
    items: List[DocumentInfo] = Field(default_factory=list, description="文献列表")
    total: int = Field(0, description="总数")
    page: int = Field(1, description="当前页码")
    page_size: int = Field(10, description="每页数量")


class DocumentLinks(BaseModel):
    """文献链接（路径格式，不含域名和端口）"""
    detail_page: str = Field(..., description="详情页面路径，如 /assist/detail/710")
    pdf_download: str = Field(..., description="PDF下载路径，如 /assist/api/documents/710/pdf")
    markdown_content: str = Field("", description="Markdown内容路径，如 /assist/api/documents/710/markdown")


class DocumentDetailResponse(BaseModel):
    """文献详情响应"""
    id: int = Field(..., description="文献ID")
    title: str = Field("", description="文献标题")
    title_en: str = Field("", description="英文标题")
    authors: str = Field("", description="作者列表")
    authors_en: str = Field("", description="英文作者列表")
    year: Optional[int] = Field(None, description="发表年份")
    doc_type: str = Field("", description="文献类型")
    language: str = Field("", description="语言")
    journal: str = Field("", description="期刊名称")
    journal_en: str = Field("", description="英文期刊名称")
    doi: str = Field("", description="DOI")
    source: str = Field("", description="来源")
    abstract: str = Field("", description="摘要")
    abstract_en: str = Field("", description="英文摘要")
    keywords: str = Field("", description="关键词")
    keywords_en: str = Field("", description="英文关键词")
    category: str = Field("", description="分类")
    status: str = Field("", description="文档状态")
    links: DocumentLinks = Field(..., description="相关链接")


class ErrorResponse(BaseModel):
    """错误响应"""
    error: str = Field(..., description="错误信息")
    detail: str = Field("", description="详细信息")


# -------------------------------
# Helper Functions
# -------------------------------

def _get_base_url() -> str:
    """获取基础URL（用于构建链接）"""
    # 在实际部署时，应该从配置或请求中获取
    return "http://localhost:8000"


def _parse_authors(authors_str: str) -> str:
    """解析作者列表字符串"""
    if not authors_str:
        return ""
    try:
        authors = json.loads(authors_str)
        if isinstance(authors, list):
            return ", ".join(authors)
    except (json.JSONDecodeError, TypeError):
        pass
    return authors_str


def _parse_keywords(keywords_str: str) -> str:
    """解析关键词字符串"""
    if not keywords_str:
        return ""
    try:
        keywords = json.loads(keywords_str)
        if isinstance(keywords, list):
            return ", ".join(keywords)
    except (json.JSONDecodeError, TypeError):
        pass
    return keywords_str


def _format_document(doc: Document) -> DocumentInfo:
    """格式化文献信息"""
    return DocumentInfo(
        id=doc.id,
        title=doc.title or "",
        title_en=doc.title_en or "",
        authors=_parse_authors(doc.authors),
        authors_en=_parse_authors(doc.authors_en),
        year=doc.year,
        doc_type=doc.doc_type or "",
        language=doc.language or "",
        journal=doc.journal or "",
        doi=doc.doi or "",
        abstract=(doc.abstract or "")[:500],  # 截断过长的摘要
        keywords=_parse_keywords(doc.keywords),
        status=doc.status.value if isinstance(doc.status, DocStatus) else doc.status,
    )


def _get_document_links(doc_id: int) -> DocumentLinks:
    """获取文献链接（只返回路径部分）"""
    base_url="http://192.168.1.183:5001"
    return DocumentLinks(
        detail_page=f"{base_url}/assist/detail/{doc_id}",
        pdf_download=f"{base_url}/assist/api/documents/{doc_id}/pdf",
        markdown_content=f"{base_url}/assist/api/documents/{doc_id}/markdown",
    )


# -------------------------------
# API Endpoints
# -------------------------------

@router.get(
    "/search",
    response_model=SearchResponse,
    summary="语义搜索知识库",
    description="使用语义搜索技术，在知识库中查找与查询内容最相关的文献片段。返回结果包含文献标题、作者、年份、期刊以及匹配的文本内容。",
)
async def search_knowledge_base(
    q: str = Query(..., description="搜索查询内容", min_length=1),
    limit: int = Query(5, description="返回结果数量", ge=1, le=20),
    db: Session = Depends(get_db),
):
    """语义搜索知识库"""
    from app.services.qdrant import search_similar
    from app.services.ollama import get_embedding

    # 使用与原始API相同的 user_id
    QDRANT_USER_ID = 0

    try:
        results = await search_similar(
            user_id=QDRANT_USER_ID,
            query=q,
            get_embedding_func=get_embedding,
            limit=limit,
        )
    except Exception as e:
        raise HTTPException(status_code=500, detail=f"搜索失败: {str(e)}")

    # Enrich results with document info from DB
    enriched = []
    for hit in results:
        doc_id = hit.get("document_id")
        doc_info = {}
        if doc_id:
            doc = db.query(Document).filter(Document.id == doc_id).first()
            if doc:
                doc_info = {
                    "title": doc.title or "",
                    "authors": _parse_authors(doc.authors),
                    "year": doc.year,
                    "journal": doc.journal or "",
                }
        enriched.append(SearchResult(
            document_id=doc_id or 0,
            title=doc_info.get("title", ""),
            authors=doc_info.get("authors", ""),
            year=doc_info.get("year"),
            journal=doc_info.get("journal", ""),
            content=hit.get("content", ""),
            score=hit.get("score", 0.0),
        ))

    return SearchResponse(
        query=q,
        results=enriched,
        total=len(enriched),
    )


@router.get(
    "/documents",
    response_model=DocumentListResponse,
    summary="列出文献",
    description="获取知识库中的文献列表。可以按状态过滤，支持分页。",
)
async def list_documents(
    status: Optional[str] = Query(None, description="状态过滤: uploaded/parsing/markdown_done/extracting/meta_done/indexing/indexed/error"),
    page: int = Query(1, description="页码", ge=1),
    page_size: int = Query(10, description="每页数量", ge=1, le=100),
    db: Session = Depends(get_db),
):
    """列出文献"""
    query = db.query(Document)

    # 状态过滤
    if status:
        try:
            status_enum = DocStatus(status)
            query = query.filter(Document.status == status_enum)
        except ValueError:
            raise HTTPException(status_code=400, detail=f"无效的状态值: {status}")

    # 获取总数
    total = query.count()

    # 分页
    items = query.offset((page - 1) * page_size).limit(page_size).all()

    return DocumentListResponse(
        items=[_format_document(doc) for doc in items],
        total=total,
        page=page,
        page_size=page_size,
    )


@router.get(
    "/documents/{doc_id}",
    response_model=DocumentDetailResponse,
    summary="获取文献详情",
    description="获取指定文献的完整元数据，包括标题、作者、摘要、DOI等信息，以及相关链接。",
)
async def get_document_detail(
    doc_id: int,
    db: Session = Depends(get_db),
):
    """获取文献详情"""
    doc = db.query(Document).filter(Document.id == doc_id).first()
    if not doc:
        raise HTTPException(status_code=404, detail=f"未找到ID为 {doc_id} 的文献")

    return DocumentDetailResponse(
        id=doc.id,
        title=doc.title or "",
        title_en=doc.title_en or "",
        authors=_parse_authors(doc.authors),
        authors_en=_parse_authors(doc.authors_en),
        year=doc.year,
        doc_type=doc.doc_type or "",
        language=doc.language or "",
        journal=doc.journal or "",
        journal_en=doc.journal_en or "",
        doi=doc.doi or "",
        source=doc.source or "",
        abstract=doc.abstract or "",
        abstract_en=doc.abstract_en or "",
        keywords=_parse_keywords(doc.keywords),
        keywords_en=_parse_keywords(doc.keywords_en),
        category=doc.category or "",
        status=doc.status.value if isinstance(doc.status, DocStatus) else doc.status,
        links=_get_document_links(doc_id),
    )


@router.get(
    "/documents/{doc_id}/links",
    response_model=DocumentLinks,
    summary="获取文献链接",
    description="获取指定文献的所有相关链接，包括详情页面、PDF下载和Markdown内容链接。",
)
async def get_document_links(
    doc_id: int,
    db: Session = Depends(get_db),
):
    """获取文献链接"""
    doc = db.query(Document).filter(Document.id == doc_id).first()
    if not doc:
        raise HTTPException(status_code=404, detail=f"未找到ID为 {doc_id} 的文献")

    return _get_document_links(doc_id)


@router.get(
    "/stats",
    summary="获取知识库统计信息",
    description="获取知识库的统计信息，包括文献总数、各状态数量等。",
)
async def get_stats(
    db: Session = Depends(get_db),
):
    """获取统计信息"""
    total = db.query(Document).count()

    status_counts = {}
    for status in DocStatus:
        count = db.query(Document).filter(Document.status == status).count()
        if count > 0:
            status_counts[status.value] = count

    return {
        "total_documents": total,
        "status_counts": status_counts,
        "indexed_count": status_counts.get("indexed", 0),
    }
