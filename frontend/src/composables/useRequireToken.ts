import { useTokenStore } from '@/stores/token'

export function useRequireToken() {
  const tokenStore = useTokenStore()

  function requireToken(): boolean {
    if (!tokenStore.isAuthenticated) {
      tokenStore.promptForToken()
      return false
    }
    return true
  }

  return { requireToken }
}
