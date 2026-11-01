/**
 * 认证状态（Pinia）。
 *
 * 升级用户体系后，这里从「只存一个 token」变成「存会话 + 当前用户 + 权限位」：
 * - 密码登录 → 后端签发会话 token（`sess_...`），存在 localStorage 的 `okb_token`；
 * - Token 登录 → 直接粘贴会话 token 或用户个人 API token（`okb_...`）；
 * - 刷新页面时 `restore()` 用本地 token 调 `/auth/me` 重新拉取角色与权限。
 *
 * 另存一份「本浏览器登录过的账号」（`localStorage.okb_accounts`，只有用户名/显示名/角色，**不存凭证**）：
 * 登出后登录页用它列出账号卡片，点一下只自动填用户名，仍需输入密码。
 * 页内没有「切换账号」入口 —— 换号一律先登出（会吊销服务端会话）再去登录页。
 *
 * 仍保留 `useTokenStore` 这个名字，避免大面积改动既有视图/组件。
 */
import { defineStore } from 'pinia'
import { accountStorage, apiGet, tokenStorage } from '@/api/client'
import type { SavedAccount } from '@/api/client'
import {
  bootstrap as apiBootstrap,
  changePassword as apiChangePassword,
  getAuthStatus,
  login as apiLogin,
  getMe,
  logout as apiLogout,
  register as apiRegister
} from '@/api/auth'
import type { AuthStatus, MeResponse, Role, User } from '@/types/auth'

export interface AuthInfo {
  role: Role | null
  permissions: Record<string, boolean>
}

interface State {
  token: string
  role: Role | null
  permissions: Record<string, boolean>
  user: User | null
  mcp: boolean
  /** 当前凭证来源：session / api_token / local_subnet */
  via: string
  mustSetPassword: boolean
  status: AuthStatus | null
  restoring: boolean
  showTokenModal: boolean
  /** 本浏览器登录过的账号（只有元信息，无凭证） */
  accounts: SavedAccount[]
}

function roleLabels(role: Role | null): string {
  const labels: Record<string, string> = {
    admin: '管理员',
    'view-only': '只读用户',
    'view-upload': '查看+上传'
  }
  return role ? labels[role] || role : '未登录'
}

