<template>
  <aside class="source-panel">
    <!-- 头部：源码 / 行范围 / 是否启用编辑 / 保存 -->
    <div class="sp-header">
      <span class="sp-title">源码</span>

      <div class="sp-range">
        <input
          v-model="rangeInput"
          class="sp-range-input"
          type="text"
          :placeholder="`如 120-260 或 120（取 50 行）`"
          @keyup.enter="applyRange"
        />
        <button class="sp-btn" :disabled="loading" @click="applyRange">加载</button>
        <button class="sp-btn" :disabled="loading" title="定位到当前阅读位置" @click="emit('locate')">定位</button>
      </div>

      <label
        class="sp-toggle"
        :class="{ disabled: !canEdit }"
        :title="canEdit ? '开启后可编辑并保存' : '需要 admin 权限才能编辑'"
      >
        <input v-model="editable" type="checkbox" :disabled="!canEdit" />
        启用编辑
      </label>

      <button
        v-if="editable"
        class="sp-btn sp-primary"
        :disabled="!dirty || saving || loading"
        @click="save"
      >
        {{ saving ? '保存中…' : '保存' }}
      </button>
      <button v-if="editable" class="sp-btn" :disabled="!dirty || saving || loading" @click="reset">
        重置
      </button>
      <button
        v-if="editable"
        class="sp-btn sp-danger"
        :disabled="!canDeleteLines"
        :title="deleteHint"
        @click="deleteLines"
      >
        删除行
      </button>

      <button class="sp-close" title="关闭" @click="requestClose">✕</button>
    </div>

    <!-- 状态行 -->
    <div class="sp-meta">
      <span v-if="loadedLines === 0" class="sp-meta-empty">未加载内容</span>
      <template v-else>
        <span>第 {{ rangeText }} 行（共 {{ loadedLines }} 行）</span>
        <span v-if="totalLines > 0" class="sp-meta-dim">/ 全文 {{ totalLines }} 行</span>
        <span v-if="dirty" class="sp-meta-dirty">● 未保存</span>
        <span v-if="!editable && canEdit" class="sp-meta-dim">只读</span>
        <span v-if="conflict" class="sp-meta-conflict">
          ⚠ 文件已被其它操作修改
          <button class="sp-link" @click="reloadWithConfirm">重新加载</button>
        </span>
      </template>
    </div>

    <!-- 编辑器主体 -->
    <div class="sp-body">
      <div v-if="loading" class="sp-hint">加载中…</div>
      <div v-else-if="loadError" class="sp-hint sp-error">
        {{ loadError }}
        <button class="sp-btn" @click="reload">重试</button>
      </div>
      <AceEditor
        v-else
        ref="aceRef"
        v-model="content"
        :read-only="!editable"
        :show-toolbar="false"
        :show-preview-controls="false"
        mode="markdown"
        theme="monokai"
        :font-size="13"
      />
    </div>

    <ConfirmDialog ref="confirmDlg" />
  </aside>
</template>

<script setup lang="ts">
/**
 * SourcePanel — 阅读页右侧 Markdown 源码面板
 *
 * - 默认加载「当前阅读位置所在 chunk」的行范围（父组件传入 startLine/endLine，0-based 含端点）
 * - 行范围输入为 1-based，与阅读页行号栏一致；API 传输用 0-based
 * - 读取使用 raw=1（文件原始内容），写入不做图片路径还原 → 「读-改-写」严格无损
 * - 保存后 emit('saved')，由父组件刷新 chunks/TOC 并回滚到编辑起点
 */
import { ref, computed, watch, onMounted, defineAsyncComponent } from 'vue'
import { getMarkdown, saveMarkdownLines } from '@/api/documents'
import { useToast } from '@/composables/useToast'
import ConfirmDialog from '@/components/ConfirmDialog.vue'

const AceEditor = defineAsyncComponent(() => import('@/components/AceEditor.vue'))

const props = defineProps<{
  docId: number
  /** 初始行范围（0-based，含端点） */
  startLine: number
  endLine: number
  /** 是否具备编辑权限（admin） */
  canEdit: boolean
}>()

