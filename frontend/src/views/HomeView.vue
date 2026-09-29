<template>
  <div class="home-view">
    <!-- 搜索栏 -->
    <div class="search-bar">
      <input
        v-model="searchQuery"
        type="text"
        :placeholder="searchPlaceholder"
        @keyup.enter="search"
      />
      <div ref="scopePanelRef" class="scope-panel-wrapper">
        <button class="btn btn-outline" @click="showScopePanel = !showScopePanel">
          搜索范围
        </button>
        <div v-if="showScopePanel" class="scope-panel">
          <label v-for="opt in searchFieldOptions" :key="opt.value" class="scope-option">
            <input
              v-model="searchFields"
              type="checkbox"
              :value="opt.value"
              @change="onScopeChange"
            />
            <span>{{ opt.label }}</span>
          </label>
        </div>
      </div>
      <button class="btn" @click="search">搜索</button>
      <select v-model="filterDocType" @change="search">
        <option value="">全部类型</option>
        <option v-for="t in docTypes" :key="t" :value="t">{{ t }}</option>
      </select>
      <select v-model="sortBy" @change="search">
        <option value="created_at">按登记时间</option>
        <option value="updated_at">按更新时间</option>
        <option value="id">按 ID</option>
        <option value="title">按标题</option>
        <option value="authors">按作者</option>
        <option value="year">按年份</option>
        <option value="doc_type">按类型</option>
        <option value="status">按状态</option>
        <option value="journal">按期刊</option>
        <option value="doi">按 DOI</option>
      </select>
      <button class="btn btn-outline" @click="toggleSortOrder">
        {{ sortOrderLabel }}
      </button>
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
            <StatusBadge :status="doc.status" />
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
          <StatusBadge :status="doc.status" />
        </div>
        <router-link :to="{ name: 'detail', params: { id: doc.id } }" class="doc-card-title">
          {{ doc.title || doc.filename || '(无标题)' }}
        </router-link>
        <div class="doc-card-meta">
          <span v-if="doc.authors">👤 {{ doc.authors }}</span>
          <span v-if="doc.year">📅 {{ doc.year }}</span>
          <span v-if="doc.doc_type">📋 {{ doc.doc_type }}</span>
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
import { ref, computed, onMounted, onUnmounted } from 'vue'
import { listDocuments, getDocTypes } from '@/api/documents'
import { useToast } from '@/composables/useToast'
import { useRequireToken } from '@/composables/useRequireToken'
import { useResponsive } from '@/composables/useResponsive'
import { usePagination } from '@/composables/usePagination'
import StatusBadge from '@/components/StatusBadge.vue'
import type { Document } from '@/types/document'

const { showError } = useToast()
const { requireToken } = useRequireToken()
const { isMobile } = useResponsive()

const { items, loading, page, total, totalPages, sortBy, sortOrder, sortOrderLabel, load, toggleSortOrder } = usePagination<Document>()

const searchQuery = ref('')
const filterDocType = ref('')
const docTypes = ref<string[]>([])

const searchFieldOptions = [
  { value: 'title', label: '标题' },
  { value: 'title_en', label: '标题（英文）' },
  { value: 'authors', label: '作者' },
  { value: 'authors_en', label: '作者（英文）' },
  { value: 'keywords', label: '关键词' },
  { value: 'keywords_en', label: '关键词（英文）' },
  { value: 'abstract', label: '摘要' },
  { value: 'abstract_en', label: '摘要（英文）' },
  { value: 'journal', label: '期刊' },
  { value: 'journal_en', label: '期刊（英文）' },
  { value: 'doi', label: 'DOI' },
  { value: 'source', label: '来源' },
  { value: 'filename', label: '文件名' },
  { value: 'category', label: '分类' },
  { value: 'doc_type', label: '文献类型' },
  { value: 'language', label: '语言' }
]
const searchFields = ref<string[]>(['title'])
const showScopePanel = ref(false)
const scopePanelRef = ref<HTMLElement | null>(null)

const searchPlaceholder = computed(() => {
  const selected = searchFieldOptions
    .filter((o) => searchFields.value.includes(o.value))
    .map((o) => o.label)
  if (selected.length === searchFieldOptions.length) return '搜索全部字段...'
  return `搜索${selected.slice(0, 3).join('、')}${selected.length > 3 ? '等' : ''}...`
})

async function doLoad() {
  if (!requireToken()) return
  await load(listDocuments, {
    q: searchQuery.value || undefined,
    doc_type_filter: filterDocType.value || undefined,
    search_fields: searchFields.value.join(',')
  })
}

function search() {
  page.value = 1
  doLoad()
}

function onScopeChange() {
  if (searchFields.value.length === 0) {
    searchFields.value = ['title']
  }
  search()
}

function closeScopePanelOnOutside(event: MouseEvent) {
  if (
    showScopePanel.value &&
    scopePanelRef.value &&
    !scopePanelRef.value.contains(event.target as Node)
  ) {
    showScopePanel.value = false
  }
}

onMounted(async () => {
  document.addEventListener('click', closeScopePanelOnOutside)
  try {
    const res = await getDocTypes()
    docTypes.value = res.doc_types
  } catch { /* ignore */ }
  doLoad()
})

onUnmounted(() => {
  document.removeEventListener('click', closeScopePanelOnOutside)
})
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

.scope-panel-wrapper {
  position: relative;
}

.scope-panel {
  position: absolute;
  top: calc(100% + 8px);
  right: 0;
  z-index: 50;
  min-width: 220px;
  max-height: 360px;
  overflow-y: auto;
  padding: 12px;
  background: var(--bg-white);
  border: 1px solid var(--border);
  border-radius: 8px;
  box-shadow: 0 8px 24px rgba(0, 0, 0, 0.12);
  display: grid;
  grid-template-columns: 1fr 1fr;
  gap: 8px 16px;
}

.scope-option {
  display: flex;
  align-items: center;
  gap: 8px;
  font-size: 13px;
  color: var(--text);
  cursor: pointer;
  white-space: nowrap;
}

.scope-option input[type='checkbox'] {
  cursor: pointer;
}

.doc-cards-container {
  display: flex;
  flex-direction: column;
  gap: 10px;
}

@media (max-width: 640px) {
  .scope-panel {
    grid-template-columns: 1fr;
    right: auto;
    left: 0;
  }
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
}
</style>