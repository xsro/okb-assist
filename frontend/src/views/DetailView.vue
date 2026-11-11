<template>
  <div class="detail-view" v-if="doc || publicDoc">
    <!-- 访客提示（未登录：只展示书目信息与摘要，其余功能一律隐藏） -->
    <div v-if="isGuest" class="guest-banner">
      <span class="guest-banner-text">当前为访客浏览：仅显示文献的书目信息与摘要。</span>
      <button class="btn btn-sm" @click="goLogin">登录查看完整信息</button>
    </div>

    <div class="detail-header">
      <h2>{{ meta.title || '(无标题)' }}</h2>
      <div v-if="!isGuest" class="detail-actions-row">
        <router-link v-if="tokenStore.canEdit" :to="{ name: 'docManage', params: { id: doc.id } }" class="btn btn-sm btn-outline">
          管理
        </router-link>
        <router-link v-if="tokenStore.canEdit" :to="{ name: 'markdownEdit', params: { id: doc.id } }" class="btn btn-sm">
          编辑 Markdown
        </router-link>
        <button v-if="tokenStore.canViewPdf" class="btn btn-sm btn-outline" :disabled="openingPdf" @click="openPdf">
          {{ openingPdf ? '准备中...' : '查看 PDF' }}
        </button>
        <router-link v-if="tokenStore.canViewMarkdown" :to="{ name: 'markdown', params: { id: doc.id } }" class="btn btn-sm btn-outline">全屏阅读</router-link>
      </div>
    </div>

    <!-- 元信息 -->
    <div class="section">
      <!-- 桌面端表格 -->
      <table v-if="!isMobile" class="info-table">
        <tr v-if="!isGuest"><th>ID</th><td>{{ doc?.id }}</td></tr>
        <tr><th>作者</th><td>{{ meta.authors || '-' }}</td></tr>
        <tr><th>期刊</th><td>{{ meta.journal || '-' }}</td></tr>
        <tr><th>年份</th><td>{{ meta.year || '-' }}</td></tr>
        <tr><th>文档类型</th><td>{{ meta.doc_type || '-' }}</td></tr>
        <tr>
          <th>DOI</th>
          <td>
            {{ meta.doi || '-' }}
            <a v-if="doiUrl" :href="doiUrl" target="_blank" rel="noopener noreferrer" class="doi-link" title="在 doi.org 打开">
              打开 ↗
            </a>
          </td>
        </tr>
        <tr><th>关键词</th><td>{{ meta.keywords || '-' }}</td></tr>
        <tr v-if="meta.source"><th>来源</th><td>{{ meta.source }}</td></tr>
        <tr v-if="meta.category"><th>分类</th><td>{{ meta.category }}</td></tr>
        <tr v-if="meta.language"><th>语言</th><td>{{ meta.language }}</td></tr>
        <tr v-if="meta.title_en"><th>英文标题</th><td>{{ meta.title_en }}</td></tr>
        <tr v-if="meta.authors_en"><th>英文作者</th><td>{{ meta.authors_en }}</td></tr>
        <tr v-if="meta.journal_en"><th>英文期刊</th><td>{{ meta.journal_en }}</td></tr>
        <tr v-if="meta.keywords_en"><th>英文关键词</th><td>{{ meta.keywords_en }}</td></tr>
        <!-- 以下为文件/库内部信息：仅登录用户可见 -->
        <template v-if="!isGuest">
        <tr><th>文件名</th><td>{{ doc?.filename }}</td></tr>
        <tr><th>文件哈希</th><td>{{ doc.file_hash || '-' }}</td></tr>
        <tr><th>PDF 文件</th><td>{{ formatFile(doc.pdf_size) }}</td></tr>
        <tr><th>Markdown 文件</th><td>{{ formatFile(doc.md_size) }}</td></tr>
        <tr><th>资源包 ZIP</th><td>{{ formatFile(doc.zip_size) }}</td></tr>
        <tr>
          <th>PDF 健康</th>
          <td>
            <span class="health-badge" :class="`health-${healthStatus}`">{{ healthLabel }}</span>
            <span v-if="doc.pdf_health_message" class="health-msg">{{ doc.pdf_health_message }}</span>
          </td>
        </tr>
        <tr><th>文件</th><td><StatusBadge :doc="doc" /></td></tr>
        <tr v-if="derivedStatus?.status === 'error' && derivedStatus.detail">
          <th>错误详情</th>
          <td class="error-message">{{ derivedStatus.detail }}</td>
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
        <tr><th>创建时间</th><td>{{ doc?.created_at }}</td></tr>
        <tr><th>更新时间</th><td>{{ doc?.updated_at }}</td></tr>
        </template>
      </table>

      <!-- 移动端堆叠 -->
      <div v-else class="info-table-stack">
        <div v-if="!isGuest" class="info-row">
          <span class="info-label">ID</span>
          <span class="info-value">{{ doc?.id }}</span>
        </div>
        <div class="info-row">
          <span class="info-label">作者</span>
          <span class="info-value">{{ meta.authors || '-' }}</span>
        </div>
        <div class="info-row">
          <span class="info-label">期刊</span>
          <span class="info-value">{{ meta.journal || '-' }}</span>
        </div>
        <div class="info-row">
          <span class="info-label">年份</span>
          <span class="info-value">{{ meta.year || '-' }}</span>
        </div>
        <div class="info-row">
          <span class="info-label">文档类型</span>
          <span class="info-value">{{ meta.doc_type || '-' }}</span>
        </div>
        <div class="info-row">
          <span class="info-label">DOI</span>
          <span class="info-value">
            {{ meta.doi || '-' }}
            <a v-if="doiUrl" :href="doiUrl" target="_blank" rel="noopener noreferrer" class="doi-link" title="在 doi.org 打开">
              打开 ↗
            </a>
          </span>
        </div>
        <div class="info-row">
          <span class="info-label">关键词</span>
          <span class="info-value">{{ meta.keywords || '-' }}</span>
        </div>
        <div v-if="meta.source" class="info-row">
          <span class="info-label">来源</span>
          <span class="info-value">{{ meta.source }}</span>
        </div>
        <div v-if="meta.category" class="info-row">
          <span class="info-label">分类</span>
          <span class="info-value">{{ meta.category }}</span>
        </div>
        <div v-if="meta.language" class="info-row">
          <span class="info-label">语言</span>
          <span class="info-value">{{ meta.language }}</span>
        </div>
        <div v-if="meta.title_en" class="info-row">
          <span class="info-label">英文标题</span>
          <span class="info-value">{{ meta.title_en }}</span>
        </div>
        <div v-if="meta.authors_en" class="info-row">
          <span class="info-label">英文作者</span>
          <span class="info-value">{{ meta.authors_en }}</span>
        </div>
        <div v-if="meta.journal_en" class="info-row">
          <span class="info-label">英文期刊</span>
          <span class="info-value">{{ meta.journal_en }}</span>
        </div>
        <div v-if="meta.keywords_en" class="info-row">
          <span class="info-label">英文关键词</span>
          <span class="info-value">{{ meta.keywords_en }}</span>
        </div>
        <!-- 以下为文件/库内部信息：仅登录用户可见 -->
        <template v-if="!isGuest">
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
          <span class="info-label">PDF 健康</span>
          <span class="info-value">
            <span class="health-badge" :class="`health-${healthStatus}`">{{ healthLabel }}</span>
            <span v-if="doc.pdf_health_message" class="health-msg">{{ doc.pdf_health_message }}</span>
          </span>
        </div>
        <div class="info-row">
          <span class="info-label">文件</span>
          <span class="info-value"><StatusBadge :doc="doc" /></span>
        </div>
        <div v-if="derivedStatus?.status === 'error' && derivedStatus.detail" class="info-row error-row">
          <span class="info-label">错误详情</span>
          <span class="info-value error-message">{{ derivedStatus.detail }}</span>
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
          <span class="info-value">{{ doc?.created_at }}</span>
        </div>
        <div class="info-row">
          <span class="info-label">更新时间</span>
          <span class="info-value">{{ doc?.updated_at }}</span>
        </div>
        </template>
      </div>
    </div>

    <!-- PDF 元信息 / Crossref / 引用条目（懒加载折叠区，仅登录可见） -->
    <div class="section extras-section" v-if="!isGuest && doc">
      <DocumentPdfInfoPanel :doc-id="doc.id" />
      <DocumentCrossrefPanel :doc-id="doc.id" />
      <DocumentCitationPanel :doc-id="doc.id" />
    </div>

    <!-- 摘要 -->
    <div v-if="meta.abstract" class="section">
      <h3>摘要</h3>
      <p class="abstract-text">{{ meta.abstract }}</p>
      <template v-if="meta.abstract_en">
        <h3>英文摘要</h3>
        <p class="abstract-text">{{ meta.abstract_en }}</p>
      </template>
    </div>

  </div>

  <div v-else-if="notFound" class="loading">
    <p>文档不存在，或尚未开放公开浏览。</p>
    <button class="btn btn-sm" @click="goLogin">前往登录</button>
  </div>
  <div v-else class="loading">加载中...</div>
