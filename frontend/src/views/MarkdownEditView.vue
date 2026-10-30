<template>
  <div class="md-edit">
    <!-- 编辑专用工具栏（全局 header 已由路由 meta.bare 隐藏） -->
    <header class="me-bar">
      <div class="me-bar-main">
        <button class="me-icon-btn" title="返回阅读" @click="goBackToReading">←</button>
        <span class="me-title">Markdown 编辑</span>
        <span v-if="docId > 0" class="me-doc-id">#{{ docId }}</span>

        <div class="me-range">
          <label class="me-label" for="me-range-input">行范围</label>
          <input
            id="me-range-input"
            v-model="rangeInput"
            class="me-input"
            type="text"
            inputmode="numeric"
            placeholder="如 1-100，留空 = 全文"
            :disabled="loading || saving"
            @keyup.enter="applyRangeInput"
          />
          <button class="me-btn" :disabled="loading || saving" @click="applyRangeInput">加载</button>
          <button
            class="me-btn me-btn-nav"
            :disabled="!canPrev || loading || saving"
            title="上一段（同样行数）"
            @click="shiftRange(-1)"
          >◀</button>
          <button
            class="me-btn me-btn-nav"
            :disabled="!canNext || loading || saving"
            title="下一段（同样行数）"
            @click="shiftRange(1)"
          >▶</button>
        </div>

        <div class="me-status">
          <span v-if="!loadError && loadedLines > 0" class="me-stat">
            第 {{ rangeText }} 行<span v-if="totalLines > 0" class="me-stat-dim"> / 全文 {{ totalLines }} 行</span>
          </span>
          <span v-if="dirty" class="me-dirty">● 未保存</span>
          <span v-if="conflict" class="me-conflict">
            ⚠ 文件已被其它操作修改
            <button class="me-link" @click="reloadWithConfirm">重新加载</button>
          </span>
        </div>

        <div class="me-actions">
          <button class="me-btn" :disabled="!dirty || saving || loading" @click="reset">重置</button>
          <button
            class="me-btn me-primary"
            :disabled="!dirty || saving || loading || !!loadError"
            title="保存 Ctrl+S"
            @click="save"
          >{{ saving ? '保存中…' : '保存' }}</button>
          <button class="me-btn" @click="goBackToReading">退出</button>
        </div>
      </div>

      <div v-if="hintText && !loadError" class="me-hint">{{ hintText }}</div>
    </header>

    <!-- 错误状态（Markdown 不存在 / 无权限 / 行范围参数非法） -->
    <div v-if="loadError" class="me-error">
      <div class="me-error-box">
        <div class="me-error-icon">⚠️</div>
        <h2 class="me-error-title">{{ errorTitle }}</h2>
        <p class="me-error-detail">{{ loadError }}</p>
        <div class="me-error-actions">
          <button class="me-btn" @click="retryLoad">重试</button>
          <button class="me-btn" @click="goBackToReading">返回阅读</button>
          <button class="me-btn me-primary" @click="goHome">文献列表</button>
        </div>
      </div>
    </div>

    <!-- 编辑器主体 -->
    <div v-else class="me-body">
      <div v-if="loading" class="me-loading">
        <span class="me-spinner"></span>
        <span>加载中…</span>
      </div>
      <AceEditor
        v-else
        v-model="content"
        mode="markdown"
        theme="monokai"
        :font-size="15"
        :show-toolbar="true"
        :show-preview-controls="true"
        :show-fullscreen-button="false"
        :first-line-number="loadedStart + 1"
      />
    </div>

    <ConfirmDialog ref="confirmDlg" />
  </div>
</template>

<script setup lang="ts">
/**
 * MarkdownEditView — 全屏 Markdown 行范围编辑器
 *
 * - 路由 `/assist/markdown/:id/edit?line=1-100` 进入，全屏专注编辑（meta.bare 隐藏全局 header/页脚）
 * - 行范围参数为 1-based：`a-b`（闭区间）、`a`（单行）、`a-`（到文件末尾）、留空（全文）
 * - 读取使用 raw=1 + 行切片，写入用 `PUT .../markdown/lines/`（line_count = 已加载行数），
 *   读-改-写严格无损（与后端 split('\n') / join('\n') 互逆语义配对）
 * - Markdown 文件不存在（404）时显示错误页，不渲染编辑器
 * - 行范围可在页面内随时修改；切换范围会自动同步到 URL query（可分享 / 刷新保持）
 * - 有未保存修改时，切换范围、退出路由、关闭页面都会先确认
 */
