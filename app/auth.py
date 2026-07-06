"""Token authentication and trusted-LAN access policy."""

import ipaddress
import secrets

from fastapi import HTTPException, Request, status

from app.config import get_settings

settings = get_settings()

TOKEN_HEADER_NAME = "X-Token"
TOKEN_COOKIE_NAME = "x_token"
TOKEN_QUERY_PARAM = "token"
TRUSTED_LAN = ipaddress.ip_network("192.168.0.0/16")
EXCLUDED_LAN_IPS = {ipaddress.ip_address("192.168.1.162")}


def verify_token(token: str | None) -> bool:
    """验证 token 是否有效"""
    if not token or not settings.upload_token:
        return False
    return secrets.compare_digest(token, settings.upload_token)


def get_client_ip(request: Request) -> str:
    """Return the best client IP visible to the app."""
    forwarded_for = request.headers.get("X-Forwarded-For", "")
    if forwarded_for:
        return forwarded_for.split(",")[0].strip()

    real_ip = request.headers.get("X-Real-IP", "")
    if real_ip:
        return real_ip.strip()

    return request.client.host if request.client else "unknown"


def is_trusted_lan_ip(client_ip: str) -> bool:
    """192.168.0.0/16 is trusted except explicitly excluded hosts."""
    try:
        ip = ipaddress.ip_address(client_ip)
    except ValueError:
        return False

    if ip.version != 4 or ip in EXCLUDED_LAN_IPS:
        return False

    return ip in TRUSTED_LAN


def is_trusted_lan_client(request: Request) -> bool:
    """Check whether this request can bypass token auth."""
    return is_trusted_lan_ip(get_client_ip(request))


def get_token_from_request(request: Request) -> str | None:
    """Extract a token from supported request locations."""
    token = request.headers.get(TOKEN_HEADER_NAME)
    if token:
        return token.strip()

    authorization = request.headers.get("Authorization", "")
    scheme, _, value = authorization.partition(" ")
    if scheme.lower() == "bearer" and value.strip():
        return value.strip()

    token = request.cookies.get(TOKEN_COOKIE_NAME)
    if token:
        return token

    token = request.query_params.get(TOKEN_QUERY_PARAM)
    if token:
        return token

    return None


def is_authenticated(request: Request) -> bool:
    """检查请求是否携带了有效 token。"""
    return verify_token(get_token_from_request(request))


def is_request_authorized(request: Request) -> bool:
    """Trusted LAN clients are allowed; all other clients need a valid token."""
    return is_trusted_lan_client(request) or is_authenticated(request)


async def get_current_user(
    request: Request,
) -> str:
    """
    获取当前访问主体。

    192.168.0.0/16（排除 192.168.1.162）无需 token；其他来源必须提供有效 token。
    """
    if is_trusted_lan_client(request):
        return f"trusted-lan:{get_client_ip(request)}"

    token = get_token_from_request(request)
    if not verify_token(token):
        raise HTTPException(
            status_code=status.HTTP_401_UNAUTHORIZED,
            detail="未授权访问，请提供有效的token",
            headers={"WWW-Authenticate": "Bearer"},
        )

    return token


async def get_optional_user(
    request: Request,
) -> str | None:
    """
    获取可选访问主体（不强制验证 token）。

    如果来源是可信 LAN 或 token 有效则返回主体，否则返回 None。
    """
    if is_trusted_lan_client(request):
        return f"trusted-lan:{get_client_ip(request)}"

    token = get_token_from_request(request)
    if token and verify_token(token):
        return token

    return None
