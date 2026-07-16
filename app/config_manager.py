"""JSON 配置文件管理模块。

配置分为两个文件：
- config.json: 服务配置（MinerU、Ollama、向量数据库），可通过前端修改
- system.json: 系统配置（token、并发数、数据库、上传目录），仅限手动修改
"""

import json
import threading
from pathlib import Path
from typing import Any

CONFIG_FILE = Path("config.json")
SYSTEM_FILE = Path("system.json")

DEFAULT_CONFIG = {
    "mineru": {
        "url": "http://127.0.0.1:8002",
        "key": "key",
        "max_tasks": 3,
        "task_timeout": 300,
    },
    "ollama": {
        "url": "http://127.0.0.1:11434",
        "key": "",
        "model": "qwen3.5:9b",
    },
    "vector_dbs": [
        {
            "id": "default",
            "name": "默认 Qdrant",
            "type": "qdrant",
            "enabled": True,
            "url": "http://127.0.0.1:6333",
            "collection": "documents",
            "embedding": {
                "source": "ollama",
                "model": "nomic-embed-text",
            },
        }
    ],
}

DEFAULT_SYSTEM = {
    "upload_token": "change-me",
    "mcp_token": "change-me",
    "max_concurrent_tasks": 3,
    "database_url": "sqlite:///./okb_assist.db",
    "uploads_folder": "uploads",
    "public_url": "http://localhost:5001",
    "subnet_url": "http://192.168.1.100:5001",
}

# 进程内缓存
_config_cache: dict | None = None
_system_cache: dict | None = None
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


def _load_json_file(filepath: Path, defaults: dict) -> dict:
    """从 JSON 文件读取配置，不存在则用默认值创建。"""
    if filepath.exists():
        try:
            with open(filepath, "r", encoding="utf-8") as f:
                data = json.load(f)
            return _deep_merge(defaults, data)
        except (json.JSONDecodeError, OSError):
            return defaults.copy()
    else:
        _write_json_file(filepath, defaults)
        return defaults.copy()


def _write_json_file(filepath: Path, data: dict) -> None:
    """写入 JSON 文件。"""
    with open(filepath, "w", encoding="utf-8") as f:
        json.dump(data, f, ensure_ascii=False, indent=2)


def load_config() -> dict:
    """从 config.json 读取服务配置。"""
    global _config_cache
    if _config_cache is not None:
        return _config_cache

    with _lock:
        if _config_cache is not None:
            return _config_cache
        _config_cache = _load_json_file(CONFIG_FILE, DEFAULT_CONFIG)
        return _config_cache


def load_system_config() -> dict:
    """从 system.json 读取系统配置。"""
    global _system_cache
    if _system_cache is not None:
        return _system_cache

    with _lock:
        if _system_cache is not None:
            return _system_cache
        _system_cache = _load_json_file(SYSTEM_FILE, DEFAULT_SYSTEM)
        return _system_cache


def get_config() -> dict:
    """获取完整配置（服务配置 + 系统配置）。"""
    config = load_config().copy()
    config.update(load_system_config())
    return config


def get_service_config() -> dict:
    """仅获取服务配置（可通过前端修改）。"""
    return load_config()


def get_system_config() -> dict:
    """仅获取系统配置（仅限手动修改）。"""
    return load_system_config()


def save_config(config: dict) -> None:
    """保存服务配置到文件并刷新缓存。仅保存 config.json 中的字段。"""
    global _config_cache
    with _lock:
        # 只保留服务配置字段
        service_keys = set(DEFAULT_CONFIG.keys())
        service_config = {k: v for k, v in config.items() if k in service_keys}
        _write_json_file(CONFIG_FILE, service_config)
        _config_cache = service_config


def reload_config() -> dict:
    """强制重新从文件加载配置。"""
    global _config_cache, _system_cache
    with _lock:
        _config_cache = None
        _system_cache = None
    return get_config()


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

    # Upload token (来自系统配置)
    if masked.get("upload_token"):
        token = masked["upload_token"]
        masked["upload_token"] = token[:4] + "***" if len(token) > 4 else "***"

    # Vector DB keys
    for db in masked.get("vector_dbs", []):
        if db.get("api_key"):
            key = db["api_key"]
            db["api_key"] = key[:4] + "***" if len(key) > 4 else "***"

    return masked


def mask_system_config(config: dict) -> dict:
    """返回脱敏后的系统配置副本。"""
    import copy
    masked = copy.deepcopy(config)

    # Upload token
    if masked.get("upload_token"):
        token = masked["upload_token"]
        masked["upload_token"] = token[:4] + "***" if len(token) > 4 else "***"

    # MCP token
    if masked.get("mcp_token"):
        token = masked["mcp_token"]
        masked["mcp_token"] = token[:4] + "***" if len(token) > 4 else "***"

    return masked
