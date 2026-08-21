<template>
  <div class="monitor-view">
    <div class="monitor-header">
      <h2>流水线监控</h2>
      <button class="btn btn-sm btn-outline" @click="refresh" :disabled="loading">
        刷新
      </button>
    </div>

    <!-- 队列概览 -->
    <div class="stats-grid">
      <div class="stat-card">
        <span class="stat-value">{{ queue.max_concurrent_tasks }}</span>
        <span class="stat-label">最大并发</span>
      </div>
      <div class="stat-card">
        <span class="stat-value">{{ queue.running_tasks }}</span>
        <span class="stat-label">运行中</span>
      </div>
      <div class="stat-card">
        <span class="stat-value">{{ queue.available_slots }}</span>
        <span class="stat-label">可用槽位</span>
      </div>
    </div>

    <!-- 活跃任务 -->
    <div class="section">
      <h3>活跃任务</h3>
      <table class="doc-table">
        <thead>
          <tr>
            <th>文档 ID</th>
            <th>标题</th>
            <th>任务类型</th>
            <th>状态</th>
            <th>开始时间</th>
          </tr>
        </thead>
        <tbody>
          <tr v-for="task in activeTasks" :key="task.doc_id">
            <td>{{ task.doc_id }}</td>
            <td>{{ task.doc_title }}</td>
            <td>{{ task.task_type }}</td>
            <td><StatusBadge :status="task.status" /></td>
            <td>{{ formatTime(task.started_at) }}</td>
          </tr>
        </tbody>
      </table>
      <div v-if="activeTasks.length === 0" class="empty-hint">
        暂无活跃任务
      </div>
    </div>

    <!-- 批量进度 -->
    <div class="section">
      <h3>批量进度</h3>
      <div v-if="batchProgress && batchProgress.active" class="batch-progress">
        <div class="progress-header">
          <span class="stage">{{ batchProgress.stage }}</span>
          <span v-if="batchProgress.vector_db_id" class="target-db">
            目标库: {{ batchProgress.vector_db_id }}
          </span>
        </div>
        <div class="progress-bar-wrap">
          <div
            class="progress-bar"
            :style="{ width: progressPercent + '%' }"
          ></div>
        </div>
        <div class="progress-stats">
          <span>总文档: {{ batchProgress.total }}</span>
          <span>已处理: {{ batchProgress.processed }}</span>
          <span>失败: {{ batchProgress.errors }}</span>
          <span v-if="batchProgress.total_batches > 0">
            批次: {{ batchProgress.current_batch }} / {{ batchProgress.total_batches }}
          </span>
        </div>
      </div>
      <div v-else class="empty-hint">
        无批量任务运行
      </div>
    </div>
  </div>
</template>

<script setup lang="ts">
import { onMounted, onUnmounted, computed } from 'vue'
import { usePipelineStore } from '@/stores/pipeline'
import StatusBadge from '@/components/StatusBadge.vue'

const pipelineStore = usePipelineStore()
const loading = computed(() => pipelineStore.loading)
const queue = computed(() => pipelineStore.queue)
const activeTasks = computed(() => pipelineStore.activeTasks)
const batchProgress = computed(() => pipelineStore.batchProgress)

const progressPercent = computed(() => {
  const p = batchProgress.value
  if (!p || p.total === 0) return 0
  return Math.min(100, Math.round((p.processed / p.total) * 100))
})

function refresh() {
  pipelineStore.fetchStatus()
}

function formatTime(iso: string) {
  return new Date(iso).toLocaleString('zh-CN')
}

onMounted(() => {
  pipelineStore.fetchStatus()
  pipelineStore.startPolling(3000)
})

onUnmounted(() => {
  pipelineStore.stopPolling()
})
</script>

<style scoped>
.monitor-header {
  display: flex;
  justify-content: space-between;
  align-items: center;
  margin-bottom: 20px;
}
.empty-hint {
  text-align: center;
  padding: 20px;
  color: var(--text-secondary);
  font-size: 13px;
}
.batch-progress {
  background: var(--bg-secondary);
  border: 1px solid var(--border);
  border-radius: 8px;
  padding: 16px;
}
.progress-header {
  display: flex;
  justify-content: space-between;
  align-items: center;
  margin-bottom: 12px;
}
.stage {
  font-weight: 600;
  text-transform: uppercase;
}
.target-db {
  font-size: 13px;
  color: var(--text-secondary);
}
.progress-bar-wrap {
  height: 12px;
  background: var(--border);
  border-radius: 6px;
  overflow: hidden;
  margin-bottom: 12px;
}
.progress-bar {
  height: 100%;
  background: var(--primary);
  transition: width 0.3s ease;
}
.progress-stats {
  display: flex;
  gap: 16px;
  flex-wrap: wrap;
  font-size: 13px;
  color: var(--text-secondary);
}
</style>