import { computed, onBeforeUnmount, onMounted, ref, watch } from 'vue'
import { onBeforeRouteLeave, useRoute, useRouter } from 'vue-router'
import { getMarkdown, saveMarkdownLines } from '@/api/documents'
import type { MarkdownResponse } from '@/types/document'
import { useToast } from '@/composables/useToast'
import { useRequireToken } from '@/composables/useRequireToken'
import AceEditor from '@/components/AceEditor.vue'
import ConfirmDialog from '@/components/ConfirmDialog.vue'

const route = useRoute()
const router = useRouter()
const { showSuccess, showError } = useToast()
const { requireToken } = useRequireToken()

const confirmDlg = ref<InstanceType<typeof ConfirmDialog> | null>(null)

// ── 状态 ──
const content = ref('')                // 编辑器内容（已加载范围）
const originalContent = ref('')        // 加载时的内容（dirty 判定 / 重置）
const baseHash = ref('')               // 加载时的文件版本（乐观并发控制）
const loadedStart = ref(0)             // 已加载范围起点（0-based）
const loadedLines = ref(0)             // 已加载范围行数
const totalLines = ref(0)              // 文件总行数
const rangeInput = ref('')             // 行范围输入框（1-based 展示）
const loading = ref(false)
const saving = ref(false)
const conflict = ref(false)
const loadError = ref('')
const errorTitle = ref('加载失败')
const hintText = ref('')

const docId = computed(() => {
  const n = Number(route.params.id)
  return Number.isFinite(n) && n > 0 ? Math.floor(n) : 0
})

const dirty = computed(() => content.value !== originalContent.value)
const rangeText = computed(() =>
  loadedLines.value <= 1
    ? `${loadedStart.value + 1}`
    : `${loadedStart.value + 1}-${loadedStart.value + loadedLines.value}`
)
const canPrev = computed(() => !loadError.value && loadedStart.value > 0)
const canNext = computed(
  () => !loadError.value && totalLines.value > 0 && loadedStart.value + loadedLines.value < totalLines.value
)

// ── 行范围解析（1-based 输入 → 0-based 起点 + 行数） ──
function parseRangeText(text: string): { start: number; count: number | null } | null {
  const t = text.trim().replace(/\s+/g, '')
  if (!t) return { start: 0, count: null } // 留空 = 全文
  const m = t.match(/^(\d+)(?:[-~到](\d*))?$/)
  if (!m) return null
  const a = parseInt(m[1], 10)
  if (!Number.isFinite(a) || a < 1) return null
  if (m[2] === undefined) return { start: a - 1, count: 1 } // 单行
  if (m[2] === '') return { start: a - 1, count: null }      // a- 到末尾
  const b = parseInt(m[2], 10)
  if (!Number.isFinite(b) || b < a) return null
  return { start: a - 1, count: b - a + 1 }
}

function formatRange(start0: number, lines: number): string {
  return lines <= 1 ? `${start0 + 1}` : `${start0 + 1}-${start0 + lines}`
}

// ── 加载 ──
async function loadRange(start0: number, count: number | null) {
  if (!docId.value) {
    errorTitle.value = '文档 ID 无效'
    loadError.value = `无法从地址中解析文档 ID：${String(route.params.id ?? '')}`
    return
  }
  loading.value = true
  loadError.value = ''
  hintText.value = ''
  try {
    const opts: { line_start?: number; line_count?: number; raw?: boolean } = {
      line_start: Math.max(0, start0),
      raw: true,
    }
    if (count !== null && count > 0) opts.line_count = count
    const res = await getMarkdown(docId.value, opts)
    applyLoaded(res, count)
  } catch (err: any) {
    handleLoadError(err)
  } finally {
    loading.value = false
  }
}

function applyLoaded(res: MarkdownResponse, requestedCount: number | null) {
  content.value = res.content
  originalContent.value = res.content
  baseHash.value = res.hash
  conflict.value = false
  loadedStart.value = res.line_start
  loadedLines.value = res.lines_returned
  totalLines.value = res.total_lines
  rangeInput.value = res.lines_returned > 0 ? formatRange(res.line_start, res.lines_returned) : String(res.line_start + 1)

  // 提示：空文件 / 请求范围超出文件末尾 / 起止行被截断
  if (res.total_lines === 0) {
    hintText.value = 'Markdown 文件为空（保存将写入新内容）'
  } else if (res.lines_returned === 0) {
    hintText.value = `第 ${res.line_start + 1} 行之后没有内容（全文共 ${res.total_lines} 行）`
  } else if (requestedCount !== null && res.lines_returned < requestedCount) {
    hintText.value = `请求的 ${requestedCount} 行超出文件末尾，已加载到全文最后一行（共 ${res.total_lines} 行）`
  }

  syncUrlRange()
}

