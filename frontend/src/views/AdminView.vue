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
        <span class="stat-value">{{ stats?.status_counts?.uploaded || 0 }}</span>
        <span class="stat-label">已上传</span>
      </div>
      <div class="stat-card">
        <span class="stat-value">{{ stats?.status_counts?.parsing || 0 }}</span>
        <span class="stat-label">解析中</span>
      </div>
      <div class="stat-card">
        <span class="stat-value">{{ stats?.status_counts?.markdown_done || 0 }}</span>
        <span class="stat-label">解析完成</span>
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

    <!-- 权限管理 -->
    <div class="section">
      <h3>权限管理</h3>
      <p class="section-desc">管理角色 token，支持 view-only、view-upload</p>

      <div v-if="permEntries.length > 0" class="perm-token-list">
        <div v-for="(entry, idx) in permEntries" :key="idx" class="perm-token-row">
          <code class="perm-token-value">{{ entry.token }}</code>
          <span class="perm-token-role" :class="'role-' + entry.role">{{ roleLabel(entry.role) }}</span>
          <button class="btn btn-sm btn-outline-danger" @click="removeToken(entry.token)" :disabled="savingPerm">删除</button>
        </div>
      </div>
      <div v-else class="empty-hint">暂无权限 token</div>

      <div class="perm-add-row">
        <select v-model="newPermRole" class="perm-select">
          <option value="view-only">view-only（只读）</option>
          <option value="view-upload">view-upload（查看+上传）</option>
        </select>
        <input
          v-model="newPermToken"
          type="text"
          :placeholder="'输入 ' + newPermRole + ' token'"
          class="perm-input"
          @keyup.enter="addToken"
        />
        <button class="btn btn-sm" @click="addToken" :disabled="savingPerm || !newPermToken.trim()">
          添加
        </button>
      </div>

      <div v-if="permError" class="error-message">{{ permError }}</div>
    </div>
  </div>
</template>

<script setup lang="ts">
import { ref, onMounted, onUnmounted, computed } from 'vue'
import { useRouter } from 'vue-router'
import {
  getStats,
  recalculateHashes,
  deduplicateDocuments,
  getPermissionTokens,
  addPermissionToken,
  deletePermissionToken
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

// ── 权限管理 ──

interface PermEntry { token: string; role: string }

const permTokens = ref<Record<string, string>>({})
const newPermToken = ref('')
const newPermRole = ref('view-only')
const savingPerm = ref(false)
const permError = ref('')

const permEntries = computed<PermEntry[]>(() => {
  return Object.entries(permTokens.value).map(([token, role]) => ({ token, role }))
})

function roleLabel(role: string): string {
  const labels: Record<string, string> = {
    'view-only': '只读',
    'view-upload': '查看+上传'
  }
  return labels[role] || role
}

async function loadPermTokens() {
  try {
    const res = await getPermissionTokens()
    permTokens.value = (res.permissions || {}) as Record<string, string>
  } catch {
    // 静默忽略
  }
}

async function addToken() {
  const token = newPermToken.value.trim()
  if (!token) return
  savingPerm.value = true
  permError.value = ''
  try {
    const res = await addPermissionToken(newPermRole.value, token)
    permTokens.value = (res.permissions || {}) as Record<string, string>
    newPermToken.value = ''
  } catch {
    permError.value = '添加失败，请重试'
  } finally {
    savingPerm.value = false
  }
}

async function removeToken(token: string) {
  const role = permTokens.value[token]
  if (!role) return
  if (!confirm(`确定删除 token: ${token}（${roleLabel(role)}）？`)) return
  savingPerm.value = true
  permError.value = ''
  try {
    const res = await deletePermissionToken(role, token)
    permTokens.value = (res.permissions || {}) as Record<string, string>
  } catch {
    permError.value = '删除失败，请重试'
  } finally {
    savingPerm.value = false
  }
}

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
  loadPermTokens()
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

/* ── 权限管理 ── */
.section-desc {
  color: var(--text-secondary);
  font-size: 13px;
  margin-top: -8px;
  margin-bottom: 16px;
}

.perm-select {
  padding: 6px 10px;
  border: 1px solid var(--border);
  border-radius: 6px;
  font-size: 13px;
  background: var(--bg-white);
  color: var(--text);
  outline: none;
  cursor: pointer;
  min-width: 140px;
}
.perm-select:focus {
  border-color: var(--primary);
}

.perm-token-role {
  display: inline-block;
  padding: 2px 8px;
  border-radius: 4px;
  font-size: 11px;
  font-weight: 600;
  white-space: nowrap;
  flex-shrink: 0;
}
.perm-token-role.role-view-only {
  background: #e3f2fd;
  color: #1565c0;
}
.perm-token-role.role-view-upload {
  background: #e8f5e9;
  color: #2e7d32;
}

.perm-token-list {
  display: flex;
  flex-direction: column;
  gap: 6px;
  margin-bottom: 12px;
}

.perm-token-row {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 8px;
  padding: 6px 10px;
  background: var(--bg-white);
  border: 1px solid var(--border);
  border-radius: 6px;
}

.perm-token-value {
  font-family: 'SF Mono', 'Cascadia Code', 'Fira Code', monospace;
  font-size: 12px;
  color: var(--text);
  word-break: break-all;
  flex: 1;
}

.btn-outline-danger {
  color: #f44336;
  border-color: #f44336;
  background: transparent;
  white-space: nowrap;
}
.btn-outline-danger:hover {
  background: #f44336;
  color: #fff;
}

.perm-add-row {
  display: flex;
  gap: 8px;
}

.perm-input {
  flex: 1;
  padding: 6px 10px;
  border: 1px solid var(--border);
  border-radius: 6px;
  font-size: 13px;
  font-family: 'SF Mono', 'Cascadia Code', 'Fira Code', monospace;
  outline: none;
}
.perm-input:focus {
  border-color: var(--primary);
  box-shadow: 0 0 0 2px rgba(25, 118, 210, 0.12);
}

.error-message {
  color: #f44336;
  font-size: 13px;
  margin-top: 8px;
}
</style>