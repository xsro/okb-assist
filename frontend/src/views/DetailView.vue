<template>
  <div class="detail-view" v-if="doc">
    <div class="detail-header">
      <h2>{{ doc.title || doc.filename || '(无标题)' }}</h2>
      <div class="detail-actions-row">
        <router-link v-if="tokenStore.role === 'admin'" :to="{ name: 'docManage', params: { id: doc.id } }" class="btn btn-sm btn-outline">
          管理
        </router-link>
        <router-link v-if="tokenStore.role === 'admin'" :to="{ name: 'markdownEdit', params: { id: doc.id } }" class="btn btn-sm">
          编辑 Markdown
        </router-link>
        <button v-if="tokenStore.role === 'admin' || tokenStore.role === 'view-only'" class="btn btn-sm btn-outline" :disabled="openingPdf" @click="openPdf">
          {{ openingPdf ? '准备中...' : '查看 PDF' }}
        </button>
        <router-link v-if="tokenStore.role === 'admin' || tokenStore.role === 'view-only'" :to="{ name: 'markdown', params: { id: doc.id } }" class="btn btn-sm btn-outline">全屏阅读</router-link>
      </div>
    </div>

    <!-- 元信息 -->
    <div class="section">
      <!-- 桌面端表格 -->
      <table v-if="!isMobile" class="info-table">
        <tr><th>ID</th><td>{{ doc.id }}</td></tr>
        <tr><th>作者</th><td>{{ doc.authors || '-' }}</td></tr>
        <tr><th>期刊</th><td>{{ doc.journal || '-' }}</td></tr>
        <tr><th>年份</th><td>{{ doc.year || '-' }}</td></tr>
        <tr><th>文档类型</th><td>{{ doc.doc_type || '-' }}</td></tr>
        <tr><th>DOI</th><td>{{ doc.doi || '-' }}</td></tr>
        <tr><th>关键词</th><td>{{ doc.keywords || '-' }}</td></tr>
        <tr><th>文件名</th><td>{{ doc.filename }}</td></tr>
        <tr><th>文件哈希</th><td>{{ doc.file_hash || '-' }}</td></tr>
        <tr><th>PDF 文件</th><td>{{ formatFile(doc.pdf_size) }}</td></tr>
        <tr><th>Markdown 文件</th><td>{{ formatFile(doc.md_size) }}</td></tr>
        <tr><th>资源包 ZIP</th><td>{{ formatFile(doc.zip_size) }}</td></tr>
        <tr><th>状态</th><td><StatusBadge :status="doc.status" :status_message="doc.status_message" /></td></tr>
        <tr v-if="doc.status === 'error' && doc.status_message">
          <th>错误详情</th>
          <td class="error-message">{{ doc.status_message }}</td>
        </tr>
        <tr>
          <th>已索引库</th>
          <td>
            <span v-if="!doc.indexed_dbs || doc.indexed_dbs.length === 0">-</span>
            <span v-else class="index-dbs">
              <span
                v-for="dbId in doc.indexed_dbs"
                :key="dbId"
                class="index-db-tag"
              >{{ dbId }}</span>
            </span>
          </td>
        </tr>
        <tr><th>创建时间</th><td>{{ doc.created_at }}</td></tr>
        <tr><th>更新时间</th><td>{{ doc.updated_at }}</td></tr>
      </table>

      <!-- 移动端堆叠 -->
      <div v-else class="info-table-stack">
        <div class="info-row">
          <span class="info-label">ID</span>
          <span class="info-value">{{ doc.id }}</span>
        </div>
        <div class="info-row">
          <span class="info-label">标题</span>
          <span class="info-value">{{ doc.title || doc.filename || '(无标题)' }}</span>
        </div>
        <div class="info-row">
          <span class="info-label">作者</span>
          <span class="info-value">{{ doc.authors || '-' }}</span>
        </div>
        <div class="info-row">
          <span class="info-label">期刊</span>
          <span class="info-value">{{ doc.journal || '-' }}</span>
        </div>
        <div class="info-row">
          <span class="info-label">年份</span>
          <span class="info-value">{{ doc.year || '-' }}</span>
        </div>
        <div class="info-row">
          <span class="info-label">文档类型</span>
          <span class="info-value">{{ doc.doc_type || '-' }}</span>
        </div>
        <div class="info-row">
          <span class="info-label">DOI</span>
          <span class="info-value">{{ doc.doi || '-' }}</span>
        </div>
        <div class="info-row">
          <span class="info-label">关键词</span>
          <span class="info-value">{{ doc.keywords || '-' }}</span>
        </div>
        <div class="info-row">
          <span class="info-label">文件哈希</span>
          <span class="info-value">{{ doc.file_hash || '-' }}</span>
        </div>
        <div class="info-row">
          <span class="info-label">PDF 文件</span>
          <span class="info-value">{{ formatFile(doc.pdf_size) }}</span>
        </div>
        <div class="info-row">
          <span class="info-label">Markdown</span>
          <span class="info-value">{{ formatFile(doc.md_size) }}</span>
        </div>
        <div class="info-row">
          <span class="info-label">资源包 ZIP</span>
          <span class="info-value">{{ formatFile(doc.zip_size) }}</span>
        </div>
        <div class="info-row">
          <span class="info-label">状态</span>
          <span class="info-value"><StatusBadge :status="doc.status" :status_message="doc.status_message" /></span>
        </div>
        <div v-if="doc.status === 'error' && doc.status_message" class="info-row error-row">
          <span class="info-label">错误详情</span>
          <span class="info-value error-message">{{ doc.status_message }}</span>
        </div>
        <div class="info-row">
          <span class="info-label">已索引库</span>
          <span class="info-value">
            <span v-if="!doc.indexed_dbs || doc.indexed_dbs.length === 0">-</span>
            <span v-else class="index-dbs">
              <span
                v-for="dbId in doc.indexed_dbs"
                :key="dbId"
                class="index-db-tag"
              >{{ dbId }}</span>
            </span>
          </span>
        </div>
        <div class="info-row">
          <span class="info-label">创建时间</span>
          <span class="info-value">{{ doc.created_at }}</span>
        </div>
        <div class="info-row">
          <span class="info-label">更新时间</span>
          <span class="info-value">{{ doc.updated_at }}</span>
        </div>
      </div>
    </div>

    <!-- 摘要 -->
    <div v-if="doc.abstract" class="section">
      <h3>摘要</h3>
      <p class="abstract-text">{{ doc.abstract }}</p>
    </div>


  </div>

  <div v-else class="loading">加载中...</div>
