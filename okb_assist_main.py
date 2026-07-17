import os

# 设置 HuggingFace 镜像环境变量（用于 FastEmbed）
os.environ.setdefault('HF_ENDPOINT', 'https://hf-mirror.com')
os.environ.setdefault('HF_HUB_DISABLE_XET', '1')

from contextlib import AsyncExitStack, asynccontextmanager

from fastapi import Request
from fastapi.responses import RedirectResponse
from fastapi.staticfiles import StaticFiles
from fastapi.templating import Jinja2Templates
from fastapi import FastAPI
from fastapi.middleware.cors import CORSMiddleware
from starlette.middleware.base import BaseHTTPMiddleware
from starlette.responses import JSONResponse

from app.database import init_db
from app.config import get_settings
from app.routers import documents, pipeline, admin, openapi, config


# ── Token 认证中间件 ──────────────────────────────────────────────────────

class TokenMiddleware(BaseHTTPMiddleware):
    """API 端点 Token 认证（MCP、页面、静态文件不校验）。"""

    # 不需要校验 token 的路径前缀
    SKIP_PREFIXES = (
        "/assist/mcp",      # MCP 有自己的 Bearer Token 认证
        "/assist/static",   # 静态文件
        "/assist/uploads",  # 上传文件
        "/assist/file",     # 文件别名（不可猜测的 PDF 路径）
        "/redirect",        # 重定向
    )

    async def dispatch(self, request, call_next):
        path = request.url.path

        # 跳过不需要校验的路径
        for prefix in self.SKIP_PREFIXES:
            if path.startswith(prefix):
                return await call_next(request)

        # 只校验 API 路由，其余全部放行
        if not path.startswith("/assist/api/"):
            return await call_next(request)

        # 获取 token（Header 或 Query）
        settings = get_settings()
        expected = settings.token

        # token 为 change-me 时跳过校验
        if not expected or expected == "change-me":
            return await call_next(request)

        # 从 Header 或 Query 获取 token
        provided = request.headers.get("X-Token") or request.query_params.get("token")

        if not provided or provided != expected:
            return JSONResponse(
                status_code=401,
                content={"detail": "未授权：请提供有效的 Token"},
            )

        return await call_next(request)


@asynccontextmanager
async def lifespan(app: FastAPI):
    """Application lifespan: startup and shutdown."""
    init_db()

    # 重置中间状态的任务（重启后恢复）
    reset_stuck_tasks()

    mcp_session_manager = None

    # Mount MCP endpoints: Streamable HTTP at /assist/mcp and SSE at /assist/mcp/sse
    try:
        from app.mcp_server import create_mcp_app, mcp
        mcp_app = create_mcp_app()
        app.mount("/assist/mcp", mcp_app)
        mcp_session_manager = mcp.session_manager
    except ImportError:
        print("Warning: mcp package not installed, MCP endpoint unavailable")
    except Exception as e:
        print(f"Warning: Failed to mount MCP: {e}")

    async with AsyncExitStack() as stack:
        if mcp_session_manager is not None:
            await stack.enter_async_context(mcp_session_manager.run())
        yield


def reset_stuck_tasks():
    """重置卡在中间状态的任务"""
    from app.database import SessionLocal
    from app.models import Document, DocStatus

    db = SessionLocal()
    try:
        # 中间状态及其重置目标
        stuck_statuses = {
            DocStatus.parsing: DocStatus.uploaded,
            DocStatus.extracting: DocStatus.markdown_done,
            DocStatus.indexing: DocStatus.meta_done,
        }

        reset_count = 0
        for current_status, target_status in stuck_statuses.items():
            stuck_docs = db.query(Document).filter(
                Document.status == current_status
            ).all()

            for doc in stuck_docs:
                print(f"  重置任务 {doc.id}: {current_status.value} -> {target_status.value}")
                doc.status = target_status
                doc.status_message = f"服务重启，状态已重置"
                doc.progress = 0
                doc.mineru_task_id = None
                reset_count += 1

        if reset_count > 0:
            db.commit()
            print(f"已重置 {reset_count} 个卡住的任务")
        else:
            print("没有需要重置的任务")

    except Exception as e:
        print(f"重置任务失败: {e}")
        db.rollback()
    finally:
        db.close()


app = FastAPI(title="OKB-Assist", description="论文与专著数据管理系统", lifespan=lifespan)

