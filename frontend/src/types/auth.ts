/** 用户与认证相关类型定义 */

export type Role = 'admin' | 'view-only' | 'view-upload'
export type UserStatus = 'pending' | 'active' | 'disabled'

/** 个人令牌的权限作用域：严格二选一（api 只能调 API，mcp 只能调 MCP） */
export type TokenScope = 'api' | 'mcp'

/** 用户的一条长期凭证（API / MCP token）元数据 —— 绝不含明文或哈希 */
export interface ApiTokenMeta {
  id: string
  label: string
  scope: TokenScope
  created_at: string
  last_used_at: string | null
}

/** 用户（后端 `to_public_json`） */
export interface User {
  id: string
  username: string
  display_name: string
  role: Role
  status: UserStatus
  mcp: boolean
  has_password: boolean
  must_set_password: boolean
  created_at: string
  updated_at: string
  last_login_at: string | null
  api_tokens: ApiTokenMeta[]
}

/** `GET /assist/api/auth/status`（免凭证） */
export interface AuthStatus {
  initialized: boolean
  user_count: number
  pending_count: number
  allow_registration: boolean
  require_approval: boolean
  registration_role: Role
  session_ttl_hours: number
  version: number
}

/** 登录 / 初始化成功后的响应 */
export interface LoginResponse {
  token: string
  expires_at: string
  role: Role
  permissions: Record<string, boolean>
  must_set_password: boolean
  user: User
}

/** `GET /assist/api/auth/check`（兼容旧前端的角色探测） */
export interface AuthCheckResponse {
  role: Role | null
  valid: boolean
  permissions: Record<string, boolean>
  mcp?: boolean
  via?: string
  must_set_password?: boolean
  user?: User | null
}

/** `GET /assist/api/auth/me` */
export interface MeResponse {
  user: User
  role: Role
  mcp: boolean
  via: string
  permissions: Record<string, boolean>
}

export interface UsersListResponse {
  count: number
  pending: number
  users: User[]
}

export interface UsersSettings {
  allow_registration: boolean
  require_approval: boolean
  registration_role: Role
  session_ttl_hours: number
}

export interface TokenIssueResponse {
  token: string
  meta: ApiTokenMeta
  detail: string
}

export interface TokensListResponse {
  tokens: ApiTokenMeta[]
  mcp_allowed?: boolean
}
