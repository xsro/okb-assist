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
</style>
