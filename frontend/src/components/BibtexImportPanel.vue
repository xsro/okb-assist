<template>
  <section class="bibtex-panel section">
    <header class="panel-head">
      <div class="head-text">
        <h3 class="panel-title">从 BibTeX 批量新建条目</h3>
        <p class="panel-desc">
          选择 <code>.bib</code> 文件（Zotero / JabRef / Google Scholar 导出的都行），按 DOI、标题比对本库，
          只为<strong>库中缺失</strong>的文献新建条目。新建的条目只有元数据、没有 PDF
          （PDF 健康显示「缺失」），之后可在文档管理页上传 / 替换 PDF 再走解析流程。
        </p>
      </div>
      <div class="head-actions">
        <input
          ref="fileInput"
          class="file-input"
          type="file"
          accept=".bib,text/plain"
          @change="onFileSelect"
        />
        <button
          class="btn btn-outline"
          :disabled="busy"
          @click="fileInput?.click()"
        >
          {{ fileName ? '重新选择 .bib 文件' : '选择 .bib 文件' }}
        </button>
      </div>
    </header>

    <div v-if="fileName" class="file-line">
      <span class="file-name" :title="fileName">{{ fileName }}</span>
      <span class="file-meta">{{ formatSize(fileSize) }}</span>
      <button
        v-if="resp && !resp.dry_run"
        class="btn btn-sm btn-outline"
        :disabled="busy"
        @click="runPreview"
      >
        再看一次缺失情况
      </button>
    </div>

    <p v-if="errorText" class="error-line">{{ errorText }}</p>
    <p v-if="loading" class="state-line">正在解析条目并比对本库…</p>

    <template v-if="resp && !loading">
      <div class="summary">
        <span class="chip">解析 <strong>{{ resp.total }}</strong> 条</span>
        <span v-if="resp.invalid_count" class="chip chip-muted">
          无效 <strong>{{ resp.invalid_count }}</strong>
        </span>
        <span v-if="resp.duplicate_count" class="chip chip-muted">
          文件内重复 <strong>{{ resp.duplicate_count }}</strong>
        </span>
        <span class="chip chip-exists">已存在 <strong>{{ resp.existing_count }}</strong></span>
        <span v-if="resp.dry_run" class="chip chip-create">
          可新建 <strong>{{ resp.missing }}</strong>
        </span>
        <template v-else>
          <span class="chip chip-create">已新建 <strong>{{ resp.created_count }}</strong></span>
          <span v-if="resp.failed_count" class="chip chip-error">
            失败 <strong>{{ resp.failed_count }}</strong>
          </span>
        </template>
        <span v-if="resp.truncated" class="chip chip-warn">
          仅处理前 {{ resp.entry_limit }} 条
        </span>
        <span v-if="resp.skipped_entries" class="chip chip-muted">
          跳过 @comment/@string <strong>{{ resp.skipped_entries }}</strong>
        </span>
      </div>

      <p v-if="resp.total === 0" class="state-line">
        没解析到任何条目，请确认这是 BibTeX（<code>.bib</code>）文件。
      </p>

      <ul v-if="resp.warnings.length" class="warn-list">
        <li v-for="(w, i) in resp.warnings" :key="i">{{ w }}</li>
      </ul>

      <div class="panel-actions">
        <button
          v-if="resp.dry_run"
          class="btn"
          :disabled="importing || resp.missing === 0"
          @click="doImport"
        >
          {{ importing ? '正在新建…' : `新建缺失的 ${resp.missing} 条` }}
        </button>
        <label v-if="resp.dry_run && resp.missing > 0" class="only-create">
          <input v-model="onlyCreate" type="checkbox" />
          只看可新建
        </label>
        <button
          v-if="!resp.dry_run"
          class="btn btn-outline"
          :disabled="busy"
          @click="reset"
        >
          完成
        </button>
      </div>

      <div v-if="visibleItems.length" class="items">
        <div
          v-for="(item, index) in visibleItems"
          :key="`${item.key}-${index}`"
          class="item"
          :class="`action-${item.action}`"
        >
          <span class="item-badge">{{ actionLabel(item.action) }}</span>
          <div class="item-main">
            <div class="item-title" :title="item.title">
              {{ item.title || '（无标题）' }}
              <span v-if="item.year" class="item-year">({{ item.year }})</span>
            </div>
            <div class="item-sub">
              <code v-if="item.key" class="item-key">{{ item.key }}</code>
              <span v-if="item.doi" class="item-doi">{{ item.doi }}</span>
              <span v-if="item.detail && item.action === 'error'" class="item-detail">
                {{ item.detail }}
              </span>
            </div>
          </div>
          <router-link
            v-if="item.existing_id"
            class="item-link"
            :to="`/assist/detail/${item.existing_id}`"
          >
            查看 #{{ item.existing_id }}
          </router-link>
          <router-link
            v-else-if="item.id"
            class="item-link"
            :to="`/assist/detail/${item.id}`"
          >
            打开 #{{ item.id }}
          </router-link>
        </div>

        <p v-if="resp.items_truncated" class="items-more">
          只显示前 {{ resp.items.length }} 条明细，其余的请看上方统计。
        </p>
      </div>
      <p v-else-if="onlyCreate" class="state-line">没有可新建的条目（全部已存在）。</p>
    </template>
  </section>
