<template>
  <div class="detail-view" v-if="doc">
    <div class="detail-header">
      <h2>{{ doc.title }}</h2>
      <div class="detail-actions">
        <router-link :to="{ name: 'docManage', params: { id: doc.id } }" class="btn btn-sm btn-outline">
          管理
        </router-link>
        <router-link :to="{ name: 'markdownEdit', params: { id: doc.id } }" class="btn btn-sm">
          编辑 Markdown
        </router-link>
      </div>
    </div>

    <!-- 元信息 -->
    <div class="section">
      <table class="info-table">
        <tr><th>ID</th><td>{{ doc.id }}</td></tr>
        <tr><th>作者</th><td>{{ doc.authors || '-' }}</td></tr>
        <tr><th>期刊</th><td>{{ doc.journal || '-' }}</td></tr>
        <tr><th>年份</th><td>{{ doc.year || '-' }}</td></tr>
        <tr><th>文档类型</th><td>{{ doc.doc_type || '-' }}</td></tr>
        <tr><th>DOI</th><td>{{ doc.doi || '-' }}</td></tr>
        <tr><th>关键词</th><td>{{ doc.keywords || '-' }}</td></tr>
        <tr><th>文件哈希</th><td>{{ doc.file_hash || '-' }}</td></tr>
        <tr><th>文件大小</th><td>{{ formatSize(doc.file_size) }}</td></tr>
        <tr><th>状态</th><td><StatusBadge :status="doc.status" /></td></tr>
        <tr><th>索引状态</th><td><StatusBadge :status="doc.index_status" /></td></tr>
        <tr><th>创建时间</th><td>{{ doc.created_at }}</td></tr>
        <tr><th>更新时间</th><td>{{ doc.updated_at }}</td></tr>
      </table>
    </div>

    <!-- 摘要 -->
    <div v-if="doc.abstract" class="section">
      <h3>摘要</h3>
      <p class="abstract-text">{{ doc.abstract }}</p>
    </div>

    <!-- Markdown 渲染 -->
    <div v-if="doc.has_markdown" class="section">
      <div class="section-header">
        <h3>全文</h3>
        <div class="page-nav">
          <button :disabled="currentPage <= 1" @click="currentPage--; loadMarkdown()">上一页</button>
          <span>第 {{ currentPage }} 页</span>
          <button :disabled="currentPage >= totalPages" @click="currentPage++; loadMarkdown()">下一页</button>
        </div>
      </div>
      <MarkdownViewer :content="markdownContent" />
    </div>

    <!-- 操作按钮 -->
    <div class="detail-actions-bottom">
      <button class="btn btn-outline" :disabled="openingPdf" @click="openPdf">
        {{ openingPdf ? '准备中...' : '查看 PDF' }}
      </button>
      <router-link :to="{ name: 'markdown', params: { id: doc.id } }" class="btn btn-outline">全屏阅读</router-link>
    </div>
  </div>

  <div v-else class="loading">加载中...</div>
</template>

<script setup lang="ts">
import { ref, onMounted, watch } from 'vue'
import { useRoute } from 'vue-router'
import { getDocument, getMarkdown, getFileAlias } from '@/api/documents'
import { useToast } from '@/composables/useToast'
import { useRequireToken } from '@/composables/useRequireToken'
import StatusBadge from '@/components/StatusBadge.vue'
import MarkdownViewer from '@/components/MarkdownViewer.vue'
import type { Document } from '@/types/document'

const route = useRoute()
const { showError } = useToast()
const { requireToken } = useRequireToken()

const doc = ref<Document | null>(null)
const markdownContent = ref('')
const currentPage = ref(1)
const totalPages = ref(1)
const openingPdf = ref(false)

function formatSize(bytes: number | null): string {
  if (!bytes) return '-'
  if (bytes < 1024) return bytes + ' B'
  if (bytes < 1024 * 1024) return (bytes / 1024).toFixed(1) + ' KB'
  return (bytes / (1024 * 1024)).toFixed(1) + ' MB'
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

async function load() {
  const id = parseInt(route.params.id as string)
  if (!requireToken()) return
  try {
    doc.value = await getDocument(id)
    if (doc.value.has_markdown) {
      await loadMarkdown()
    }
  } catch {
    showError('加载失败')
  }
}

async function loadMarkdown() {
  const id = parseInt(route.params.id as string)
  try {
    const res = await getMarkdown(id, currentPage.value)
    markdownContent.value = res.content
    totalPages.value = res.total_pages
  } catch {
    showError('加载 Markdown 失败')
  }
}

watch(() => route.params.id, load)
onMounted(load)
</script>

<style scoped>
.detail-header {
  display: flex;
  justify-content: space-between;
  align-items: flex-start;
  margin-bottom: 20px;
  gap: 16px;
}
.detail-actions, .detail-actions-bottom {
  display: flex;
  gap: 8px;
}
.detail-actions-bottom {
  margin-top: 24px;
  padding-top: 16px;
  border-top: 1px solid var(--border);
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
.abstract-text {
  margin-top: 8px;
  line-height: 1.8;
  color: var(--text-secondary);
}
.section-header {
  display: flex;
  justify-content: space-between;
  align-items: center;
  margin-bottom: 16px;
}
.page-nav {
  display: flex;
  gap: 8px;
  align-items: center;
}
.loading {
  text-align: center;
  padding: 60px;
  color: var(--text-secondary);
}
</style>