const emit = defineEmits<{
  (e: 'close'): void
  (e: 'locate'): void
  (e: 'saved', info: { lineStart: number; lineCount: number; linesWritten: number }): void
}>()

const { showSuccess, showError } = useToast()

const confirmDlg = ref<InstanceType<typeof ConfirmDialog> | null>(null)

// ── 状态 ──
const content = ref('')            // 编辑器内容
const originalContent = ref('')    // 加载时的原始内容（用于 dirty 判定与重置）
const baseHash = ref('')           // 加载时的文件版本（乐观并发控制）
const conflict = ref(false)        // 保存时检测到文件被其它操作修改
const loadedStart = ref(0)         // 已加载范围的起点（0-based）
const loadedLines = ref(0)         // 已加载范围的行数
const totalLines = ref(0)
const loading = ref(false)
const saving = ref(false)
const loadError = ref('')
const editable = ref(false)
const rangeInput = ref('')

// Ace 实例（取选区行范围，用于精准删行）
const aceRef = ref<{ getSelectedRowRange?: () => { startRow: number; endRow: number } | null } | null>(null)

/** 单次「单行号」加载的默认行数 */
const DEFAULT_SPAN = 50

const dirty = computed(() => content.value !== originalContent.value)
const rangeText = computed(() =>
  loadedLines.value === 0
    ? ''
    : loadedLines.value === 1
      ? `${loadedStart.value + 1}`
      : `${loadedStart.value + 1}-${loadedStart.value + loadedLines.value}`
)

/** 解析 1-based 输入 "a-b" / "a" → 0-based 区间 */
function parseRange(text: string): { start: number; end: number } | null {
  const t = text.trim()
  if (!t) return null
  const m = t.match(/^(\d+)\s*(?:[-~到]\s*(\d+))?$/)
  if (!m) return null
  const a = parseInt(m[1], 10)
  if (!Number.isFinite(a) || a < 1) return null
  const b = m[2] !== undefined ? parseInt(m[2], 10) : a + DEFAULT_SPAN - 1
  if (!Number.isFinite(b) || b < a) return null
  return { start: a - 1, end: b - 1 }
}

/** 加载指定 0-based 行范围 */
async function loadRange(start0: number, end0: number) {
  const start = Math.max(0, start0)
  const count = Math.max(1, end0 - start0 + 1)
  loading.value = true
  loadError.value = ''
  try {
    const res = await getMarkdown(props.docId, { line_start: start, line_count: count, raw: true })
    content.value = res.content
    originalContent.value = res.content
    baseHash.value = res.hash
    conflict.value = false
    loadedStart.value = res.line_start
    loadedLines.value = res.lines_returned
    totalLines.value = res.total_lines
    rangeInput.value = loadedLines.value === 1
      ? `${res.line_start + 1}`
      : `${res.line_start + 1}-${res.line_start + res.lines_returned}`
  } catch (err: any) {
    loadError.value = err?.message || '加载失败'
  } finally {
    loading.value = false
  }
}

function applyRange() {
  const r = parseRange(rangeInput.value)
  if (!r) {
    showError('行范围格式无效，示例：120-260 或 120')
    return
  }
  void loadRange(r.start, r.end)
}

function reload() {
  void loadRange(loadedStart.value, loadedStart.value + Math.max(0, loadedLines.value - 1))
}

function reset() {
  content.value = originalContent.value
}

/** 冲突后重新加载（丢弃本地修改前先确认） */
async function reloadWithConfirm() {
  if (dirty.value) {
    const ok = await confirmDlg.value?.show('重新加载？', '将丢弃当前未保存的修改，加载服务器上的最新内容。')
    if (!ok) return
  }
  await loadRange(loadedStart.value, loadedStart.value + Math.max(0, loadedLines.value - 1))
}

/** 从 axios 错误中取后端 detail / status */
function errInfo(err: any): { status?: number; detail?: string } {
  return {
    status: err?.response?.status,
    detail: err?.response?.data?.detail,
  }
}

