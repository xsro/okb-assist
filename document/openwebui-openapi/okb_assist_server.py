#!/usr/bin/env python3
"""
OKB-Assist OpenAPI Tool Server

独立的 OpenAPI 工具服务器，可以单独部署用于 OpenWebUI 连接。
也可以直接使用 OKB-Assist 主服务的 /assist/openapi/ 端点。

使用方法：
    # 方式1：直接使用 OKB-Assist 主服务
    # 访问 http://your-server:8000/openapi.json

    # 方式2：单独部署此服务器
    pip install fastapi uvicorn requests
    python okb_assist_server.py --port 8001

    # 在 OpenWebUI 中配置
    # URL: http://localhost:8001/openapi.json
"""

import os
import json
import argparse
import requests
from typing import Optional, List
from fastapi import FastAPI, HTTPException, Query
from fastapi.middleware.cors import CORSMiddleware
from pydantic import BaseModel, Field

# -------------------------------
# Configuration
# -------------------------------

OKB_ASSIST_URL = os.getenv("OKB_ASSIST_URL", "http://localhost:8000")
OKB_ASSIST_TOKEN = os.getenv("OKB_ASSIST_TOKEN", "")

# -------------------------------
# FastAPI App
# -------------------------------

app = FastAPI(
    title="OKB-Assist OpenAPI Tool Server",
    version="1.0.0",
    description="OpenAPI Tool Server for connecting OpenWebUI to OKB-Assist knowledge base.",
)

# CORS
app.add_middleware(
    CORSMiddleware,
    allow_origins=["*"],
    allow_credentials=True,
    allow_methods=["*"],
    allow_headers=["*"],
)

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
    year: Optional[int] = Field(None, description="发表年份")
    doc_type: str = Field("", description="文献类型")
    journal: str = Field("", description="期刊名称")
    doi: str = Field("", description="DOI")
    abstract: str = Field("", description="摘要")
    status: str = Field("", description="文档状态")


class DocumentListResponse(BaseModel):
    """文献列表响应"""
    items: List[DocumentInfo] = Field(default_factory=list, description="文献列表")
    total: int = Field(0, description="总数")


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
    abstract: str = Field("", description="摘要")
    abstract_en: str = Field("", description="英文摘要")
    keywords: str = Field("", description="关键词")
    keywords_en: str = Field("", description="英文关键词")
    status: str = Field("", description="文档状态")
    detail_page: str = Field("", description="详情页面路径")
    pdf_download: str = Field("", description="PDF下载路径")
    markdown_content: str = Field("", description="Markdown内容路径")


# -------------------------------
# Helper Functions
# -------------------------------

def _get_headers() -> dict:
    """获取请求头"""
    headers = {"Content-Type": "application/json"}
    if OKB_ASSIST_TOKEN:
        headers["X-Token"] = OKB_ASSIST_TOKEN
    return headers


def _get_base_url() -> str:
    """获取基础URL"""
    return OKB_ASSIST_URL.rstrip("/")


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


# -------------------------------
# API Endpoints
# -------------------------------

@app.get(
    "/search",
    response_model=SearchResponse,
    summary="语义搜索知识库",
    description="使用语义搜索技术，在知识库中查找与查询内容最相关的文献片段。",
)
async def search_knowledge_base(
    q: str = Query(..., description="搜索查询内容", min_length=1),
    limit: int = Query(5, description="返回结果数量", ge=1, le=20),
):
    """语义搜索知识库"""
    try:
        response = requests.get(
            f"{_get_base_url()}/assist/api/documents/search",
            params={"q": q, "limit": limit},
            headers=_get_headers(),
            timeout=30,
        )
        response.raise_for_status()
        data = response.json()
    except requests.exceptions.ConnectionError:
        raise HTTPException(status_code=502, detail="无法连接到 OKB-Assist 服务")
    except requests.exceptions.Timeout:
        raise HTTPException(status_code=504, detail="请求超时")
    except requests.exceptions.RequestException as e:
        raise HTTPException(status_code=500, detail=f"请求失败: {str(e)}")

    results = []
    for hit in data.get("results", []):
        results.append(SearchResult(
            document_id=hit.get("document_id", 0),
            title=hit.get("title", ""),
            authors=hit.get("authors", ""),
            year=hit.get("year"),
            journal=hit.get("journal", ""),
            content=hit.get("content", ""),
            score=hit.get("score", 0.0),
        ))

    return SearchResponse(
        query=q,
        results=results,
        total=len(results),
    )


