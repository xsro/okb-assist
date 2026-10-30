<template>
  <CollapsiblePanel
    title="Crossref 记录"
    :badge="badge"
    :hint="data?.exists ? fileName : ''"
    :loading="loading"
    :error="error"
    @open="handleOpen"
  >
    <template v-if="data">
      <!-- 无记录 -->
      <div v-if="!data.exists" class="empty-block">
        <p class="empty-hint">
          尚未保存 Crossref 记录。你可以先用 DOI 触发一次获取（后台任务，成功后在此刷新）。
        </p>
        <div class="empty-actions">
          <button
            v-if="tokenStore.canEdit"
            class="btn btn-sm"
            :disabled="fetching"
            @click="fetchFromCrossref"
          >
            {{ fetching ? '已提交，等待结果…' : '从 Crossref 获取' }}
          </button>
          <button class="btn btn-sm btn-outline" :disabled="loading" @click="reload">刷新</button>
        </div>
      </div>

      <!-- 有记录 -->
      <template v-else>
        <div class="record-meta" v-if="data.modified_at">
          <span class="meta-label">抓取时间</span>
          <span class="meta-value">{{ formatTime(data.modified_at) }}</span>
        </div>

        <table class="kv-table" v-if="summaryEntries.length">
          <tr v-for="entry in summaryEntries" :key="entry.label">
            <th>{{ entry.label }}</th>
            <td>{{ entry.value }}</td>
          </tr>
        </table>
        <p v-else class="empty-hint">Crossref 记录中没有可展示的字段（可查看原始 JSON）。</p>

        <details class="raw-block">
          <summary>原始 JSON（{{ rawSize }}）</summary>
          <div class="raw-actions">
            <button class="btn btn-sm btn-outline" @click="copyRaw">复制 JSON</button>
          </div>
          <pre class="raw-json">{{ rawJson }}</pre>
        </details>
      </template>
    </template>
  </CollapsiblePanel>
</template>

<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import CollapsiblePanel from '@/components/CollapsiblePanel.vue'
import { getCrossrefRecord } from '@/api/documents'
import { crossrefDocument } from '@/api/pipeline'
import { useToast } from '@/composables/useToast'
import { useTokenStore } from '@/stores/token'
import type { CrossrefRecordResponse } from '@/types/document'

const props = defineProps<{ docId: number }>()

const { showError, showSuccess, showInfo } = useToast()
const tokenStore = useTokenStore()

const data = ref<CrossrefRecordResponse | null>(null)
const loading = ref(false)
const error = ref('')
const fetching = ref(false)
/** 面板是否已被用户展开（切换文档时决定是否重新加载） */
const opened = ref(false)

const badge = computed(() => {
  if (!data.value) return ''
  return data.value.exists ? '已保存' : '未获取'
})

const fileName = computed(() => {
  const path = data.value?.path || ''
  return path.split('/').pop() || ''
})

/** 记录的摘要字段（来自后端解析结果） */
const summaryEntries = computed(() => {
  const parsed = data.value?.parsed || {}
  const entries: { label: string; value: string }[] = []
  const push = (label: string, raw: unknown) => {
    const value = formatValue(raw)
    if (value) entries.push({ label, value })
  }
  push('标题', parsed.title)
  push('作者', parsed.authors)
  push('期刊/丛书', parsed.journal)
  push('年份', parsed.year)
  push('文献类型', parsed.doc_type)
  push('出版社', parsed.source)
  push('DOI', parsed.doi)
  push('关键词', parsed.keywords)
  push('语言', parsed.language)

  // 补充 Crossref 原始 message 中的卷/期/页（解析结果不含这些字段）
  const message = extractMessage()
  push('卷', message?.volume)
  push('期', message?.issue)
  push('页码', message?.page ?? message?.['article-number'])
  push('ISSN', message?.ISSN)
  push('ISBN', message?.ISBN)
  return entries
})

const rawJson = computed(() => {
  if (!data.value?.raw) return ''
  return JSON.stringify(data.value.raw, null, 2)
})

