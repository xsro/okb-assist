import { defineStore } from 'pinia'
import { ref, computed } from 'vue'

export interface ErrorRecord {
  id: number
  message: string
  source: 'vue' | 'unhandled' | 'promise' | 'component'
  timestamp: Date
  component?: string
  stack?: string
  acknowledged: boolean
}

export const useErrorStore = defineStore('error', () => {
  const errors = ref<ErrorRecord[]>([])
  const maxErrors = 50
  let nextId = 0

  /** 添加一条错误记录 */
  function add(err: Omit<ErrorRecord, 'id' | 'timestamp' | 'acknowledged'>) {
    const id = ++nextId
    errors.value.unshift({
      ...err,
      id,
      timestamp: new Date(),
      acknowledged: false
    })
    // 限制存储上限
    if (errors.value.length > maxErrors) {
      errors.value = errors.value.slice(0, maxErrors)
    }
  }

  /** 标记某条错误为已确认 */
  function acknowledge(id: number) {
    const e = errors.value.find((x) => x.id === id)
    if (e) e.acknowledged = true
  }

  /** 标记所有错误为已确认 */
  function acknowledgeAll() {
    errors.value.forEach((e) => (e.acknowledged = true))
  }

  /** 清除所有已确认的错误 */
  function clearAcknowledged() {
    errors.value = errors.value.filter((e) => !e.acknowledged)
  }

  /** 清除全部 */
  function clearAll() {
    errors.value = []
  }

  const unacknowledgedCount = computed(() =>
    errors.value.filter((e) => !e.acknowledged).length
  )

  const latestError = computed(() =>
    errors.value.length > 0 ? errors.value[0] : null
  )

  return {
    errors,
    unacknowledgedCount,
    latestError,
    add,
    acknowledge,
    acknowledgeAll,
    clearAcknowledged,
    clearAll
  }
})