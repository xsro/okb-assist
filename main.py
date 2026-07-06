from contextlib import asynccontextmanager
import ipaddress

from fastapi import Request, HTTPException, Depends
from fastapi.responses import RedirectResponse, JSONResponse
from fastapi.staticfiles import StaticFiles
from fastapi.templating import Jinja2Templates
from fastapi import FastAPI
from fastapi.middleware.cors import CORSMiddleware
from starlette.middleware.base import BaseHTTPMiddleware

from app.database import init_db
from app.config import get_settings
from app.auth import get_current_user, is_authenticated
from app.routers import documents, pipeline, admin, openapi

settings = get_settings()

# 192.168 子网（需要排除的 nginx/frpc 代理 IP）
PROXY_IPS = {"192.168.1.162"}


class AuthMiddleware(BaseHTTPMiddleware):
    """
    认证中间件

    免认证路径：
    - 页面: /assist/, /assist/detail/, /assist/monitor, /assist/login, /assist/markdown/
    - 静态资源: /assist/static/, /assist/uploads/
    - 只读 API: GET /assist/api/documents/, GET /assist/api/documents/{id}

    需要认证的路径：
    - 其他所有路径
    """

    # 免认证的页面路径前缀
    EXEMPT_PAGE_PATHS = {
        "/assist/login",
        "/assist/static/",
        "/assist/uploads/",
        "/assist/monitor",
        "/assist/detail/",
        "/assist/markdown/",
    }

    # 免认证的精确路径
    EXEMPT_EXACT_PATHS = {"/assist/", "/assist"}

    # 免认证的 API 端点
    EXEMPT_API_ENDPOINTS = {
        "/assist/api/auth/token",
        "/assist/api/auth/check",
    }

    # 免认证的 API 路径（只读）
    EXEMPT_API_PATHS = {
        "/assist/api/documents/search",
        "/assist/api/documents/",
    }

    def _is_local_subnet(self, client_ip: str) -> bool:
        """检查是否是 192.168 子网（排除代理服务器 IP）"""
        try:
            if client_ip in PROXY_IPS:
                return False

            ip = ipaddress.ip_address(client_ip)
            if ip.version == 4 and str(ip).startswith("192.168."):
                return True
        except ValueError:
            pass
        return False

    def _is_exempt_path(self, path: str) -> bool:
        """检查路径是否免认证"""
        # 精确路径
        if path in self.EXEMPT_EXACT_PATHS:
            return True

        # 免认证的 API 端点
        if path in self.EXEMPT_API_ENDPOINTS:
            return True

        # 页面路径前缀
        for exempt in self.EXEMPT_PAGE_PATHS:
            if path.startswith(exempt):
                return True

        # 只读 API（GET 请求）
        for exempt in self.EXEMPT_API_PATHS:
            if path.startswith(exempt) or path == exempt.rstrip("/"):
                return True

        # 特殊处理: /assist/api/documents/{id} (GET)
        if path.startswith("/assist/api/documents/") and path.count("/") == 4:
            # 可能是 /assist/api/documents/123 这种格式
            parts = path.split("/")
            if len(parts) == 5 and parts[4].isdigit():
                return True

        return False

    def _is_get_request(self, request: Request) -> bool:
        """检查是否是 GET 请求"""
        return request.method == "GET"

    async def dispatch(self, request: Request, call_next):
        path = request.url.path

        # 获取客户端 IP
        client_ip = request.headers.get("X-Forwarded-For", "")
        if client_ip:
            client_ip = client_ip.split(",")[0].strip()
        else:
            client_ip = request.client.host if request.client else "unknown"

        # 192.168 子网直接放行
        if self._is_local_subnet(client_ip):
            return await call_next(request)

        # 免认证路径放行
        if self._is_exempt_path(path):
            return await call_next(request)

        # 验证 token
        authenticated = is_authenticated(request)

        if not authenticated:
            # API 请求返回 401
            if path.startswith("/assist/api/") or path.startswith("/assist/openapi/"):
                return JSONResponse(
                    status_code=401,
                    content={"detail": "未授权访问，请提供有效的token"}
                )
            # 页面请求重定向到登录页面
            return RedirectResponse(url="/assist/login", status_code=302)

        return await call_next(request)


@asynccontextmanager
async def lifespan(app: FastAPI):
    """Application lifespan: startup and shutdown."""
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
app.mount("/assist/uploads", StaticFiles(directory="uploads"), name="uploads")

# Templates
templates = Jinja2Templates(directory="app/templates")

# Include routers
app.include_router(documents.router)
app.include_router(pipeline.router)
app.include_router(admin.router)
app.include_router(openapi.router)


# ==================== 认证端点 ====================

@app.post("/assist/api/auth/token")
async def auth_token(request: Request):
    """
    Token 认证端点（兼容 OAuth2PasswordBearer）

    实际上我们使用预设的 token，这里只是兼容 Swagger UI
    """
    # 支持 form-data 或 JSON
    try:
        body = await request.json()
        token = body.get("token") or body.get("access_token")
    except:
        form = await request.form()
        token = form.get("token") or form.get("access_token")

    if not token:
        # 从 header 获取
        token = request.headers.get("X-Token")

    if not token:
        # 从 cookie 获取
        token = request.cookies.get("x_token")

    if token == settings.upload_token:
        return {"access_token": token, "token_type": "bearer"}

    raise HTTPException(status_code=401, detail="无效的 token")


@app.get("/assist/api/auth/check")
async def auth_check(request: Request):
    """检查认证状态"""
    from app.auth import is_authenticated
    authenticated = is_authenticated(request)
    return {"authenticated": authenticated}


# ==================== 公开页面（无需认证） ====================

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


# ==================== 需要认证的页面 ====================

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


@app.get("/assist/doc/{doc_id}")
def doc_manage_page(request: Request, doc_id: int, user: str = Depends(get_current_user)):
    return templates.TemplateResponse(name="doc_manage.html", request=request, context={"doc_id": doc_id})


if __name__ == "__main__":
    import uvicorn
    uvicorn.run("main:app", host="0.0.0.0", port=5001, reload=True)