const rawSize = computed(() => {
  const bytes = new Blob([rawJson.value]).size
  if (bytes < 1024) return `${bytes} B`
  return `${(bytes / 1024).toFixed(1)} KB`
})

function extractMessage(): Record<string, any> | null {
  const raw = data.value?.raw as Record<string, any> | null
  if (!raw) return null
  if (raw.message && typeof raw.message === 'object') return raw.message
  return raw
}

function formatValue(raw: unknown): string {
  if (raw == null) return ''
  if (Array.isArray(raw)) return raw.filter(v => v != null && String(v).trim() !== '').join('；')
  if (typeof raw === 'object') return JSON.stringify(raw)
  return String(raw).trim()
}

function formatTime(value: string): string {
  const d = new Date(value)
  if (Number.isNaN(d.getTime())) return value
  return d.toLocaleString()
}

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
    if (opened.value) reload()
  }
)

async function load() {
  if (data.value) return
  await reload()
}

async function reload() {
  if (loading.value) return
  loading.value = true
  error.value = ''
  try {
    data.value = await getCrossrefRecord(props.docId)
  } catch (e) {
    const detail = (e as { response?: { data?: { detail?: string } } })?.response?.data?.detail
    error.value = detail || 'Crossref 记录加载失败'
  } finally {
    loading.value = false
  }
}

/** 触发后台 Crossref 获取任务，并轮询等待记录出现 */
async function fetchFromCrossref() {
  if (fetching.value) return
  fetching.value = true
  try {
    await crossrefDocument(props.docId)
    showInfo('已提交 Crossref 获取任务')
    for (let i = 0; i < 5; i++) {
      await new Promise(resolve => setTimeout(resolve, 2000))
      const res = await getCrossrefRecord(props.docId)
      if (res.exists) {
        data.value = res
        showSuccess('已获取 Crossref 记录')
        return
      }
    }
    showInfo('尚未获取到记录，可稍后点「刷新」查看')
  } catch (e) {
    const detail = (e as { response?: { data?: { detail?: string } } })?.response?.data?.detail
    showError(detail || 'Crossref 获取失败')
  } finally {
    fetching.value = false
  }
}

async function copyRaw() {
  try {
    await navigator.clipboard.writeText(rawJson.value)
    showSuccess('已复制 Crossref JSON')
  } catch {
    showError('复制失败')
  }
}
</script>

<style scoped>
.record-meta {
  display: flex;
  gap: 8px;
  align-items: baseline;
  margin-bottom: 10px;
  font-size: 13px;
}

.meta-label {
  color: var(--text-muted);
}

.meta-value {
  color: var(--text-secondary);
}

.kv-table {
  width: 100%;
  border-collapse: collapse;
}

.kv-table th,
.kv-table td {
  padding: 6px 10px;
  border-bottom: 1px solid var(--border);
  font-size: 13px;
  text-align: left;
  vertical-align: top;
  word-break: break-word;
}

.kv-table th {
  width: 110px;
  color: var(--text-secondary);
  font-weight: 600;
  white-space: nowrap;
}

.empty-block {
  display: flex;
  flex-direction: column;
  gap: 10px;
}

.empty-hint {
  margin: 0;
  font-size: 13px;
  color: var(--text-muted);
  line-height: 1.6;
}

.empty-actions {
  display: flex;
  gap: 8px;
  flex-wrap: wrap;
}

.raw-block {
  margin-top: 14px;
  font-size: 13px;
}

.raw-block summary {
  cursor: pointer;
  color: var(--text-secondary);
  padding: 4px 0;
}

.raw-actions {
  margin: 8px 0;
}

.raw-json {
  max-height: 420px;
  overflow: auto;
  margin: 0;
  padding: 12px;
  border: 1px solid var(--border);
  border-radius: 4px;
  background: var(--bg-footer);
  font-size: 12px;
  line-height: 1.5;
  white-space: pre;
}

@media (max-width: 768px) {
  .kv-table th {
    width: 80px;
  }
}
</style>
