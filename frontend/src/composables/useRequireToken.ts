import { useRoute, useRouter } from 'vue-router'
import { useTokenStore } from '@/stores/token'

/**
 * 需要登录才能执行的操作（上传 / 编辑 / 日志等）的前置校验。
 *
 * 未登录不再弹「Token 登录」面板，而是直接跳到登录页，并把当前地址作为回跳目标：
 * 登录页上可以选本机登录过的账号（仍需输入密码）或登录新账号。
 */
export function useRequireToken() {
  const tokenStore = useTokenStore()
  const router = useRouter()
  const route = useRoute()

  function requireToken(): boolean {
    if (!tokenStore.isAuthenticated) {
      router.replace({ path: '/assist/login', query: { redirect: route.fullPath } })
      return false
    }
    return true
  }

  return { requireToken }
}
