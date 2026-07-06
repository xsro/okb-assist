from contextlib import asynccontextmanager
import ipaddress

from fastapi import Request, HTTPException
from fastapi.responses import RedirectResponse, JSONResponse
from fastapi.staticfiles import StaticFiles
from fastapi.templating import Jinja2Templates
from fastapi import FastAPI
from fastapi.middleware.cors import CORSMiddleware
from starlette.middleware.base import BaseHTTPMiddleware

from app.database import init_db
from app.config import get_settings
from app.routers import documents, pipeline, admin, openapi

settings = get_settings()

# 192.168 子网（需要认证的IP）
BLOCKED_SUBNET_IP = "192.168.1.162"


class AuthMiddleware(BaseHTTPMiddleware):
    """认证中间件：192.168子网放行（除了192.168.1.162），其他需要token"""

    # 不需要认证的路径
    EXEMPT_PATHS = {"/assist/login", "/assist/static/", "/assist/monitor", "/assist/detail/"}

    # 不需要认证的精确路径
    EXEMPT_EXACT_PATHS = {"/assist/", "/assist"}

    def _is_local_subnet(self, client_ip: str) -> bool:
        """检查是否是192.168子网（除了指定IP）"""
        try:
            ip = ipaddress.ip_address(client_ip)
            # 检查是否是192.168.x.x
            if ip.version == 4 and str(ip).startswith("192.168."):
                # 排除指定IP
                if str(ip) == BLOCKED_SUBNET_IP:
                    return False
                return True
        except ValueError:
            pass
        return False

    def _is_exempt_path(self, path: str) -> bool:
        """检查路径是否免认证"""
        # 检查精确路径
        if path in self.EXEMPT_EXACT_PATHS:
            return True
        # 检查前缀路径
        for exempt in self.EXEMPT_PATHS:
            if path.startswith(exempt):
                return True
        return False

    async def dispatch(self, request: Request, call_next):
        # 获取客户端IP（优先从X-Forwarded-For获取，适配nginx代理）
        client_ip = request.headers.get("X-Forwarded-For", "")
        if client_ip:
            # X-Forwarded-For 可能包含多个IP，取第一个（真实客户端IP）
            client_ip = client_ip.split(",")[0].strip()
        else:
            client_ip = request.client.host if request.client else "unknown"

        # 检查是否是本地子网（放行）
        if self._is_local_subnet(client_ip):
            response = await call_next(request)
            return response

        # 检查是否是免认证路径
        if self._is_exempt_path(request.url.path):
            response = await call_next(request)
            return response

        # 验证token
        token = request.headers.get("X-Token")

        if not token or token != settings.upload_token:
            # 如果是API请求，返回401
            if request.url.path.startswith("/assist/api/") or request.url.path.startswith("/assist/openapi/"):
                return JSONResponse(
                    status_code=401,
                    content={"detail": "未授权访问，请提供有效的token"}
                )
            # 如果是页面请求，重定向到登录页面
            return RedirectResponse(url="/assist/login", status_code=302)

        response = await call_next(request)
        return response


@asynccontextmanager
async def lifespan(app: FastAPI):
    """Application lifespan: startup and shutdown."""
    # Startup
    init_db()
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
    # Shutdown (nothing to clean up)


app = FastAPI(title="OKB-Assist", description="论文与专著数据管理系统", lifespan=lifespan)

# CORS middleware - 允许所有来源（开发环境）
app.add_middleware(
    CORSMiddleware,
    allow_origins=["*"],
    allow_credentials=True,
    allow_methods=["*"],
    allow_headers=["*"],
)

# Auth middleware - 认证中间件
app.add_middleware(AuthMiddleware)

# Mount static files
app.mount("/assist/static", StaticFiles(directory="static"), name="static")

# Mount uploads directory for serving images
app.mount("/assist/uploads", StaticFiles(directory="uploads"), name="uploads")

# Templates
templates = Jinja2Templates(directory="app/templates")

# Include routers
app.include_router(documents.router)
app.include_router(pipeline.router)
app.include_router(admin.router)
app.include_router(openapi.router)


@app.get("/")
def root():
    return RedirectResponse(url="/assist/")


@app.get("/assist/login")
def login_page(request: Request):
    return templates.TemplateResponse(name="login.html", request=request)


@app.get("/assist/")
def index(request: Request):
    return templates.TemplateResponse(name="index.html", request=request)


@app.get("/assist/upload")
def upload_page(request: Request):
    return templates.TemplateResponse(name="upload.html", request=request)


@app.get("/assist/detail/{doc_id}")
def detail_page(request: Request, doc_id: int):
    return templates.TemplateResponse(name="detail.html", request=request, context={"doc_id": doc_id})


@app.get("/assist/admin")
def admin_page(request: Request):
    return templates.TemplateResponse(name="admin.html", request=request)


@app.get("/assist/markdown/{doc_id}")
def markdown_page(request: Request, doc_id: int):
    return templates.TemplateResponse(name="markdown.html", request=request, context={"doc_id": doc_id})


@app.get("/assist/monitor")
def monitor_page(request: Request):
    return templates.TemplateResponse(name="monitor.html", request=request)


@app.get("/assist/point")
def point_page(request: Request):
    return templates.TemplateResponse(name="point.html", request=request)


@app.get("/assist/tools")
def tools_page(request: Request):
    return templates.TemplateResponse(name="tools.html", request=request)


@app.get("/assist/doc/{doc_id}")
def doc_manage_page(request: Request, doc_id: int):
    return templates.TemplateResponse(name="doc_manage.html", request=request, context={"doc_id": doc_id})


if __name__ == "__main__":
    import uvicorn
    uvicorn.run("main:app", host="0.0.0.0", port=5001, reload=True)
