"""
认证模块 - 使用 OAuth2PasswordBearer 进行 token 认证
"""

from fastapi import Depends, HTTPException, Request, status
from fastapi.security import OAuth2PasswordBearer
from app.config import get_settings

settings = get_settings()

# OAuth2 scheme - 定义 token 获取方式
oauth2_scheme = OAuth2PasswordBearer(
    tokenUrl="/assist/api/auth/token",
    auto_error=False  # 不自动抛出错误，我们自己处理
)


def verify_token(token: str) -> bool:
    """验证 token 是否有效"""
    return token == settings.upload_token


async def get_current_user(
    request: Request,
    token: str = Depends(oauth2_scheme)
) -> str:
    """
    获取当前用户（验证 token）

    支持从以下位置获取 token：
    1. Authorization header (Bearer token)
    2. Cookie (x_token)
    3. Query 参数 (token)
    """
    # 如果 OAuth2 没有获取到 token，尝试其他方式
    if not token:
        # 从 cookie 获取
        token = request.cookies.get("x_token")

    if not token:
        # 从 query 参数获取
        token = request.query_params.get("token")

    if not token or not verify_token(token):
        raise HTTPException(
            status_code=status.HTTP_401_UNAUTHORIZED,
            detail="未授权访问，请提供有效的token",
            headers={"WWW-Authenticate": "Bearer"},
        )

    return token


async def get_optional_user(
    request: Request,
    token: str = Depends(oauth2_scheme)
) -> str | None:
    """
    获取可选用户（不强制验证 token）

    如果 token 有效则返回 token，否则返回 None
    """
    # 如果 OAuth2 没有获取到 token，尝试其他方式
    if not token:
        token = request.cookies.get("x_token")

    if not token:
        token = request.query_params.get("token")

    if token and verify_token(token):
        return token

    return None


def is_authenticated(request: Request) -> bool:
    """
    检查请求是否已认证（用于中间件）
    """
    # 从 header 获取
    token = request.headers.get("X-Token")

    if not token:
        # 从 cookie 获取
        token = request.cookies.get("x_token")

    if not token:
        # 从 query 参数获取
        token = request.query_params.get("token")

    return token is not None and verify_token(token)
