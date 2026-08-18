import ipaddress
import os

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

        # 图片资源跳过认证（文件名是 SHA256 哈希，不可猜测）
        if "/image/" in path and path.startswith("/assist/api/documents/"):
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

        # 本地局域网白名单：来自 192.168.1.0/24 的请求免 token 校验。
        # 注意：若经由反向代理，client IP 可能来自 X-Forwarded-For，存在被伪造的可能；
        # 对本地辅助工具，放行 LAN 的便利性优先，故不做额外防护。
        client_ip = self._get_client_ip(request)
        try:
            if ipaddress.ip_address(client_ip) in ipaddress.ip_network("192.168.1.0/24"):
                return await call_next(request)
        except ValueError:
            # 无法解析的 IP 直接落入下方 token 校验流程
            pass

        # 从 Header 或 Query 获取 token
        provided = request.headers.get("X-Token") or request.query_params.get("token")

        if not provided or provided != expected:
            return JSONResponse(
                status_code=401,
                content={"detail": "未授权：请提供有效的 Token"},
            )

        return await call_next(request)

    @staticmethod
    def _get_client_ip(request):
        """获取客户端真实 IP。

        优先取 X-Forwarded-For 的第一个地址（经代理时），否则用直连的
        request.client.host。
        """
        forwarded = request.headers.get("X-Forwarded-For")
        if forwarded:
            return forwarded.split(",")[0].strip()
        if request.client is not None:
            return request.client.host
        return ""


@asynccontextmanager
async def lifespan(app: FastAPI):
    """Application lifespan: startup and shutdown."""
    init_db()

    # 重置中间状态的任务（重启后恢复）
    reset_stuck_tasks()

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


# ── 挂载 MCP 端点（两个完全独立的端点，避免会话混淆） ─────────────────
#   - Streamable HTTP: 精确 Route /assist/mcp/stream  (推荐的标准传输)
#   - SSE (legacy)   : Mount /assist/mcp            → /assist/mcp/sse
#
# Streamable 必须用精确 Route（而非 Mount）注册，原因：
#   1. Mount 要求末尾斜杠 (/assist/mcp/stream/)，否则根路径 404；
#   2. SSE 的 Mount 前缀 /assist/mcp 会先于 Streamable 抢走 /assist/mcp/stream。
# 用精确 Route 后客户端可直接连 /assist/mcp/stream（POST initialize 无法跟随
# 307 重定向，所以必须精确匹配）。该子应用自带 AuthenticationMiddleware +
# AuthContextMiddleware，Bearer Token 校验不依赖父应用中间件。
# Route 必须在 SSE Mount 之前注册，确保 /assist/mcp/stream 命中精确 Route 而非
# 被 SSE 的 /assist/mcp 前缀吞掉。
# 注册必须在模块加载时（app 创建后）完成，否则 Starlette 活动路由表不可靠。
mcp_session_manager = None
try:
    from app.mcp_server import create_sse_app, create_streamable_app, mcp
    # 先注册 Streamable HTTP 精确 Route（必须在 SSE Mount 之前）
    app.add_route(
        "/assist/mcp/stream",
        create_streamable_app(),
        methods=["GET", "POST", "DELETE", "OPTIONS", "HEAD"],
    )
    # 再注册 SSE Mount
    app.mount("/assist/mcp", create_sse_app())
    mcp_session_manager = mcp.session_manager
except ImportError:
    print("Warning: mcp package not installed, MCP endpoint unavailable")
except Exception as e:
    print(f"Warning: Failed to mount MCP: {e}")


# ── 文件别名路由（无需 token，路径不可猜测） ─────────────────────────────
@app.get("/assist/file/{filename}")
async def serve_file_alias(filename: str):
    """通过别名访问 PDF 文件（无需 token，URL 不可猜测）。"""
    from app.routers.documents import get_doc_by_alias
    from app.paths import get_pdf_path
    from fastapi.responses import FileResponse

    doc_id = get_doc_by_alias(filename)
    if doc_id is None:
        return JSONResponse(status_code=404, content={"detail": "文件不存在或链接已过期"})

    from app.database import SessionLocal
    from app.models import Document
    db = SessionLocal()
    try:
        doc = db.query(Document).filter(Document.id == doc_id).first()
        abs_path = get_pdf_path(doc_id)
        if not doc or not os.path.isfile(abs_path):
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


@app.get("/assist/markdown/{doc_id}/edit")
def markdown_edit_page(request: Request, doc_id: int):
    return templates.TemplateResponse(name="markdown_edit.html", request=request, context={"doc_id": doc_id})


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
