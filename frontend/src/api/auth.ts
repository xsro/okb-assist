/** 认证 / 账户相关 API */
import { apiDelete, apiGet, apiPost } from '@/api/client'
import type {
  AuthCheckResponse,
  AuthStatus,
  LoginResponse,
  MeResponse,
  TokenIssueResponse,
  TokenScope,
  TokensListResponse,
  User
} from '@/types/auth'

/** 系统认证状态（无需凭证，登录页据此决定展示登录 / 注册 / 初始化） */
export function getAuthStatus(): Promise<AuthStatus> {
  return apiGet<AuthStatus>('/assist/api/auth/status')
}

/** 密码登录 → 会话 token */
export function login(username: string, password: string): Promise<LoginResponse> {
  return apiPost<LoginResponse>('/assist/api/auth/login', { username, password })
}

/** 自助注册（可能进入待审核状态） */
export function register(payload: {
  username: string
  password: string
  display_name?: string
}): Promise<{ user: User; pending_approval: boolean; detail: string }> {
  return apiPost('/assist/api/auth/register', payload)
}

/** 首次初始化：创建首个管理员并直接登录 */
export function bootstrap(payload: {
  username?: string
  password: string
  display_name?: string
}): Promise<LoginResponse> {
  return apiPost<LoginResponse>('/assist/api/auth/bootstrap', payload)
}

/** 撤销当前会话 */
export function logout(): Promise<{ ok: boolean; revoked: boolean }> {
  return apiPost('/assist/api/auth/logout')
}

/** 角色 / 权限探测（兼容旧接口，也用于 Token 登录与启动时恢复会话） */
export function checkAuth(): Promise<AuthCheckResponse> {
  return apiGet<AuthCheckResponse>('/assist/api/auth/check')
}

export function getMe(): Promise<MeResponse> {
  return apiGet<MeResponse>('/assist/api/auth/me')
}

/** 修改自己的密码（有密码时需提供 old_password） */
export function changePassword(
  newPassword: string,
  oldPassword = ''
): Promise<{ ok: boolean; revoked_sessions: number; user: User }> {
  return apiPost('/assist/api/auth/password', {
    new_password: newPassword,
    old_password: oldPassword
  })
}

// ── 我的访问令牌（自助） ────────────────────────────────

export function listMyTokens(): Promise<TokensListResponse> {
  return apiGet<TokensListResponse>('/assist/api/auth/tokens')
}

/** 签发个人 token（明文只返回一次）。`token` 留空 = 自动生成 okb_… */
export function issueMyToken(payload: {
  label: string
  scope: TokenScope
  token?: string
}): Promise<TokenIssueResponse> {
  return apiPost<TokenIssueResponse>('/assist/api/auth/tokens', payload)
}

export function revokeMyToken(tokenId: string): Promise<{ ok: boolean }> {
  return apiDelete(`/assist/api/auth/tokens/${encodeURIComponent(tokenId)}`)
}
