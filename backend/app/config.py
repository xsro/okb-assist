"""配置模块 — 桥接层。

所有属性动态从 config_manager 读取，保证运行时修改即时生效。
原有 `from app.config import get_settings` 的调用方无需改动。
"""

from app.config_manager import get_config, get_system_config, get_active_vector_db


class Settings:
    """动态配置代理，属性从 JSON 配置实时读取。"""

    # ── MinerU ──
    @property
    def mineru_url(self) -> str:
        return get_config()["mineru"]["url"].rstrip("/")

    @property
    def mineru_key(self) -> str:
        return get_config()["mineru"]["key"]

    @property
    def mineru_tasks(self) -> int:
        return get_config()["mineru"].get("max_tasks", 3)

    @property
    def mineru_task_timeout(self) -> int:
        """MinerU 任务轮询超时时间（秒）。"""
        return get_config()["mineru"].get("task_timeout", 300)

    # ── Ollama ──
    @property
    def ollama_url(self) -> str:
        return get_config()["ollama"]["url"]

    @property
    def ollama_key(self) -> str:
        return get_config()["ollama"].get("key", "")

    @property
    def ollama_model(self) -> str:
        return get_config()["ollama"]["model"]

    # ── FastEmbed ──
    @property
    def fastembed_url(self) -> str:
        return get_config().get("fastembed", {}).get("url", "http://127.0.0.1:8003")

    # ── Embedding 配置（从活跃向量数据库配置中获取） ──
    @property
    def embedding_source(self) -> str:
        """获取 embedding 来源: 'ollama' 或 'fastembed'。"""
        db = get_active_vector_db()
        if db and "embedding" in db:
            return db["embedding"].get("source", "ollama")
        return "ollama"

    @property
    def embedding_model(self) -> str:
        """获取 embedding 模型名称。"""
        db = get_active_vector_db()
        if db and "embedding" in db:
            return db["embedding"].get("model", "nomic-embed-text")
        return "nomic-embed-text"

    # ── Qdrant（兼容旧代码，默认取第一个 enabled 的 qdrant 类型） ──
    @property
    def qdrant_url(self) -> str:
        db = get_active_vector_db()
        if db and db["type"] == "qdrant":
            return db.get("url", "http://127.0.0.1:6333")
        return "http://127.0.0.1:6333"

    @property
    def qdrant_collection(self) -> str:
        db = get_active_vector_db()
        if db and db["type"] == "qdrant":
            return db.get("collection", "documents")
        return "documents"

    # ── 系统配置（从 system.json 读取） ──
    @property
    def token(self) -> str:
        return get_system_config().get("token", "change-me")

    @property
    def mcp_token(self) -> str:
        return get_system_config().get("mcp_token", "change-me")

    @property
    def max_concurrent_tasks(self) -> int:
        return get_system_config().get("max_concurrent_tasks", 3)

    @property
    def database_url(self) -> str:
        return get_system_config().get("database_url", "sqlite:///./okb_assist.db")

    @property
    def uploads_folder(self) -> str:
        return get_system_config().get("uploads_folder", "uploads")

    @property
    def public_url(self) -> str:
        return get_system_config().get("public_url", "http://localhost:5001")

    @property
    def subnet_url(self) -> str:
        return get_system_config().get("subnet_url", "http://192.168.1.100:5001")


_settings_instance = Settings()


def get_settings() -> Settings:
    return _settings_instance
