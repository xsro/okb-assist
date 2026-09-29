<template>
  <div id="app">
    <AppHeader />
    <MobileMenu v-if="showMobileMenu" @close="showMobileMenu = false" />
    <Toast />
    <main class="main-content">
      <ErrorBoundary>
        <router-view />
      </ErrorBoundary>
    </main>
    <TokenModal />
    <footer class="app-footer">
      <span>OKB-Assist</span>
    </footer>
  </div>
</template>

<script setup lang="ts">
import { onMounted, onUnmounted, ref, provide } from 'vue'
import { useTokenStore } from '@/stores/token'
import { useErrorStore } from '@/stores/error'
import AppHeader from '@/components/AppHeader.vue'
import Toast from '@/components/Toast.vue'
import TokenModal from '@/components/TokenModal.vue'
import MobileMenu from '@/components/MobileMenu.vue'
import ErrorBoundary from '@/components/ErrorBoundary.vue'

const tokenStore = useTokenStore()
const showMobileMenu = ref(false)

// 向子组件提供菜单控制函数
provide('toggleMobileMenu', () => {
  showMobileMenu.value = !showMobileMenu.value
})
provide('closeMobileMenu', () => {
  showMobileMenu.value = false
})

onMounted(() => {
  window.addEventListener('auth:required', () => {
    tokenStore.promptForToken()
  })
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
})
</script>

<style scoped>
.main-content {
  min-height: calc(100vh - 56px);
  padding: 20px 24px;
  max-width: 1400px;
  margin: 0 auto;
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