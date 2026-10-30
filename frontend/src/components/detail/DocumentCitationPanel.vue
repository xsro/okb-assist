<template>
  <CollapsiblePanel
    title="引用条目"
    :badge="badge"
    :hint="data ? (data.source === 'crossref' ? '数据来源：Crossref 记录' : '数据来源：文献元数据') : ''"
    :loading="loading"
    :error="error"
    @open="handleOpen"
  >
    <template v-if="data">
      <div class="citation-toolbar">
        <div class="format-switch">
          <button
            v-for="option in FORMATS"
            :key="option.value"
            type="button"
            class="format-btn"
            :class="{ active: format === option.value }"
            @click="switchFormat(option.value)"
          >
            {{ option.label }}
          </button>
        </div>
        <div class="toolbar-actions">
          <button class="btn btn-sm btn-outline" @click="copy">复制</button>
          <button class="btn btn-sm btn-outline" @click="download">下载 {{ fileExt }}</button>
        </div>
      </div>

      <p v-if="data.source === 'document'" class="source-note">
        未找到 Crossref 记录，条目由文献元数据生成（缺少卷、期、页码等信息）。
      </p>

      <pre class="citation-text">{{ data.content }}</pre>

      <div class="citation-meta">
        <span class="meta-item">引用键：<code>{{ data.key }}</code></span>
        <span class="meta-item">{{ data.content.length }} 字符</span>
      </div>
    </template>
  </CollapsiblePanel>
</template>

<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import CollapsiblePanel from '@/components/CollapsiblePanel.vue'
import { getCitation } from '@/api/documents'
import { useToast } from '@/composables/useToast'
import type { CitationFormat, CitationResponse } from '@/types/document'

const FORMATS: { value: CitationFormat; label: string }[] = [
  { value: 'bibtex', label: 'BibTeX' },
  { value: 'gbt7714', label: 'GB/T 7714' }
]

const props = defineProps<{ docId: number }>()

const { showError, showSuccess } = useToast()

const data = ref<CitationResponse | null>(null)
const loading = ref(false)
const error = ref('')
const format = ref<CitationFormat>('bibtex')
/** 面板是否已被用户展开（切换文档时决定是否重新加载） */
const opened = ref(false)

const badge = computed(() => {
  if (!data.value) return ''
  return FORMATS.find(f => f.value === data.value?.format)?.label || ''
})

const fileExt = computed(() => (format.value === 'bibtex' ? '.bib' : '.txt'))

function handleOpen() {
  opened.value = true
  load()
}

/** 切换文档时清空旧数据，面板已展开则立即重新加载 */
watch(
  () => props.docId,
  () => {
    data.value = null
    error.value = ''
    if (opened.value) fetchCitation(format.value)
  }
)

async function load() {
  if (data.value) return
  await fetchCitation(format.value)
}

async function switchFormat(next: CitationFormat) {
  if (next === format.value) return
  format.value = next
  await fetchCitation(next)
}

async function fetchCitation(fmt: CitationFormat) {
  if (loading.value) return
  loading.value = true
  error.value = ''
  try {
    data.value = await getCitation(props.docId, fmt, 'auto')
  } catch (e) {
    const detail = (e as { response?: { data?: { detail?: string } } })?.response?.data?.detail
    error.value = detail || '引用条目生成失败'
  } finally {
    loading.value = false
  }
}

async function copy() {
  if (!data.value) return
  try {
    await navigator.clipboard.writeText(data.value.content)
    showSuccess('已复制引用条目')
  } catch {
    showError('复制失败')
  }
}

function download() {
  if (!data.value) return
  const blob = new Blob([data.value.content], { type: 'text/plain;charset=utf-8' })
  const url = URL.createObjectURL(blob)
  const a = document.createElement('a')
  a.href = url
  a.download = `${data.value.key}${fileExt.value}`
  a.click()
  URL.revokeObjectURL(url)
}
</script>

<style scoped>
.citation-toolbar {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 10px;
  flex-wrap: wrap;
  margin-bottom: 10px;
}

.format-switch {
  display: inline-flex;
  border: 1px solid var(--border);
  border-radius: 4px;
  overflow: hidden;
}

.format-btn {
  padding: 6px 14px;
  border: none;
  background: var(--bg-white);
  color: var(--text-secondary);
  font-size: 13px;
  cursor: pointer;
  min-height: 34px;
}

.format-btn + .format-btn {
  border-left: 1px solid var(--border);
}

.format-btn.active {
  background: var(--primary);
  color: #fff;
}

.toolbar-actions {
  display: flex;
  gap: 8px;
}

.source-note {
  margin: 0 0 8px;
  padding: 8px 10px;
  border-radius: 4px;
  background: var(--warning-bg);
  color: var(--warning-text);
  font-size: 12px;
  line-height: 1.6;
}

.citation-text {
  margin: 0;
  padding: 12px;
  max-height: 320px;
  overflow: auto;
  border: 1px solid var(--border);
  border-radius: 4px;
  background: var(--bg-footer);
  font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
  font-size: 12.5px;
  line-height: 1.7;
  white-space: pre-wrap;
  word-break: break-word;
}

.citation-meta {
  display: flex;
  gap: 16px;
  flex-wrap: wrap;
  margin-top: 8px;
  font-size: 12px;
  color: var(--text-muted);
}

.citation-meta code {
  color: var(--text-secondary);
}
</style>
