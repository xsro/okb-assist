<template>
  <div class="markdown-view">
    <div class="markdown-header">
      <button class="btn btn-sm btn-outline" @click="goBack">返回</button>
      <div class="page-nav">
        <button :disabled="currentPage <= 1" @click="currentPage--; load()">上一页</button>
        <span>第 {{ currentPage }} / {{ totalPages }} 页</span>
        <button :disabled="currentPage >= totalPages" @click="currentPage++; load()">下一页</button>
      </div>
    </div>
    <MarkdownViewer :content="content" />
  </div>
</template>

<script setup lang="ts">
import { ref, onMounted, watch } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import { getMarkdown } from '@/api/documents'
import { useToast } from '@/composables/useToast'
import { useRequireToken } from '@/composables/useRequireToken'
import MarkdownViewer from '@/components/MarkdownViewer.vue'

const route = useRoute()
const router = useRouter()
const { showError } = useToast()
const { requireToken } = useRequireToken()

const content = ref('')
const currentPage = ref(1)
const totalPages = ref(1)

function goBack() {
  router.back()
}

async function load() {
  const id = parseInt(route.params.id as string)
  if (!requireToken()) return
  try {
    const res = await getMarkdown(id, currentPage.value)
    content.value = res.content
    totalPages.value = res.total_pages
  } catch {
    showError('加载失败')
  }
}

watch(() => route.params.id, load)
onMounted(load)
</script>

<style scoped>
.markdown-header {
  display: flex;
  justify-content: space-between;
  align-items: center;
  margin-bottom: 20px;
  padding-bottom: 16px;
  border-bottom: 1px solid var(--border);
}
.page-nav {
  display: flex;
  gap: 12px;
  align-items: center;
}
</style>