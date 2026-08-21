import ipaddress
import json
import os

from contextlib import AsyncExitStack, asynccontextmanager

from fastapi import Request
from fastapi.responses import RedirectResponse
from fastapi.staticfiles import StaticFiles
from starlette.staticfiles import NotModifiedResponse
from fastapi import FastAPI
from fastapi.middleware.cors import CORSMiddleware
from starlette.middleware.base import BaseHTTPMiddleware
from starlette.responses import JSONResponse, FileResponse

from app.database import init_db
from app.config import get_settings
from app.routers import documents, pipeline, admin, openapi, config


BASE_DIR = os.path.dirname(os.path.abspath(__file__))
# 后端位于 backend/ 子目录，前端构建产物在项目根目录的 frontend/dist/
FRONTEND_DIST_DIR = os.path.abspath(os.path.join(BASE_DIR, "..", "frontend", "dist"))
FRONTEND_INDEX_PATH = os.path.join(FRONTEND_DIST_DIR, "index.html")


def resolve_frontend_file(path: str):
    """Resolve a requested frontend file inside frontend/dist."""
    normalized_path = os.path.normpath(path.lstrip("/"))
    candidate = os.path.abspath(os.path.join(FRONTEND_DIST_DIR, normalized_path))

    if os.path.commonpath([FRONTEND_DIST_DIR, candidate]) != FRONTEND_DIST_DIR:
        return None
    if os.path.isfile(candidate):
        return candidate
    return None


# ── 缓存策略 ──────────────────────────────────────────────────────────────

# 带内容 hash 的静态资源（Vite 构建产物文件名包含 hash），可安全长期缓存
LONG_CACHE_EXTENSIONS = {
    ".js", ".css", ".png", ".jpg", ".jpeg", ".gif", ".svg", ".ico",
    ".woff", ".woff2", ".ttf", ".eot", ".map", ".webp", ".avif",
}

# 中等缓存时长：1 天（适用于无 hash 的普通资源）
MIDDLE_CACHE_MAX_AGE = 86400

# 长期缓存时长：1 年（适用于带 hash 的资源）
LONG_CACHE_MAX_AGE = 31536000


def _get_cache_control(filepath: str) -> str:
    """根据文件扩展名返回合适的 Cache-Control 头值。

    策略：
    - index.html / 路由回退页面：不缓存，确保用户始终获取最新版本
    - 带 hash 的静态资源（JS/CSS/图片/字体）：public, max-age=1年, immutable
    - 其他文件：public, max-age=1天
    """
    basename = os.path.basename(filepath)
    ext = os.path.splitext(basename)[1].lower()

    # index.html 或无后缀的路由回退页面：不缓存
    if basename == "index.html" or not ext:
        return "no-cache, must-revalidate"

    # 带 hash 的静态资源：长期缓存
    if ext in LONG_CACHE_EXTENSIONS:
        return f"public, max-age={LONG_CACHE_MAX_AGE}, immutable"

    # 其他：中期缓存
    return f"public, max-age={MIDDLE_CACHE_MAX_AGE}"


# ── Token 认证中间件 ──────────────────────────────────────────────────────

