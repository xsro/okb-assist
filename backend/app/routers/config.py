"""配置管理 API。"""

import httpx
from fastapi import APIRouter, HTTPException
from pydantic import BaseModel

from app.config_manager import (
    get_config, get_service_config, get_system_config,
    save_config, mask_sensitive, mask_system_config, reload_config
)

router = APIRouter(prefix="/assist/api/config", tags=["config"])


class ConfigUpdateRequest(BaseModel):
    """服务配置更新请求体（仅服务配置可修改）。"""
    mineru: dict | None = None
    ollama: dict | None = None
    vector_dbs: list[dict] | None = None


class TestConnectionRequest(BaseModel):
    """连接测试请求体。"""
    type: str  # mineru | ollama | qdrant | milvus | chroma
    url: str = ""
    key: str = ""
    model: str = ""
    embed_model: str = ""
    collection: str = ""


@router.get("")
def get_current_config():
    """获取当前配置（敏感字段脱敏）。"""
    config = get_config()
    return mask_sensitive(config)


@router.get("/system")
def get_system_config_api():
    """获取系统配置（只读，敏感字段脱敏）。"""
    config = get_system_config()
    return mask_system_config(config)


@router.put("")
def update_config(req: ConfigUpdateRequest):
    """更新服务配置。仅更新请求中非 null 的字段。"""
    config = get_service_config()

    if req.mineru is not None:
        config["mineru"] = {**config.get("mineru", {}), **req.mineru}

    if req.ollama is not None:
        config["ollama"] = {**config.get("ollama", {}), **req.ollama}

    if req.vector_dbs is not None:
        # 校验每个 vector_db 必须有 id 和 type
        for db in req.vector_dbs:
            if not db.get("id"):
                raise HTTPException(status_code=400, detail="向量数据库配置必须包含 id 字段")
            if not db.get("type"):
                raise HTTPException(status_code=400, detail="向量数据库配置必须包含 type 字段")
            if db["type"] not in ("qdrant", "milvus", "chroma"):
                raise HTTPException(status_code=400, detail=f"不支持的向量数据库类型: {db['type']}")
        config["vector_dbs"] = req.vector_dbs

    save_config(config)
    return {"detail": "配置已保存", "config": mask_sensitive(get_config())}


@router.post("/reload")
def reload():
    """强制重新从文件加载配置。"""
    config = reload_config()
    return {"detail": "配置已重新加载", "config": mask_sensitive(config)}


@router.post("/test")
async def test_connection(req: TestConnectionRequest):
    """测试服务连接。"""
    if req.type == "mineru":
        return await _test_mineru(req.url, req.key)
    elif req.type == "ollama":
        return await _test_ollama(req.url, req.model)
    elif req.type == "qdrant":
        return await _test_qdrant(req.url)
    elif req.type in ("milvus", "chroma"):
        return {"status": "unsupported", "detail": f"{req.type} 暂未实现连接测试"}
    else:
        raise HTTPException(status_code=400, detail=f"未知的服务类型: {req.type}")


async def _test_mineru(url: str, key: str) -> dict:
    """测试 MinerU 连接。"""
    url = url.rstrip("/")
    try:
        async with httpx.AsyncClient(timeout=5.0) as client:
            headers = {}
            if key:
                headers["Authorization"] = f"Bearer {key}"
            response = await client.get(f"{url}/health", headers=headers)
            if response.status_code == 200:
                data = response.json()
                return {
                    "status": "connected",
                    "detail": "MinerU 连接成功",
                    "version": data.get("version", "unknown"),
                }
            return {"status": "error", "detail": f"HTTP {response.status_code}"}
    except httpx.ConnectError:
        return {"status": "disconnected", "detail": f"无法连接到 {url}"}
    except Exception as e:
        return {"status": "error", "detail": str(e)}


async def _test_ollama(url: str, model: str) -> dict:
    """测试 Ollama 连接。"""
    url = url.rstrip("/")
    try:
        async with httpx.AsyncClient(timeout=5.0) as client:
            response = await client.get(f"{url}/api/tags")
            if response.status_code == 200:
                models = [m["name"] for m in response.json().get("models", [])]
                model_found = not model or any(model in m for m in models)
                return {
                    "status": "connected",
                    "detail": "Ollama 连接成功",
                    "models": models,
                    "model_available": model_found,
                }
            return {"status": "error", "detail": f"HTTP {response.status_code}"}
    except httpx.ConnectError:
        return {"status": "disconnected", "detail": f"无法连接到 {url}"}
    except Exception as e:
        return {"status": "error", "detail": str(e)}


async def _test_qdrant(url: str) -> dict:
    """测试 Qdrant 连接。"""
    url = url.rstrip("/")
    try:
        from qdrant_client import QdrantClient
        client = QdrantClient(url=url)
        collections = client.get_collections().collections
        return {
            "status": "connected",
            "detail": "Qdrant 连接成功",
            "collections": [c.name for c in collections],
        }
    except Exception as e:
        return {"status": "disconnected", "detail": f"无法连接到 {url}: {e}"}