/** 删行要求「编辑器内容与服务器一致」：否则编辑后的行号无法映射回文件行号 */
const canDeleteLines = computed(() =>
  props.canEdit && editable.value && !dirty.value && !saving.value && !loading.value && loadedLines.value > 0
)
const deleteHint = computed(() => {
  if (!props.canEdit) return '需要 admin 权限'
  if (!editable.value) return '先启用编辑'
  if (dirty.value) return '有未保存的修改，请先保存或重置后再删除（避免行号错位）'
  return '删除选中行；未选中时删除当前加载的整个行范围'
})

/**
 * 删除行：`delete: true` 显式语义（空字符串固定等于 1 个空行，不能表达删行）。
 * 仅在内容与服务器一致时允许，可将选区行号安全映射回文件行号。
 */
async function deleteLines() {
  if (!canDeleteLines.value) return
  const sel = aceRef.value?.getSelectedRowRange?.()
  let start = loadedStart.value
  let count = loadedLines.value
  if (sel && sel.endRow > sel.startRow) {
    start = loadedStart.value + sel.startRow
    count = sel.endRow - sel.startRow + 1
  }
  const scope = sel && sel.endRow > sel.startRow ? '选中的' : '当前加载范围内的全部'
  const ok = await confirmDlg.value?.show(
    '删除行？',
    `将删除${scope}第 ${start + 1}-${start + count} 行（共 ${count} 行），此操作不可撤销。`
  )
  if (!ok) return

  saving.value = true
  try {
    const res = await saveMarkdownLines(props.docId, {
      line_start: start,
      line_count: count,
      content: '',
      delete: true,
      base_hash: baseHash.value || undefined,
    })
    baseHash.value = res.hash
    totalLines.value = res.total_lines
    conflict.value = false
    showSuccess(`已删除 ${count} 行`)
    // 重新加载同一位置，让后续行补上（服务端会按文件尾截断）
    await loadRange(start, start + Math.max(1, loadedLines.value) - 1)
    emit('saved', {
      lineStart: start,
      lineCount: count,
      linesWritten: res.lines_written,
    })
  } catch (err: any) {
    const { status, detail } = errInfo(err)
    if (status === 409) {
      conflict.value = true
      showError(detail || '文件已被其它操作修改，请重新加载后再保存')
    } else {
      showError(detail || err?.message || '删除失败')
    }
  } finally {
    saving.value = false
  }
}

