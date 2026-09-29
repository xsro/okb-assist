import { createApp } from 'vue'
import { createPinia } from 'pinia'
import router from '@/router'
import App from '@/App.vue'
import { useErrorStore } from '@/stores/error'
import '@/styles/main.scss'

const app = createApp(App)
const pinia = createPinia()

app.use(pinia)
app.use(router)

// ── 全局 Vue 错误处理 ──────────────────────────────────

app.config.errorHandler = (err, instance, info) => {
  const errorStore = useErrorStore()
  const message = err instanceof Error ? err.message : String(err)
  errorStore.add({
    message: `${message}（${info}）`,
    source: 'vue',
    component: instance?.$options?.name || instance?.$el?.tagName || '(unknown)',
    stack: err instanceof Error ? err.stack : undefined
  })
}

app.config.warnHandler = (msg, instance, trace) => {
  // 将 Vue 警告也记录到错误日志（仅开发模式）
  if (import.meta.env.DEV) {
    const errorStore = useErrorStore()
    errorStore.add({
      message: `${msg}${trace ? '\n' + trace : ''}`,
      source: 'vue'
    })
  }
}

// ── 全局未捕获 Promise 异常 ────────────────────────────

const unhandledRejectionHandler = (event: PromiseRejectionEvent) => {
  const errorStore = useErrorStore()
  const reason = event.reason
  const message = reason instanceof Error ? reason.message : String(reason)
  errorStore.add({
    message,
    source: 'promise',
    stack: reason instanceof Error ? reason.stack : undefined
  })
}
window.addEventListener('unhandledrejection', unhandledRejectionHandler)

// ── 全局 JS 运行时错误 ──────────────────────────────────

const errorHandler = (event: ErrorEvent) => {
  const errorStore = useErrorStore()
  errorStore.add({
    message: event.message || 'Unknown script error',
    source: 'unhandled',
    stack: event.error?.stack
  })
}
window.addEventListener('error', errorHandler)

// ── 挂载应用 ────────────────────────────────────────────

app.mount('#app')