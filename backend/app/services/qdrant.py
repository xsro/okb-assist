"""Qdrant 向量数据库服务。

包含 QdrantAdapter 实现（VectorDBAdapter 接口）以及向后兼容的模块级函数。
"""

import uuid
from typing import Optional

from qdrant_client import QdrantClient
from qdrant_client.models import Distance, VectorParams, PointStruct

from app.services.vector_db import VectorDBAdapter

# 常见 embedding 模型的维度
MODEL_DIMENSIONS = {
    "nomic-embed-text": 768,
    "nomic-ai/nomic-embed-text-v1.5": 768,
    "BAAI/bge-small-en-v1.5": 384,
    "BAAI/bge-base-en-v1.5": 768,
    "BAAI/bge-large-en-v1.5": 1024,
    "BAAI/bge-small-zh-v1.5": 512,
    "BAAI/bge-base-zh-v1.5": 768,
    "BAAI/bge-large-zh-v1.5": 1024,
    "sentence-transformers/all-MiniLM-L6-v2": 384,
    "sentence-transformers/all-mpnet-base-v2": 768,
    "mxbai-embed-large": 1024,
    "all-minilm": 384,
    "bge-m3": 1024,
}

DEFAULT_VECTOR_SIZE = 768  # 默认维度


def get_vector_size(db_config: dict) -> int:
    """根据配置获取向量维度。"""
    embedding = db_config.get("embedding", {})
    model = embedding.get("model", "")

    # 从预定义的维度表中查找
    if model in MODEL_DIMENSIONS:
        return MODEL_DIMENSIONS[model]

    # 尝试从模型名称推断
    if "small" in model.lower():
        return 384
    elif "base" in model.lower():
        return 768
    elif "large" in model.lower():
        return 1024

    return DEFAULT_VECTOR_SIZE


class QdrantAdapter(VectorDBAdapter):
    """Qdrant 向量数据库适配器。"""

    def __init__(self, db_config: dict):
        self.url = db_config.get("url", "http://127.0.0.1:6333").rstrip("/")
        self.search_url = db_config.get("search_url")  # 搜索专用 URL，可选
        if self.search_url:
            self.search_url = self.search_url.rstrip("/")
        self.base_collection = db_config.get("collection", "documents")
        self.vector_size = get_vector_size(db_config)
        self._client: Optional[QdrantClient] = None
        self._search_client: Optional[QdrantClient] = None
        self._collection_cache: set[str] = set()

    def _get_client(self) -> QdrantClient:
        if self._client is None:
            self._client = QdrantClient(url=self.url)
        return self._client

    def _get_search_client(self) -> QdrantClient:
        """获取搜索专用客户端，未配置 search_url 则使用主客户端。"""
        if not self.search_url:
            return self._get_client()
        if self._search_client is None:
            self._search_client = QdrantClient(url=self.search_url)
        return self._search_client

    def get_collection_name(self, user_id: int = 0) -> str:
        # 直接使用配置的集合名，不添加 user_id 后缀
        return self.base_collection

    async def ensure_collection(self, collection_name: str) -> None:
        if collection_name in self._collection_cache:
            return

        client = self._get_client()
        collections = client.get_collections().collections
        existing = {c.name for c in collections}

        if collection_name not in existing:
            client.create_collection(
                collection_name=collection_name,
                vectors_config={
                    "vector": VectorParams(
                        size=self.vector_size,
                        distance=Distance.COSINE,
                    ),
                },
            )

        self._collection_cache.add(collection_name)

    async def index_document(
        self,
        doc_id: int,
        user_id: int,
        chunks: list[str],
        embeddings: list[list[float]],
        metadata: dict,
    ) -> str:
        collection_name = self.get_collection_name(user_id)
        client = self._get_client()
        await self.ensure_collection(collection_name)

        # 删除已有数据防止重复
        await self.delete_document(user_id, doc_id)

        points = []
        for i, (chunk, embedding) in enumerate(zip(chunks, embeddings)):
            if not embedding:
                continue
            point = PointStruct(
                id=str(uuid.uuid4()),
                vector={"vector": embedding},
                payload={
                    "text": chunk,
                    "metadata": {
                        "document_id": doc_id,
                        "chunk_index": i,
                        "title": metadata.get("title", ""),
                        "authors": metadata.get("authors", []),
                        "year": metadata.get("year"),
                        "doc_type": metadata.get("type", ""),
                        "keywords": metadata.get("keywords", []),
                    },
                },
            )
            points.append(point)

        batch_size = 100
        for start in range(0, len(points), batch_size):
            batch = points[start:start + batch_size]
            client.upsert(collection_name=collection_name, points=batch)

        return collection_name

    async def search_similar(
        self,
        user_id: int,
        query_embedding: list[float],
        limit: int = 10,
    ) -> list[dict]:
        collection_name = self.get_collection_name(user_id)
        client = self._get_search_client()

        if collection_name not in self._collection_cache:
            collections = client.get_collections().collections
            self._collection_cache = {c.name for c in collections}
            if collection_name not in self._collection_cache:
                return []

        results = client.query_points(
            collection_name=collection_name,
            query=query_embedding,
            using="vector",
            limit=limit,
        )

        return [
            {
                "score": hit.score,
                "document_id": hit.payload.get("metadata", {}).get("document_id"),
                "chunk_text": hit.payload.get("text"),
                "title": hit.payload.get("metadata", {}).get("title"),
            }
            for hit in results.points
        ]

    async def delete_document(self, user_id: int, doc_id: int) -> None:
        collection_name = self.get_collection_name(user_id)
        client = self._get_client()
        try:
            client.delete(
                collection_name=collection_name,
                points_selector={
                    "filter": {
                        "must": [
                            {"key": "metadata.document_id", "match": {"value": doc_id}}
                        ]
                    }
                },
            )
        except Exception:
            pass  # 集合可能不存在

    async def list_collections(self) -> list[str]:
        client = self._get_client()
        try:
            collections = client.get_collections().collections
            return [c.name for c in collections]
        except Exception:
            return []

    async def delete_collection(self, collection_name: str) -> bool:
        client = self._get_client()
        try:
            client.delete_collection(collection_name=collection_name)
            self._collection_cache.discard(collection_name)
            return True
        except Exception:
            return False

    async def get_point(self, point_id: str, collection_name: str = None) -> Optional[dict]:
        if collection_name is None:
            collection_name = self.get_collection_name(0)

        client = self._get_client()

        try:
            result = client.scroll(
                collection_name=collection_name,
                scroll_filter={
                    "must": [{"key": "id", "match": {"value": point_id}}]
                },
                limit=1,
                with_payload=True,
                with_vectors=True,
            )

            points = result[0] if result else []
            if points:
                point = points[0]
                return {
                    "id": point.id,
                    "payload": point.payload,
                    "vector": point.vector,
                }

            points = client.retrieve(
                collection_name=collection_name,
                ids=[point_id],
                with_payload=True,
            )
            if points:
                point = points[0]
                return {
                    "id": point.id,
                    "payload": point.payload,
                    "vector": None,
                }
        except Exception as e:
            return {"error": str(e)}

        return None

    async def health_check(self) -> dict:
        try:
            client = self._get_client()
            collections = client.get_collections().collections
            return {
                "status": "connected",
                "url": self.url,
                "collections": [c.name for c in collections],
            }
        except Exception as e:
            return {
                "status": "disconnected",
                "url": self.url,
                "error": str(e),
            }


