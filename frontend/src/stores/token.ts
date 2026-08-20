import { defineStore } from 'pinia'
import { tokenStorage } from '@/api/client'

export const useTokenStore = defineStore('token', {
  state: () => ({
    token: tokenStorage.get() || '',
    showTokenModal: false
  }),
  getters: {
    isAuthenticated: (state) => !!state.token
  },
  actions: {
    setToken(token: string) {
      this.token = token
      tokenStorage.set(token)
    },
    clearToken() {
      this.token = ''
      tokenStorage.clear()
    },
    promptForToken() {
      this.showTokenModal = true
    },
    closeTokenModal() {
      this.showTokenModal = false
    }
  }
})
