"""向量数据库抽象层。

统一接口支持 Qdrant、Milvus、Chroma 等多种向量数据库。
通过 get_vector_db() 工厂函数根据配置返回对应 adapter 实例。
"""

from abc import ABC, abstractmethod
from typing import Optional

from app.config_manager import get_active_vector_db, get_vector_db_by_id


class VectorDBAdapter(ABC):
    """向量数据库统一接口。"""

    @abstractmethod
    async def ensure_collection(self, collection_name: str) -> None:
        """确保集合存在，不存在则创建。"""
        ...

    @abstractmethod
    async def index_document(
        self,
        doc_id: int,
        user_id: int,
        chunks: list[str],
        embeddings: list[list[float]],
        metadata: dict,
    ) -> str:
        """索引文档到向量数据库。返回集合名称。"""
        ...

    @abstractmethod
    async def search_similar(
        self,
        user_id: int,
        query_embedding: list[float],
        limit: int = 10,
    ) -> list[dict]:
        """语义搜索，返回相似文档片段列表。"""
        ...

    @abstractmethod
    async def delete_document(self, user_id: int, doc_id: int) -> None:
        """删除指定文档的所有向量点。"""
        ...

    @abstractmethod
    async def list_collections(self) -> list[str]:
        """列出所有集合。"""
        ...

    @abstractmethod
    async def delete_collection(self, collection_name: str) -> bool:
        """删除集合。返回是否成功。"""
        ...

    @abstractmethod
    async def get_point(self, point_id: str, collection_name: str = None) -> Optional[dict]:
        """根据 ID 获取向量点。"""
        ...

    @abstractmethod
    async def health_check(self) -> dict:
        """健康检查，返回状态信息。"""
        ...

    @abstractmethod
    def get_collection_name(self, user_id: int) -> str:
        """根据 user_id 生成集合名称。"""
        ...


def get_vector_db(db_id: str = None) -> VectorDBAdapter:
    """根据配置返回对应的 adapter 实例。

    Args:
        db_id: 向量数据库配置 ID。为 None 时使用第一个 enabled 的配置。
    """
    if db_id:
        db_config = get_vector_db_by_id(db_id)
    else:
        db_config = get_active_vector_db()

    if not db_config:
        raise ValueError("没有可用的向量数据库配置，请在配置页面添加")

    db_type = db_config.get("type", "qdrant")

    if db_type == "qdrant":
        from app.services.qdrant import QdrantAdapter
        return QdrantAdapter(db_config)
    elif db_type == "milvus":
        from app.services.milvus import MilvusAdapter
        return MilvusAdapter(db_config)
    elif db_type == "chroma":
        from app.services.chroma import ChromaAdapter
        return ChromaAdapter(db_config)
    else:
        raise ValueError(f"不支持的向量数据库类型: {db_type}")