function handleLoadError(err: any) {
  const status = err?.response?.status
  const detail = err?.response?.data?.detail || err?.message || '加载失败'
  if (status === 404) {
    errorTitle.value = 'Markdown 不存在'
    loadError.value = `${detail}（文档 #${docId.value || '?'}）`
  } else if (status === 401 || status === 403) {
    errorTitle.value = '没有编辑权限'
    loadError.value = detail
  } else if (status === 400) {
    errorTitle.value = '无法读取 Markdown'
    loadError.value = detail
  } else {
    errorTitle.value = '加载失败'
    loadError.value = detail
  }
  content.value = ''
  originalContent.value = ''
  loadedLines.value = 0
}

/** 把当前加载范围写回 URL query（line=1-100），保持可分享 / 刷新一致 */
function syncUrlRange() {
  if (loadedLines.value <= 0) return
  const line = formatRange(loadedStart.value, loadedLines.value)
  if (route.query.line === line) return
  void router.replace({ query: { ...route.query, line } })
}

/** 切换范围前，若有未保存修改先确认 */
async function guardedLoad(start0: number, count: number | null) {
  if (dirty.value) {
    const ok = await confirmDlg.value?.show('放弃修改？', '切换行范围会丢弃当前未保存的修改。')
    if (!ok) return
  }
  await loadRange(start0, count)
}

function applyRangeInput() {
  const parsed = parseRangeText(rangeInput.value)
  if (!parsed) {
    showError('行范围格式无效，示例：1-100、120、120-（到末尾）或留空表示全文')
    return
  }
  void guardedLoad(parsed.start, parsed.count)
}

/** 上一段 / 下一段（保持相同行数） */
function shiftRange(dir: -1 | 1) {
  const size = Math.max(1, loadedLines.value)
  const nextStart = loadedStart.value + dir * size
  if (nextStart < 0) return
  void guardedLoad(nextStart, size)
}

function reload() {
  void loadRange(loadedStart.value, Math.max(1, loadedLines.value))
}

async function reloadWithConfirm() {
  if (dirty.value) {
    const ok = await confirmDlg.value?.show('重新加载？', '将丢弃当前未保存的修改，加载服务器上的最新内容。')
    if (!ok) return
  }
  reload()
}

function reset() {
  content.value = originalContent.value
}

function retryLoad() {
  loadError.value = ''
  const parsed = typeof route.query.line === 'string' ? parseRangeText(route.query.line) : null
  if (parsed) void loadRange(parsed.start, parsed.count)
  else void loadRange(loadedStart.value, Math.max(1, loadedLines.value))
}

// ── 保存 ──
async function save() {
  if (saving.value || loading.value || loadError.value || !dirty.value) return
  saving.value = true
  try {
    const res = await saveMarkdownLines(docId.value, {
      line_start: loadedStart.value,
      line_count: loadedLines.value,
      content: content.value,
      base_hash: baseHash.value || undefined,
    })
    originalContent.value = content.value
    baseHash.value = res.hash
    conflict.value = false
    loadedLines.value = res.lines_written
    totalLines.value = res.total_lines
    rangeInput.value = res.lines_written > 0
      ? formatRange(res.line_start, res.lines_written)
      : String(res.line_start + 1)
    hintText.value = ''
    syncUrlRange()
    showSuccess(`已保存第 ${rangeText.value} 行`)
  } catch (err: any) {
    const status = err?.response?.status
    const detail = err?.response?.data?.detail
    if (status === 409) {
      conflict.value = true
      showError(detail || '文件已被其它操作修改，请重新加载后再保存')
    } else {
      showError(detail || err?.message || '保存失败')
    }
  } finally {
    saving.value = false
  }
}

// ── 导航 ──
function goBackToReading() {
  if (!docId.value) {
    void router.push({ name: 'home' })
    return
  }
  void router.push({
    name: 'markdown',
    params: { id: String(docId.value) },
    query: loadedStart.value > 0 ? { target_line: String(loadedStart.value + 1) } : {},
  })
}

