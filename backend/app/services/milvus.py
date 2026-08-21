"""Milvus 向量数据库适配器（骨架实现）。

需要安装依赖: pip install pymilvus
"""

from typing import Optional

from app.services.vector_db import VectorDBAdapter

VECTOR_SIZE = 768  # nomic-embed-text default dimension


class MilvusAdapter(VectorDBAdapter):
    """Milvus 向量数据库适配器。"""

    def __init__(self, db_config: dict):
        self.url = db_config.get("url", "http://127.0.0.1:19530")
        self.base_collection = db_config.get("collection", "documents")
        self.api_key = db_config.get("api_key", "")
        self._client = None

    def _get_client(self):
        if self._client is None:
            try:
                from pymilvus import connections, Collection
                # 解析 host:port
                host_port = self.url.replace("http://", "").replace("https://", "").rstrip("/")
                parts = host_port.split(":")
                host = parts[0]
                port = int(parts[1]) if len(parts) > 1 else 19530

                kwargs = {"host": host, "port": port}
                if self.api_key:
                    kwargs["token"] = self.api_key
                connections.connect(**kwargs)
            except ImportError:
                raise ImportError("请安装 pymilvus: pip install pymilvus")
        return self._client

    def get_collection_name(self, user_id: int) -> str:
        return f"{self.base_collection}_{user_id}"

    async def ensure_collection(self, collection_name: str) -> None:
        raise NotImplementedError("Milvus 适配器尚未实现，请安装 pymilvus 后重试")

    async def index_document(
        self, doc_id: int, user_id: int,
        chunks: list[str], embeddings: list[list[float]], metadata: dict,
    ) -> str:
        raise NotImplementedError("Milvus 适配器尚未实现")

    async def search_similar(
        self, user_id: int, query_embedding: list[float], limit: int = 10,
    ) -> list[dict]:
        raise NotImplementedError("Milvus 适配器尚未实现")

    async def delete_document(self, user_id: int, doc_id: int) -> None:
        raise NotImplementedError("Milvus 适配器尚未实现")

    async def list_collections(self) -> list[str]:
        raise NotImplementedError("Milvus 适配器尚未实现")

    async def delete_collection(self, collection_name: str) -> bool:
        raise NotImplementedError("Milvus 适配器尚未实现")

    async def get_point(self, point_id: str, collection_name: str = None) -> Optional[dict]:
        raise NotImplementedError("Milvus 适配器尚未实现")

    async def health_check(self) -> dict:
        try:
            self._get_client()
            return {"status": "connected", "url": self.url}
        except Exception as e:
            return {"status": "disconnected", "url": self.url, "error": str(e)}