</template>

<script setup lang="ts">
import { ref, computed, onMounted, watch } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import { getDocument, getFileAlias, getPublicDocument } from '@/api/documents'
import { useToast } from '@/composables/useToast'
import { useRequireToken } from '@/composables/useRequireToken'
import { useResponsive } from '@/composables/useResponsive'
import { useTokenStore } from '@/stores/token'
import StatusBadge from '@/components/StatusBadge.vue'
import { deriveDocStatus } from '@/utils/docStatus'
import DocumentPdfInfoPanel from '@/components/detail/DocumentPdfInfoPanel.vue'
import DocumentCrossrefPanel from '@/components/detail/DocumentCrossrefPanel.vue'
import DocumentCitationPanel from '@/components/detail/DocumentCitationPanel.vue'
import type { Document, PublicDocument } from '@/types/document'

const route = useRoute()
const { showError } = useToast()
const { requireToken } = useRequireToken()
// isMobile 注入（仅响应式需要）
const { isMobile } = useResponsive()
const tokenStore = useTokenStore()
const router = useRouter()

const doc = ref<Document | null>(null)
/** 访客（未登录）拿到的公开信息：只有书目元数据 + 摘要 */
const publicDoc = ref<PublicDocument | null>(null)
const notFound = ref(false)

/** 未登录即访客：隐藏操作按钮、PDF 元信息等，只展示书目信息与摘要 */
const isGuest = computed(() => !tokenStore.isAuthenticated)

