/** 用户管理 API（admin） */
import { apiDelete, apiGet, apiPost, apiPut } from '@/api/client'
import type {
  ApiTokenMeta,
  Role,
  TokenIssueResponse,
  TokenScope,
  User,
  UsersListResponse,
  UsersSettings,
  UserStatus
} from '@/types/auth'

export function listUsers(status?: UserStatus): Promise<UsersListResponse> {
  return apiGet<UsersListResponse>('/assist/api/admin/users', status ? { status } : undefined)
}

export function createUser(payload: {
  username: string
  password?: string
  display_name?: string
  role?: Role
  status?: UserStatus
  mcp?: boolean
}): Promise<{ user: User }> {
  return apiPost('/assist/api/admin/users', payload)
}

export function updateUser(
  id: string,
  patch: {
    display_name?: string
    role?: Role
    status?: UserStatus
    mcp?: boolean
  }
): Promise<{ user: User }> {
  return apiPut(`/assist/api/admin/users/${encodeURIComponent(id)}`, patch)
}

/** 审核通过（pending → active） */
export function approveUser(id: string): Promise<{ user: User }> {
  return apiPost(`/assist/api/admin/users/${encodeURIComponent(id)}/approve`)
}

export function resetUserPassword(
  id: string,
  password: string,
  revokeTokens = false
): Promise<{ ok: boolean; user: User; revoked_sessions: number; revoked_tokens: number }> {
  return apiPost(`/assist/api/admin/users/${encodeURIComponent(id)}/password`, {
    password,
    revoke_tokens: revokeTokens
  })
}

export function deleteUser(id: string): Promise<{ ok: boolean; username: string }> {
  return apiDelete(`/assist/api/admin/users/${encodeURIComponent(id)}`)
}

// ── 用户的 token（admin 视角） ──────────────────────────

export function listUserTokens(id: string): Promise<{ tokens: ApiTokenMeta[] }> {
  return apiGet(`/assist/api/admin/users/${encodeURIComponent(id)}/tokens`)
}

export function issueUserToken(
  id: string,
  payload: { label: string; scope: TokenScope; token?: string }
): Promise<TokenIssueResponse> {
  return apiPost(`/assist/api/admin/users/${encodeURIComponent(id)}/tokens`, payload)
}

export function revokeUserToken(id: string, tokenId: string): Promise<{ ok: boolean }> {
  return apiDelete(
    `/assist/api/admin/users/${encodeURIComponent(id)}/tokens/${encodeURIComponent(tokenId)}`
  )
}

// ── 注册策略 / 会话 TTL ─────────────────────────────────

export function getUsersSettings(): Promise<UsersSettings> {
  return apiGet<UsersSettings>('/assist/api/admin/users/settings')
}

export function updateUsersSettings(patch: Partial<UsersSettings>): Promise<UsersSettings> {
  return apiPut('/assist/api/admin/users/settings', patch)
}
