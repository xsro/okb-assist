<template>
  <div class="home-view">
    <!-- 搜索栏 -->
    <div class="search-bar">
      <input
        v-model="searchQuery"
        type="text"
        placeholder="搜索标题、作者、关键词..."
        @keyup.enter="search"
      />
      <button class="btn" @click="search">搜索</button>
      <select v-model="filterDocType" @change="search">
        <option value="">全部类型</option>
        <option v-for="t in docTypes" :key="t" :value="t">{{ t }}</option>
      </select>
      <select v-model="sortBy" @change="search">
        <option value="created_at">按登记时间</option>
        <option value="title">按标题</option>
        <option value="year">按年份</option>
      </select>
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
import { ref, onMounted, computed } from 'vue'
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
const page = ref(1)
const pageSize = 20
const total = ref(0)
const docTypes = ref<string[]>([])

const totalPages = computed(() => Math.max(1, Math.ceil(total.value / pageSize)))

async function load() {
  if (!requireToken()) return
  loading.value = true
  try {
    const res = await listDocuments({
      q: searchQuery.value,
      doc_type_filter: filterDocType.value || undefined,
      sort_by: sortBy.value as 'created_at' | 'title' | 'year',
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

onMounted(async () => {
  try {
    docTypes.value = await getDocTypes()
  } catch { /* ignore */ }
  load()
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
</style>