<template>
  <div class="duplicates-view">
    <h2>去重</h2>
    <p class="hint">检测标题相似的文献组，确认后合并。</p>

    <div v-if="groups" class="groups-list">
      <div v-for="(group, idx) in groups" :key="idx" class="group-card">
        <h4>组 {{ idx + 1 }} ({{ group.length }} 篇)</h4>
        <table class="doc-table">
          <thead>
            <tr><th>ID</th><th>标题</th><th>作者</th><th>年份</th><th>操作</th></tr>
          </thead>
          <tbody>
            <tr v-for="doc in group" :key="doc.id">
              <td>{{ doc.id }}</td>
              <td>{{ doc.title }}</td>
              <td>{{ doc.authors || '-' }}</td>
              <td>{{ doc.year || '-' }}</td>
              <td>
                <button class="btn btn-sm btn-danger" @click="merge(group, doc.id)">合并到此文档</button>
              </td>
            </tr>
          </tbody>
        </table>
      </div>
    </div>

    <div v-else class="empty">暂无相似文献</div>
  </div>
</template>

<script setup lang="ts">
import { ref, onMounted } from 'vue'
import { listDocuments } from '@/api/documents'
import { useToast } from '@/composables/useToast'
import { useRequireToken } from '@/composables/useRequireToken'
import type { Document } from '@/types/document'

const { showError, showInfo } = useToast()
const { requireToken } = useRequireToken()
const groups = ref<Document[][] | null>(null)

async function load() {
  if (!requireToken()) return
  try {
    // 获取所有文档，然后按标题相似度分组
    const res = await listDocuments({ page_size: 1000 })
    // 简单实现：按标题前缀分组
    const map = new Map<string, Document[]>()
    for (const doc of res.items) {
      const key = doc.title.substring(0, 4).toLowerCase()
      if (!map.has(key)) map.set(key, [])
      map.get(key)!.push(doc)
    }
    groups.value = Array.from(map.values()).filter((g) => g.length > 1)
  } catch {
    showError('加载失败')
  }
}

function merge(group: Document[], targetId: number) {
  showInfo('合并功能开发中')
}

onMounted(load)
</script>

<style scoped>
.hint {
  color: var(--text-secondary);
  margin-bottom: 20px;
}
.group-card {
  margin-bottom: 24px;
  padding: 16px;
  background: #fff;
  border: 1px solid var(--border);
  border-radius: 8px;
}
.group-card h4 {
  margin-bottom: 12px;
}
.empty {
  text-align: center;
  padding: 60px;
  color: var(--text-secondary);
}
</style>