@app.get(
    "/documents",
    response_model=DocumentListResponse,
    summary="列出文献",
    description="获取知识库中的文献列表。可以按状态过滤，支持分页。",
)
async def list_documents(
    status: Optional[str] = Query(None, description="状态过滤: indexed/meta_done/markdown_done/error"),
    page: int = Query(1, description="页码", ge=1),
    page_size: int = Query(10, description="每页数量", ge=1, le=100),
):
    """列出文献"""
    params = {
        "page": page,
        "page_size": page_size,
    }
    if status:
        params["status_filter"] = status

    try:
        response = requests.get(
            f"{_get_base_url()}/assist/api/documents/",
            params=params,
            headers=_get_headers(),
            timeout=30,
        )
        response.raise_for_status()
        data = response.json()
    except requests.exceptions.ConnectionError:
        raise HTTPException(status_code=502, detail="无法连接到 OKB-Assist 服务")
    except requests.exceptions.Timeout:
        raise HTTPException(status_code=504, detail="请求超时")
    except requests.exceptions.RequestException as e:
        raise HTTPException(status_code=500, detail=f"请求失败: {str(e)}")

    items = []
    for doc in data.get("items", []):
        items.append(DocumentInfo(
            id=doc.get("id", 0),
            title=doc.get("title", ""),
            title_en=doc.get("title_en", ""),
            authors=_parse_authors(doc.get("authors", "")),
            year=doc.get("year"),
            doc_type=doc.get("doc_type", ""),
            journal=doc.get("journal", ""),
            doi=doc.get("doi", ""),
            abstract=(doc.get("abstract", "") or "")[:500],
            status=doc.get("status", ""),
        ))

    return DocumentListResponse(
        items=items,
        total=data.get("total", 0),
    )


@app.get(
    "/documents/{doc_id}",
    response_model=DocumentDetailResponse,
    summary="获取文献详情",
    description="获取指定文献的完整元数据，包括标题、作者、摘要、DOI等信息，以及相关链接。",
)
async def get_document_detail(doc_id: int):
    """获取文献详情"""
    try:
        response = requests.get(
            f"{_get_base_url()}/assist/api/documents/{doc_id}",
            headers=_get_headers(),
            timeout=30,
        )
        response.raise_for_status()
        doc = response.json()
    except requests.exceptions.ConnectionError:
        raise HTTPException(status_code=502, detail="无法连接到 OKB-Assist 服务")
    except requests.exceptions.Timeout:
        raise HTTPException(status_code=504, detail="请求超时")
    except requests.exceptions.RequestException as e:
        if "404" in str(e):
            raise HTTPException(status_code=404, detail=f"未找到ID为 {doc_id} 的文献")
        raise HTTPException(status_code=500, detail=f"请求失败: {str(e)}")

    base_url = _get_base_url()

    return DocumentDetailResponse(
        id=doc.get("id", 0),
        title=doc.get("title", ""),
        title_en=doc.get("title_en", ""),
        authors=_parse_authors(doc.get("authors", "")),
        authors_en=_parse_authors(doc.get("authors_en", "")),
        year=doc.get("year"),
        doc_type=doc.get("doc_type", ""),
        language=doc.get("language", ""),
        journal=doc.get("journal", ""),
        journal_en=doc.get("journal_en", ""),
        doi=doc.get("doi", ""),
        abstract=doc.get("abstract", ""),
        abstract_en=doc.get("abstract_en", ""),
        keywords=_parse_keywords(doc.get("keywords", "")),
        keywords_en=_parse_keywords(doc.get("keywords_en", "")),
        status=doc.get("status", ""),
        detail_page=f"{base_url}/redirect/{doc_id}",
        pdf_download=f"{base_url}/assist/api/documents/{doc_id}/pdf" if doc.get("file_path") else "",
        markdown_content=f"{base_url}/assist/api/documents/{doc_id}/markdown" if doc.get("markdown_path") else "",
    )


@app.get(
    "/stats",
    summary="获取知识库统计信息",
    description="获取知识库的统计信息，包括文献总数、各状态数量等。",
)
async def get_stats():
    """获取统计信息"""
    try:
        response = requests.get(
            f"{_get_base_url()}/assist/api/admin/stats",
            headers=_get_headers(),
            timeout=30,
        )
        response.raise_for_status()
        data = response.json()
    except requests.exceptions.ConnectionError:
        raise HTTPException(status_code=502, detail="无法连接到 OKB-Assist 服务")
    except requests.exceptions.Timeout:
        raise HTTPException(status_code=504, detail="请求超时")
    except requests.exceptions.RequestException as e:
        raise HTTPException(status_code=500, detail=f"请求失败: {str(e)}")

    return {
        "total_documents": data.get("documents", 0),
        "status_counts": data.get("status_counts", {}),
        "indexed_count": data.get("status_counts", {}).get("indexed", 0),
    }


# -------------------------------
# Main
# -------------------------------

if __name__ == "__main__":
    import uvicorn

    parser = argparse.ArgumentParser(description="OKB-Assist OpenAPI Tool Server")
    parser.add_argument("--host", default="0.0.0.0", help="Host to bind")
    parser.add_argument("--port", type=int, default=8001, help="Port to bind")
    parser.add_argument("--okb-url", default=None, help="OKB-Assist service URL")
    parser.add_argument("--okb-token", default=None, help="OKB-Assist access token")

    args = parser.parse_args()

    if args.okb_url:
        OKB_ASSIST_URL = args.okb_url
    if args.okb_token:
        OKB_ASSIST_TOKEN = args.okb_token

    print(f"Starting OKB-Assist OpenAPI Tool Server on {args.host}:{args.port}")
    print(f"OKB-Assist URL: {OKB_ASSIST_URL}")
    print(f"OpenAPI Schema: http://{args.host}:{args.port}/openapi.json")

    uvicorn.run(app, host=args.host, port=args.port)
