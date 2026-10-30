<template>
  <CollapsiblePanel
    title="PDF 元信息"
    :badge="pageBadge"
    :hint="data?.filename || ''"
    :loading="loading"
    :error="error"
    @open="handleOpen"
  >
    <template v-if="data">
      <div v-if="data.pdf.error" class="pdf-error">⚠ {{ data.pdf.error }}</div>

      <!-- 概要 -->
      <div class="pdf-summary">
        <div class="summary-item">
          <span class="summary-label">页数</span>
          <span class="summary-value">{{ data.pdf.page_count ?? '未知' }}</span>
        </div>
        <div class="summary-item">
          <span class="summary-label">文件大小</span>
          <span class="summary-value">{{ formatSize(data.pdf.file_size) }}</span>
        </div>
        <div class="summary-item">
          <span class="summary-label">PDF 版本</span>
          <span class="summary-value">{{ data.pdf.pdf_version || '未知' }}</span>
        </div>
        <div class="summary-item">
          <span class="summary-label">加密</span>
          <span class="summary-value">
            <span v-if="data.pdf.encrypted" class="tag tag-warn">是</span>
            <span v-else class="tag">否</span>
          </span>
        </div>
        <div class="summary-item">
          <span class="summary-label">Info 字段</span>
          <span class="summary-value">{{ infoEntries.length }} 项</span>
        </div>
      </div>

      <!-- Info 字典 -->
      <div class="sub-block">
        <div class="sub-title">
          Info 字典
          <button class="btn btn-sm btn-outline" @click="copyJson">复制 JSON</button>
        </div>
        <p v-if="infoEntries.length === 0" class="empty-hint">PDF 未写入 Info 字典</p>
        <table v-else class="kv-table">
          <tr v-for="[key, value] in infoEntries" :key="key">
            <th>{{ key }}</th>
            <td>{{ value }}</td>
          </tr>
        </table>
      </div>

      <!-- 启发式抽取 -->
      <div class="sub-block">
        <div class="sub-title">启发式抽取</div>
        <p v-if="heuristicEntries.length === 0" class="empty-hint">未能从正文中抽取到字段</p>
        <template v-else>
          <p class="sub-note">由 PDF 文本扫描得到，可能与实际文献信息不一致，仅供参考。</p>
          <table class="kv-table">
            <tr v-for="entry in heuristicEntries" :key="entry.key">
              <th>{{ entry.label }}</th>
              <td>{{ entry.value }}</td>
            </tr>
          </table>
        </template>
      </div>
    </template>
  </CollapsiblePanel>
</template>

<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import CollapsiblePanel from '@/components/CollapsiblePanel.vue'
import { getPdfInfo } from '@/api/documents'
import { useToast } from '@/composables/useToast'
import type { PdfInfoResponse } from '@/types/document'

const props = defineProps<{ docId: number }>()

const { showError, showSuccess } = useToast()

const data = ref<PdfInfoResponse | null>(null)
const loading = ref(false)
const error = ref('')
/** 面板是否已被用户展开（切换文档时决定是否重新加载） */
const opened = ref(false)

const pageBadge = computed(() => {
  if (data.value?.pdf?.page_count != null) return `${data.value.pdf.page_count} 页`
  return ''
})

/** Info 字典按常用字段优先展示 */
const INFO_ORDER = [
  'Title',
  'Author',
  'Subject',
  'Keywords',
  'Creator',
  'Producer',
  'CreationDate',
  'ModDate',
  'doi',
  'Format',
  'Encryption'
]

const infoEntries = computed<[string, string][]>(() => {
  const info = data.value?.pdf?.info || {}
  const keys = Object.keys(info)
  keys.sort((a, b) => {
    const ia = INFO_ORDER.findIndex(k => k.toLowerCase() === a.toLowerCase())
    const ib = INFO_ORDER.findIndex(k => k.toLowerCase() === b.toLowerCase())
    if (ia === -1 && ib === -1) return a.localeCompare(b)
    if (ia === -1) return 1
    if (ib === -1) return -1
    return ia - ib
  })
  return keys.map(k => [k, String(info[k])] as [string, string])
})

