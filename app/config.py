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
    ollama_model: str = "qcwind/qwen3-8b-instruct-Q4-K-M:latest"
    ollama_embed_model: str = "nomic-embed-text"

    # Qdrant
    qdrant_url: str = "http://127.0.0.1:6333"
    qdrant_collection: str = "documents"

    # JWT
    jwt_secret: str = "change-me-to-a-random-secret"
    jwt_algorithm: str = "HS256"
    jwt_expire_minutes: int = 1440

    # Database
    database_url: str = "sqlite:///./okb_assist.db"

    model_config = {"env_file": ".env", "env_file_encoding": "utf-8"}


@lru_cache
def get_settings() -> Settings:
    return Settings()
