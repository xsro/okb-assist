<template>
  <div id="app">
    <!-- bare 路由（如全屏 Markdown 编辑）隐藏全局导航与页脚，把整块视口交给页面 -->
    <AppHeader v-if="!isBareRoute" />
    <MobileMenu v-if="showMobileMenu && !isBareRoute" @close="showMobileMenu = false" />
    <Toast />
    <main class="main-content" :class="{ 'main-content-bare': isBareRoute }">
      <ErrorBoundary>
        <router-view />
      </ErrorBoundary>
    </main>
    <TokenModal />
    <footer v-if="!isBareRoute && route.name !== 'login'" class="app-footer">
      <span>OKB-Assist</span>
    </footer>
  </div>
</template>

<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref, provide } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import { useTokenStore } from '@/stores/token'
import { useErrorStore } from '@/stores/error'
import AppHeader from '@/components/AppHeader.vue'
import Toast from '@/components/Toast.vue'
import TokenModal from '@/components/TokenModal.vue'
import MobileMenu from '@/components/MobileMenu.vue'
import ErrorBoundary from '@/components/ErrorBoundary.vue'

const tokenStore = useTokenStore()
const showMobileMenu = ref(false)

const route = useRoute()
const router = useRouter()
/** 全屏路由：隐藏 AppHeader / 页脚 / 内边距（路由 meta.bare = true） */
const isBareRoute = computed(() => route.meta?.bare === true)

// 向子组件提供菜单控制函数
provide('toggleMobileMenu', () => {
  showMobileMenu.value = !showMobileMenu.value
})
provide('closeMobileMenu', () => {
  showMobileMenu.value = false
})

function onAuthRequired() {
  tokenStore.clearToken()
  const current = router.currentRoute.value
  if (current.name !== 'login') {
    router.replace({ path: '/assist/login', query: { redirect: current.fullPath } })
  }
}

onMounted(async () => {
  window.addEventListener('auth:required', onAuthRequired)
  // 等首次导航完成：否则硬导航（直接输入/外链打开 URL）时 `route` 还是初始 START_LOCATION，
  // `route.fullPath` 会是 `/`，把 ?redirect= 覆盖成 `/`（深链登录后回跳失效）。
  await router.isReady()
  // 启动时用本地 token 恢复会话（刷新页面后重新取角色/权限）
  if (tokenStore.token) {
    await tokenStore.restore()
    if (!tokenStore.role) {
      onAuthRequired()
    }
  } else if (route.name !== 'login') {
    router.replace({ path: '/assist/login', query: { redirect: route.fullPath } })
  }
})

// 处理全局的 Vue 运行时警告，记录到错误日志
const errorStore = useErrorStore()
const originalConsoleError = console.error
console.error = (...args: unknown[]) => {
  const msg = args.map(String).join(' ')
  // 过滤掉 vue-tsc 等不相关的警告
  if (msg.includes('[Vue warn]')) {
    errorStore.add({ message: msg, source: 'vue' })
  }
  originalConsoleError.apply(console, args)
}

onUnmounted(() => {
  console.error = originalConsoleError
  window.removeEventListener('auth:required', onAuthRequired)
})
</script>

<style scoped>
.main-content {
  min-height: calc(100vh - 56px);
  padding: 20px 24px;
  max-width: 1400px;
  margin: 0 auto;
}
.main-content-bare {
  height: 100vh;
  min-height: 0;
  max-width: none;
  margin: 0;
  padding: 0;
  display: flex;
  flex-direction: column;
  overflow: hidden;
}
/* 全屏（bare）路由：错误边界外壳必须是纵向 flex 容器，
   否则内部 height:100% / flex:1 的页面（如 Markdown 编辑器）会塌陷成内容高度 */
.main-content-bare :deep(.error-boundary-wrap) {
  display: flex;
  flex-direction: column;
  flex: 1;
  min-height: 0;
}
.app-footer {
  text-align: center;
  padding: 16px;
  color: #999;
  font-size: 12px;
  border-top: 1px solid #eee;
  background: #fafafa;
}
</style>