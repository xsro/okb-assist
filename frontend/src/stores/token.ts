import { defineStore } from 'pinia'
import { tokenStorage, tokenHistoryStorage } from '@/api/client'
import { apiGet } from '@/api/client'

export interface AuthInfo {
  role: 'admin' | 'view-only' | 'view-upload' | null
  permissions: Record<string, boolean>
}

export const useTokenStore = defineStore('token', {
  state: (): {
    token: string
    role: AuthInfo['role'],
    permissions: Record<string, boolean>,
    showTokenModal: boolean,
    tokenHistory: { token: string; role: string }[]
  } => ({
    token: tokenStorage.get() || '',
    role: null,
    permissions: {},
    showTokenModal: false,
    tokenHistory: tokenHistoryStorage.get().map(h => ({ token: h.token, role: h.role }))
  }),
  getters: {
    isAuthenticated: (state) => !!state.token,
    canViewPdf: (state) => state.role === 'admin' || state.role === 'view-only' || state.role === 'view-upload',
    canViewMarkdown: (state) => state.role === 'admin' || state.role === 'view-only' || state.role === 'view-upload',
    canEdit: (state) => state.role === 'admin',
    canDelete: (state) => state.role === 'admin',
    canAdmin: (state) => state.role === 'admin',
    canConfig: (state) => state.role === 'admin',
    canUpload: (state) => state.role === 'admin' || state.role === 'view-upload',
    canPipeline: (state) => state.role === 'admin',
  },
  actions: {
    async setToken(token: string) {
      this.token = token
      tokenStorage.set(token)
      try {
        const res = await apiGet<{ role: string; permissions: Record<string, boolean> }>('/assist/api/auth/check')
        this.role = res.role as AuthInfo['role']
        this.permissions = res.permissions
        // 保存到历史
        if (this.role) {
          tokenHistoryStorage.add(token, this.role)
          this.tokenHistory = tokenHistoryStorage.get().map(h => ({ token: h.token, role: h.role }))
        }
      } catch {
        this.role = null
        this.permissions = {}
      }
    },
    clearToken() {
      this.token = ''
      this.role = null
      this.permissions = {}
      tokenStorage.clear()
    },
    removeFromHistory(token: string) {
      tokenHistoryStorage.remove(token)
      this.tokenHistory = tokenHistoryStorage.get().map(h => ({ token: h.token, role: h.role }))
    },
    clearHistory() {
      tokenHistoryStorage.clear()
      this.tokenHistory = []
    },
    promptForToken() {
      this.showTokenModal = true
    },
    closeTokenModal() {
      this.showTokenModal = false
    }
  }
})