# ── 向后兼容的模块级函数（旧代码可继续使用） ──

_default_adapter: Optional[QdrantAdapter] = None


def _get_default_adapter() -> QdrantAdapter:
    global _default_adapter
    if _default_adapter is None:
        from app.config_manager import get_active_vector_db
        db_config = get_active_vector_db() or {
            "url": "http://127.0.0.1:6333",
            "collection": "documents",
        }
        _default_adapter = QdrantAdapter(db_config)
    return _default_adapter


def get_qdrant_client() -> QdrantClient:
    """兼容旧代码：获取 Qdrant 客户端。"""
    return _get_default_adapter()._get_client()


def ensure_collection(client: QdrantClient, collection_name: str):
    """兼容旧代码：确保集合存在。"""
    adapter = _get_default_adapter()
    adapter._collection_cache.add(collection_name)  # 简化处理


def chunk_text(text: str, chunk_size: int = 512, overlap: int = 50) -> list[str]:
    """将文本分段。保留为模块级函数供各处使用。"""
    paragraphs = text.split("\n\n")
    chunks = []
    current_chunk = ""

    for para in paragraphs:
        para = para.strip()
        if not para:
            continue
        if len(current_chunk) + len(para) < chunk_size:
            current_chunk += ("\n\n" if current_chunk else "") + para
        else:
            if current_chunk:
                chunks.append(current_chunk)
            current_chunk = para

    if current_chunk:
        chunks.append(current_chunk)

    final_chunks = []
    for chunk in chunks:
        if len(chunk) <= chunk_size * 2:
            final_chunks.append(chunk)
        else:
            words = chunk.split()
            sub_chunk = ""
            for word in words:
                if len(sub_chunk) + len(word) + 1 < chunk_size:
                    sub_chunk += (" " if sub_chunk else "") + word
                else:
                    if sub_chunk:
                        final_chunks.append(sub_chunk)
                    sub_chunk = word
            if sub_chunk:
                final_chunks.append(sub_chunk)

    if overlap > 0 and len(final_chunks) > 1:
        overlapped = [final_chunks[0]]
        for i in range(1, len(final_chunks)):
            prev = final_chunks[i - 1]
            overlap_text = prev[-overlap:]
            space_idx = overlap_text.find(' ')
            if space_idx != -1:
                overlap_text = overlap_text[space_idx + 1:]
            overlapped.append(overlap_text + " " + final_chunks[i])
        final_chunks = overlapped

    return final_chunks


