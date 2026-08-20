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

// 响应拦截：401 处理
client.interceptors.response.use(
  (response: AxiosResponse) => response,
  (error: unknown) => {
    const axiosError = error as { response?: { status?: number } }
    if (axiosError.response?.status === 401) {
      tokenStorage.clear()
      window.dispatchEvent(new CustomEvent('auth:required'))
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

export async function apiUpload<T>(path: string, formData: FormData): Promise<T> {
  const res = await client.post<T>(path, formData, {
    headers: { 'Content-Type': 'multipart/form-data' }
  })
  return res.data
}