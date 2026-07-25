"""
FastEmbed 独立 HTTP 服务

将 FastEmbed embedding 模型常驻内存，通过 HTTP 接口提供 embedding 服务。
主项目通过 HTTP 调用本服务，无需在主项目中安装 fastembed 依赖。

启动方式:
    python scripts/fastembed_server.py
    或
    bash scripts/start-fastembed.sh

API:
    POST /embed   — 获取 embedding 向量
    GET  /health  — 健康检查
"""

import os
import sys
import argparse
import logging
from typing import Optional

# 设置 HuggingFace 镜像（必须在 import fastembed 之前）
if not os.environ.get("HF_ENDPOINT"):
    os.environ["HF_ENDPOINT"] = "https://hf-mirror.com"
if not os.environ.get("HF_HUB_DISABLE_XET"):
    os.environ["HF_HUB_DISABLE_XET"] = "1"

from fastapi import FastAPI, HTTPException
from pydantic import BaseModel

logging.basicConfig(level=logging.INFO, format="%(asctime)s %(levelname)s %(message)s")
logger = logging.getLogger("fastembed-server")

app = FastAPI(title="FastEmbed Server", version="1.0.0")

# ── 模型缓存 ──────────────────────────────────────────────────────────────

_models: dict[str, object] = {}  # model_name -> TextEmbedding instance
_default_model: Optional[str] = None


def _get_model(model_name: str):
    """获取或加载模型（懒加载 + 缓存）。"""
    if model_name not in _models:
        logger.info(f"Loading model: {model_name}")
        from fastembed import TextEmbedding
        _models[model_name] = TextEmbedding(model_name=model_name)
        logger.info(f"Model loaded: {model_name}")
    return _models[model_name]


# ── 请求/响应模型 ──────────────────────────────────────────────────────────

class EmbedRequest(BaseModel):
    texts: list[str]
    model: Optional[str] = None  # None 时使用默认模型


class EmbedResponse(BaseModel):
    embeddings: list[list[float]]
    model: str
    dimensions: int


class HealthResponse(BaseModel):
    status: str
    default_model: str
    loaded_models: list[str]


# ── API 端点 ───────────────────────────────────────────────────────────────

@app.get("/health", response_model=HealthResponse)
def health():
    """健康检查。"""
    return HealthResponse(
        status="ok",
        default_model=_default_model or "",
        loaded_models=list(_models.keys()),
    )


@app.post("/embed", response_model=EmbedResponse)
def embed(req: EmbedRequest):
    """获取 embedding 向量。

    请求: {"texts": ["text1", "text2"], "model": "optional-model-name"}
    响应: {"embeddings": [[0.1, ...], [0.2, ...]], "model": "...", "dimensions": 384}
    """
    if not req.texts:
        raise HTTPException(status_code=400, detail="texts 不能为空")

    model_name = req.model or _default_model
    if not model_name:
        raise HTTPException(status_code=400, detail="未指定 model 且无默认模型")

    try:
        model = _get_model(model_name)
        embeddings = list(model.embed(req.texts))
        result = [emb.tolist() for emb in embeddings]
        dims = len(result[0]) if result else 0
        return EmbedResponse(embeddings=result, model=model_name, dimensions=dims)
    except Exception as e:
        logger.error(f"Embed error: {e}")
        raise HTTPException(status_code=500, detail=f"Embedding 失败: {str(e)}")


# ── 启动 ───────────────────────────────────────────────────────────────────

def main():
    global _default_model

    parser = argparse.ArgumentParser(description="FastEmbed HTTP Server")
    parser.add_argument("--host", default="0.0.0.0", help="监听地址")
    parser.add_argument("--port", type=int, default=8003, help="监听端口")
    parser.add_argument("--model", default=None, help="预加载的默认模型名称")
    args = parser.parse_args()

    _default_model = args.model

    # 预加载默认模型
    if _default_model:
        _get_model(_default_model)

    import uvicorn
    logger.info(f"Starting FastEmbed server on {args.host}:{args.port}")
    if _default_model:
        logger.info(f"Default model: {_default_model}")
    uvicorn.run(app, host=args.host, port=args.port, log_level="info")


if __name__ == "__main__":
    main()
