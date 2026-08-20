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

    <!-- 加载提示 -->
    <div v-if="loading" class="loading-hint">加载中...</div>

    <!-- 文档表格 -->
    <table v-else class="doc-table">
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
        <tr v-for="doc in documents" :key="doc.id">
          <td>{{ doc.id }}</td>
          <td>
            <router-link :to="{ name: 'detail', params: { id: doc.id } }">
              {{ doc.title }}
            </router-link>
          </td>
          <td>{{ doc.authors || '-' }}</td>
          <td>{{ doc.year || '-' }}</td>
          <td>{{ doc.doc_type || '-' }}</td>
          <td>
            <StatusBadge :status="doc.status" />
            <span v-if="doc.index_status" class="ml-8">
              <StatusBadge :status="doc.index_status" />
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

    <!-- 分页 -->
    <div class="pagination">
      <button :disabled="page <= 1" @click="page--; load()">上一页</button>
      <span>第 {{ page }} 页，共 {{ total }} 条</span>
      <button :disabled="page >= totalPages" @click="page++; load()">下一页</button>
    </div>

    <!-- 空状态 -->
    <div v-if="documents.length === 0 && !loading" class="empty">
      暂无文献，<router-link :to="{ name: 'upload' }">立即上传</router-link>
    </div>
  </div>
</template>

<script setup lang="ts">
import { ref, onMounted, onUnmounted, computed } from 'vue'
import { listDocuments, getDocTypes } from '@/api/documents'
import { useToast } from '@/composables/useToast'
import { useRequireToken } from '@/composables/useRequireToken'
import StatusBadge from '@/components/StatusBadge.vue'
import type { Document } from '@/types/document'

const { showError } = useToast()
const { requireToken } = useRequireToken()

const documents = ref<Document[]>([])
const loading = ref(false)
const searchQuery = ref('')
const filterDocType = ref('')
const sortBy = ref('created_at')
const sortOrder = ref<'asc' | 'desc'>('desc')
const page = ref(1)
const pageSize = 20
const total = ref(0)
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

const sortOrderLabel = computed(() => (sortOrder.value === 'asc' ? '升序' : '降序'))
const totalPages = computed(() => Math.max(1, Math.ceil(total.value / pageSize)))
const searchPlaceholder = computed(() => {
  const selected = searchFieldOptions
    .filter((o) => searchFields.value.includes(o.value))
    .map((o) => o.label)
  if (selected.length === searchFieldOptions.length) return '搜索全部字段...'
  return `搜索${selected.slice(0, 3).join('、')}${selected.length > 3 ? '等' : ''}...`
})

function toggleSortOrder() {
  sortOrder.value = sortOrder.value === 'asc' ? 'desc' : 'asc'
  search()
}

async function load() {
  if (!requireToken()) return
  loading.value = true
  try {
    const res = await listDocuments({
      q: searchQuery.value,
      doc_type_filter: filterDocType.value || undefined,
      search_fields: searchFields.value.join(','),
      sort_by: sortBy.value,
      sort_order: sortOrder.value,
      page: page.value,
      page_size: pageSize
    })
    documents.value = res.items
    total.value = res.total
  } catch (e) {
    showError('加载失败')
  } finally {
    loading.value = false
  }
}

function search() {
  page.value = 1
  load()
}

function onScopeChange() {
  // 至少保留一个字段
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
  load()
})

onUnmounted(() => {
  document.removeEventListener('click', closeScopePanelOnOutside)
})
</script>

<style scoped>
.ml-8 { margin-left: 8px; }
.empty,
.loading-hint {
  text-align: center;
  padding: 60px 20px;
  color: var(--text-secondary);
}

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
  background: #fff;
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
  color: var(--text-primary);
  cursor: pointer;
  white-space: nowrap;
}

.scope-option input[type='checkbox'] {
  cursor: pointer;
}

@media (max-width: 640px) {
  .scope-panel {
    grid-template-columns: 1fr;
    right: auto;
    left: 0;
  }
}
</style>