function goHome() {
  void router.push({ name: 'home' })
}

// ── 副作用 ──
// URL query 变化（浏览器前进/后退、外部跳转）→ 重新加载对应范围
watch(
  () => route.query.line,
  (line) => {
    if (typeof line !== 'string') return
    if (line === formatRange(loadedStart.value, loadedLines.value)) return
    const parsed = parseRangeText(line)
    if (!parsed) {
      showError(`URL 中的行范围无效：${line}`)
      return
    }
    if (dirty.value) {
      // 用户用浏览器前进/后退切换范围：不静默丢弃修改，回退地址栏
      void router.replace({ query: { ...route.query, line: formatRange(loadedStart.value, loadedLines.value) } })
      showError('当前有未保存的修改，请先保存或重置后再切换行范围')
      return
    }
    void loadRange(parsed.start, parsed.count)
  }
)

// Ctrl+S / Cmd+S 保存；有未保存修改时关闭页面前提示
function onKeydown(e: KeyboardEvent) {
  if ((e.ctrlKey || e.metaKey) && (e.key === 's' || e.key === 'S')) {
    e.preventDefault()
    void save()
  }
}

function onBeforeUnload(e: BeforeUnloadEvent) {
  if (!dirty.value) return
  e.preventDefault()
  e.returnValue = ''
}

onBeforeRouteLeave(async () => {
  if (!dirty.value) return true
  const ok = await confirmDlg.value?.show('放弃修改？', '当前有未保存的修改，离开将丢弃这些改动。')
  return !!ok
})

onMounted(() => {
  window.addEventListener('keydown', onKeydown)
  window.addEventListener('beforeunload', onBeforeUnload)

  if (!requireToken()) return
  const q = route.query.line
  if (typeof q === 'string' && q.trim()) {
    const parsed = parseRangeText(q)
    if (!parsed) {
      errorTitle.value = '行范围无效'
      loadError.value = `无法解析 URL 中的行范围：${q}（示例：1-100、120、120-）`
      return
    }
    void loadRange(parsed.start, parsed.count)
    return
  }
  // 未指定范围 → 全文
  void loadRange(0, null)
})

onBeforeUnmount(() => {
  window.removeEventListener('keydown', onKeydown)
  window.removeEventListener('beforeunload', onBeforeUnload)
})
</script>

