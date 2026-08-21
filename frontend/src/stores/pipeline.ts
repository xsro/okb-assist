import { defineStore } from 'pinia'
import { getPipelineStatus, getActiveTasks, getBatchStatus } from '@/api/pipeline'
import type { PipelineStatus, ActiveTask, BatchProgress } from '@/types/pipeline'

export const usePipelineStore = defineStore('pipeline', {
  state: () => ({
    status: null as PipelineStatus | null,
    activeTasks: [] as ActiveTask[],
    batchProgress: null as BatchProgress | null,
    loading: false,
    error: null as string | null,
    _timer: null as ReturnType<typeof setInterval> | null
  }),
  getters: {
    queue: (state) =>
      state.status ?? {
        max_concurrent_tasks: 0,
        running_tasks: 0,
        available_slots: 0
      }
  },
  actions: {
    async fetchStatus() {
      this.loading = true
      this.error = null
      try {
        const [status, taskRes, batchRes] = await Promise.all([
          getPipelineStatus(),
          getActiveTasks(),
          getBatchStatus()
        ])
        this.status = status
        this.activeTasks = taskRes.tasks
        this.batchProgress = batchRes
      } catch (e) {
        this.error = e instanceof Error ? e.message : '获取流水线状态失败'
      } finally {
        this.loading = false
      }
    },
    startPolling(intervalMs = 3000) {
      this.stopPolling()
      this._timer = setInterval(() => this.fetchStatus(), intervalMs)
    },
    stopPolling() {
      if (this._timer) {
        clearInterval(this._timer)
        this._timer = null
      }
    }
  }
})