class TokenMiddleware(BaseHTTPMiddleware):
    """API 端点 Token 认证（MCP、页面、静态文件不校验）。"""

    # 不需要校验 token 的路径前缀
    SKIP_PREFIXES = (
        "/assist/mcp",      # MCP 有自己的 Bearer Token 认证
        "/assist/assets",   # 前端静态资源
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

# CORS middleware — 限定前端来源
app.add_middleware(
    CORSMiddleware,
    allow_origins=[
        "http://localhost:5173",   # 前端开发服务器
        "http://localhost:5001",   # 生产：同源（FastAPI serving 静态文件）
    ],
    allow_credentials=True,
    allow_methods=["GET", "POST", "PUT", "DELETE", "PATCH", "OPTIONS"],
    allow_headers=["Content-Type", "X-Token", "Authorization"],
)

# Token 认证中间件
app.add_middleware(TokenMiddleware)

class CachedStaticFiles(StaticFiles):
    """带缓存头的 StaticFiles，为上传文件添加合理的 Cache-Control。"""

    def file_response(self, full_path, stat_result, scope, status_code=200):
        from starlette.datastructures import Headers
        response = FileResponse(
            full_path,
            status_code=status_code,
            stat_result=stat_result,
            headers={"Cache-Control": _get_cache_control(str(full_path))},
        )
        request_headers = Headers(scope=scope)
        if self.is_not_modified(response.headers, request_headers):
            return NotModifiedResponse(response.headers)
        return response


_settings = get_settings()
os.makedirs(_settings.uploads_folder, exist_ok=True)
app.mount("/assist/uploads", CachedStaticFiles(directory=_settings.uploads_folder), name="uploads")

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
        return FileResponse(
            abs_path,
            media_type="application/pdf",
            headers={"Cache-Control": f"public, max-age={MIDDLE_CACHE_MAX_AGE}"},
        )
    finally:
        db.close()


# ==================== 根路由与重定向 ====================

@app.get("/")
def root():
    return RedirectResponse(url="/assist")


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


# ==================== SPA Fallback ====================
# 所有 /assist/ 下的非 API / 非 MCP / 非上传 / 非文件别名请求：
#   1. 如果命中 frontend/dist 中的真实文件，直接返回该文件；
#   2. 否则返回前端 index.html，由 Vue Router 处理客户端路由。

@app.get("/assist", include_in_schema=False)
@app.get("/assist/{full_path:path}", include_in_schema=False)
async def serve_spa(full_path: str = "", request: Request = None):
    """Serve the built frontend under /assist/."""
    from fastapi import HTTPException
    from fastapi.responses import FileResponse, RedirectResponse

    # 这些前缀由其他路由处理，不应 fallback；
    # 如果请求路径缺少末尾斜杠（如 /assist/api/documents），
    # 重定向到带斜杠的地址，让 APIRouter 的 redirect_slashes 生效。
    skip_prefixes = (
        "api/",
        "mcp/",
        "uploads/",
        "file/",
    )
    for prefix in skip_prefixes:
        if full_path.startswith(prefix):
            if request is not None and not full_path.endswith("/"):
                target = f"/assist/{full_path}/"
                query = str(request.url.query)
                if query:
                    target += "?" + query
                return RedirectResponse(url=target)
            raise HTTPException(status_code=404, detail="Not Found")

    if full_path:
        frontend_file = resolve_frontend_file(full_path)
        if frontend_file is not None:
            cache_control = _get_cache_control(frontend_file)
            return FileResponse(frontend_file, headers={"Cache-Control": cache_control})

    if os.path.exists(FRONTEND_INDEX_PATH):
        cache_control = _get_cache_control(FRONTEND_INDEX_PATH)
        return FileResponse(FRONTEND_INDEX_PATH, headers={"Cache-Control": cache_control})
    return {"detail": f"前端未构建，请运行 cd {os.path.dirname(FRONTEND_DIST_DIR)} && pnpm run build"}


if __name__ == "__main__":
    import argparse
    import uvicorn

    parser = argparse.ArgumentParser(
        description="OKB-Assist 启动参数"
    )
    parser.add_argument(
        "--host",
        default="0.0.0.0",
        help="监听地址 (默认: 0.0.0.0)",
    )
    parser.add_argument(
        "--port",
        type=int,
        default=5001,
        help="监听端口 (默认: 5001)",
    )
    parser.add_argument(
        "--reload",
        action="store_true",
        default=False,
        help="启用自动重载（开发模式）",
    )
    args = parser.parse_args()

    uvicorn.run(
        "okb_assist_main:app",
        host=args.host,
        port=args.port,
        reload=args.reload,
    )
