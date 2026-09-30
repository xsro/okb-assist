<template>
  <div class="home-view">
    <!-- 搜索栏 -->
    <div class="search-bar">
      <div class="search-input-wrap" ref="searchWrapRef">
        <div class="search-input-inner">
          <!-- 语法高亮背景层 -->
          <div
            v-if="searchQuery"
            class="search-highlight"
            aria-hidden="true"
            v-html="highlightedQuery"
          ></div>
          <input
            ref="searchInputRef"
            v-model="searchQuery"
            type="text"
            class="search-input"
            placeholder='搜索文献（例：transformer title:attention year:>2020）'
            autocomplete="off"
            spellcheck="false"
            @keyup.enter="search"
            @focus="showSyntaxHelp = true"
            @blur="onSearchBlur"
            @input="onSearchInput"
          />
        </div>
        <button class="btn btn-search" @click="search" :disabled="loading">
          <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round">
            <circle cx="11" cy="11" r="8"/>
            <line x1="21" y1="21" x2="16.65" y2="16.65"/>
          </svg>
        </button>
      </div>

      <!-- 语法帮助面板 -->
      <Transition name="fade">
        <div v-if="showSyntaxHelp && searchQuery.trim() === ''" class="syntax-help" ref="syntaxHelpRef">
          <div class="syntax-help-header">高级搜索语法</div>
          <div class="syntax-help-grid">
            <div v-for="item in syntaxHelpItems" :key="item.syntax" class="syntax-help-item">
              <code>{{ item.syntax }}</code>
              <span>{{ item.description }}</span>
            </div>
          </div>
        </div>
      </Transition>

      <!-- 活跃筛选条件（chips） -->
      <div v-if="activeFilterChips.length > 0" class="active-filters">
        <span
          v-for="(chip, i) in activeFilterChips"
          :key="i"
          class="filter-chip"
        >
          <span class="chip-label">{{ chip }}</span>
          <button class="chip-remove" @click="clearSearch" title="清除筛选">×</button>
        </span>
        <button class="btn btn-xs btn-ghost" @click="clearSearch">清除全部</button>
      </div>

      <!-- 排序控制（由 sort:/order: 语法控制） -->
      <div class="search-controls">
      </div>
    </div>

    <!-- 加载状态 -->
    <div v-if="loading" class="loading-skeleton">
      <div v-for="n in 5" :key="n" class="skeleton-row">
        <div class="sk sk-id"></div>
        <div class="sk sk-title"></div>
        <div class="sk sk-authors"></div>
        <div class="sk sk-year"></div>
        <div class="sk sk-type"></div>
        <div class="sk sk-status"></div>
        <div class="sk sk-action"></div>
      </div>
    </div>

    <!-- 桌面端表格 -->
    <table v-if="!isMobile && !loading" class="doc-table">
      <thead>
        <tr>
          <th>ID</th>
          <th>标题</th>
          <th>作者</th>
          <th>年份</th>
          <th>类型</th>
          <th>状态</th>
          <th>操作</th>
        </tr>
      </thead>
      <tbody>
        <tr v-for="doc in items" :key="doc.id">
          <td>{{ doc.id }}</td>
          <td>
            <router-link :to="{ name: 'detail', params: { id: doc.id } }">
              {{ doc.title || doc.filename || '(无标题)' }}
            </router-link>
          </td>
          <td>{{ doc.authors || '-' }}</td>
          <td>{{ doc.year || '-' }}</td>
          <td>{{ doc.doc_type || '-' }}</td>
          <td>
            <StatusBadge :status="doc.status" :status_message="doc.status_message" />
            <span v-if="doc.indexed_dbs && doc.indexed_dbs.length" class="index-dbs">
              <span
                v-for="dbId in doc.indexed_dbs"
                :key="dbId"
                class="index-db-tag"
              >{{ dbId }}</span>
            </span>
          </td>
          <td>
            <router-link
              :to="{ name: 'docManage', params: { id: doc.id } }"
              class="btn btn-sm btn-outline"
            >
              管理
            </router-link>
          </td>
        </tr>
      </tbody>
    </table>

    <!-- 移动端卡片 -->
    <div v-else-if="!loading" class="doc-cards-container">
      <div v-for="doc in items" :key="doc.id" class="doc-card">
        <div class="doc-card-header">
          <span class="doc-card-id">#{{ doc.id }}</span>
          <StatusBadge :status="doc.status" :status_message="doc.status_message" />
        </div>
        <router-link :to="{ name: 'detail', params: { id: doc.id } }" class="doc-card-title">
          {{ doc.title || doc.filename || '(无标题)' }}
        </router-link>
        <div class="doc-card-meta">
          <span v-if="doc.authors">👤 {{ doc.authors }}</span>
          <span v-if="doc.year">📅 {{ doc.year }}</span>
          <span v-if="doc.doc_type">📋 {{ doc.doc_type }}</span>
        </div>
        <div class="doc-card-files">
          <span>PDF: {{ formatFile(doc.pdf_size) }}</span>
          <span>MD: {{ formatFile(doc.md_size) }}</span>
          <span>ZIP: {{ formatFile(doc.zip_size) }}</span>
        </div>
        <div v-if="doc.indexed_dbs && doc.indexed_dbs.length" class="doc-card-indexes">
          <span
            v-for="dbId in doc.indexed_dbs"
            :key="dbId"
            class="index-db-tag"
          >{{ dbId }}</span>
        </div>
        <div class="doc-card-actions">
          <router-link
            :to="{ name: 'docManage', params: { id: doc.id } }"
            class="btn btn-sm btn-outline"
          >
            管理
          </router-link>
        </div>
      </div>
    </div>

    <!-- 分页 -->
    <div v-if="total > 0" class="pagination">
      <button :disabled="page <= 1" @click="page--; doLoad()">上一页</button>
      <span>第 {{ page }} 页，共 {{ total }} 条</span>
      <button :disabled="page >= totalPages" @click="page++; doLoad()">下一页</button>
    </div>

    <!-- 空状态 -->
    <div v-if="items.length === 0 && !loading" class="empty">
      <svg class="empty-icon" width="64" height="64" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round">
        <path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z"></path>
        <polyline points="14 2 14 8 20 8"></polyline>
        <line x1="12" y1="18" x2="12" y2="12"></line>
        <line x1="9" y1="15" x2="15" y2="15"></line>
      </svg>
      <p>暂无文献</p>
      <router-link :to="{ name: 'upload' }" class="btn">立即上传</router-link>
    </div>
  </div>
