"""
OpenAPI Tool Server for OpenWebUI

提供标准化的工具接口，供 OpenWebUI 通过 OpenAPI 协议连接和调用。

OpenWebUI 配置:
- URL: http://your-server:8000/openapi.json (主服务)
- 或使用独立服务器: http://your-server:8001/openapi.json
"""

import json
import os
from typing import Optional, List
from fastapi import APIRouter, Depends, HTTPException, Query
from pydantic import BaseModel, Field
from sqlalchemy.orm import Session

from app.database import get_db
from app.models import Document, DocStatus
from app.paths import get_markdown_path, get_pdf_path

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
    doc_type: str = Field("", description="Zotero 文献类型 (journalArticle, book, conferencePaper, thesis, report, preprint, bookSection, etc.)")
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


class DocumentDetailResponse(BaseModel):
    """文献详情响应"""
    id: int = Field(..., description="文献ID")
    title: str = Field("", description="文献标题")
    title_en: str = Field("", description="英文标题")
    authors: str = Field("", description="作者列表")
    authors_en: str = Field("", description="英文作者列表")
    year: Optional[int] = Field(None, description="发表年份")
    doc_type: str = Field("", description="Zotero 文献类型")
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
    detail_page: str = Field("", description="详情页面路径")
    pdf_download: str = Field("", description="PDF下载路径")
    markdown_content: str = Field("", description="Markdown内容路径")


class ErrorResponse(BaseModel):
    """错误响应"""
    error: str = Field(..., description="错误信息")
    detail: str = Field("", description="详细信息")


# -------------------------------
# Helper Functions
# -------------------------------

def _get_base_url() -> str:
    """获取基础URL（用于构建链接）"""
    from app.config import get_settings
    return get_settings().public_url


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



# -------------------------------
# API Endpoints
# -------------------------------

@router.get(
    "/grep-search",
    response_model=SearchResponse,
    summary="全文搜索知识库（轻量）",
    description="使用系统 grep 在知识库中进行全文搜索。无需向量数据库和 embedding 模型，适用于嵌入式设备。返回匹配的文本片段及文献信息。",
)
async def grep_search_knowledge_base(
    q: str = Query(..., description="搜索关键词（支持正则表达式）", min_length=1),
    limit: int = Query(10, description="返回结果数量", ge=1, le=50),
    context: int = Query(2, description="匹配行前后的上下文行数", ge=0, le=10),
    doc_ids: Optional[str] = Query(None, description="文档 ID 限定范围，支持逗号与区间（如 '1,2,5-100'），留空搜索全部"),
    algorithm: str = Query("full", description="搜索算法：full=全量扫描(原), fast=元数据预筛候选"),
    regex: bool = Query(True, description="是否按正则匹配（False 时按字面量匹配）"),
    db: Session = Depends(get_db),
):
    """全文搜索知识库（基于 grep，轻量版）。支持通过 doc_ids 限定搜索范围。
    algorithm 可选 full（全量扫描）/ fast（元数据预筛）；regex 控制是否正则匹配。"""
    from app.services.grep_search import grep_search as do_grep
    from app.services.grep_search import parse_doc_ids

    # 解析 doc_ids（支持逗号与区间，如 1,2,5-100）
    try:
        id_list = parse_doc_ids(doc_ids)
    except ValueError:
        raise HTTPException(status_code=400, detail="doc_ids 格式无效，支持逗号与区间，如 1,2,5-100")

    try:
        # 透传 algorithm / regex 参数：默认 full + 正则，与原行为一致
        results = await do_grep(
            query=q,
            context_lines=context,
            limit=limit,
            doc_ids=id_list,
            algorithm=algorithm,
            regex=regex,
            db=db,
        )
    except Exception as e:
        raise HTTPException(status_code=500, detail=f"搜索失败: {str(e)}")

    # 补充文档元数据
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
            score=0.0,  # grep 没有相似度分数
        ))

    return SearchResponse(
        query=q,
        results=enriched,
        total=len(enriched),
    )


@router.get(
    "/search",
    response_model=SearchResponse,
    summary="语义搜索知识库",
    description="使用语义搜索技术，在知识库中查找与查询内容最相关的文献片段。返回结果包含文献标题、作者、年份、期刊以及匹配的文本内容。",
)
async def search_knowledge_base(
    q: str = Query(..., description="搜索查询内容", min_length=1),
    limit: int = Query(5, description="返回结果数量", ge=1, le=20),
    vector_db_id: Optional[str] = Query(None, description="向量数据库 ID，不填则使用默认"),
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
            vector_db_id=vector_db_id,
        )
    except ValueError as e:
        raise HTTPException(status_code=400, detail=str(e))
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

    base_url = _get_base_url()

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
        detail_page=f"{base_url}/redirect/{doc_id}",
        pdf_download=f"{base_url}/assist/api/documents/{doc_id}/pdf" if os.path.exists(get_pdf_path(doc_id)) else "",
        markdown_content=f"{base_url}/assist/api/documents/{doc_id}/markdown" if os.path.exists(get_markdown_path(doc_id)) else "",
    )


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