export const useTokenStore = defineStore('token', {
  state: (): State => ({
    token: tokenStorage.get() || '',
    role: null,
    permissions: {},
    user: null,
    mcp: false,
    via: '',
    mustSetPassword: false,
    status: null,
    restoring: false,
    showTokenModal: false,
    accounts: accountStorage.get()
  }),

  getters: {
    isAuthenticated: (state) => !!state.token,
    isAdmin: (state) => state.role === 'admin',
    /** 显示名（优先备注名） */
    displayName: (state) => state.user?.display_name || state.user?.username || '',
    roleLabel: (state) => roleLabels(state.role),
    canViewPdf: (state) =>
      state.permissions['view_pdf'] ?? ['admin', 'view-only'].includes(state.role || ''),
    canViewMarkdown: (state) =>
      state.permissions['view_markdown'] ??
      ['admin', 'view-only', 'view-upload'].includes(state.role || ''),
    canEdit: (state) => state.permissions['edit'] ?? state.role === 'admin',
    canDelete: (state) => state.permissions['delete'] ?? state.role === 'admin',
    canAdmin: (state) => state.permissions['admin'] ?? state.role === 'admin',
    canConfig: (state) => state.permissions['config'] ?? state.role === 'admin',
    canUpload: (state) =>
      state.permissions['upload'] ?? ['admin', 'view-upload'].includes(state.role || ''),
    canPipeline: (state) => state.permissions['pipeline'] ?? state.role === 'admin',
    canMcp: (state) => state.permissions['mcp'] === true || state.mcp
  },

  actions: {
    /** 应用一份 `/auth/me` 结果 */
    applyMe(res: MeResponse) {
      this.role = res.role || null
      this.permissions = res.permissions || {}
      this.mcp = res.mcp === true
      this.user = res.user || null
      this.via = res.via || ''
      this.mustSetPassword = res.must_set_password === true
      this.rememberAccount()
    },

    /** 记住当前账号（登录页的「选择账号」列表用，不涉及任何凭证） */
    rememberAccount() {
      const username = this.user?.username
      if (!username) return
      accountStorage.remember({
        username,
        displayName: this.user?.display_name || '',
        role: this.role || ''
      })
      this.accounts = accountStorage.get()
    },

    /** Token 登录：校验并保存（会话 token 或用户 API token 均可）。失败时带回服务端提示 */
    async setToken(token: string): Promise<{ ok: boolean; detail?: string }> {
      this.token = token.trim()
      tokenStorage.set(this.token)
      try {
        this.applyMe(await getMe())
        return { ok: true }
      } catch (e) {
        this.clearToken()
        const err = e as { response?: { data?: { detail?: string } } }
        return { ok: false, detail: err.response?.data?.detail }
      }
    },

    /** 密码登录 */
    async login(username: string, password: string) {
      const res = await apiLogin(username, password)
      this.token = res.token
      tokenStorage.set(res.token)
      this.role = res.role
      this.permissions = res.permissions
      this.user = res.user
      this.mcp = res.user.mcp
      this.via = 'session'
      this.mustSetPassword = res.must_set_password
      this.rememberAccount()
      return res
    },

    /** 自助注册（可能待审核） */
    async register(username: string, password: string, displayName = '') {
      const res = await apiRegister({ username, password, display_name: displayName })
      await this.refreshStatus()
      return res
    },

    /** 首次初始化：创建首个管理员并直接登录 */
    async bootstrap(username: string, password: string, displayName = '') {
      const res = await apiBootstrap({
        username: username || undefined,
        password,
        display_name: displayName || undefined
      })
      this.token = res.token
      tokenStorage.set(res.token)
      this.role = res.role
      this.permissions = res.permissions
      this.user = res.user
      this.mcp = res.user.mcp
      this.via = 'session'
      this.mustSetPassword = res.must_set_password
      this.rememberAccount()
      await this.refreshStatus()
      return res
    },

    /** 登出：先撤销服务端会话，再清本地（账号列表保留，供登录页快速选账号） */
    async logout() {
      // 登出前刷新一次快照（角色/备注名可能在本次会话内变过）
      this.rememberAccount()
      try {
        if (this.token) await apiLogout()
      } catch {
        /* 忽略：本地照样清 */
      }
      this.clearToken()
    },

    clearToken() {
      this.token = ''
      this.role = null
      this.permissions = {}
      this.user = null
      this.mcp = false
      this.via = ''
      this.mustSetPassword = false
      tokenStorage.clear()
    },

    /** 启动时恢复：用本地 token 重新取角色与权限 */
    async restore() {
      if (!this.token) return
      this.restoring = true
      try {
        this.applyMe(await getMe())
      } catch {
        // 令牌过期 / 被撤销 / MCP 专属令牌：静默清掉本地状态
        this.clearToken()
      } finally {
        this.restoring = false
      }
    },

    /** 拉取系统认证状态（登录页用） */
    async refreshStatus() {
      try {
        this.status = await getAuthStatus()
      } catch {
        this.status = null
      }
      return this.status
    },

    /** 刷新当前用户信息（改密 / 改角色后） */
    async refreshMe() {
      try {
        const res = await apiGet<{
          user: User
          role: Role
          mcp: boolean
          permissions: Record<string, boolean>
        }>('/assist/api/auth/me')
        this.user = res.user
        this.role = res.role
        this.mcp = res.mcp
        this.via = res.via
        this.permissions = res.permissions
        this.mustSetPassword = res.user.must_set_password
        this.rememberAccount()
        return res
      } catch {
        return null
      }
    },

    /** 修改自己的密码（成功后后端会撤销会话，需要重新登录） */
    async changePassword(newPassword: string, oldPassword = '') {
      const res = await apiChangePassword(newPassword, oldPassword)
      this.user = res.user
      this.mustSetPassword = false
      return res
    },

    /** 从登录页的账号列表里移除一个账号 */
    forgetAccount(username: string) {
      accountStorage.forget(username)
      this.accounts = accountStorage.get()
    },

    clearAccounts() {
      accountStorage.clear()
      this.accounts = []
    },

    /** 打开账号面板（已登录时的当前账号 / 登出 / 账户设置） */
    promptForToken() {
      this.showTokenModal = true
    },

    closeTokenModal() {
      this.showTokenModal = false
    }
  }
})

/** 语义化别名：新代码可以叫 useAuthStore */
export const useAuthStore = useTokenStore

export { roleLabels }