</template>

<script setup lang="ts">
import { ref, computed, onMounted } from 'vue'
import { listDocuments } from '@/api/documents'
import { useToast } from '@/composables/useToast'
import { useRequireToken } from '@/composables/useRequireToken'
import { useResponsive } from '@/composables/useResponsive'
import { usePagination } from '@/composables/usePagination'
import StatusBadge from '@/components/StatusBadge.vue'
import type { Document } from '@/types/document'
import {
  parseSearchQuery,
  parsedQueryToParams,
  buildSearchParams,
  SYNTAX_HELP_ITEMS,
  SEARCH_FIELD_DEFS
} from '@/utils/searchParser'

const { showError } = useToast()
const { requireToken } = useRequireToken()
const { isMobile } = useResponsive()

const { items, loading, page, total, totalPages, load } = usePagination<Document>()

const searchQuery = ref('')
const showSyntaxHelp = ref(false)
const searchWrapRef = ref<HTMLElement | null>(null)
const searchInputRef = ref<HTMLInputElement | null>(null)
const syntaxHelpRef = ref<HTMLElement | null>(null)

const syntaxHelpItems = SYNTAX_HELP_ITEMS

// 排序字段中文名映射
const SORT_LABELS: Record<string, string> = {
  created_at: '登记时间',
  updated_at: '更新时间',
  title: '标题',
  authors: '作者',
  year: '年份',
  doc_type: '类型',
  status: '状态',
  journal: '期刊',
  doi: 'DOI',
  id: 'ID',
}