/** 启发式字段的中文标签 */
const HEURISTIC_LABELS: Record<string, string> = {
  title: '标题',
  authors: '作者',
  journal: '期刊',
  year: '年份',
  doi: 'DOI',
  volume: '卷',
  issue: '期',
  pages: '页码',
  keywords: '关键词',
  abstract: '摘要'
}

const heuristicEntries = computed<{ key: string; label: string; value: string }[]>(() => {
  const h = data.value?.pdf?.heuristics || {}
  const out: { key: string; label: string; value: string }[] = []
  for (const key of Object.keys(HEURISTIC_LABELS)) {
    const raw = h[key]
    const value = formatValue(raw)
    if (!value) continue
    out.push({ key, label: HEURISTIC_LABELS[key], value })
  }
  // 未收录的额外字段一并展示
  for (const key of Object.keys(h)) {
    if (key in HEURISTIC_LABELS) continue
    const value = formatValue(h[key])
    if (!value) continue
    out.push({ key, label: key, value })
  }
  return out
})

function formatValue(raw: unknown): string {
  if (raw == null) return ''
  if (Array.isArray(raw)) return raw.filter(v => v != null && String(v).trim() !== '').join('；')
  if (typeof raw === 'object') return JSON.stringify(raw)
  return String(raw).trim()
}

function formatSize(bytes: number | null | undefined): string {
  if (bytes == null) return '未知'
  if (bytes < 1024) return `${bytes} B`
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`
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
    if (opened.value) load()
  }
)

async function load() {
  if (loading.value || data.value) return
  loading.value = true
  error.value = ''
  try {
    data.value = await getPdfInfo(props.docId)
  } catch (e) {
    const detail = (e as { response?: { data?: { detail?: string } } })?.response?.data?.detail
    error.value = detail || 'PDF 元信息加载失败'
  } finally {
    loading.value = false
  }
}

async function copyJson() {
  if (!data.value) return
  try {
    await navigator.clipboard.writeText(JSON.stringify(data.value.pdf, null, 2))
    showSuccess('已复制 PDF 元信息 JSON')
  } catch {
    showError('复制失败')
  }
}
</script>

<style scoped>
.pdf-summary {
  display: grid;
  grid-template-columns: repeat(auto-fit, minmax(120px, 1fr));
  gap: 10px;
  margin-bottom: 14px;
}

.summary-item {
  display: flex;
  flex-direction: column;
  gap: 2px;
  padding: 8px 10px;
  border: 1px solid var(--border);
  border-radius: 4px;
  background: var(--bg-footer);
}

.summary-label {
  font-size: 12px;
  color: var(--text-muted);
}

.summary-value {
  font-size: 14px;
  color: var(--text);
}

.tag {
  display: inline-block;
  padding: 0 8px;
  border-radius: 4px;
  background: var(--bg-muted);
  font-size: 12px;
}

.tag-warn {
  background: var(--warning-bg);
  color: var(--warning-text);
}

.sub-block + .sub-block {
  margin-top: 16px;
}

.sub-title {
  display: flex;
  align-items: center;
  gap: 10px;
  margin-bottom: 6px;
  font-size: 13px;
  font-weight: 600;
  color: var(--text-secondary);
}

.sub-note {
  margin: 0 0 6px;
  font-size: 12px;
  color: var(--text-muted);
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
  width: 130px;
  color: var(--text-secondary);
  font-weight: 600;
  white-space: nowrap;
}

.empty-hint {
  margin: 0;
  font-size: 13px;
  color: var(--text-muted);
}

.pdf-error {
  margin-bottom: 12px;
  padding: 8px 10px;
  border-radius: 4px;
  background: var(--danger-bg);
  color: var(--danger-text);
  font-size: 13px;
}

@media (max-width: 768px) {
  .kv-table th {
    width: 90px;
  }
}
</style>