@router.get(
    "/search-info",
    summary="搜索文献元数据",
    description="按标题、作者、期刊、关键词、摘要、DOI 等字段模糊搜索，返回匹配文献的完整信息。",
)
async def search_document_info(
    q: str = Query(..., description="搜索关键词", min_length=1),
    limit: int = Query(10, description="返回结果数量", ge=1, le=50),
    db: Session = Depends(get_db),
):
    """搜索文献元数据（标题、作者、期刊等）"""
    like = f"%{q}%"
    query = db.query(Document).filter(
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

    docs = query.order_by(Document.updated_at.desc()).limit(limit).all()

    return {
        "query": q,
        "total": len(docs),
        "results": [_format_document(doc) for doc in docs],
    }


@router.get(
    "/documents/{doc_id}/markdown",
    summary="读取文献 Markdown 内容",
    description="分页读取文献的 Markdown 内容。使用 full=true 可获取全文。",
)
async def read_markdown(
    doc_id: int,
    page: int = Query(1, description="页码", ge=1),
    page_size: int = Query(5000, description="每页字符数", ge=100, le=100000),
    full: bool = Query(False, description="true 时返回全文，忽略分页参数"),
    db: Session = Depends(get_db),
):
    """读取文献 Markdown 内容"""
    doc = db.query(Document).filter(Document.id == doc_id).first()
    if not doc:
        raise HTTPException(status_code=404, detail=f"未找到 ID 为 {doc_id} 的文献")
    abs_path = get_markdown_path(doc_id)
    if not os.path.exists(abs_path):
        raise HTTPException(status_code=404, detail="该文献尚未生成 Markdown")

    with open(abs_path, "r", encoding="utf-8") as f:
        content = f.read()

    if full:
        return {
            "doc_id": doc_id,
            "content": content,
            "total_length": len(content),
            "page": 1,
            "total_pages": 1,
        }

    # 分页
    from app.routers.documents import _split_into_pages
    pages = _split_into_pages(content, page_size)
    total_pages = len(pages)

    if page < 1 or page > total_pages:
        page = 1

    return {
        "doc_id": doc_id,
        "content": pages[page - 1] if pages else "",
        "total_length": len(content),
        "page": page,
        "total_pages": total_pages,
    }


@router.get(
    "/documents/{doc_id}/abstract",
    summary="获取文献摘要",
    description="获取指定文献的摘要信息，包含原文摘要和英文摘要（如有）。",
)
async def get_abstract(
    doc_id: int,
    db: Session = Depends(get_db),
):
    """获取文献摘要"""
    doc = db.query(Document).filter(Document.id == doc_id).first()
    if not doc:
        raise HTTPException(status_code=404, detail=f"未找到 ID 为 {doc_id} 的文献")

    return {
        "doc_id": doc_id,
        "title": doc.title or "",
        "abstract": doc.abstract or "",
        "abstract_en": doc.abstract_en or "",
    }


@router.get(
    "/documents/{doc_id}/pdf-url",
    summary="获取 PDF 链接",
    description="获取指定文献的 PDF 下载链接。",
)
async def get_pdf_url(
    doc_id: int,
    db: Session = Depends(get_db),
):
    """获取 PDF 链接"""
    doc = db.query(Document).filter(Document.id == doc_id).first()
    if not doc:
        raise HTTPException(status_code=404, detail=f"未找到 ID 为 {doc_id} 的文献")

    base_url = _get_base_url()
    return {
        "doc_id": doc_id,
        "title": doc.title or "",
        "pdf_url": f"{base_url}/assist/api/documents/{doc_id}/pdf" if os.path.exists(get_pdf_path(doc_id)) else "",
        "detail_page": f"{base_url}/redirect/{doc_id}",
    }


@router.get(
    "/doc-types",
    summary="列出文献类型",
    description="列出知识库中所有已使用的文献类型及其数量。",
)
async def list_doc_types(
    db: Session = Depends(get_db),
):
    """列出所有文献类型"""
    from sqlalchemy import func

    results = (
        db.query(Document.doc_type, func.count(Document.id))
        .filter(Document.doc_type.isnot(None), Document.doc_type != "")
        .group_by(Document.doc_type)
        .order_by(func.count(Document.id).desc())
        .all()
    )

    return {
        "types": [
            {"doc_type": doc_type, "count": count}
            for doc_type, count in results
        ],
        "total_types": len(results),
    }