// 活跃筛选条件 chips
const activeFilterChips = computed(() => {
  const chips: string[] = []
  if (!searchQuery.value) return chips
  const pq = parseSearchQuery(searchQuery.value)
  const p = parsedQueryToParams(pq)
  if (p.status_filter) chips.push(`状态: ${p.status_filter}`)
  if (p.doc_type_filter) chips.push(`类型: ${p.doc_type_filter}`)
  if (p.year) chips.push(`年份: ${p.year}`)
  if (p.year_min !== undefined && p.year_max !== undefined && p.year_min === p.year_max) {
    // 已在 year 字段处理
  } else {
    if (p.year_min !== undefined) chips.push(`年份 ≥ ${p.year_min}`)
    if (p.year_max !== undefined) chips.push(`年份 ≤ ${p.year_max}`)
  }
  if (p.journal) chips.push(`期刊: ${p.journal}`)
  if (p.authors) chips.push(`作者: ${p.authors}`)
  if (p.sort_by) chips.push(`排序: ${SORT_LABELS[p.sort_by] || p.sort_by}`)
  if (p.sort_order) chips.push(`方向: ${p.sort_order === 'asc' ? '升序' : '降序'}`)
  return chips
})

// 语法高亮：给 field: 前缀上色
const highlightedQuery = computed(() => {
  const q = searchQuery.value
  if (!q) return ''
  // 给 field:value 的 field: 部分着色
  let result = q
  for (const def of SEARCH_FIELD_DEFS) {
    const re = new RegExp(`(\\b${def.prefix}:)`, 'gi')
    result = result.replace(re, `<span class="hl-field">$1</span>`)
  }
  // 给否定前缀着色
  result = result.replace(/(\b-\w+:)/g, '<span class="hl-negate">$1</span>')
  // 给 OR 着色
  result = result.replace(/\bOR\b/g, '<span class="hl-or">OR</span>')
  // 给引号着色
  result = result.replace(/("[^"]*")/g, '<span class="hl-quote">$1</span>')
  return result
})

async function doLoad() {
  if (!requireToken()) return
  const query = searchQuery.value.trim()

  if (query === '') {
    // 无搜索词时正常加载全部
    await load(listDocuments, {})
    return
  }

  // 解析高级搜索语法
  const pq = parseSearchQuery(query)
  const p = parsedQueryToParams(pq)

  await load(listDocuments, {
    q: p.q || undefined,
    search_fields: p.search_fields || undefined,
    status_filter: p.status_filter || undefined,
    doc_type_filter: p.doc_type_filter || undefined,
    year: p.year,
    year_min: p.year_min,
    year_max: p.year_max,
    journal: p.journal || undefined,
    authors: p.authors || undefined,
    sort_by: p.sort_by || undefined,
    sort_order: p.sort_order || undefined,
  })
}

function search() {
  page.value = 1
  doLoad()
}

function clearSearch() {
  searchQuery.value = ''
  search()
}

function onSearchInput() {
  // 输入时关闭语法帮助
  if (searchQuery.value.trim()) {
    showSyntaxHelp.value = false
  }
}

function onSearchBlur() {
  // 延迟关闭语法帮助，允许点击帮助面板
  setTimeout(() => {
    showSyntaxHelp.value = false
  }, 200)
}

onMounted(async () => {
  doLoad()
})

function formatSize(bytes: number | null): string {
  if (!bytes) return '-'
  if (bytes < 1024) return bytes + ' B'
  if (bytes < 1024 * 1024) return (bytes / 1024).toFixed(1) + ' KB'
  return (bytes / (1024 * 1024)).toFixed(1) + ' MB'
}

function formatFile(size: number | null): string {
  if (size == null) return '不存在'
  return formatSize(size)
}
</script>

<style scoped>
.index-dbs {
  display: inline-flex;
  gap: 4px;
  margin-left: 8px;
  flex-wrap: wrap;
  vertical-align: middle;
}
.index-db-tag {
  display: inline-block;
  padding: 1px 6px;
  border-radius: 4px;
  background: var(--bg);
  border: 1px solid var(--border);
  font-size: 11px;
  color: var(--text-secondary);
}
.empty {
  text-align: center;
  padding: 60px 20px;
  color: var(--text-secondary);
}
.empty-icon {
  margin-bottom: 16px;
  color: var(--border);
}
.empty p {
  margin-bottom: 16px;
  font-size: 15px;
}

/* ── 文件列 ──────────────────────────────────────────── */
.col-files {
  min-width: 130px;
}
.file-sizes {
  display: flex;
  flex-direction: column;
  gap: 2px;
  font-size: 11px;
  line-height: 1.4;
}
.file-size-item {
  display: inline-flex;
  gap: 4px;
  align-items: center;
}
.file-label {
  color: var(--text-secondary);
  font-weight: 600;
  min-width: 26px;
}
.file-value {
  color: var(--text);
}

.doc-card-files {
  display: flex;
  flex-wrap: wrap;
  gap: 8px;
  padding: 2px 14px 6px;
  font-size: 11px;
  color: var(--text-secondary);
}

/* ── 骨架屏 ──────────────────────────────────────────── */
.loading-skeleton {
  background: var(--bg-white);
  border: 1px solid var(--border);
  border-radius: 8px;
  overflow: hidden;
}
.skeleton-row {
  display: grid;
  grid-template-columns: 60px 1fr 180px 80px 80px 120px 80px;
  gap: 8px;
  padding: 14px 16px;
  border-bottom: 1px solid var(--border);
  align-items: center;
}
.skeleton-row:last-child {
  border-bottom: none;
}

@keyframes shimmer {
  0% { background-position: -200px 0; }
  100% { background-position: calc(200px + 100%) 0; }
}

.sk {
  height: 14px;
  border-radius: 4px;
  background: linear-gradient(90deg, var(--bg) 25%, var(--bg-muted) 50%, var(--bg) 75%);
  background-size: 200px 100%;
  animation: shimmer 1.5s infinite ease-in-out;
}
.sk-id { width: 40px; }
.sk-title { width: 100%; }
.sk-authors { width: 80%; }
.sk-year { width: 60%; }
.sk-type { width: 60%; }
.sk-status { width: 70%; }
.sk-action { width: 50px; }

/* ── 搜索栏 ──────────────────────────────────────────── */
.search-bar {
  position: relative;
}

.search-input-wrap {
  display: flex;
  align-items: stretch;
  gap: 0;
  flex: 1;
  min-width: 200px;
  position: relative;
}

.search-input-inner {
  position: relative;
  flex: 1;
  min-height: 38px;
}

.search-input {
  width: 100%;
  height: 100%;
  padding: 8px 40px 8px 14px;
  border: 1px solid var(--border);
  border-radius: 8px 0 0 8px;
  font-size: 14px;
  background: var(--bg-white);
  color: var(--text);
  outline: none;
  transition: border-color 0.2s, box-shadow 0.2s;
  position: relative;
  z-index: 2;
  font-family: inherit;
  caret-color: var(--primary);
  min-height: 38px;
}

.search-input:focus {
  border-color: var(--primary);
  box-shadow: 0 0 0 3px rgba(25, 118, 210, 0.12);
}

.search-input::placeholder {
  color: var(--text-muted);
  font-size: 13px;
}

/* 语法高亮背景层 */
.search-highlight {
  position: absolute;
  top: 0;
  left: 0;
  right: 0;
  bottom: 0;
  padding: 8px 40px 8px 14px;
  font-size: 14px;
  font-family: inherit;
  white-space: pre;
  overflow: hidden;
  pointer-events: none;
  z-index: 1;
  color: transparent;
  line-height: 1.6;
}

.search-highlight :deep(.hl-field) {
  color: #1976d2;
  font-weight: 600;
}
.search-highlight :deep(.hl-negate) {
  color: #d32f2f;
  font-weight: 600;
}
.search-highlight :deep(.hl-or) {
  color: #e65100;
  font-weight: 700;
}
.search-highlight :deep(.hl-quote) {
  color: #2e7d32;
}

.btn-search {
  display: flex;
  align-items: center;
  justify-content: center;
  padding: 0 14px;
  background: var(--primary);
  color: #fff;
  border: 1px solid var(--primary);
  border-radius: 0 8px 8px 0;
  cursor: pointer;
  transition: background 0.2s;
  min-height: 38px;
}
.btn-search:hover {
  background: var(--primary-dark);
}
.btn-search:disabled {
  opacity: 0.6;
  cursor: not-allowed;
}

/* 语法帮助面板 */
.syntax-help {
  position: absolute;
  top: calc(100% + 4px);
  left: 0;
  right: 0;
  z-index: 100;
  background: var(--bg-white);
  border: 1px solid var(--border);
  border-radius: 8px;
  box-shadow: 0 8px 24px rgba(0, 0, 0, 0.12);
  padding: 12px 16px;
  max-height: 320px;
  overflow-y: auto;
}

.syntax-help-header {
  font-size: 12px;
  font-weight: 600;
  color: var(--text-secondary);
  text-transform: uppercase;
  letter-spacing: 0.5px;
  margin-bottom: 10px;
  padding-bottom: 8px;
  border-bottom: 1px solid var(--border);
}

.syntax-help-grid {
  display: grid;
  grid-template-columns: 1fr 1fr;
  gap: 6px 16px;
}

.syntax-help-item {
  display: flex;
  align-items: center;
  gap: 8px;
  font-size: 13px;
  padding: 4px 0;
}

.syntax-help-item code {
  background: var(--bg);
  padding: 2px 6px;
  border-radius: 4px;
  font-size: 12px;
  color: var(--primary);
  white-space: nowrap;
  flex-shrink: 0;
}

.syntax-help-item span {
  color: var(--text-secondary);
  line-height: 1.4;
}

/* 过渡动画 */
.fade-enter-active,
.fade-leave-active {
  transition: opacity 0.15s ease;
}
.fade-enter-from,
.fade-leave-to {
  opacity: 0;
}

/* 活跃筛选 chips */
.active-filters {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: 6px;
  padding: 0 2px;
}

.filter-chip {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  padding: 2px 8px;
  background: var(--primary-light);
  border: 1px solid var(--primary);
  border-radius: 12px;
  font-size: 12px;
  color: var(--primary);
}

.chip-remove {
  background: none;
  border: none;
  color: var(--primary);
  font-size: 14px;
  cursor: pointer;
  padding: 0 2px;
  line-height: 1;
  border-radius: 50%;
}
.chip-remove:hover {
  background: rgba(25, 118, 210, 0.15);
}

.btn-ghost {
  background: none;
  border: none;
  color: var(--text-secondary);
  font-size: 12px;
  padding: 2px 6px;
  cursor: pointer;
}
.btn-ghost:hover {
  color: var(--danger);
}

.search-controls {
  display: flex;
  align-items: center;
  gap: 8px;
}

.ctrl-select {
  padding: 6px 10px;
  border: 1px solid var(--border);
  border-radius: 6px;
  font-size: 13px;
  background: var(--bg-white);
  color: var(--text);
  outline: none;
  cursor: pointer;
  min-height: 34px;
}
.ctrl-select:focus {
  border-color: var(--primary);
}

/* ── 其它 ────────────────────────────────────────────── */
.doc-cards-container {
  display: flex;
  flex-direction: column;
  gap: 10px;
}

@media (max-width: 640px) {
  .skeleton-row {
    grid-template-columns: 1fr;
    gap: 8px;
  }
  .sk-id { width: 60px; }
  .sk-title { width: 100%; }
  .sk-authors { width: 70%; }
  .sk-year { width: 40%; }
  .sk-type { width: 40%; }
  .sk-status { width: 50%; }
  .sk-action { width: 60px; }
  .syntax-help-grid {
    grid-template-columns: 1fr;
  }
}
</style>