# CORS middleware
app.add_middleware(
    CORSMiddleware,
    allow_origins=["*"],
    allow_credentials=True,
    allow_methods=["*"],
    allow_headers=["*"],
)

# Token 认证中间件
app.add_middleware(TokenMiddleware)

# Mount static files
app.mount("/assist/static", StaticFiles(directory="static"), name="static")

_settings = get_settings()
os.makedirs(_settings.uploads_folder, exist_ok=True)
app.mount("/assist/uploads", StaticFiles(directory=_settings.uploads_folder), name="uploads")

# Templates
templates = Jinja2Templates(directory="app/templates")

# Include routers
app.include_router(documents.router)
app.include_router(pipeline.router)
app.include_router(admin.router)
app.include_router(openapi.router)
app.include_router(config.router)


# ── 文件别名路由（无需 token，路径不可猜测） ─────────────────────────────
@app.get("/assist/file/{filename}")
async def serve_file_alias(filename: str):
    """通过别名访问 PDF 文件（无需 token，URL 不可猜测）。"""
    from app.routers.documents import get_doc_by_alias
    from app.utils import to_absolute_path
    from fastapi.responses import FileResponse

    doc_id = get_doc_by_alias(filename)
    if doc_id is None:
        return JSONResponse(status_code=404, content={"detail": "文件不存在或链接已过期"})

    from app.database import SessionLocal
    from app.models import Document
    db = SessionLocal()
    try:
        doc = db.query(Document).filter(Document.id == doc_id).first()
        if not doc or not doc.file_path:
            return JSONResponse(status_code=404, content={"detail": "文件不存在"})
        abs_path = to_absolute_path(doc.file_path)
        if not os.path.isfile(abs_path):
            return JSONResponse(status_code=404, content={"detail": "文件不存在"})
        return FileResponse(abs_path, media_type="application/pdf")
    finally:
        db.close()


# ==================== 页面路由 ====================

@app.get("/")
def root():
    return RedirectResponse(url="/assist/")


@app.get("/redirect/{doc_id}")
def redirect_by_network(request: Request, doc_id: int):
    """根据请求Host自动跳转到对应地址的详情页"""
    from urllib.parse import urlparse
    _settings = get_settings()
    host = request.headers.get("host", "")
    public_host = urlparse(_settings.public_url).netloc
    if host == public_host:
        base = _settings.public_url
    else:
        base = _settings.subnet_url
    return RedirectResponse(url=f"{base}/assist/detail/{doc_id}")


@app.get("/assist/")
def index(request: Request):
    return templates.TemplateResponse(name="index.html", request=request)


@app.get("/assist/detail/{doc_id}")
def detail_page(request: Request, doc_id: int):
    return templates.TemplateResponse(name="detail.html", request=request, context={"doc_id": doc_id})


@app.get("/assist/monitor")
def monitor_page(request: Request):
    return templates.TemplateResponse(name="monitor.html", request=request)


@app.get("/assist/markdown/{doc_id}")
def markdown_page(request: Request, doc_id: int):
    return templates.TemplateResponse(name="markdown.html", request=request, context={"doc_id": doc_id})


# ==================== 管理页面 ====================

@app.get("/assist/upload")
def upload_page(request: Request):
    return templates.TemplateResponse(name="upload.html", request=request)


@app.get("/assist/admin")
def admin_page(request: Request):
    return templates.TemplateResponse(name="admin.html", request=request)


@app.get("/assist/point")
def point_page(request: Request):
    return templates.TemplateResponse(name="point.html", request=request)


@app.get("/assist/tools")
def tools_page(request: Request):
    return templates.TemplateResponse(name="tools.html", request=request)


@app.get("/assist/duplicates")
def duplicates_page(request: Request):
    return templates.TemplateResponse(name="duplicates.html", request=request)


@app.get("/assist/doc/{doc_id}")
def doc_manage_page(request: Request, doc_id: int):
    return templates.TemplateResponse(name="doc_manage.html", request=request, context={"doc_id": doc_id})


@app.get("/assist/config")
def config_page(request: Request):
    return templates.TemplateResponse(name="config.html", request=request)


if __name__ == "__main__":
    import uvicorn
    uvicorn.run("okb_assist_main:app", host="0.0.0.0", port=5001, reload=True)
