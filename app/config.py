from pydantic_settings import BaseSettings
from functools import lru_cache


class Settings(BaseSettings):
    # MinerU
    mineru_url: str = "http://127.0.0.1:8002"
    mineru_key: str = "key"
    mineru_tasks: int = 3

    # Ollama
    ollama_url: str = "http://127.0.0.1:11434"
    ollama_key: str = ""
    ollama_model: str = "qwen3.5:9b"
    ollama_embed_model: str = "nomic-embed-text"

    # Qdrant
    qdrant_url: str = "http://127.0.0.1:6333"
    qdrant_collection: str = "documents"

    # Upload token
    upload_token: str = "change-me"

    # Task queue
    max_concurrent_tasks: int = 3

    # Database
    database_url: str = "sqlite:///./okb_assist.db"

    # 文件存储目录（PDF + Markdown + 解析产物）
    uploads_folder: str = "uploads"

    model_config = {"env_file": ".env", "env_file_encoding": "utf-8"}


@lru_cache
def get_settings() -> Settings:
    return Settings()