/**
 * 页面展示用的书目字段（登录 / 访客两种数据源统一）：
 * 访客没有 `doc`，只能读 `publicDoc`，所以模板里这些行一律走 `meta`。
 */
const meta = computed(() => {
  const d = doc.value
  const p = publicDoc.value
  return {
    title: d?.title || p?.title || '',
    title_en: d?.title_en || p?.title_en || '',
    authors: d?.authors || p?.authors || '',
    authors_en: d?.authors_en || p?.authors_en || '',
    journal: d?.journal || p?.journal || '',
    journal_en: d?.journal_en || p?.journal_en || '',
    year: d?.year ?? p?.year ?? null,
    doc_type: d?.doc_type || p?.doc_type || '',
    doi: d?.doi || p?.doi || '',
    keywords: d?.keywords || p?.keywords || '',
    keywords_en: d?.keywords_en || p?.keywords_en || '',
    source: d?.source || p?.source || '',
    category: d?.category || p?.category || '',
    language: d?.language || p?.language || '',
    abstract: d?.abstract || p?.abstract || '',
    abstract_en: d?.abstract_en || p?.abstract_en || ''
  }
})

/** 登录 / 注册后回到当前详情页 */
function goLogin() {
  router.push({ path: '/assist/login', query: { redirect: route.fullPath } })
}

/** 状态由前端推导（后端只给文件事实与运行时字段） */
const derivedStatus = computed(() => (doc.value ? deriveDocStatus(doc.value) : null))

const openingPdf = ref(false)