</template>

<script setup lang="ts">
import { computed, ref } from 'vue'
import { importBibtex } from '@/api/documents'
import type { BibtexImportItem, BibtexImportResponse } from '@/types/document'
import { useToast } from '@/composables/useToast'
import { useRequireToken } from '@/composables/useRequireToken'

/** 后端上限（`MAX_BIBTEX_BYTES`），超了本地先拦一道 */
const MAX_FILE_BYTES = 8 * 1024 * 1024

const ACTION_LABEL: Record<BibtexImportItem['action'], string> = {
  created: '已新建',
  create: '可新建',
  exists: '已存在',
  duplicate: '文件内重复',
  invalid: '无效',
  error: '失败'
}

const { showSuccess, showError } = useToast()
const { requireToken } = useRequireToken()

const fileInput = ref<HTMLInputElement>()
const fileName = ref('')
const fileSize = ref(0)
/** `.bib` 原始文本（预览与正式导入复用同一份内容） */
const content = ref('')
const loading = ref(false)
const importing = ref(false)
const resp = ref<BibtexImportResponse | null>(null)
const errorText = ref('')
const onlyCreate = ref(false)

const busy = computed(() => loading.value || importing.value)

const visibleItems = computed(() => {
  const items = resp.value?.items ?? []
  if (!onlyCreate.value) return items
  return items.filter((item) => item.action === 'create')
})

function actionLabel(action: BibtexImportItem['action']) {
  return ACTION_LABEL[action] ?? action
}

function formatSize(bytes: number) {
  if (bytes <= 0) return '0 B'
  const units = ['B', 'KB', 'MB']
  const i = Math.min(units.length - 1, Math.floor(Math.log(bytes) / Math.log(1024)))
  return `${(bytes / Math.pow(1024, i)).toFixed(1)} ${units[i]}`
}

async function onFileSelect(e: Event) {
  const input = e.target as HTMLInputElement
  const file = input.files?.[0]
  input.value = ''
  if (!file) return

  if (file.size > MAX_FILE_BYTES) {
    reset()
    fileName.value = file.name
    fileSize.value = file.size
    errorText.value = `文件过大（${formatSize(file.size)}），上限 ${formatSize(MAX_FILE_BYTES)}`
    return
  }

  errorText.value = ''
  resp.value = null
  onlyCreate.value = false
  fileName.value = file.name
  fileSize.value = file.size
  try {
    content.value = await file.text()
  } catch (err) {
    errorText.value = `读取文件失败：${err instanceof Error ? err.message : String(err)}`
    return
  }
  if (!content.value.trim()) {
    errorText.value = '文件内容为空'
    return
  }
  await runPreview()
}

async function runPreview() {
  if (!content.value) return
  if (!requireToken()) return
  loading.value = true
  errorText.value = ''
  try {
    resp.value = await importBibtex(content.value, true)
    onlyCreate.value = false
    if (resp.value.missing === 0) {
      showSuccess(`解析到 ${resp.value.total} 条，库中都已存在`)
    }
  } catch (e: any) {
    resp.value = null
    errorText.value = e?.response?.data?.detail || e?.message || '解析失败'
  } finally {
    loading.value = false
  }
}

async function doImport() {
  if (!content.value || !resp.value) return
  if (!requireToken()) return
  importing.value = true
  errorText.value = ''
  try {
    const result = await importBibtex(content.value, false)
    resp.value = result
    onlyCreate.value = false
    if (result.created_count > 0) {
      showSuccess(`已新建 ${result.created_count} 条${result.failed_count ? `，失败 ${result.failed_count} 条` : ''}`)
    } else if (result.failed_count > 0) {
      showError(`新建失败 ${result.failed_count} 条`)
    } else {
      showSuccess('没有需要新建的条目')
    }
  } catch (e: any) {
    errorText.value = e?.response?.data?.detail || e?.message || '导入失败'
    showError(errorText.value)
  } finally {
    importing.value = false
  }
}

function reset() {
  resp.value = null
  errorText.value = ''
  content.value = ''
  fileName.value = ''
  fileSize.value = 0
  onlyCreate.value = false
}
</script>

<style scoped>
.bibtex-panel {
  border: 1px solid var(--border);
  border-radius: 12px;
  padding: 20px;
  background: var(--bg-white);
}

.panel-head {
  display: flex;
  justify-content: space-between;
  align-items: flex-start;
  gap: 16px;
  flex-wrap: wrap;
  margin-bottom: 12px;
}

