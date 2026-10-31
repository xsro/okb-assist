import axios, { AxiosInstance, AxiosRequestConfig, AxiosResponse } from 'axios'

// ── Token 管理 ──────────────────────────────────────────

export const tokenStorage = {
  get(): string | null {
    return localStorage.getItem('okb_token')
  },
  set(token: string) {
    localStorage.setItem('okb_token', token)
  },
  clear() {
    localStorage.removeItem('okb_token')
  }
}

// ── 本地记住的账号（只存账号元信息，绝不保存任何凭证） ──────────────────
//
// 用途：登出后在登录页列出「本浏览器登录过的账号」，点一下自动填好用户名，
// 仍需输入密码。**不存 token**：登出会吊销服务端会话，保存的 `sess_` 也没用了，
// 长期保存明文凭证本身也是风险。

export interface SavedAccount {
  username: string
  displayName: string
  role: string
  /** 上次成功登录时间（毫秒时间戳） */
  lastUsed: number
}

const ACCOUNTS_KEY = 'okb_accounts'
/** 旧版本把明文 token 存在这个键里，已废弃（读到就删） */
const LEGACY_TOKEN_HISTORY_KEY = 'okb_token_history'
const MAX_ACCOUNTS = 10

function normalizeAccount(raw: unknown): SavedAccount | null {
  const item = raw as Partial<SavedAccount> | null
  const username = typeof item?.username === 'string' ? item.username.trim() : ''
  if (!username) return null
  return {
    username,
    displayName: typeof item?.displayName === 'string' ? item.displayName : '',
    role: typeof item?.role === 'string' ? item.role : '',
    lastUsed: typeof item?.lastUsed === 'number' ? item.lastUsed : 0
  }
}

export const accountStorage = {
  get(): SavedAccount[] {
    try {
      const raw = JSON.parse(localStorage.getItem(ACCOUNTS_KEY) || '[]')
      if (!Array.isArray(raw)) return []
      return raw
        .map(normalizeAccount)
        .filter((a): a is SavedAccount => !!a)
        .sort((a, b) => b.lastUsed - a.lastUsed)
        .slice(0, MAX_ACCOUNTS)
    } catch {
      return []
    }
  },
  /** 记录/刷新一个账号（按用户名归并，最近登录的排最前） */
  remember(account: { username: string; displayName?: string; role?: string }) {
    const username = (account.username || '').trim()
    if (!username) return
    const list = this.get().filter((a) => a.username !== username)
    list.unshift({
      username,
      displayName: (account.displayName || '').trim(),
      role: account.role || '',
      lastUsed: Date.now()
    })
    localStorage.setItem(ACCOUNTS_KEY, JSON.stringify(list.slice(0, MAX_ACCOUNTS)))
  },
  forget(username: string) {
    const list = this.get().filter((a) => a.username !== username)
    localStorage.setItem(ACCOUNTS_KEY, JSON.stringify(list))
  },
  clear() {
    localStorage.removeItem(ACCOUNTS_KEY)
  },
  /** 清理旧版本留在本地的明文 token 历史 */
  purgeLegacy() {
    try {
      localStorage.removeItem(LEGACY_TOKEN_HISTORY_KEY)
    } catch {
      /* 忽略 */
    }
  }
}

// ── Axios 实例 ──────────────────────────────────────────

const client: AxiosInstance = axios.create({
  baseURL: '',
  timeout: 30000,
  headers: {
    'Content-Type': 'application/json'
  }
})

// 请求拦截：注入 X-Token
client.interceptors.request.use(
  (config: AxiosRequestConfig) => {
    const token = tokenStorage.get()
    if (token && config.headers) {
      ;(config.headers as Record<string, string>)['X-Token'] = token
    }
    return config
  },
  (error: unknown) => Promise.reject(error)
)

// 响应拦截：401 处理（登录/注册/探测接口的 401 属于正常业务错误，不跳转）
const AUTH_ENDPOINTS_WITHOUT_REDIRECT = [
  '/assist/api/auth/login',
  '/assist/api/auth/register',
  '/assist/api/auth/bootstrap',
  '/assist/api/auth/status',
  '/assist/api/auth/check'
]

client.interceptors.response.use(
  (response: AxiosResponse) => response,
  (error: unknown) => {
    const axiosError = error as { config?: { url?: string }; response?: { status?: number } }
    if (axiosError.response?.status === 401) {
      const url = axiosError.config?.url || ''
      const isAuthEndpoint = AUTH_ENDPOINTS_WITHOUT_REDIRECT.some((p) => url.includes(p))
      if (!isAuthEndpoint) {
        tokenStorage.clear()
        window.dispatchEvent(new CustomEvent('auth:required'))
      }
    }
    return Promise.reject(error)
  }
)

export default client

// ── 通用请求包装 ────────────────────────────────────────

export async function apiGet<T>(path: string, params?: Record<string, unknown>): Promise<T> {
  const res = await client.get<T>(path, { params })
  return res.data
}

export async function apiPost<T>(
  path: string,
  data?: unknown,
  config?: AxiosRequestConfig
): Promise<T> {
  const res = await client.post<T>(path, data, config)
  return res.data
}

export async function apiPut<T>(path: string, data?: unknown): Promise<T> {
  const res = await client.put<T>(path, data)
  return res.data
}

export async function apiDelete<T>(path: string): Promise<T> {
  const res = await client.delete<T>(path)
  return res.data
}

export async function apiUpload<T>(
  path: string,
  formData: FormData,
  config?: AxiosRequestConfig
): Promise<T> {
  const res = await client.post<T>(path, formData, {
    ...config,
    headers: { 'Content-Type': 'multipart/form-data', ...(config?.headers || {}) }
  })
  return res.data
}