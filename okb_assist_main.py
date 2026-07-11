import os

# 设置 HuggingFace 镜像环境变量（用于 FastEmbed）
os.environ.setdefault('HF_ENDPOINT', 'https://hf-mirror.com')
os.environ.setdefault('HF_HUB_DISABLE_XET', '1')

from contextlib import asynccontextmanager

from fastapi import Request, HTTPException, Depends
from fastapi.responses import RedirectResponse, JSONResponse
from fastapi.staticfiles import StaticFiles
from fastapi.templating import Jinja2Templates
from fastapi import FastAPI
from fastapi.middleware.cors import CORSMiddleware
from starlette.middleware.base import BaseHTTPMiddleware

from app.database import init_db
from app.auth import (
    get_current_user,
    get_token_from_request,
    is_authenticated,
    is_request_authorized,
    is_trusted_lan_client,
    verify_token,
)
from app.config import get_settings
from app.routers import documents, pipeline, admin, openapi, config


class AuthMiddleware(BaseHTTPMiddleware):
    """Apply token auth except for trusted LAN clients and public bootstrap paths."""

    PUBLIC_EXACT_PATHS = {
        "/",
        "/favicon.ico",
        "/assist/login",
        "/assist/api/auth/token",
        "/assist/api/auth/check",
    }
    PUBLIC_PREFIXES = (
        "/assist/static/",
    )
    API_PREFIXES = (
        "/assist/api/",
        "/assist/openapi/",
    )

    def _is_public_path(self, path: str) -> bool:
        if path in self.PUBLIC_EXACT_PATHS:
            return True

        return any(path.startswith(prefix) for prefix in self.PUBLIC_PREFIXES)

    def _is_api_path(self, path: str) -> bool:
        return path == "/openapi.json" or any(path.startswith(prefix) for prefix in self.API_PREFIXES)

    async def dispatch(self, request: Request, call_next):
        path = request.url.path

        if request.method == "OPTIONS" or self._is_public_path(path):
            return await call_next(request)

        if is_request_authorized(request):
            return await call_next(request)

        if self._is_api_path(path):
            return JSONResponse(
                status_code=401,
                content={"detail": "未授权访问，请提供有效的token"},
            )

        return RedirectResponse(url="/assist/login", status_code=302)


@asynccontextmanager
async def lifespan(app: FastAPI):
    """Application lifespan: startup and shutdown."""
    init_db()

    # 重置中间状态的任务（重启后恢复）
    reset_stuck_tasks()

    # Mount MCP SSE endpoint
    try:
        from app.mcp_server import mcp
        mcp_app = mcp.sse_app()
        app.mount("/assist/mcp", mcp_app)
    except ImportError:
        print("Warning: mcp package not installed, MCP endpoint unavailable")
    except Exception as e:
        print(f"Warning: Failed to mount MCP: {e}")
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

# Auth middleware
app.add_middleware(AuthMiddleware)

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


# ==================== 认证端点 ====================

@app.post("/assist/api/auth/token")
async def auth_token(request: Request):
    """Validate the configured access token."""
    token = None

    try:
        body = await request.json()
    except Exception:
        body = None

    if isinstance(body, dict):
        token = body.get("token") or body.get("access_token")

    if not token:
        try:
            form = await request.form()
        except Exception:
            form = {}
        token = form.get("token") or form.get("access_token")

    if not token:
        token = get_token_from_request(request)

    if verify_token(token):
        return {"access_token": token, "token_type": "bearer"}

    raise HTTPException(status_code=401, detail="无效的 token")


@app.get("/assist/api/auth/check")
async def auth_check(request: Request):
    """检查当前请求是否允许访问受保护资源。"""
    trusted_client = is_trusted_lan_client(request)
    token_authenticated = is_authenticated(request)
    return {
        "authenticated": trusted_client or token_authenticated,
        "token_authenticated": token_authenticated,
        "trusted_client": trusted_client,
        "auth_required": not trusted_client,
    }


# ==================== 页面路由 ====================

@app.get("/")
def root():
    return RedirectResponse(url="/assist/")


@app.get("/assist/login")
def login_page(request: Request):
    return templates.TemplateResponse(name="login.html", request=request)


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
def upload_page(request: Request, user: str = Depends(get_current_user)):
    return templates.TemplateResponse(name="upload.html", request=request)


@app.get("/assist/admin")
def admin_page(request: Request, user: str = Depends(get_current_user)):
    return templates.TemplateResponse(name="admin.html", request=request)


@app.get("/assist/point")
def point_page(request: Request, user: str = Depends(get_current_user)):
    return templates.TemplateResponse(name="point.html", request=request)


@app.get("/assist/tools")
def tools_page(request: Request, user: str = Depends(get_current_user)):
    return templates.TemplateResponse(name="tools.html", request=request)


@app.get("/assist/duplicates")
def duplicates_page(request: Request, user: str = Depends(get_current_user)):
    return templates.TemplateResponse(name="duplicates.html", request=request)


@app.get("/assist/doc/{doc_id}")
def doc_manage_page(request: Request, doc_id: int, user: str = Depends(get_current_user)):
    return templates.TemplateResponse(name="doc_manage.html", request=request, context={"doc_id": doc_id})


@app.get("/assist/config")
def config_page(request: Request, user: str = Depends(get_current_user)):
    return templates.TemplateResponse(name="config.html", request=request)


if __name__ == "__main__":
    import uvicorn
    uvicorn.run("okb_assist_main:app", host="0.0.0.0", port=5001, reload=True)