.head-text {
  min-width: 260px;
  flex: 1;
}

.panel-title {
  margin: 0 0 6px;
  font-size: 16px;
  color: var(--text);
}

.panel-desc {
  margin: 0;
  font-size: 13px;
  line-height: 1.7;
  color: var(--text-secondary);
}

.panel-desc code {
  padding: 1px 4px;
  border-radius: 3px;
  background: var(--bg-muted);
  font-size: 12px;
}

.head-actions {
  display: flex;
  gap: 8px;
}

.file-input {
  display: none;
}

.file-line {
  display: flex;
  align-items: center;
  gap: 10px;
  flex-wrap: wrap;
  margin: 0 0 12px;
  font-size: 13px;
  color: var(--text-secondary);
}

.file-name {
  max-width: 60vw;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  color: var(--text);
}

.file-meta {
  color: var(--text-muted);
}

.error-line {
  margin: 0 0 12px;
  padding: 10px 12px;
  border-radius: 6px;
  background: var(--danger-bg);
  color: var(--danger-text);
  font-size: 13px;
  line-height: 1.6;
}

.state-line {
  margin: 0 0 12px;
  font-size: 13px;
  color: var(--text-secondary);
}

.summary {
  display: flex;
  gap: 8px;
  flex-wrap: wrap;
  margin-bottom: 12px;
}

.chip {
  padding: 4px 10px;
  border-radius: 999px;
  background: var(--bg-muted);
  color: var(--text-secondary);
  font-size: 12px;
}

.chip strong {
  color: var(--text);
}

.chip-create {
  background: var(--primary-light);
  color: var(--primary-dark);
}

.chip-create strong {
  color: var(--primary-dark);
}

.chip-exists {
  background: var(--success-bg);
  color: var(--success-text);
}

.chip-error {
  background: var(--danger-bg);
  color: var(--danger-text);
}

.chip-warn {
  background: var(--warning-bg);
  color: var(--warning-text);
}

.chip-muted {
  background: transparent;
  border: 1px dashed var(--border);
}

.warn-list {
  margin: 0 0 12px;
  padding-left: 18px;
  font-size: 12px;
  line-height: 1.7;
  color: var(--warning-text);
}

.panel-actions {
  display: flex;
  align-items: center;
  gap: 12px;
  flex-wrap: wrap;
  margin-bottom: 12px;
}

.only-create {
  display: flex;
  align-items: center;
  gap: 6px;
  font-size: 12px;
  color: var(--text-secondary);
  cursor: pointer;
}

.only-create input {
  width: auto;
}

.items {
  display: flex;
  flex-direction: column;
  gap: 6px;
  max-height: 420px;
  overflow: auto;
}

.item {
  display: flex;
  align-items: center;
  gap: 10px;
  padding: 8px 12px;
  border: 1px solid var(--border);
  border-radius: 8px;
  background: var(--bg-white);
}

.item.action-create {
  border-color: #90caf9;
  background: #f5faff;
}

.item.action-created {
  border-color: #a5d6a7;
  background: var(--success-bg);
}

.item.action-error {
  border-color: #ef9a9a;
  background: var(--danger-bg);
}

.item-badge {
  flex: none;
  padding: 2px 8px;
  border-radius: 4px;
  background: var(--bg-muted);
  color: var(--text-secondary);
  font-size: 11px;
  white-space: nowrap;
}

.item.action-create .item-badge {
  background: var(--primary);
  color: #fff;
}

.item.action-created .item-badge {
  background: var(--success);
  color: #fff;
}

.item.action-error .item-badge {
  background: var(--danger);
  color: #fff;
}

.item-main {
  flex: 1;
  min-width: 0;
}

.item-title {
  font-size: 13px;
  color: var(--text);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.item-year {
  color: var(--text-muted);
}

.item-sub {
  display: flex;
  gap: 10px;
  align-items: baseline;
  flex-wrap: wrap;
  margin-top: 2px;
  font-size: 11.5px;
  color: var(--text-muted);
}

.item-key {
  color: var(--text-secondary);
}

.item-doi,
.item-detail {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  max-width: 48vw;
}

.item-detail {
  color: var(--danger-text);
}

.item-link {
  flex: none;
  font-size: 12px;
  color: var(--primary);
  text-decoration: none;
  white-space: nowrap;
}

.item-link:hover {
  text-decoration: underline;
}

.items-more {
  margin: 4px 0 0;
  font-size: 12px;
  color: var(--text-muted);
}

@media (max-width: 768px) {
  .bibtex-panel {
    padding: 16px;
  }

  .panel-head {
    flex-direction: column;
  }

  .head-actions {
    width: 100%;
  }

  .head-actions .btn {
    flex: 1;
  }

  .item {
    align-items: flex-start;
    flex-wrap: wrap;
  }

  .item-doi,
  .item-detail {
    max-width: 70vw;
  }

  .panel-actions .btn {
    flex: 1;
  }
}
</style>
