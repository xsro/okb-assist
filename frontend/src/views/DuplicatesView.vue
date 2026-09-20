<template>
  <div class="duplicates-view">
    <h2>去重</h2>
    <p class="hint">检测标题相似的文献组（归一化标题 + 编辑距离相似度），确认后合并。</p>

    <div v-if="groups && groups.length" class="groups-list">
      <div v-for="(group, idx) in groups" :key="idx" class="group-card">
        <h4>组 {{ idx + 1 }} · {{ group.count }} 篇 · 相似标题：{{ group.normalized_title }}</h4>
        <table class="doc-table">
          <thead>
            <tr><th>ID</th><th>标题</th><th>作者</th><th>年份</th><th>操作</th></tr>
          </thead>
          <tbody>
            <tr v-for="doc in group.documents" :key="doc.id">
              <td>{{ doc.id }}</td>
              <td>{{ doc.title }}</td>
              <td>{{ doc.authors || '-' }}</td>
              <td>{{ doc.year || '-' }}</td>
              <td>
                <button class="btn btn-sm btn-danger" @click="merge(group.documents, doc.id)">合并到此文档</button>
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
import { getSimilarTitles } from '@/api/documents'
import { useToast } from '@/composables/useToast'
import { useRequireToken } from '@/composables/useRequireToken'
import type { Document, SimilarTitleGroup } from '@/types/document'

const { showError, showInfo } = useToast()
const { requireToken } = useRequireToken()
const groups = ref<SimilarTitleGroup['groups'] | null>(null)

async function load() {
  if (!requireToken()) return
  try {
    const res = await getSimilarTitles()
    groups.value = res.groups
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

@media (max-width: 768px) {
  .group-card {
    padding: 12px;
    margin-bottom: 16px;
  }

  .group-card h4 {
    font-size: 15px;
  }

  .doc-table th,
  .doc-table td {
    padding: 8px 10px;
    font-size: 12px;
  }

  .doc-table .btn {
    padding: 8px 10px;
    font-size: 12px;
  }

  .empty {
    padding: 40px 20px;
  }
}

@media (max-width: 480px) {
  .group-card {
    padding: 10px;
  }

  .doc-table th,
  .doc-table td {
    padding: 6px 8px;
    font-size: 11px;
  }
}
</style>
