<template>
  <div class="markdown-view">
    <div class="markdown-header">
      <button class="btn btn-sm btn-outline" @click="goBack">返回</button>
      <div class="markdown-controls">
        <button
          class="btn btn-sm"
          :class="viewAll ? 'btn-outline' : 'btn-primary'"
          @click="toggleViewAll"
        >
          {{ viewAll ? '分页查看' : '查看全部' }}
        </button>
        <button
          class="btn btn-sm"
          :class="showSource ? 'btn-primary' : 'btn-outline'"
          @click="showSource = !showSource"
        >
          {{ showSource ? '预览' : '源码' }}
        </button>
        <div v-if="!viewAll" class="page-nav">
          <button :disabled="currentPage <= 1" @click="prevPage">上一页</button>
          <span>第 {{ currentPage }} / {{ totalPages }} 页</span>
          <button :disabled="currentPage >= totalPages" @click="nextPage">下一页</button>
        </div>
        <span v-else class="total-info">共 {{ totalLength.toLocaleString() }} 字符</span>
        <select
          v-if="!viewAll"
          v-model="pageSize"
          class="page-size-select"
          @change="onPageSizeChange"
        >
          <option v-for="opt in pageSizeOptions" :key="opt.value" :value="opt.value">
            {{ opt.label }}
          </option>
        </select>
        <select v-model="mathMode" class="math-mode-select" @change="reloadMath">
          <option v-for="opt in mathModeOptions" :key="opt.value" :value="opt.value">
            {{ opt.label }}
          </option>
        </select>
        <label class="image-toggle">
          <input v-model="loadImages" type="checkbox" />
          <span>加载图片</span>
        </label>
      </div>
    </div>
    <div class="markdown-content">
      <MarkdownViewer :content="content" :math-mode="mathMode" :load-images="loadImages" :show-source="showSource" :highlight="highlight" />
    </div>
  </div>
</template>

<script setup lang="ts">
import { ref, onMounted, watch, nextTick } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import { getMarkdown } from '@/api/documents'
import { useToast } from '@/composables/useToast'
import { useRequireToken } from '@/composables/useRequireToken'
import MarkdownViewer from '@/components/MarkdownViewer.vue'

const route = useRoute()
const router = useRouter()
const { showError } = useToast()
const { requireToken } = useRequireToken()

const highlight = ref('')
const content = ref('')
const currentPage = ref(1)
const totalPages = ref(1)
const totalLength = ref(0)
const viewAll = ref(false)
const showSource = ref(false)
const pageSize = ref(100000)
const mathMode = ref<'none' | 'katex' | 'mathjax'>('katex')
const loadImages = ref(false)

const LARGE_FILE_THRESHOLD = 50000

const pageSizeOptions = [
  { value: 10000, label: '1万字符/页' },
  { value: 50000, label: '5万字符/页' },
  { value: 100000, label: '10万字符/页' },
  { value: 200000, label: '20万字符/页' },
  { value: 500000, label: '50万字符/页' },
  { value: 1000000, label: '100万字符/页' }
]

const mathModeOptions = [
  { value: 'none', label: '不渲染公式' },
  { value: 'katex', label: 'KaTeX' },
  { value: 'mathjax', label: 'MathJax' }
]

function scrollToHighlight() {
  if (!highlight.value.trim()) return
  nextTick(() => {
    const marks = document.querySelectorAll('.markdown-viewer mark, .markdown-viewer .search-highlight')
    if (marks.length > 0) {
      marks[0].scrollIntoView({ behavior: 'smooth', block: 'center' })
    }
  })
}

function goBack() {
  router.back()
}

function nextPage() {
  if (currentPage.value < totalPages.value) {
    currentPage.value++
    load()
  }
}

function prevPage() {
  if (currentPage.value > 1) {
    currentPage.value--
    load()
  }
}

function toggleViewAll() {
  viewAll.value = !viewAll.value
  if (viewAll.value && totalLength.value > LARGE_FILE_THRESHOLD) {
    mathMode.value = 'none'
    loadImages.value = false
  }
  showSource.value = false
  load()
}

function onPageSizeChange() {
  currentPage.value = 1
  load()
}

function reloadMath() {
  // mathMode 变化会触发 MarkdownViewer 重新渲染
}

async function load() {
  const id = parseInt(route.params.id as string)
  if (!requireToken()) return
  try {
    const res = await getMarkdown(id, {
      page: currentPage.value,
      page_size: pageSize.value,
      full: viewAll.value
    })
    content.value = res.content
    totalPages.value = res.total_pages
    totalLength.value = res.total_length
    await nextTick()
    scrollToHighlight()
  } catch {
    showError('加载失败')
  }
}

watch(() => route.params.id, () => {
  currentPage.value = 1
  viewAll.value = false
  load()
})

watch(() => route.query.highlight, (val) => {
  highlight.value = (val as string) || ''
}, { immediate: true })
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
  flex-wrap: wrap;
  gap: 12px;
}

.markdown-controls {
  display: flex;
  align-items: center;
  gap: 12px;
  flex-wrap: wrap;
}

.page-nav {
  display: flex;
  gap: 12px;
  align-items: center;
}

.total-info {
  font-size: 13px;
  color: var(--text-secondary);
}

.page-size-select,
.math-mode-select {
  padding: 4px 8px;
  border: 1px solid var(--border);
  border-radius: 4px;
  font-size: 13px;
  background: #fff;
  color: var(--text-primary);
}

.image-toggle {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  font-size: 13px;
  color: var(--text-primary);
  cursor: pointer;
  user-select: none;
}

.image-toggle input {
  cursor: pointer;
}

.markdown-content {
  background: #fff;
  border: 1px solid var(--border);
  border-radius: 8px;
  padding: 24px;
  min-height: 400px;
}

.markdown-content :deep(pre) {
  background: #f5f5f5;
  padding: 12px;
  border-radius: 6px;
  overflow-x: auto;
}

.markdown-content :deep(img) {
  max-width: 100%;
  height: auto;
}

@media (max-width: 768px) {
  .markdown-header {
    flex-direction: column;
    align-items: flex-start;
    gap: 12px;
    padding-bottom: 12px;
  }

  .markdown-controls {
    width: 100%;
    flex-wrap: wrap;
    gap: 8px;
  }

  .page-nav {
    width: 100%;
    justify-content: center;
    gap: 8px;
  }

  .page-nav button {
    padding: 8px 12px;
    font-size: 13px;
  }

  .page-size-select,
  .math-mode-select {
    flex: 1;
    min-width: 100px;
  }

  .image-toggle {
    flex: 1;
    justify-content: center;
  }
}

@media (max-width: 480px) {
  .markdown-controls {
    flex-direction: column;
    gap: 6px;
  }

  .page-nav {
    order: 3;
    width: 100%;
  }

  .page-nav span {
    width: 100%;
    text-align: center;
  }

  .page-nav button {
    flex: 1;
  }
}
</style>