/** 保存：把当前编辑器的行写回 [loadedStart, loadedStart + loadedLines) */
async function save() {
  if (!props.canEdit) return
  saving.value = true
  try {
    const res = await saveMarkdownLines(props.docId, {
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
    rangeInput.value = res.lines_written === 1
      ? `${res.line_start + 1}`
      : `${res.line_start + 1}-${res.line_start + res.lines_written}`
    showSuccess('源码已保存')
    emit('saved', {
      lineStart: res.line_start,
      lineCount: res.line_count,
      linesWritten: res.lines_written,
    })
  } catch (err: any) {
    const { status, detail } = errInfo(err)
    if (status === 409) {
      // 保留本地修改，提示冲突并给出重新加载入口
      conflict.value = true
      showError(detail || '文件已被其它操作修改，请重新加载后再保存')
    } else {
      showError(detail || err?.message || '保存失败')
    }
  } finally {
    saving.value = false
  }
}

async function requestClose() {
  if (dirty.value) {
    const ok = await confirmDlg.value?.show('放弃修改？', '当前源码有未保存的修改，关闭面板将丢弃这些改动。')
    if (!ok) return
  }
  emit('close')
}

// 父组件「定位」后更新范围 → 重新加载
watch(
  () => [props.startLine, props.endLine] as const,
  ([s, e]) => {
    const changed = s !== loadedStart.value || (e - s + 1) !== loadedLines.value
    if (changed) void loadRange(s, e)
  }
)

onMounted(() => {
  void loadRange(props.startLine, props.endLine)
})
</script>

<style scoped>
.source-panel {
  position: fixed;
  top: 56px;
  right: 0;
  bottom: 0;
  width: min(46vw, 760px);
  display: flex;
  flex-direction: column;
  background: #fff;
  border-left: 1px solid var(--border);
  box-shadow: -8px 0 24px rgba(0, 0, 0, 0.08);
  z-index: 400;
}

/* ── 头部 ── */
.sp-header {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 8px 12px;
  border-bottom: 1px solid var(--border);
  flex-wrap: wrap;
}

.sp-title {
  font-size: 13px;
  font-weight: 700;
  color: var(--text);
  white-space: nowrap;
}

.sp-range {
  display: flex;
  align-items: center;
  gap: 4px;
}

.sp-range-input {
  width: 150px;
  padding: 5px 8px;
  border: 1px solid var(--border);
  border-radius: 6px;
  font-size: 13px;
  font-family: inherit;
  color: var(--text);
  background: #fff;
}
.sp-range-input:focus {
  outline: none;
  border-color: var(--primary);
}

.sp-toggle {
  display: inline-flex;
  align-items: center;
  gap: 5px;
  font-size: 13px;
  color: var(--text-secondary);
  cursor: pointer;
  white-space: nowrap;
  user-select: none;
}
.sp-toggle input[type='checkbox'] { accent-color: var(--primary); }
.sp-toggle.disabled { opacity: 0.5; cursor: not-allowed; }

.sp-btn {
  padding: 5px 10px;
  border: 1px solid var(--border);
  border-radius: 6px;
  background: #fff;
  color: var(--text);
  font-size: 13px;
  font-family: inherit;
  cursor: pointer;
  transition: all 0.12s;
  white-space: nowrap;
}
.sp-btn:hover:not(:disabled) { background: var(--bg-secondary); }
.sp-btn:disabled { opacity: 0.45; cursor: not-allowed; }

.sp-primary {
  border-color: var(--primary);
  background: var(--primary);
  color: #fff;
}
.sp-primary:hover:not(:disabled) { background: #1d4ed8; }

.sp-danger {
  color: #b91c1c;
  border-color: #fca5a5;
}
.sp-danger:hover:not(:disabled) { background: #fef2f2; border-color: #b91c1c; }

.sp-close {
  margin-left: auto;
  width: 28px;
  height: 28px;
  border: none;
  border-radius: 6px;
  background: transparent;
  color: var(--text-secondary);
  font-size: 14px;
  cursor: pointer;
}
.sp-close:hover { background: var(--bg-secondary); color: var(--text); }

/* ── 状态行 ── */
.sp-meta {
  display: flex;
  align-items: center;
  gap: 6px;
  padding: 5px 12px;
  border-bottom: 1px solid var(--border);
  background: var(--bg);
  font-size: 12px;
  color: var(--text-secondary);
  min-height: 26px;
}
.sp-meta-dim { color: var(--text-tertiary); }
.sp-meta-dirty { color: #b45309; font-weight: 600; }
.sp-meta-empty { color: var(--text-tertiary); }
.sp-meta-conflict {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  color: #b91c1c;
  font-weight: 600;
}
.sp-link {
  border: none;
  background: transparent;
  padding: 0 2px;
  color: var(--primary);
  font-size: 12px;
  font-family: inherit;
  text-decoration: underline;
  cursor: pointer;
}

/* ── 主体 ── */
.sp-body {
  flex: 1;
  min-height: 0;
  display: flex;
  flex-direction: column;
}

.sp-body :deep(.ace-editor-wrapper) { border: none; border-radius: 0; }

.sp-hint {
  display: flex;
  align-items: center;
  justify-content: center;
  gap: 8px;
  flex: 1;
  font-size: 13px;
  color: var(--text-secondary);
}
.sp-error { color: #c00; }

/* ── 移动端：全屏覆盖 ── */
@media (max-width: 768px) {
  .source-panel {
    top: 0;
    width: 100vw;
    border-left: none;
    box-shadow: none;
  }
  .sp-range-input { width: 110px; }
}
</style>
