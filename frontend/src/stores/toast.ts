import { defineStore } from 'pinia'
import { ref } from 'vue'

export type ToastType = 'success' | 'error' | 'info'

export interface ToastItem {
  id: number
  message: string
  type: ToastType
  remaining: number  // 倒计时剩余秒数
  duration: number   // 总时长（秒）
}

export const useToastStore = defineStore('toast', () => {
  const toasts = ref<ToastItem[]>([])
  let nextId = 0

  function add(message: string, type: ToastType = 'info', duration: number = 10) {
    const id = ++nextId
    const seconds = Math.max(1, duration)
    toasts.value.push({ id, message, type, remaining: seconds, duration: seconds })

    // 每秒更新倒计时
    const timer = setInterval(() => {
      const toast = toasts.value.find((t) => t.id === id)
      if (!toast) {
        clearInterval(timer)
        return
      }
      toast.remaining -= 1
      if (toast.remaining <= 0) {
        clearInterval(timer)
        remove(id)
      }
    }, 1000)
  }

  function remove(id: number) {
    toasts.value = toasts.value.filter((t) => t.id !== id)
  }

  return { toasts, add, remove }
})