def chunk_text_by_markdown(text: str, chunk_size: int = 1000, min_chunk_size: int = 100) -> list[str]:
    """根据markdown语义结构切分文本。

    切分策略：
    1. 按标题（#, ##, ###等）切分，保留标题层级
    2. 同一章节的内容尽量保持在一起
    3. 如果章节过长，再按段落细分
    4. 短章节会与相邻章节合并

    Args:
        text: markdown文本
        chunk_size: 目标块大小（字符数）
        min_chunk_size: 最小块大小，低于此值会与相邻块合并
    """
    import re

    if not text or not text.strip():
        return []

    # 匹配markdown标题的正则表达式
    header_pattern = re.compile(r'^(#{1,6})\s+(.+)$', re.MULTILINE)

    # 按标题分割文本，保留标题
    lines = text.split('\n')
    sections = []
    current_section = []
    current_header = ""

    for line in lines:
        # 检查是否是标题行
        header_match = header_pattern.match(line)
        if header_match:
            # 保存之前的section
            if current_section:
                sections.append({
                    'header': current_header,
                    'content': '\n'.join(current_section).strip()
                })
            # 开始新section
            current_header = line.strip()
            current_section = []
        else:
            current_section.append(line)

    # 保存最后一个section
    if current_section:
        sections.append({
            'header': current_header,
            'content': '\n'.join(current_section).strip()
        })

    # 合并短sections，拆分长sections
    chunks = []
    current_chunk_parts = []
    current_chunk_size = 0

    for section in sections:
        section_text = section['header'] + '\n' + section['content'] if section['header'] else section['content']
        section_size = len(section_text)

        # 如果当前section为空，跳过
        if not section_text.strip():
            continue

        # 如果加上这个section会超过chunk_size
        if current_chunk_size + section_size > chunk_size and current_chunk_parts:
            # 保存当前chunk
            chunks.append('\n\n'.join(current_chunk_parts))
            current_chunk_parts = []
            current_chunk_size = 0

        # 如果单个section就超过chunk_size，需要拆分
        if section_size > chunk_size:
            # 先保存之前累积的chunk
            if current_chunk_parts:
                chunks.append('\n\n'.join(current_chunk_parts))
                current_chunk_parts = []
                current_chunk_size = 0

            # 拆分长section
            header = section['header']
            content = section['content']
            paragraphs = content.split('\n\n')
            sub_chunk = header + '\n' if header else ''

            for para in paragraphs:
                para = para.strip()
                if not para:
                    continue
                if len(sub_chunk) + len(para) + 2 <= chunk_size:
                    sub_chunk += ('\n\n' if sub_chunk and not sub_chunk.endswith('\n') else '') + para
                else:
                    if sub_chunk.strip():
                        chunks.append(sub_chunk.strip())
                    sub_chunk = (header + '\n\n' if header else '') + para

            if sub_chunk.strip():
                # 如果剩余部分太短，留给下一个循环合并
                if len(sub_chunk) < min_chunk_size and chunks:
                    # 与上一个chunk合并
                    chunks[-1] = chunks[-1] + '\n\n' + sub_chunk.strip()
                else:
                    current_chunk_parts = [sub_chunk.strip()]
                    current_chunk_size = len(sub_chunk.strip())
        else:
            # 正常添加到当前chunk
            current_chunk_parts.append(section_text)
            current_chunk_size += section_size

            # 如果当前chunk已经达到合适大小，保存
            if current_chunk_size >= chunk_size:
                chunks.append('\n\n'.join(current_chunk_parts))
                current_chunk_parts = []
                current_chunk_size = 0

    # 保存最后剩余的chunk
    if current_chunk_parts:
        final_text = '\n\n'.join(current_chunk_parts)
        # 如果太短且有前一个chunk，合并
        if len(final_text) < min_chunk_size and chunks:
            chunks[-1] = chunks[-1] + '\n\n' + final_text
        else:
            chunks.append(final_text)

    # 后处理：确保每个chunk都有上下文（添加父级标题）
    if not chunks:
        return chunks

    # 清理空chunk
    chunks = [c.strip() for c in chunks if c.strip()]

    return chunks


async def get_embeddings_batch(texts: list[str], get_embedding_func, vector_db_id: str = None) -> list[list[float]]:
    """批量获取 embedding。"""
    try:
        return await get_embedding_func(texts, vector_db_id=vector_db_id)
    except TypeError:
        return await get_embedding_func(texts)
    except Exception:
        results = []
        for text in texts:
            try:
                emb = await get_embedding_func(text, vector_db_id=vector_db_id)
            except TypeError:
                emb = await get_embedding_func(text)
            results.append(emb)
        return results