</template>

<script setup lang="ts">
import { ref, onMounted, watch } from 'vue'
import { useRoute } from 'vue-router'
import { getDocument, getFileAlias } from '@/api/documents'
import { useToast } from '@/composables/useToast'
import { useRequireToken } from '@/composables/useRequireToken'
import { useResponsive } from '@/composables/useResponsive'
import { useTokenStore } from '@/stores/token'
import StatusBadge from '@/components/StatusBadge.vue'
import type { Document } from '@/types/document'

const route = useRoute()
const { showError } = useToast()
const { requireToken } = useRequireToken()
// isMobile 注入（仅响应式需要）
const { isMobile } = useResponsive()
const tokenStore = useTokenStore()

const doc = ref<Document | null>(null)

const openingPdf = ref(false)

function formatSize(bytes: number | null): string {
  if (!bytes) return '-'
  if (bytes < 1024) return bytes + ' B'
  if (bytes < 1024 * 1024) return (bytes / 1024).toFixed(1) + ' KB'
  return (bytes / (1024 * 1024)).toFixed(1) + ' MB'
}

function formatFile(size: number | null): string {
  if (size == null) return '❌ 不存在'
  return formatSize(size)
}

async function openPdf() {
  if (!doc.value) return
  openingPdf.value = true
  try {
    const { url } = await getFileAlias(doc.value.id)
    window.open(url, '_blank')
  } catch {
    showError('无法打开 PDF')
  } finally {
    openingPdf.value = false
  }
}

/** 加载文档详情 */
async function load() {
  const id = parseInt(route.params.id as string)
  if (!requireToken()) return

  try {
    doc.value = await getDocument(id)
  } catch {
    showError('加载失败')
  }
}

watch(() => route.params.id, load)
onMounted(() => {
  load()
})
</script>

<style scoped>
.detail-header {
  margin-bottom: 20px;
}
.detail-header h2 {
  margin-bottom: 12px;
}
.detail-actions-row {
  display: flex;
  flex-wrap: wrap;
  gap: 8px;
}
.info-table {
  width: 100%;
  border-collapse: collapse;
}
.info-table th, .info-table td {
  padding: 10px 16px;
  border-bottom: 1px solid var(--border);
  text-align: left;
  vertical-align: top;
}
.info-table th {
  width: 120px;
  font-weight: 600;
  color: var(--text-secondary);
  background: #f7f7f7;
}

/* 移动端信息表堆叠 */
.info-table-stack {
  display: flex;
  flex-direction: column;
  gap: 0;
}

.info-table-stack .info-row {
  display: flex;
  align-items: flex-start;
  padding: 12px 16px;
  border-bottom: 1px solid var(--border);
  gap: 12px;
}

.info-table-stack .info-row:last-child {
  border-bottom: none;
}

.info-table-stack .info-label {
  width: 100px;
  flex-shrink: 0;
  font-weight: 600;
  color: var(--text-secondary);
  font-size: 13px;
  padding-top: 2px;
}

.info-table-stack .info-value {
  flex: 1;
  font-size: 14px;
  color: var(--text);
  word-break: break-word;
}
.abstract-text {
  margin-top: 8px;
  line-height: 1.8;
  color: var(--text-secondary);
}

.index-dbs {
  display: inline-flex;
  gap: 6px;
  flex-wrap: wrap;
}
.index-db-tag {
  display: inline-block;
  padding: 2px 8px;
  border-radius: 4px;
  background: var(--bg-secondary);
  border: 1px solid var(--border);
  font-size: 12px;
  color: var(--text-secondary);
}
.error-message {
  color: var(--danger-text);
  font-size: 13px;
  line-height: 1.6;
  word-break: break-word;
}

.error-row {
  background: var(--danger-bg);
}

.error-row .info-label {
  color: var(--danger-text);
}

.loading {
  text-align: center;
  padding: 60px;
  color: var(--text-secondary);
}
</style>