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

    <!-- 服务状态 -->
    <div class="section">
      <h3>服务状态</h3>
      <table class="doc-table">
        <thead>
          <tr><th>服务</th><th>状态</th><th>详情</th><th>操作</th></tr>
        </thead>
        <tbody>
          <tr v-for="name in serviceNames" :key="name">
            <td>{{ serviceName(name) }}</td>
            <td>
              <span class="status-dot" :class="isOk(serviceStatus?.[name]) ? 'ok' : 'error'"></span>
              {{ isOk(serviceStatus?.[name]) ? '正常' : '异常' }}
            </td>
            <td>{{ serviceDetail(serviceStatus?.[name]) }}</td>
            <td>
              <button class="btn btn-sm btn-outline" @click="testConnection(name)">测试</button>
            </td>
          </tr>
        </tbody>
      </table>
    </div>

    <!-- 向量库状态 -->
    <div class="section">
      <h3>向量库</h3>
      <table class="doc-table">
        <thead>
          <tr><th>ID</th><th>名称</th><th>类型</th><th>状态</th><th>详情</th></tr>
        </thead>
        <tbody>
          <tr v-for="db in serviceStatus?.vector_dbs || []" :key="db.id">
            <td>{{ db.id }}</td>
            <td>{{ db.name }}</td>
            <td>{{ db.type }}</td>
            <td>
              <span class="status-dot" :class="isOk(db) ? 'ok' : 'error'"></span>
              {{ isOk(db) ? '正常' : '异常' }}
            </td>
            <td>{{ serviceDetail(db) }}</td>
          </tr>
        </tbody>
      </table>
    </div>

    <!-- 维护操作 -->
    <div class="section">
      <h3>维护操作</h3>
      <div class="action-buttons">
        <button class="btn btn-outline" @click="batchParse">批量解析已上传</button>
        <button class="btn btn-outline" @click="migrate">数据库迁移</button>
        <button class="btn btn-outline" @click="recalcHashes">重新计算哈希</button>
        <button class="btn btn-outline" @click="resetIndex">重置索引</button>
      </div>
    </div>
  </div>
</template>

<script setup lang="ts">
import { ref, onMounted, computed } from 'vue'
import {
  getStats,
  getServiceStatus,
  migrateDatabase,
  recalculateHashes,
  resetIndex
} from '@/api/admin'
import { startBatchParse } from '@/api/pipeline'
import { useToast } from '@/composables/useToast'
import type { SystemStats, ServiceStatus, ServiceStatusItem } from '@/types/config'

const { showToast, showInfo, showError } = useToast()

const stats = ref<SystemStats | null>(null)
const serviceStatus = ref<ServiceStatus | null>(null)

const serviceNames = computed(() => ['mineru', 'ollama', 'fastembed', 'qdrant'])

function serviceName(key: string) {
  const map: Record<string, string> = {
    mineru: 'MinerU',
    ollama: 'Ollama',
    fastembed: 'FastEmbed',
    qdrant: 'Qdrant'
  }
  return map[key] || key
}

function isOk(item: ServiceStatusItem | ServiceStatusItem[] | undefined): boolean {
  if (!item) return false
  if (Array.isArray(item)) return item.every((i) => i.status === 'connected')
  return item.status === 'connected'
}

function serviceDetail(item: ServiceStatusItem | ServiceStatusItem[] | undefined): string {
  if (!item) return '-'
  if (Array.isArray(item)) return `${item.length} 个`
  return item.error || item.url || item.status || '-'
}

function formatSize(bytes: number): string {
  if (bytes < 1024) return bytes + ' B'
  if (bytes < 1024 * 1024) return (bytes / 1024).toFixed(1) + ' KB'
  if (bytes < 1024 * 1024 * 1024) return (bytes / (1024 * 1024)).toFixed(1) + ' MB'
  return (bytes / (1024 * 1024 * 1024)).toFixed(1) + ' GB'
}

async function load() {
  try {
    const [s, sv] = await Promise.all([getStats(), getServiceStatus()])
    stats.value = s
    serviceStatus.value = sv
  } catch (e) {
    showError('加载失败')
  }
}

async function testConnection(service: string) {
  try {
    const sv = await getServiceStatus()
    const item = sv[service as keyof ServiceStatus]
    if (!item || Array.isArray(item)) {
      showError(`未知服务: ${service}`)
      return
    }
    showToast(`${service}: ${item.status}`, isOk(item) ? 'success' : 'error')
  } catch {
    showError('连接测试失败')
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

async function migrate() {
  if (!confirm('确定执行数据库迁移？')) return
  try {
    const res = await migrateDatabase()
    showToast(res.message, 'success')
  } catch {
    showError('数据库迁移失败')
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

async function resetIndex() {
  if (!confirm('确定重置所有索引？此操作不可逆！')) return
  try {
    const res = await resetIndex()
    showToast(res.message, 'success')
  } catch {
    showError('索引重置失败')
  }
}

onMounted(load)
</script>

<style scoped>
.status-dot {
  display: inline-block;
  width: 8px;
  height: 8px;
  border-radius: 50%;
  margin-right: 6px;
}
.status-dot.ok { background: var(--success); }
.status-dot.error { background: var(--danger); }
</style>