<style scoped>
/* ── 全屏容器（父级 main-content-bare 已置零内边距） ── */
.md-edit {
  display: flex;
  flex-direction: column;
  flex: 1;
  min-height: 0;
  height: 100%;
  background: var(--bg, #f5f6f8);
}

/* ── 工具栏 ── */
.me-bar {
  flex-shrink: 0;
  padding: 6px 12px;
  background: #fff;
  border-bottom: 1px solid var(--border, #e5e7eb);
  box-shadow: 0 1px 3px rgba(0, 0, 0, 0.04);
}

.me-bar-main {
  display: flex;
  align-items: center;
  gap: 10px;
  flex-wrap: wrap;
}

.me-icon-btn {
  width: 30px;
  height: 30px;
  border: 1px solid var(--border, #e5e7eb);
  border-radius: 6px;
  background: #fff;
  color: var(--text, #1f2937);
  font-size: 15px;
  line-height: 1;
  cursor: pointer;
  flex-shrink: 0;
}
.me-icon-btn:hover { background: var(--bg-secondary, #f3f4f6); }

.me-title {
  font-size: 14px;
  font-weight: 700;
  color: var(--text, #1f2937);
  white-space: nowrap;
}

.me-doc-id {
  font-size: 12px;
  font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
  color: var(--text-secondary, #6b7280);
  background: var(--bg-secondary, #f3f4f6);
  border-radius: 4px;
  padding: 1px 6px;
}

.me-range {
  display: flex;
  align-items: center;
  gap: 4px;
}

.me-label {
  font-size: 12px;
  color: var(--text-secondary, #6b7280);
  white-space: nowrap;
}

.me-input {
  width: 150px;
  padding: 5px 8px;
  border: 1px solid var(--border, #e5e7eb);
  border-radius: 6px;
  font-size: 13px;
  font-family: inherit;
  color: var(--text, #1f2937);
  background: #fff;
}
.me-input:focus {
  outline: none;
  border-color: var(--primary, #1976d2);
  box-shadow: 0 0 0 2px rgba(25, 118, 210, 0.12);
}

.me-btn {
  padding: 5px 10px;
  border: 1px solid var(--border, #e5e7eb);
  border-radius: 6px;
  background: #fff;
  color: var(--text, #1f2937);
  font-size: 13px;
  font-family: inherit;
  cursor: pointer;
  white-space: nowrap;
  transition: all 0.12s;
}
.me-btn:hover:not(:disabled) { background: var(--bg-secondary, #f3f4f6); }
.me-btn:disabled { opacity: 0.45; cursor: not-allowed; }

.me-btn-nav { padding: 5px 8px; }

.me-primary {
  border-color: var(--primary, #1976d2);
  background: var(--primary, #1976d2);
  color: #fff;
}
.me-primary:hover:not(:disabled) { background: #1565c0; }

.me-status {
  display: flex;
  align-items: center;
  gap: 8px;
  font-size: 12px;
  color: var(--text-secondary, #6b7280);
  min-width: 0;
}

.me-stat { white-space: nowrap; }
.me-stat-dim { color: var(--text-tertiary, #9ca3af); }
.me-dirty { color: #b45309; font-weight: 600; white-space: nowrap; }
.me-conflict {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  color: #b91c1c;
  font-weight: 600;
  white-space: nowrap;
}

.me-link {
  border: none;
  background: transparent;
  padding: 0 2px;
  color: var(--primary, #1976d2);
  font-size: 12px;
  font-family: inherit;
  text-decoration: underline;
  cursor: pointer;
}

.me-actions {
  display: flex;
  align-items: center;
  gap: 6px;
  margin-left: auto;
}

.me-hint {
  margin-top: 5px;
  font-size: 12px;
  color: #b45309;
}

/* ── 编辑器主体 ── */
.me-body {
  flex: 1;
  min-height: 0;
  display: flex;
  flex-direction: column;
}
.me-body :deep(.ace-editor-wrapper) {
  border: none;
  border-radius: 0;
  flex: 1;
  min-height: 0;
}

.me-loading {
  flex: 1;
  display: flex;
  align-items: center;
  justify-content: center;
  gap: 10px;
  font-size: 13px;
  color: var(--text-secondary, #6b7280);
}

.me-spinner {
  width: 16px;
  height: 16px;
  border: 2px solid #d1d5db;
  border-top-color: var(--primary, #1976d2);
  border-radius: 50%;
  animation: me-spin 0.8s linear infinite;
}
@keyframes me-spin { to { transform: rotate(360deg); } }

/* ── 错误状态 ── */
.me-error {
  flex: 1;
  display: flex;
  align-items: center;
  justify-content: center;
  padding: 24px;
}

.me-error-box {
  max-width: 560px;
  width: 100%;
  text-align: center;
  background: #fff;
  border: 1px solid var(--border, #e5e7eb);
  border-radius: 12px;
  padding: 32px 28px;
  box-shadow: 0 4px 20px rgba(0, 0, 0, 0.06);
}

.me-error-icon { font-size: 32px; line-height: 1; }

.me-error-title {
  margin: 10px 0 6px;
  font-size: 18px;
  color: var(--text, #1f2937);
}

.me-error-detail {
  margin: 0 0 20px;
  font-size: 13px;
  color: var(--text-secondary, #6b7280);
  word-break: break-all;
}

.me-error-actions {
  display: flex;
  justify-content: center;
  gap: 10px;
  flex-wrap: wrap;
}

/* ── 移动端 ── */
@media (max-width: 768px) {
  /* 顶栏压缩到 1–2 行：把垂直空间尽量留给编辑器与预览 */
  .me-bar { padding: 4px 6px; }
  .me-bar-main { gap: 4px; row-gap: 4px; }
  .me-icon-btn { width: 26px; height: 26px; font-size: 13px; }
  .me-title { font-size: 12px; }
  .me-doc-id { font-size: 11px; padding: 0 4px; }
  .me-label { display: none; }
  .me-range { gap: 3px; }
  .me-input { width: 72px; padding: 3px 5px; font-size: 12px; }
  .me-btn { padding: 3px 7px; font-size: 12px; }
  .me-btn-nav { padding: 3px 5px; }
  .me-status { order: 10; width: 100%; font-size: 11px; gap: 6px; }
  .me-actions { margin-left: auto; gap: 4px; }
  .me-hint { margin-top: 3px; font-size: 11px; }
}
</style>
