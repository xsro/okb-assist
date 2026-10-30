<template>
  <div class="ace-editor-wrapper" :class="{ 'ace-fullscreen': fullscreen }">
    <!-- Markdown 快捷插入工具栏 -->
    <div v-if="showToolbar" class="ace-md-toolbar">
      <button class="tb-btn" @click="insertMarkdown('bold')" title="加粗 Ctrl+B"><strong>B</strong></button>
      <button class="tb-btn" @click="insertMarkdown('italic')" title="斜体 Ctrl+I"><em>I</em></button>
      <button class="tb-btn" @click="insertMarkdown('strike')" title="删除线"><s>S</s></button>
      <span class="tb-sep"></span>
      <button class="tb-btn" @click="insertMarkdown('h1')" title="标题1">H1</button>
      <button class="tb-btn" @click="insertMarkdown('h2')" title="标题2">H2</button>
      <button class="tb-btn" @click="insertMarkdown('h3')" title="标题3">H3</button>
      <span class="tb-sep"></span>
      <button class="tb-btn" @click="insertMarkdown('ul')" title="无序列表">•列表</button>
      <button class="tb-btn" @click="insertMarkdown('ol')" title="有序列表">1.列表</button>
      <button class="tb-btn" @click="insertMarkdown('quote')" title="引用">❝</button>
      <span class="tb-sep"></span>
      <button class="tb-btn" @click="insertMarkdown('link')" title="链接">🔗</button>
      <button class="tb-btn" @click="insertMarkdown('image')" title="图片">🖼</button>
      <button class="tb-btn" @click="insertMarkdown('code')" title="行内代码">`代码`</button>
      <button class="tb-btn" @click="insertMarkdown('codeblock')" title="代码块">```</button>
      <button class="tb-btn" @click="insertMarkdown('inline-math')" title="行内公式">$…$</button>
      <button class="tb-btn" @click="insertMarkdown('block-math')" title="块级公式">$$…$$</button>
      <button class="tb-btn" @click="insertMarkdown('table')" title="表格">▦</button>
      <span class="tb-sep"></span>
      <button class="tb-btn" @click="insertMarkdown('hr')" title="分割线">—</button>
    </div>

    <!-- 主编辑区：桌面端左编辑器 / 右预览，移动端上编辑器 / 下预览 -->
    <div
      ref="bodyEl"
      class="ace-body"
      :class="{ 'ace-split': splitView, 'ace-preview-only': preview && !splitView, 'ace-dragging': dragging }"
      :style="{ '--split-ratio': splitRatio + '%' }"
    >
      <div class="ace-editor-area">
        <div ref="editorEl" class="ace-editor-container"></div>
      </div>

      <!-- 可拖拽分隔条（分屏时显示，双击复位 50:50） -->
      <div
        v-if="splitView"
        class="ace-splitter"
        :class="{ 'ace-splitter-h': isMobile }"
        role="separator"
        :aria-orientation="isMobile ? 'horizontal' : 'vertical'"
        :title="isMobile ? '拖动调整上下比例，双击复位' : '拖动调整左右比例，双击复位'"
        @pointerdown.prevent="startDrag"
        @dblclick="resetRatio"
      ></div>

      <div v-if="preview || splitView" class="ace-preview">
        <div class="ace-preview-head">
          <span class="pv-title">{{ splitView ? '预览' : '预览（全文）' }}</span>

          <!-- 用户可设定的渲染行范围（回车 / 失焦生效） -->
          <span v-if="splitView" class="pv-range-edit" title="预览渲染的行范围（输入后回车生效）">
            <span class="pv-lab">第</span>
            <input
              class="pv-input"
              type="number"
              inputmode="numeric"
              :min="minLine"
              :max="maxLine"
              :value="rangeStartLine"
              aria-label="预览起始行"
              @change="onStartLineInput"
              @keyup.enter="onStartLineInput"
            />
            <span class="pv-lab">–</span>
            <input
              class="pv-input"
              type="number"
              inputmode="numeric"
              :min="minLine"
              :max="maxLine"
              :value="rangeEndLine"
              aria-label="预览结束行"
              @change="onEndLineInput"
              @keyup.enter="onEndLineInput"
            />
            <span class="pv-lab">行</span>
            <span class="pv-total">/ 本段 {{ totalRows }} 行</span>
          </span>

          <span v-else class="pv-range">
            第 {{ fullRange.start }}–{{ fullRange.end }} 行<span class="pv-total"> / 本段 {{ totalRows }} 行</span>
          </span>

          <button
            v-if="splitView"
            class="pv-btn"
            title="把预览渲染范围设为编辑器当前可视范围"
            @click="syncPreviewToViewport"
          >
            <span class="pv-btn-icon">⤓</span><span class="pv-btn-text">同步可见</span>
          </button>

          <button
            v-if="splitView && !isMobile"
            class="pv-ratio"
            title="复位为左右各一半"
            @click="resetRatio"
          >{{ Math.round(splitRatio) }} : {{ Math.round(100 - splitRatio) }}</button>
        </div>
        <div ref="previewScrollEl" class="ace-preview-scroll">
          <MarkdownViewer :content="previewContent" trusted />
        </div>
      </div>
    </div>

    <!-- 底部状态栏 -->
    <div class="ace-statusbar">
      <div class="status-left">
        <button class="stat-btn" @click="search" title="搜索 Ctrl+F">🔍 搜索</button>
        <button class="stat-btn" @click="replace" title="替换 Ctrl+H">🔧 替换</button>
        <span class="tb-sep"></span>
        <template v-if="showPreviewControls">
          <button class="stat-btn" @click="toggleSplit" :title="splitView ? '单栏编辑（只看源码）' : '分屏预览'">
            {{ splitView ? '☰ 编辑' : '⬜ 分屏' }}
          </button>
          <button
            class="stat-btn"
            @click="togglePreview"
            :title="preview ? '退出全屏预览' : (splitView ? '进入全屏预览（隐藏编辑器）' : '全屏预览')"
          >
            {{ preview ? '✕ 预览' : (splitView ? '⛶ 全屏预览' : '👁 预览') }}
          </button>
        </template>
      </div>
      <div class="status-center">
        <button class="stat-btn" @click="zoomOut" title="缩小字体">A−</button>
        <span class="status-fontsize">{{ fontSize }}px</span>
        <button class="stat-btn" @click="zoomIn" title="放大字体">A+</button>
        <span class="tb-sep"></span>
        <button class="stat-btn" @click="undo" title="撤销 Ctrl+Z">↩</button>
        <button class="stat-btn" @click="redo" title="重做 Ctrl+Shift+Z">↪</button>
      </div>
      <div class="status-right">
        <span class="status-cursor">第 {{ cursorRow }} 行, 第 {{ cursorCol }} 列</span>
        <span v-if="selectedLen > 0" class="status-select">已选 {{ selectedLen }} 字符</span>
        <span class="status-wordcount">{{ wordCount }} 字</span>
        <button v-if="showFullscreenButton" class="stat-btn" @click="toggleFullscreen" title="全屏">
          {{ fullscreen ? '⛶ 还原' : '⛶ 全屏' }}
        </button>
      </div>
    </div>
  </div>
</template>

<script setup lang="ts">
import { ref, onMounted, onBeforeUnmount, watch, computed, nextTick } from 'vue'
import ace from 'ace-builds'
import type { Ace } from 'ace-builds'
import MarkdownViewer from '@/components/MarkdownViewer.vue'
import { useResponsive } from '@/composables/useResponsive'

// ===== 静态预加载 Ace 核心模块（theme/mode 无自引用，静态 import 安全） =====
import 'ace-builds/src-noconflict/theme-monokai'
import 'ace-builds/src-noconflict/mode-markdown'
// 静态预注册 snippets：生产构建里没有可推断的 basePath，
// 一旦走动态加载就会拼出相对 URL（如 /assist/markdown/1/snippets/markdown.js）
// 请求回 index.html → "Unexpected token '<'"。本项目只用 markdown/text，直接预注册。
import 'ace-builds/src-noconflict/snippets/markdown'
import 'ace-builds/src-noconflict/snippets/text'



const props = withDefaults(defineProps<{
  modelValue: string
  mode?: string
  theme?: string
  fontSize?: number
  showToolbar?: boolean
  /** 是否显示“分屏/预览”按钮（工具页公式编辑器等纯编辑场景用不到） */
  showPreviewControls?: boolean
  /** 只读模式 */
  readOnly?: boolean
  /** 行号列起始行号（编辑行切片时用于显示真实文件行号） */
  firstLineNumber?: number
  /** 是否显示状态栏的全屏切换按钮（页面本身已占满视口时可关闭） */
  showFullscreenButton?: boolean
}>(), {
  showToolbar: true,
  showPreviewControls: true,
  readOnly: false,
  firstLineNumber: 1,
  showFullscreenButton: true,
})

const emit = defineEmits(['update:modelValue'])

const editorEl = ref<HTMLElement | null>(null)
const bodyEl = ref<HTMLElement | null>(null)
let editor: Ace.Editor | null = null

const { isMobile } = useResponsive()

// --- 状态 ---
// 默认开启分屏（桌面端左编辑右预览 / 移动端上编辑下预览）
// 纯编辑场景（如工具页公式编辑器）传入 showPreviewControls=false，不自动分屏
const preview = ref(false)
const splitView = ref(!!props.showPreviewControls)
const fullscreen = ref(false)
const currentFontSize = ref(props.fontSize || 16)
const cursorRow = ref(1)
const cursorCol = ref(1)
const selectedLen = ref(0)

/** 分屏比例（编辑器面板占比，百分比），可拖拽/双击复位 */
const splitRatio = ref(50)
const dragging = ref(false)
const MIN_RATIO = 20
const MAX_RATIO = 80

// 视口可见行范围（用于「同步可见」按钮）
const visibleFirstRow = ref(0)
const visibleLastRow = ref(0)

// 用于预览的内容引用
const contentRef = ref(props.modelValue)

// 预览的渲染行范围（0-based，含端点，相对 contentRef）
const previewStartRow = ref(0)
const previewEndRow = ref(0)
const previewScrollEl = ref<HTMLElement | null>(null)
/** 单次预览最多渲染的行数（防止一次渲染十万行卡死） */
const MAX_PREVIEW_LINES = 1000

const wordCount = computed(() => {
  const text = props.modelValue || ''
  const chineseChars = (text.match(/[一-龥]/g) || []).length
  const otherChars = text.replace(/[一-龥]/g, '').length
  return chineseChars + otherChars
})

/** 本段总行数 */
const totalRows = computed(() => (contentRef.value || '').split('\n').length)
/** 文件行号偏移（编辑器首行行号 - 1） */
const lineOffset = computed(() => (props.firstLineNumber || 1) - 1)
const minLine = computed(() => 1 + lineOffset.value)
const maxLine = computed(() => totalRows.value + lineOffset.value)
const rangeStartLine = computed(() => previewStartRow.value + lineOffset.value + 1)
const rangeEndLine = computed(() => previewEndRow.value + lineOffset.value + 1)

/** 全屏预览（本段全文）的行范围 */
const fullRange = computed(() => ({ start: minLine.value, end: maxLine.value }))

/** 预览内容：按用户设定的行范围切片（全屏预览时为本段全文） */
const previewContent = computed(() => {
  if (!splitView.value) return contentRef.value
  const lines = contentRef.value.split('\n')
  const start = previewStartRow.value
  const end = Math.min(previewEndRow.value + 1, lines.length)
  if (!(end > start)) return contentRef.value
  return lines.slice(start, end).join('\n')
})

/**
 * 行范围边界可能落在 $$ 块级公式中，导致预览里出现半截 "$$"，
 * 这里把范围向外吸附到完整的公式块。
 */
function snapRangeToMathBlocks(start: number, end: number): [number, number] {
  const lines = (contentRef.value || '').split('\n')
  const isFence = (i: number) => (lines[i] ?? '').trim() === '$$'

  // 起始行落在块内 → 回到块首
  let open = -1
  for (let i = 0; i <= start && i < lines.length; i++) {
    if (isFence(i)) open = open === -1 ? i : -1
  }
  if (open !== -1) start = open

  // 结束行之后仍有未闭合的 $$ → 向后找到闭合行
  open = -1
  for (let i = 0; i <= end && i < lines.length; i++) {
    if (isFence(i)) open = open === -1 ? i : -1
  }
  if (open !== -1) {
    for (let i = end + 1; i < lines.length; i++) {
      if (isFence(i)) { end = i; break }
    }
  }
  return [start, end]
}

/** 设置预览渲染范围（文件行号，1-based，自动矫正顺序/边界/最大行数/公式块） */
function setPreviewRange(startLine: number, endLine: number) {
  const total = totalRows.value
  if (total <= 0) return
  const lo = minLine.value
  const hi = maxLine.value
  const clampLine = (n: number) => Math.min(Math.max(lo, Math.round(n)), hi)

  let start = clampLine(startLine) - lineOffset.value - 1
  let end = clampLine(endLine) - lineOffset.value - 1
  if (end < start) [start, end] = [end, start]
  ;[start, end] = snapRangeToMathBlocks(start, end)
  if (end - start + 1 > MAX_PREVIEW_LINES) {
    end = Math.min(total - 1, start + MAX_PREVIEW_LINES - 1)
    start = Math.max(0, end - MAX_PREVIEW_LINES + 1)
  }
  previewStartRow.value = start
  previewEndRow.value = end
}

function onStartLineInput(e: Event) {
  const el = e.target as HTMLInputElement
  const n = parseInt(el.value, 10)
  if (!Number.isNaN(n)) {
    // 起始行超过当前结束行时，同步把结束行向后推（而不是把两者对调）
    setPreviewRange(n, Math.max(rangeEndLine.value, n))
  }
  el.value = String(rangeStartLine.value)
}

function onEndLineInput(e: Event) {
  const el = e.target as HTMLInputElement
  const n = parseInt(el.value, 10)
  if (!Number.isNaN(n)) {
    // 结束行小于当前起始行时，同步把起始行向前拉
    setPreviewRange(Math.min(rangeStartLine.value, n), n)
  }
  el.value = String(rangeEndLine.value)
}

/** 把预览渲染范围同步为编辑器当前可视范围 */
function syncPreviewToViewport() {
  updateVisibleRange()
  const total = totalRows.value
  if (total <= 0) return
  const start = Math.min(Math.max(0, visibleFirstRow.value), total - 1)
  let end = Math.min(Math.max(start, visibleLastRow.value), total - 1)
  // 至少展示 6 行，避免可视范围过窄时预览几乎为空
  if (end - start < 5) end = Math.min(total - 1, start + 5)
  setPreviewRange(start + lineOffset.value + 1, end + lineOffset.value + 1)
}

// 首次自动同步（等容器尺寸稳定后）只做一次，之后完全由用户控制
let initialRangeSynced = false
function ensureInitialPreviewRange() {
  if (initialRangeSynced) return
  initialRangeSynced = true
  syncPreviewToViewport()
}

// --- 初始化编辑器 ---
function initEditor() {
  if (!editorEl.value) return

  // basePath：开发模式使用本地路径；生产模式下所有模块已通过 Vite 打包注入，无需 basePath
  // 注意：worker 在生产模式已禁用 (useWorker: false)，因此无需 CDN 回退
  ace.config.set('basePath', import.meta.env.DEV ? '/@fs/node_modules/ace-builds/src-noconflict' : '')
  if (import.meta.env.DEV) {
    ace.config.set('workerPath', '/@fs/node_modules/ace-builds/src-noconflict')
  }

  editor = ace.edit(editorEl.value, {
    mode: `ace/mode/${props.mode || 'markdown'}`,
    theme: `ace/theme/${props.theme || 'monokai'}`,
    fontSize: currentFontSize.value,
    firstLineNumber: props.firstLineNumber,
    showPrintMargin: false,
    showGutter: true,
    highlightActiveLine: true,
    wrap: true,
    autoScrollEditorIntoView: true,
    dragDelay: 100,
    useWorker: import.meta.env.DEV,
    tabSize: 2,
  })

  editor.setValue(props.modelValue, -1)
  contentRef.value = props.modelValue
  editor.setReadOnly(!!props.readOnly)

  // 监听内容变化
  editor.on('change', () => {
    const value = editor!.getValue()
    contentRef.value = value
    emit('update:modelValue', value)
  })

  // 动态加载扩展模块（ext-searchbox 含自引用依赖，需要 AMD 完整上下文）
  import('ace-builds/src-noconflict/ext-searchbox')
  import('ace-builds/src-noconflict/ext-language_tools').then(() => {
    // language_tools 加载完毕后再开启补全选项
    if (!editor) return
    editor.setOptions({
      enableBasicAutocompletion: true,
      enableLiveAutocompletion: true,
      enableSnippets: true,
    })
  })

  // 监听光标位置
  editor.selection.on('changeCursor', updateCursor)
  editor.selection.on('changeSelection', updateCursor)

  // 视口范围初始同步 + 滚动/编辑时更新。
  // 注意：Ace 没有 'scroll' 事件；session 的 'changeScrollTop' 只覆盖程序化滚动，
  // 鼠标滚轮/拖动滚动条走的是 renderer 里那个原生可滚动的 .ace_scrollbar-v，
  // 必须直接监听它的 scroll 事件，否则预览不会跟随编辑器滚动。
  updateVisibleRange()
  editor.session.on('changeScrollTop', updateVisibleRange)
  editor.session.on('change', updateVisibleRange)
  bindNativeScroll()

  // Ctrl+滚轮缩放
  editor.on('mousewheel', (e: any) => {
    if (e.ctrlKey) {
      e.preventDefault()
      e.stopPropagation()
      if (e.deltaY < 0) zoomIn()
      else zoomOut()
    }
  })

  // 注册 Ctrl+B / Ctrl+I 快捷键
  editor.commands.addCommand({
    name: 'bold',
    bindKey: { win: 'Ctrl-B', mac: 'Command-B' },
    exec: () => insertMarkdown('bold')
  })
  editor.commands.addCommand({
    name: 'italic',
    bindKey: { win: 'Ctrl-I', mac: 'Command-I' },
    exec: () => insertMarkdown('italic')
  })
}

// --- 光标更新 ---
function updateCursor() {
  if (!editor) return
  const pos = editor.getCursorPosition()
  // 行号加上 firstLineNumber 偏移，切片编辑时与文件真实行号一致
  cursorRow.value = pos.row + (props.firstLineNumber || 1)
  cursorCol.value = pos.column + 1
  const sel = editor.selection.getRange()
  selectedLen.value = Math.abs(
    editor.session.getTextRange(sel).length
  )
}

// 原生滚动容器：鼠标滚轮与拖动滚动条的滚动都发生在 renderer 的滚动条元素上
// （.ace_scrollbar-v 为实际可滚动元素，同时兼容监听 .ace_scroller）
let nativeScrollEls: HTMLElement[] = []

function bindNativeScroll() {
  unbindNativeScroll()
  const candidates: (HTMLElement | null | undefined)[] = [
    editor?.renderer?.scrollBarV?.element as HTMLElement | undefined,
    editorEl.value?.querySelector('.ace_scrollbar-v'),
    editorEl.value?.querySelector('.ace_scroller'),
  ]
  for (const el of candidates) {
    if (!el || nativeScrollEls.includes(el)) continue
    nativeScrollEls.push(el)
    el.addEventListener('scroll', updateVisibleRange, { passive: true })
  }
}

function unbindNativeScroll() {
  for (const el of nativeScrollEls) el.removeEventListener('scroll', updateVisibleRange)
  nativeScrollEls = []
}

/** 同步视口行到 visibleContent，分屏预览只渲染可见部分 */
function updateVisibleRange() {
  if (!editor) return
  // 注意：ace 1.44 的 getFirstVisibleRow() / getLastVisibleRow() 返回的已经是
  // **文档行号**（VirtualRenderer.$computeLayerConfig 内部已做过 screenToDocumentRow 转换），
  // 不能再调一次 screenToDocumentRow（早期写法同时错了这两个点，
  // 导致自动换行时行号跳跃、预览只能覆盖部分可见行）。
  const first = editor.getFirstVisibleRow()
  const last = editor.getLastVisibleRow()
  visibleFirstRow.value = Number.isFinite(first) ? Math.max(0, first) : 0
  visibleLastRow.value = Number.isFinite(last) ? Math.max(0, last) : 0
}

// --- Markdown 插入 ---
function insertMarkdown(type: string) {
  if (!editor) return
  const session = editor.session
  const sel = editor.getSelectionRange()
  const selectedText = editor.session.getTextRange(sel)
  const cursorPos = editor.getCursorPosition()

  const insertions: Record<string, [string, string, number]> = {
    bold:       ['**', '**', 0],
    italic:     ['*', '*', 0],
    strike:     ['~~', '~~', 0],
    code:       ['`', '`', 0],
    link:       ['[', '](url)', 0],
    image:      ['![', '](url)', 0],
    h1:         ['# ', '', -1],
    h2:         ['## ', '', -1],
    h3:         ['### ', '', -1],
    ul:         ['\n- ', '', -1],
    ol:         ['\n1. ', '', -1],
    quote:      ['\n> ', '', -1],
    hr:         ['\n---\n', '', -1],
    codeblock:  ['\n```\n', '\n```\n', 0],
    'inline-math': ['$', '$', 0],
    'block-math':  ['\n$$\n', '\n$$', 0],
  }

  if (type === 'table') {
    const tbl = '\n| 标题 | 标题 |\n|------|------|\n| 内容 | 内容 |\n'
    editor.session.insert(cursorPos, tbl)
    editor.focus()
    return
  }

  const [prefix, suffix, offset] = insertions[type] || ['', '', 0]

  if (selectedText) {
    // 有选中文本：包裹选中内容
    editor.session.replace(sel, prefix + selectedText + suffix)
    // 选中插入后的内容
    const newStart = { row: sel.start.row, column: sel.start.column + prefix.length }
    const newEnd = { row: sel.start.row, column: sel.start.column + prefix.length + selectedText.length + suffix.length }
    editor.selection.setRange({ start: newStart, end: newEnd })
  } else {
    // 没有选中文本：插入并定位光标
    const insertPos = { row: cursorPos.row, column: cursorPos.column }
    editor.session.insert(insertPos, prefix + suffix)

    // 将光标定位到中间
    if (offset !== -1) {
      const midCol = cursorPos.column + prefix.length
      editor.selection.moveCursorTo(cursorPos.row, midCol)
      if (type === 'link' || type === 'image') {
        // 选中 url 部分
        const urlStart = { row: cursorPos.row, column: midCol }
        const urlEnd = { row: cursorPos.row, column: cursorPos.column + prefix.length + suffix.length - 1 }
        editor.selection.setRange({ start: urlStart, end: urlEnd })
        // 再回退到 url 之前来选中 link text
        const textStart = { row: cursorPos.row, column: cursorPos.column + (type === 'link' ? 1 : 2) }
        const textEnd = { row: cursorPos.row, column: cursorPos.column + prefix.length }
        editor.selection.setRange({ start: textStart, end: textEnd })
      }
    } else {
      // 行首插入，光标在行尾
      const newRow = cursorPos.row + 1
      editor.selection.moveCursorTo(newRow, 0)
    }
  }

  editor.focus()
}

// --- 视图操作 ---
function resizeEditor() {
  nextTick(() => {
    if (!editor) return
    editor.resize()
    updateVisibleRange()
  })
}

function toggleSplit() {
  splitView.value = !splitView.value
  preview.value = false
  resizeEditor()
}

function togglePreview() {
  preview.value = !preview.value
  splitView.value = false
  resizeEditor()
}

/** 分屏比例复位为 50:50 */
function resetRatio() {
  splitRatio.value = 50
  resizeEditor()
}

/** 开始拖拽分隔条（pointer 事件同时覆盖鼠标与触屏） */
function startDrag(e: PointerEvent) {
  if (!bodyEl.value) return
  dragging.value = true
  window.addEventListener('pointermove', onDrag)
  window.addEventListener('pointerup', endDrag)
  window.addEventListener('pointercancel', endDrag)
  e.preventDefault()
}

function onDrag(e: PointerEvent) {
  if (!dragging.value || !bodyEl.value) return
  const rect = bodyEl.value.getBoundingClientRect()
  const pct = isMobile.value
    ? ((e.clientY - rect.top) / Math.max(rect.height, 1)) * 100
    : ((e.clientX - rect.left) / Math.max(rect.width, 1)) * 100
  splitRatio.value = Math.min(MAX_RATIO, Math.max(MIN_RATIO, pct))
  if (editor) editor.resize()
}

function endDrag() {
  dragging.value = false
  window.removeEventListener('pointermove', onDrag)
  window.removeEventListener('pointerup', endDrag)
  window.removeEventListener('pointercancel', endDrag)
  resizeEditor()
}

function toggleFullscreen() {
  fullscreen.value = !fullscreen.value
  resizeEditor()
}

function zoomIn() {
  if (currentFontSize.value >= 30) return
  currentFontSize.value += 2
  if (editor) editor.setFontSize(currentFontSize.value)
}

function zoomOut() {
  if (currentFontSize.value <= 8) return
  currentFontSize.value -= 2
  if (editor) editor.setFontSize(currentFontSize.value)
}

function undo() {
  if (!editor) return
  editor.execCommand('undo')
}

function redo() {
  if (!editor) return
  editor.execCommand('redo')
}

function search() {
  if (!editor) return
  editor.execCommand('find')
}

function replace() {
  if (!editor) return
  editor.execCommand('replace')
}

/** 当前选区所在行范围（0-based，含端点，相对编辑器内容）；无编辑器时返回 null */
defineExpose({
  getSelectedRowRange(): { startRow: number; endRow: number } | null {
    if (!editor) return null
    const r = editor.getSelectionRange()
    return { startRow: r.start.row, endRow: r.end.row }
  },
  /** 插入行内公式 $…$ */
  insertInlineMath() {
    if (editor) insertMarkdown('inline-math')
  },
  /** 插入块级公式 $$…$$ */
  insertBlockMath() {
    if (editor) insertMarkdown('block-math')
  },
})

// --- 生命周期 ---
// 容器尺寸变化（分屏比例、窗口缩放、移动端旋转）后，Ace 需要重新测量尺寸
let resizeObs: ResizeObserver | null = null

onMounted(() => {
  initEditor()
  // 首次布局完成后再量一次：挂载瞬间容器可能还没拿到高度，
  // 否则“可见行”停在 0-0。
  resizeEditor()
  // 首次设定预览范围必须等布局/字体就绪，否则只能量到一两行
  requestAnimationFrame(() => requestAnimationFrame(ensureInitialPreviewRange))
  if (typeof ResizeObserver !== 'undefined' && editorEl.value) {
    resizeObs = new ResizeObserver(() => {
      if (!editor) return
      editor.resize()
      updateVisibleRange()
      ensureInitialPreviewRange()
    })
    resizeObs.observe(editorEl.value)
  }
})

onBeforeUnmount(() => {
  endDrag()
  unbindNativeScroll()
  resizeObs?.disconnect()
  resizeObs = null
  if (editor) {
    editor.session.off('changeScrollTop', updateVisibleRange)
    editor.session.off('change', updateVisibleRange)
    editor.commands.removeCommands(['bold', 'italic'])
    editor.destroy()
    editor = null
  }
})

// 断点切换（桌面左右 ⇄ 移动上下）后编辑器必须重新测量尺寸
watch(isMobile, () => {
  resizeEditor()
})

// --- 外部响应 ---
watch(() => props.modelValue, (newVal) => {
  // 注意：不能用 isFocused() 拦住，否则「程序式换内容」（如切换行范围）
  // 在编辑器有焦点时会静默丢失。父子值不一致即同步。
  if (editor && newVal !== editor.getValue()) {
    editor.setValue(newVal, -1)
    contentRef.value = newVal
    // 整段内容被替换（如换行范围）：重新按新的可视范围设定预览范围
    nextTick(() => {
      resizeEditor()
      initialRangeSynced = true
      syncPreviewToViewport()
      if (previewScrollEl.value) previewScrollEl.value.scrollTop = 0
    })
  }
})

// 行范围变化时重置预览范围（否则旧行号会越界）
watch(() => props.firstLineNumber, (n) => {
  if (editor) editor.setOptions({ firstLineNumber: n || 1 })
  nextTick(() => syncPreviewToViewport())
})

// 预览范围变化时回到顶部（同一范围内编辑时保留滚动位置）
watch([previewStartRow, previewEndRow], () => {
  nextTick(() => {
    if (previewScrollEl.value) previewScrollEl.value.scrollTop = 0
  })
})

watch(() => props.theme, (newTheme) => {
  if (editor) editor.setTheme(`ace/theme/${newTheme}`)
})

watch(() => props.readOnly, (ro) => {
  if (editor) editor.setReadOnly(!!ro)
})

watch(() => props.mode, (newMode) => {
  if (editor) editor.session.setMode(`ace/mode/${newMode}`)
})
</script>

<style scoped>
/* ========== 外层容器 ========== */
.ace-editor-wrapper {
  display: flex;
  flex-direction: column;
  height: 100%;
  border: 1px solid var(--border);
  border-radius: 8px;
  overflow: hidden;
  background: #1e1e1e;
  position: relative;
}

.ace-editor-wrapper.ace-fullscreen {
  position: fixed;
  top: 0; left: 0;
  width: 100vw;
  height: 100vh;
  z-index: 9999;
  border-radius: 0;
  border: none;
}

/* ========== Markdown 快捷工具栏 ========== */
.ace-md-toolbar {
  display: flex;
  align-items: center;
  gap: 2px;
  padding: 4px 8px;
  background: #2d2d2d;
  border-bottom: 1px solid #3c3c3c;
  flex-wrap: wrap;
  flex-shrink: 0;
}

.tb-btn {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  min-width: 28px;
  height: 26px;
  padding: 0 6px;
  border: none;
  border-radius: 3px;
  background: transparent;
  color: #cccccc;
  cursor: pointer;
  font-size: 12px;
  font-family: inherit;
  white-space: nowrap;
  transition: all 0.15s;
}

.tb-btn:hover {
  background: #3c3c3c;
  color: #ffffff;
}

.tb-btn:active {
  background: #505050;
}

.tb-btn strong { font-weight: 700; font-size: 13px; }
.tb-btn em { font-style: italic; font-size: 13px; }
.tb-btn s { text-decoration: line-through; font-size: 12px; }

.tb-sep {
  width: 1px;
  height: 18px;
  background: #3c3c3c;
  margin: 0 4px;
  flex-shrink: 0;
}

/* ========== 主体编辑区：桌面左编辑/右预览，移动上编辑/下预览 ========== */
.ace-body {
  flex: 1 1 auto;
  display: flex;
  flex-direction: column;
  min-height: 0;
  min-width: 0;
  overflow: hidden;
  /* 编辑器面板占比，由拖拽分隔条控制 */
  --split-ratio: 50%;
}

/* 仅预览（全屏阅读）模式：隐藏编辑器 */
.ace-body.ace-preview-only .ace-editor-area {
  display: none;
}

/* 桌面端：左右分栏 */
@media (min-width: 769px) {
  .ace-body.ace-split { flex-direction: row; }

  .ace-body.ace-split .ace-editor-area {
    flex: 0 0 var(--split-ratio);
    width: auto;
  }

  .ace-body.ace-split .ace-preview {
    flex: 1 1 auto;
    border-left: 1px solid var(--border);
    border-top: none;
  }
}

/* 移动端：上下堆叠（编辑器在上，预览在下） */
@media (max-width: 768px) {
  .ace-body.ace-split { flex-direction: column; }

  .ace-body.ace-split .ace-editor-area {
    flex: 0 0 var(--split-ratio);
    min-height: 120px;
  }

  .ace-body.ace-split .ace-preview {
    flex: 1 1 auto;
    min-height: 0;
    border-left: none;
    border-top: 1px solid var(--border);
  }
}

.ace-editor-area {
  display: flex;
  flex-direction: column;
  flex: 1 1 auto;
  min-height: 0;
  min-width: 0;
  position: relative;
}

.ace-editor-container {
  flex: 1 1 auto;
  min-height: 0;
  position: relative;
}

/* 分隔条：桌面垂直（左右拖），移动水平（上下拖） */
.ace-splitter {
  flex: 0 0 6px;
  background: #2d2d2d;
  cursor: col-resize;
  touch-action: none;
  position: relative;
  transition: background 0.15s;
}

.ace-splitter::after {
  content: '';
  position: absolute;
  top: 50%;
  left: 50%;
  transform: translate(-50%, -50%);
  width: 2px;
  height: 32px;
  border-radius: 2px;
  background: #4b4b4b;
}

.ace-splitter:hover,
.ace-body.ace-dragging .ace-splitter {
  background: var(--primary, #1976d2);
}

.ace-splitter-h {
  flex: 0 0 10px;
  cursor: row-resize;
}

.ace-splitter-h::after {
  width: 32px;
  height: 2px;
}

/* 预览面板 */
.ace-preview {
  flex: 1 1 auto;
  display: flex;
  flex-direction: column;
  min-height: 0;
  min-width: 0;
  overflow: hidden;
  background: #ffffff;
}

.ace-preview-head {
  flex: 0 0 auto;
  display: flex;
  align-items: center;
  flex-wrap: wrap;
  gap: 6px;
  padding: 3px 10px;
  min-height: 26px;
  background: #f7f7f8;
  border-bottom: 1px solid var(--border, #e5e7eb);
  font-size: 11px;
  user-select: none;
}

.pv-title { font-weight: 600; color: #374151; }
.pv-hint { color: #9ca3af; }

/* 当前预览的行范围（全屏预览时只读展示） */
.pv-range {
  padding: 1px 6px;
  border-radius: 3px;
  background: #eceef1;
  color: #4b5563;
  font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
  font-size: 11px;
  line-height: 16px;
  white-space: nowrap;
}

/* 行范围输入框组 */
.pv-range-edit {
  display: inline-flex;
  align-items: center;
  gap: 2px;
  color: #6b7280;
  white-space: nowrap;
}

.pv-lab { color: #9ca3af; }

.pv-input {
  width: 46px;
  height: 18px;
  padding: 0 3px;
  border: 1px solid #d8dbe0;
  border-radius: 3px;
  background: #fff;
  color: #374151;
  font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
  font-size: 11px;
  line-height: 16px;
  text-align: center;
  -moz-appearance: textfield;
  appearance: textfield;
}

.pv-input:focus {
  outline: none;
  border-color: var(--primary, #1976d2);
  box-shadow: 0 0 0 2px rgba(25, 118, 210, 0.12);
}

/* 去掉 number 输入的上下箭头，避免占宽 */
.pv-input::-webkit-outer-spin-button,
.pv-input::-webkit-inner-spin-button {
  -webkit-appearance: none;
  margin: 0;
}

.pv-total { color: #9ca3af; margin-left: 4px; }

/* 头部小按钮（同步可见范围） */
.pv-btn {
  display: inline-flex;
  align-items: center;
  gap: 3px;
  padding: 1px 6px;
  border: 1px solid #d8dbe0;
  border-radius: 3px;
  background: #fff;
  color: #4b5563;
  font-size: 11px;
  font-family: inherit;
  line-height: 16px;
  cursor: pointer;
}

.pv-btn:hover { background: #eef2f7; border-color: #c4cad2; color: #1f2937; }
.pv-btn:active { background: #e3e9f0; }
.pv-btn-icon { font-size: 11px; }

.pv-ratio {
  margin-left: auto;
  padding: 1px 6px;
  border: none;
  border-radius: 3px;
  background: transparent;
  color: #9ca3af;
  font-size: 11px;
  font-family: inherit;
  cursor: pointer;
}
.pv-ratio:hover { background: #e5e7eb; color: #4b5563; }

.ace-preview-scroll {
  flex: 1 1 auto;
  min-height: 0;
  overflow-y: auto;
  overflow-x: hidden;
  -webkit-overflow-scrolling: touch;
  padding: 20px 24px;
  overscroll-behavior: contain;
}

/* ========== 状态栏 ========== */
.ace-statusbar {
  display: flex;
  align-items: center;
  padding: 2px 10px;
  background: #252526;
  border-top: 1px solid #3c3c3c;
  font-size: 12px;
  color: #999;
  gap: 8px;
  flex-shrink: 0;
  min-height: 26px;
  flex-wrap: wrap;
}

.status-left,
.status-center,
.status-right {
  display: flex;
  align-items: center;
  gap: 4px;
}

.status-left { margin-right: auto; }
.status-center { gap: 2px; }
.status-right { margin-left: auto; gap: 8px; }

.stat-btn {
  display: inline-flex;
  align-items: center;
  height: 22px;
  padding: 0 6px;
  border: none;
  border-radius: 3px;
  background: transparent;
  color: #999;
  cursor: pointer;
  font-size: 11px;
  white-space: nowrap;
  transition: all 0.15s;
}

.stat-btn:hover {
  background: #3c3c3c;
  color: #ddd;
}

.status-fontsize {
  min-width: 28px;
  text-align: center;
  color: #aaa;
  font-size: 11px;
}

.status-cursor,
.status-select,
.status-wordcount {
  color: #888;
  font-size: 11px;
  white-space: nowrap;
}

/* ========== 响应式 ========== */
@media (max-width: 768px) {
  .ace-md-toolbar {
    padding: 3px 4px;
    gap: 1px;
    overflow-x: auto;
    -webkit-overflow-scrolling: touch;
    flex-wrap: nowrap;
    scrollbar-width: none;
  }

  .ace-md-toolbar::-webkit-scrollbar {
    display: none;
  }

  .tb-btn {
    min-width: 36px;
    height: 36px;
    font-size: 11px;
    padding: 0 4px;
    flex-shrink: 0;
  }

  .tb-btn strong,
  .tb-btn em,
  .tb-btn s {
    font-size: 13px;
  }

  .tb-sep {
    flex-shrink: 0;
  }

  .ace-preview-scroll {
    padding: 12px 12px 20px;
  }

  /* 窄屏：只保留行范围输入框，隐去冗长说明与按钮文字 */
  .pv-hint,
  .pv-total,
  .pv-btn-text {
    display: none;
  }

  .ace-preview-head {
    gap: 4px;
    padding: 3px 8px;
  }

  .pv-input { width: 40px; }

  .status-cursor,
  .status-select {
    display: none;
  }

  .ace-statusbar {
    flex-wrap: wrap;
    gap: 6px;
    padding: 6px 8px;
  }

  .status-left,
  .status-center,
  .status-right {
    width: auto;
    flex: 0 1 auto;
  }

  .status-left {
    margin-right: 0;
    order: 1;
  }

  .status-center {
    order: 2;
  }

  .status-right {
    margin-left: auto;
    order: 3;
    justify-content: flex-start;
  }

  .status-wordcount {
    font-size: 10px;
  }

  .stat-btn {
    min-width: 36px;
    height: 30px;
    font-size: 11px;
  }

  .status-fontsize {
    font-size: 11px;
    padding: 0 4px;
  }
}

@media (max-width: 480px) {
  .ace-md-toolbar {
    padding: 2px;
  }

  .tb-btn {
    min-width: 40px;
    height: 40px;
    font-size: 10px;
  }

  .ace-statusbar {
    padding: 4px 6px;
    gap: 4px;
  }

  .status-left,
  .status-center,
  .status-right {
    gap: 2px;
  }
}
</style>