/** 由 DOI 生成 doi.org 跳转地址；无法识别时返回空字符串 */
const doiUrl = computed(() => {
  const raw = (meta.value.doi || '').trim()
  if (!raw) return ''
  // 去掉常见前缀：https://doi.org/、http://dx.doi.org/、doi: 等
  const cleaned = raw
    .replace(/^https?:\/\/(dx\.)?doi\.org\//i, '')
    .replace(/^doi:\s*/i, '')
    .trim()
  if (!cleaned) return ''
  return 'https://doi.org/' + cleaned
})

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

// ── PDF 健康（由管理页「批量检查 PDF 健康」后台任务写入）──
/** 状态值：ok / warning / error / missing；未检查时为 unchecked */
const healthStatus = computed(() => doc.value?.pdf_health || 'unchecked')

const healthLabel = computed(() => {
  const labels: Record<string, string> = {
    ok: '✓ 正常',
    warning: '⚠ 警告',
    error: '✕ 异常',
    missing: '✕ 缺失',
    unchecked: '未检查'
  }
  return labels[healthStatus.value] || healthStatus.value
})

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

/** 加载文档详情（访客走公开端点，只拿书目信息 + 摘要） */
async function load() {
  const id = parseInt(route.params.id as string)
  if (!Number.isFinite(id)) {
    notFound.value = true
    return
  }
  notFound.value = false

  // 访客：不要求登录，走后端公开端点。
  // 若未开启 `public_detail_enabled`，该端点返回 401，axios 拦截器会统一跳登录页。
  if (isGuest.value) {
    doc.value = null
    try {
      publicDoc.value = await getPublicDocument(id)
    } catch (e) {
      publicDoc.value = null
      const status = (e as { response?: { status?: number } }).response?.status
      if (status === 404) notFound.value = true
      // 401 已由全局拦截器处理（跳登录页）
    }
    return
  }

  if (!requireToken()) return

  try {
    doc.value = await getDocument(id)
    publicDoc.value = null
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

/* 访客提示横幅（未登录） */
.guest-banner {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
  flex-wrap: wrap;
  margin-bottom: 16px;
  padding: 10px 16px;
  border: 1px solid var(--border);
  border-radius: 8px;
  background: var(--bg-secondary);
  color: var(--text-secondary);
  font-size: 13px;
}
.guest-banner-text {
  line-height: 1.6;
}

/* 访客占位卡（代替登录后才可用的折叠区） */
.guest-lock {
  border: 1px dashed var(--border);
  border-radius: 8px;
  background: transparent;
  text-align: center;
  padding: 24px 16px;
}
.guest-lock h3 {
  margin-bottom: 8px;
}
.guest-lock-text {
  color: var(--text-secondary);
  font-size: 13px;
  margin-bottom: 14px;
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
.doi-link {
  display: inline-block;
  margin-left: 8px;
  padding: 1px 8px;
  border: 1px solid var(--border);
  border-radius: 4px;
  background: var(--bg-hover);
  color: var(--primary);
  font-size: 12px;
  line-height: 1.6;
  text-decoration: none;
  vertical-align: middle;
  white-space: nowrap;
}
.doi-link:hover {
  background: var(--border);
}
.abstract-text {
  margin-top: 8px;
  line-height: 1.8;
  color: var(--text-secondary);
}

/* PDF 元信息 / Crossref / 引用条目折叠区 */
.extras-section {
  background: transparent;
  border: none;
  padding: 0;
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

/* PDF 健康徽章 */
.health-badge {
  display: inline-block;
  padding: 2px 8px;
  border-radius: 10px;
  font-size: 12px;
  font-weight: 600;
  white-space: nowrap;
  border: 1px solid transparent;
}
.health-badge.health-ok {
  color: #1b7f4b;
  background: #e8f7ee;
  border-color: #b7e2c9;
}
.health-badge.health-warning {
  color: #9a6400;
  background: #fff6e0;
  border-color: #f0d69b;
}
.health-badge.health-error,
.health-badge.health-missing {
  color: var(--danger-text);
  background: #fdecec;
  border-color: #f3c1c1;
}
.health-badge.health-unchecked {
  color: var(--text-secondary);
  background: var(--bg-secondary);
  border-color: var(--border);
}
.health-msg {
  margin-left: 8px;
  font-size: 12px;
  color: var(--text-secondary);
  word-break: break-all;
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