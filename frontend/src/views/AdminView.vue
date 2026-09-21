<template>
  <div class="admin-view">
    <h2>管理后台</h2>

    <!-- 统计卡片 -->
    <div class="stats-grid">
      <div class="stat-card">
        <span class="stat-value">{{ stats?.total_documents || 0 }}</span>
        <span class="stat-label">总文档数</span>
      </div>
      <div class="stat-card">
        <span class="stat-value">{{ stats?.indexed_count || 0 }}</span>
        <span class="stat-label">已索引</span>
      </div>
      <div class="stat-card">
        <span class="stat-value">{{ stats?.error_count || 0 }}</span>
        <span class="stat-label">错误</span>
      </div>
      <div class="stat-card">
        <span class="stat-value">{{ formatSize(stats?.total_size || 0) }}</span>
        <span class="stat-label">总大小</span>
      </div>
    </div>

    <!-- 维护操作 -->
    <div class="section">
      <h3>维护操作</h3>
      <div class="action-buttons">
        <button class="btn btn-outline" @click="batchParse">批量解析已上传</button>
        <button class="btn btn-outline" @click="recalcHashes">重新计算哈希</button>
        <button class="btn btn-outline" @click="dedup">文献去重</button>
        <button class="btn btn-outline" @click="goDuplicates">手动去重</button>
      </div>
    </div>

    <!-- 流水线监控 -->
    <div class="section monitor-section">
      <div class="section-header">
        <h3>流水线监控</h3>
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
      <div class="subsection">
        <h4>活跃任务</h4>
        <div class="table-wrap">
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

      <!-- 批量进度 -->
      <div class="subsection">
        <h4>批量进度</h4>
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
  </div>
</template>

<script setup lang="ts">
import { ref, onMounted, onUnmounted, computed } from 'vue'
import { useRouter } from 'vue-router'
import {
  getStats,
  recalculateHashes,
  deduplicateDocuments
} from '@/api/admin'
import { startBatchParse } from '@/api/pipeline'
import { usePipelineStore } from '@/stores/pipeline'
import { useToast } from '@/composables/useToast'
import StatusBadge from '@/components/StatusBadge.vue'
import type { SystemStats } from '@/types/config'

const { showToast, showInfo, showError } = useToast()
const router = useRouter()
const pipelineStore = usePipelineStore()

const stats = ref<SystemStats | null>( null)
const loading = computed(() => pipelineStore.loading)
const queue = computed(() => pipelineStore.queue)
const activeTasks = computed(() => pipelineStore.activeTasks)
const batchProgress = computed(() => pipelineStore.batchProgress)

const progressPercent = computed(() => {
  const p = batchProgress.value
  if (!p || p.total === 0) return 0
  return Math.min(100, Math.round((p.processed / p.total) * 100))
})

function formatSize(bytes: number): string {
  if (bytes < 1024) return bytes + ' B'
  if (bytes < 1024 * 1024) return (bytes / 1024).toFixed(1) + ' KB'
  if (bytes < 1024 * 1024 * 1024) return (bytes / (1024 * 1024)).toFixed(1) + ' MB'
  return (bytes / (1024 * 1024 * 1024)).toFixed(1) + ' GB'
}

async function load() {
  try {
    const s = await getStats()
    stats.value = s
  } catch (e) {
    showError('加载失败')
  }
}

async function batchParse() {
  if (!confirm('确定批量解析所有已上传/待解析的文档？')) return
  try {
    const res = await startBatchParse()
    showInfo(res.detail)
  } catch {
    showError('批量解析提交失败')
  }
}

async function recalcHashes() {
  if (!confirm('确定重新计算所有文档哈希？')) return
  try {
    const res = await recalculateHashes()
    showToast(res.message, 'success')
  } catch {
    showError('哈希重算失败')
  }
}

async function dedup() {
  if (!confirm('确定按文件哈希去重？相同哈希的文档将保留最早的一个，其余删除。此操作不可逆！')) return
  try {
    const res = await deduplicateDocuments()
    showToast(res.message, 'success')
  } catch {
    showError('文献去重失败')
  }
}

function goDuplicates() {
  router.push('/assist/duplicates')
}

function refresh() {
  pipelineStore.fetchStatus()
}

function formatTime(iso: string) {
  return new Date(iso).toLocaleString('zh-CN')
}

onMounted(() => {
  load()
  pipelineStore.fetchStatus()
  pipelineStore.startPolling(3000)
})

onUnmounted(() => {
  pipelineStore.stopPolling()
})
</script>

<style scoped>
.section-header {
  display: flex;
  justify-content: space-between;
  align-items: center;
  margin-bottom: 16px;
}

.subsection {
  margin-top: 20px;
}

.subsection h4 {
  margin: 0 0 12px 0;
  font-size: 15px;
  color: var(--text-primary);
}

.table-wrap {
  overflow-x: auto;
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

@media (max-width: 768px) {
  .stats-grid {
    grid-template-columns: repeat(2, 1fr);
    gap: 10px;
  }

  .stat-card {
    padding: 14px;
  }

  .stat-value {
    font-size: 24px;
  }

  .action-buttons {
    flex-wrap: wrap;
    gap: 8px;
  }

  .action-buttons .btn {
    flex: 1;
    min-width: 120px;
    padding: 10px 14px;
    font-size: 13px;
  }
}

@media (max-width: 480px) {
  .stats-grid {
    grid-template-columns: 1fr;
    gap: 8px;
  }

  .stat-card {
    padding: 12px;
  }

  .stat-value {
    font-size: 22px;
  }

  .action-buttons .btn {
    min-width: 100px;
    font-size: 12px;
    padding: 8px 10px;
  }
}
</style>