"""JSON 配置文件管理模块。

所有配置项统一存储在 config.json 中，支持运行时读写。
替代原 .env + pydantic-settings 方案。
"""

import json
import threading
from pathlib import Path
from typing import Any

CONFIG_FILE = Path("config.json")

DEFAULT_CONFIG = {
    "mineru": {
        "url": "http://127.0.0.1:8002",
        "key": "key",
        "max_tasks": 3,
    },
    "ollama": {
        "url": "http://127.0.0.1:11434",
        "key": "",
        "model": "qwen3.5:9b",
        "embed_model": "nomic-embed-text",
    },
    "vector_dbs": [
        {
            "id": "default",
            "name": "默认 Qdrant",
            "type": "qdrant",
            "enabled": True,
            "url": "http://127.0.0.1:6333",
            "collection": "documents",
        }
    ],
    "upload_token": "change-me",
    "max_concurrent_tasks": 3,
    "database_url": "sqlite:///./okb_assist.db",
    "uploads_folder": "uploads",
}

# 进程内缓存
_cache: dict | None = None
_lock = threading.Lock()


def _deep_merge(base: dict, override: dict) -> dict:
    """递归合并，override 中的值覆盖 base。"""
    result = base.copy()
    for key, value in override.items():
        if key in result and isinstance(result[key], dict) and isinstance(value, dict):
            result[key] = _deep_merge(result[key], value)
        else:
            result[key] = value
    return result


def load_config() -> dict:
    """从 config.json 读取配置。文件不存在则用默认值创建。"""
    global _cache
    if _cache is not None:
        return _cache

    with _lock:
        if _cache is not None:
            return _cache

        if CONFIG_FILE.exists():
            try:
                with open(CONFIG_FILE, "r", encoding="utf-8") as f:
                    data = json.load(f)
                # 用默认值补充缺失字段
                _cache = _deep_merge(DEFAULT_CONFIG, data)
            except (json.JSONDecodeError, OSError):
                _cache = DEFAULT_CONFIG.copy()
        else:
            _cache = DEFAULT_CONFIG.copy()
            _write_config(_cache)

        return _cache


def get_config() -> dict:
    """获取当前配置（带缓存）。"""
    return load_config()


def save_config(config: dict) -> None:
    """保存配置到文件并刷新缓存。"""
    global _cache
    with _lock:
        _write_config(config)
        _cache = config


def _write_config(config: dict) -> None:
    """写入 config.json 文件。"""
    with open(CONFIG_FILE, "w", encoding="utf-8") as f:
        json.dump(config, f, ensure_ascii=False, indent=2)


def reload_config() -> dict:
    """强制重新从文件加载配置。"""
    global _cache
    with _lock:
        _cache = None
    return load_config()


def get_active_vector_db() -> dict | None:
    """获取第一个 enabled 的向量数据库配置。"""
    cfg = get_config()
    for db in cfg.get("vector_dbs", []):
        if db.get("enabled"):
            return db
    return None


def get_vector_db_by_id(db_id: str) -> dict | None:
    """根据 id 获取向量数据库配置。"""
    cfg = get_config()
    for db in cfg.get("vector_dbs", []):
        if db.get("id") == db_id:
            return db
    return None


def mask_sensitive(config: dict) -> dict:
    """返回脱敏后的配置副本（隐藏 key、token 等敏感字段）。"""
    import copy
    masked = copy.deepcopy(config)

    # MinerU key
    if masked.get("mineru", {}).get("key"):
        key = masked["mineru"]["key"]
        masked["mineru"]["key"] = key[:4] + "***" if len(key) > 4 else "***"

    # Ollama key
    if masked.get("ollama", {}).get("key"):
        key = masked["ollama"]["key"]
        masked["ollama"]["key"] = key[:4] + "***" if len(key) > 4 else "***"

    # Upload token
    if masked.get("upload_token"):
        token = masked["upload_token"]
        masked["upload_token"] = token[:4] + "***" if len(token) > 4 else "***"

    # Vector DB keys
    for db in masked.get("vector_dbs", []):
        if db.get("api_key"):
            key = db["api_key"]
            db["api_key"] = key[:4] + "***" if len(key) > 4 else "***"

    return masked