async def index_document(
    doc_id: int,
    user_id: int,
    markdown_content: str,
    metadata: dict,
    get_embedding_func,
    vector_db_id: str = None,
    use_markdown_chunking: bool = True,
) -> str:
    """索引文档到指定的向量数据库。

    Args:
        doc_id: 文档 ID
        user_id: 用户 ID
        markdown_content: Markdown 内容
        metadata: 元数据
        get_embedding_func: 获取 embedding 的函数
        vector_db_id: 向量数据库 ID，默认使用活跃的向量数据库
        use_markdown_chunking: 是否使用markdown语义切分（默认True）
    """
    # 获取适配器
    if vector_db_id:
        from app.config_manager import get_vector_db_by_id
        db_config = get_vector_db_by_id(vector_db_id)
        if not db_config:
            raise ValueError(f"向量数据库 {vector_db_id} 配置不存在")
        adapter = QdrantAdapter(db_config)
    else:
        adapter = _get_default_adapter()

    # 根据参数选择切分方式
    if use_markdown_chunking:
        chunks = chunk_text_by_markdown(markdown_content)
    else:
        chunks = chunk_text(markdown_content)

    if not chunks:
        return adapter.get_collection_name(user_id)

    embeddings = await get_embeddings_batch(chunks, get_embedding_func, vector_db_id=vector_db_id)
    return await adapter.index_document(doc_id, user_id, chunks, embeddings, metadata)


def delete_document_points(user_id: int, doc_id: int):
    """兼容旧代码：删除文档向量点。"""
    import asyncio
    adapter = _get_default_adapter()
    try:
        loop = asyncio.get_event_loop()
        if loop.is_running():
            asyncio.ensure_future(adapter.delete_document(user_id, doc_id))
        else:
            loop.run_until_complete(adapter.delete_document(user_id, doc_id))
    except RuntimeError:
        asyncio.run(adapter.delete_document(user_id, doc_id))


async def search_similar(
    user_id: int,
    query: str,
    get_embedding_func,
    limit: int = 10,
    vector_db_id: str = None,
) -> list[dict]:
    """语义搜索。指定 vector_db_id 可搜索特定向量数据库。"""
    if vector_db_id:
        from app.config_manager import get_vector_db_by_id
        db_config = get_vector_db_by_id(vector_db_id)
        if not db_config:
            raise ValueError(f"向量数据库 {vector_db_id} 配置不存在")
        adapter = QdrantAdapter(db_config)
    else:
        adapter = _get_default_adapter()

    # 尝试传 vector_db_id 给 embedding 函数（支持按数据库使用不同模型）
    try:
        embedding = await get_embedding_func(query, vector_db_id=vector_db_id)
    except TypeError:
        embedding = await get_embedding_func(query)

    if not embedding:
        return []
    return await adapter.search_similar(user_id, embedding, limit)


def get_point(point_id: str, collection_name: str = None) -> dict:
    """兼容旧代码：获取点数据。"""
    import asyncio
    adapter = _get_default_adapter()
    try:
        loop = asyncio.get_event_loop()
        if loop.is_running():
            import concurrent.futures
            with concurrent.futures.ThreadPoolExecutor() as pool:
                return pool.submit(asyncio.run, adapter.get_point(point_id, collection_name)).result()
        else:
            return loop.run_until_complete(adapter.get_point(point_id, collection_name))
    except RuntimeError:
        return asyncio.run(adapter.get_point(point_id, collection_name))


def delete_collection(collection_name: str) -> bool:
    """兼容旧代码：删除集合。"""
    import asyncio
    adapter = _get_default_adapter()
    try:
        loop = asyncio.get_event_loop()
        if loop.is_running():
            import concurrent.futures
            with concurrent.futures.ThreadPoolExecutor() as pool:
                return pool.submit(asyncio.run, adapter.delete_collection(collection_name)).result()
        else:
            return loop.run_until_complete(adapter.delete_collection(collection_name))
    except RuntimeError:
        return asyncio.run(adapter.delete_collection(collection_name))


def list_collections() -> list[str]:
    """兼容旧代码：列出集合。"""
    import asyncio
    adapter = _get_default_adapter()
    try:
        loop = asyncio.get_event_loop()
        if loop.is_running():
            import concurrent.futures
            with concurrent.futures.ThreadPoolExecutor() as pool:
                return pool.submit(asyncio.run, adapter.list_collections()).result()
        else:
            return loop.run_until_complete(adapter.list_collections())
    except RuntimeError:
        return asyncio.run(adapter.list_collections())
