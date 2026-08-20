<template>
  <div class="point-view">
    <h2>向量库管理</h2>
    <p class="hint">查看和管理向量库中的文档向量点。</p>
    <div class="action-buttons">
      <button class="btn btn-outline" @click="load">刷新</button>
    </div>
    <div v-if="collections" class="collections-list">
      <div v-for="c in collections" :key="c.name" class="collection-card">
        <h4>{{ c.name }}</h4>
        <p>向量数: {{ c.vectors_count || 0 }}</p>
      </div>
    </div>
    <div v-else class="empty">暂无数据</div>
  </div>
</template>

<script setup lang="ts">
import { ref, onMounted } from 'vue'
import { getQdrantCollections } from '@/api/admin'
import { useToast } from '@/composables/useToast'
import { useRequireToken } from '@/composables/useRequireToken'

const { showError } = useToast()
const { requireToken } = useRequireToken()
const collections = ref<any[] | null>(null)

async function load() {
  if (!requireToken()) return
  try {
    const res = await getQdrantCollections()
    collections.value = res.collections
  } catch {
    showError('加载失败')
  }
}

onMounted(load)
</script>

<style scoped>
.hint {
  color: var(--text-secondary);
  margin-bottom: 16px;
}
.collections-list {
  margin-top: 16px;
}
.collection-card {
  padding: 16px;
  margin-bottom: 12px;
  background: #fff;
  border: 1px solid var(--border);
  border-radius: 8px;
}
.collection-card h4 {
  margin-bottom: 4px;
}
.empty {
  text-align: center;
  padding: 60px;
  color: var(--text-secondary);
}
</style>