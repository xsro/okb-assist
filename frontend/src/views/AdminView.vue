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
  </div>
</template>

<script setup lang="ts">
import { ref, onMounted } from 'vue'
import { useRouter } from 'vue-router'
import {
  getStats,
  recalculateHashes,
  deduplicateDocuments
} from '@/api/admin'
import { startBatchParse } from '@/api/pipeline'
import { useToast } from '@/composables/useToast'
import type { SystemStats } from '@/types/config'

const { showToast, showInfo, showError } = useToast()
const router = useRouter()

const stats = ref<SystemStats | null>(null)

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

onMounted(load)
</script>

<style scoped>
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