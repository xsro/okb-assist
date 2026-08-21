"""ChromaDB 向量数据库适配器（骨架实现）。

需要安装依赖: pip install chromadb
"""

from typing import Optional

from app.services.vector_db import VectorDBAdapter


class ChromaAdapter(VectorDBAdapter):
    """ChromaDB 向量数据库适配器。"""

    def __init__(self, db_config: dict):
        self.url = db_config.get("url", "http://127.0.0.1:8000")
        self.base_collection = db_config.get("collection", "documents")
        self._client = None

    def _get_client(self):
        if self._client is None:
            try:
                import chromadb
                # ChromaDB 支持 HTTP 和持久化两种模式
                if self.url.startswith("http"):
                    self._client = chromadb.HttpClient(host=self.url.rstrip("/"))
                else:
                    self._client = chromadb.PersistentClient(path=self.url)
            except ImportError:
                raise ImportError("请安装 chromadb: pip install chromadb")
        return self._client

    def get_collection_name(self, user_id: int) -> str:
        return f"{self.base_collection}_{user_id}"

    async def ensure_collection(self, collection_name: str) -> None:
        raise NotImplementedError("ChromaDB 适配器尚未实现，请安装 chromadb 后重试")

    async def index_document(
        self, doc_id: int, user_id: int,
        chunks: list[str], embeddings: list[list[float]], metadata: dict,
    ) -> str:
        raise NotImplementedError("ChromaDB 适配器尚未实现")

    async def search_similar(
        self, user_id: int, query_embedding: list[float], limit: int = 10,
    ) -> list[dict]:
        raise NotImplementedError("ChromaDB 适配器尚未实现")

    async def delete_document(self, user_id: int, doc_id: int) -> None:
        raise NotImplementedError("ChromaDB 适配器尚未实现")

    async def list_collections(self) -> list[str]:
        raise NotImplementedError("ChromaDB 适配器尚未实现")

    async def delete_collection(self, collection_name: str) -> bool:
        raise NotImplementedError("ChromaDB 适配器尚未实现")

    async def get_point(self, point_id: str, collection_name: str = None) -> Optional[dict]:
        raise NotImplementedError("ChromaDB 适配器尚未实现")

    async def health_check(self) -> dict:
        try:
            self._get_client()
            return {"status": "connected", "url": self.url}
        except Exception as e:
            return {"status": "disconnected", "url": self.url, "error": str(e)}
