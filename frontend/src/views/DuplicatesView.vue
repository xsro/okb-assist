<template>
  <div class="duplicates-view">
    <h2>去重</h2>

    <!-- 检测模式：标题相似 / DOI 重复 -->
    <div class="mode-tabs">
      <button
        class="btn btn-sm"
        :class="mode === 'title' ? 'btn-primary' : 'btn-outline'"
        @click="mode = 'title'"
      >
        标题相似（{{ titleGroups.length }} 组）
      </button>
      <button
        class="btn btn-sm"
        :class="mode === 'doi' ? 'btn-primary' : 'btn-outline'"
        @click="mode = 'doi'"
      >
        DOI 重复（{{ doiGroups.length }} 组）
      </button>
      <button class="btn btn-sm btn-outline" :disabled="loading" @click="load">
        {{ loading ? '检测中…' : '重新检测' }}
      </button>
    </div>

    <p class="hint">
      <template v-if="mode === 'title'">
        按「归一化标题 + 编辑距离相似度」分组，确认后合并到指定文档。
      </template>
      <template v-else>
        按「归一化 DOI」分组（忽略 <code>https://doi.org/</code> 前缀、大小写与尾部标点），确认后合并到指定文档。
      </template>
    </p>

    <div v-if="currentGroups.length" class="groups-list">
      <div v-for="(group, idx) in currentGroups" :key="`${mode}-${idx}`" class="group-card">
        <h4>组 {{ idx + 1 }} · {{ group.documents.length }} 篇 · {{ group.label }}</h4>
        <table class="doc-table">
          <thead>
            <tr><th>ID</th><th>标题</th><th>作者</th><th>年份</th><th>DOI</th><th>操作</th></tr>
          </thead>
          <tbody>
            <tr v-for="doc in group.documents" :key="doc.id">
              <td>{{ doc.id }}</td>
              <td>{{ doc.title }}</td>
              <td>{{ doc.authors || '-' }}</td>
              <td>{{ doc.year || '-' }}</td>
              <td class="doi-cell">{{ doc.doi || '-' }}</td>
              <td>
                <button
                  class="btn btn-sm btn-danger"
                  :disabled="merging"
                  @click="merge(group.documents, doc.id)"
                >
                  {{ merging ? '合并中…' : '合并到此文档' }}
                </button>
              </td>
            </tr>
          </tbody>
        </table>
      </div>
    </div>

    <div v-else class="empty">
      {{ loading ? '检测中…' : mode === 'title' ? '暂无相似文献' : '暂无 DOI 重复的文献' }}
    </div>
  </div>
</template>

<script setup lang="ts">
import { ref, computed, onMounted } from 'vue'
import { getSimilarTitles, getSimilarDois, mergeDocuments } from '@/api/documents'
import { useToast } from '@/composables/useToast'
import { useRequireToken } from '@/composables/useRequireToken'
import type { Document, SimilarTitleGroup, SimilarDoiGroup } from '@/types/document'

const { showSuccess, showError } = useToast()
const { requireToken } = useRequireToken()

const mode = ref<'title' | 'doi'>('title')
const titleGroups = ref<SimilarTitleGroup['groups']>([])
const doiGroups = ref<SimilarDoiGroup['groups']>([])
const loading = ref(false)
const merging = ref(false)

/** 两种模式统一成同一渲染结构 */
const currentGroups = computed<{ label: string; documents: Document[] }[]>(() =>
  mode.value === 'title'
    ? titleGroups.value.map((g) => ({
        label: `相似标题：${g.normalized_title}`,
        documents: g.documents,
      }))
    : doiGroups.value.map((g) => ({
        label: `DOI：${g.doi}`,
        documents: g.documents,
      }))
)

async function load() {
  if (!requireToken()) return
  loading.value = true
  // 两个检测都是 admin 级只读查询，一次都拉回来，切换 tab 不再等
  const [titles, dois] = await Promise.allSettled([getSimilarTitles(), getSimilarDois()])
  if (titles.status === 'fulfilled') {
    titleGroups.value = titles.value.groups
  } else {
    showError('标题相似检测失败')
  }
  if (dois.status === 'fulfilled') {
    doiGroups.value = dois.value.groups
  } else {
    showError('DOI 重复检测失败')
  }
  loading.value = false
}

async function merge(group: Document[], targetId: number) {
  if (merging.value) return
  const sourceIds = group.filter((d) => d.id !== targetId).map((d) => d.id)
  if (sourceIds.length === 0) return
  const targetTitle = group.find((d) => d.id === targetId)?.title || targetId
  if (
    !confirm(
      `确定将本组其余 ${sourceIds.length} 篇文献合并到「${targetTitle}」吗？\n其余文献将被永久删除（文件与记录），此操作不可逆！`
    )
  ) {
    return
  }
  merging.value = true
  try {
    const res = await mergeDocuments(targetId, sourceIds)
    showSuccess(res.detail || `已合并到文档 ${targetId}`)
    await load()
  } catch {
    showError('合并失败')
  } finally {
    merging.value = false
  }
}

onMounted(load)
</script>

<style scoped>
.mode-tabs {
  display: flex;
  flex-wrap: wrap;
  gap: 8px;
  margin-bottom: 12px;
}

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

.doi-cell {
  font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
  font-size: 12px;
  word-break: break-all;
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
