<template>
  <div class="ace-editor-wrapper" :class="{ 'ace-fullscreen': fullscreen }">
    <!-- Markdown 快捷插入工具栏 -->
    <div class="ace-md-toolbar">
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
      <button class="tb-btn" @click="insertMarkdown('table')" title="表格">▦</button>
      <span class="tb-sep"></span>
      <button class="tb-btn" @click="insertMarkdown('hr')" title="分割线">—</button>
    </div>

    <!-- 主编辑区：分屏模式 -->
    <div class="ace-body" :class="{ 'ace-split': splitView }">
      <div class="ace-editor-area" :class="{ 'ace-side-by-side': splitView }">
        <div ref="editorEl" class="ace-editor-container"></div>
      </div>
      <div v-if="preview || splitView" class="ace-preview" :class="{ 'ace-side-by-side': splitView }">
        <MarkdownViewer :content="splitView ? visibleContent : contentRef" />
      </div>
    </div>

    <!-- 底部状态栏 -->
    <div class="ace-statusbar">
      <div class="status-left">
        <button class="stat-btn" @click="search" title="搜索 Ctrl+F">🔍 搜索</button>
        <button class="stat-btn" @click="replace" title="替换 Ctrl+H">🔧 替换</button>
        <span class="tb-sep"></span>
        <button class="stat-btn" @click="toggleSplit" :title="splitView ? '单栏编辑' : '分屏预览'">
          {{ splitView ? '☰ 编辑' : '⬜ 分屏' }}
        </button>
        <button class="stat-btn" @click="preview = !preview" :title="preview ? '关闭预览' : '全屏预览'">
          {{ preview ? '✕ 预览' : '👁 预览' }}
        </button>
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
        <button class="stat-btn" @click="toggleFullscreen" title="全屏">
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

// ===== 静态预加载 Ace 核心模块（theme/mode 无自引用，静态 import 安全） =====
import 'ace-builds/src-noconflict/theme-monokai'
import 'ace-builds/src-noconflict/mode-markdown'



const props = defineProps<{
  modelValue: string
  mode?: string
  theme?: string
  fontSize?: number
  showToolbar?: boolean
}>()

const emit = defineEmits(['update:modelValue'])

const editorEl = ref<HTMLElement | null>(null)
let editor: Ace.Editor | null = null

// --- 状态 ---
const preview = ref(false)
const splitView = ref(false)
const fullscreen = ref(false)
const currentFontSize = ref(props.fontSize || 16)
const cursorRow = ref(1)
const cursorCol = ref(1)
const selectedLen = ref(0)

// 视口可见行范围（用于分屏同步预览）
const visibleFirstRow = ref(0)
const visibleLastRow = ref(0)

// 用于预览的内容引用
const contentRef = ref(props.modelValue)

const wordCount = computed(() => {
  const text = props.modelValue || ''
  const chineseChars = (text.match(/[一-龥]/g) || []).length
  const otherChars = text.replace(/[一-龥]/g, '').length
  return chineseChars + otherChars
})

/** 分屏模式下：仅取编辑器视口可见的文字行 */
const visibleContent = computed(() => {
  const lines = contentRef.value.split('\n')
  const start = visibleFirstRow.value
  const end = Math.min(visibleLastRow.value + 1, lines.length)
  return lines.slice(start, end).join('\n')
})

// --- 初始化编辑器 ---
function initEditor() {
  if (!editorEl.value) return

  // basePath：开发模式用本地 node_modules；生产用 CDN 兜住 extension 的动态请求
  // 不设 packaged，让 language_tools 的合法动态加载能走 basePath
  ace.config.set('basePath', import.meta.env.DEV
    ? '/@fs/node_modules/ace-builds/src-noconflict'
    : 'https://cdn.jsdelivr.net/npm/ace-builds@1.44.0/src-noconflict'
  )
  if (import.meta.env.DEV) {
    ace.config.set('workerPath', '/@fs/node_modules/ace-builds/src-noconflict')
  }

  editor = ace.edit(editorEl.value, {
    mode: `ace/mode/${props.mode || 'markdown'}`,
    theme: `ace/theme/${props.theme || 'monokai'}`,
    fontSize: currentFontSize.value,
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

  // 视口范围初始同步 + 滚动时更新
  updateVisibleRange()
  editor.session.on('scroll', updateVisibleRange)

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
  cursorRow.value = pos.row + 1
  cursorCol.value = pos.column + 1
  const sel = editor.selection.getRange()
  selectedLen.value = Math.abs(
    editor.session.getTextRange(sel).length
  )
}

/** 同步视口行到 visibleContent，分屏预览只渲染可见部分 */
function updateVisibleRange() {
  if (!editor) return
  const firstVis = editor.getFirstVisibleRow()
  const lastVis = editor.getLastVisibleRow()
  // 转换 visual row → document (logical) row (处理自动换行)
  const firstDoc = editor.session.screenToDocumentRow(firstVis, 0)
  const lastDoc = editor.session.screenToDocumentRow(lastVis, 0)
  visibleFirstRow.value = firstDoc.row
  visibleLastRow.value = lastDoc.row
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
function toggleSplit() {
  splitView.value = !splitView.value
  preview.value = false
  if (editor) nextTick(() => { editor!.resize(); updateVisibleRange() })
}

function togglePreview() {
  preview.value = !preview.value
  splitView.value = false
  if (editor) nextTick(() => { editor!.resize(); updateVisibleRange() })
}

function toggleFullscreen() {
  fullscreen.value = !fullscreen.value
  if (editor) nextTick(() => { editor!.resize(); updateVisibleRange() })
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

// --- 生命周期 ---
onMounted(() => {
  initEditor()
})

onBeforeUnmount(() => {
  if (editor) {
    editor.commands.removeCommands(['bold', 'italic'])
    editor.destroy()
    editor = null
  }
})

// --- 外部响应 ---
watch(() => props.modelValue, (newVal) => {
  if (editor && !editor.isFocused() && newVal !== editor.getValue()) {
    editor.setValue(newVal, -1)
    contentRef.value = newVal
  }
})

watch(() => props.theme, (newTheme) => {
  if (editor) editor.setTheme(`ace/theme/${newTheme}`)
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

/* ========== 主体编辑区 ========== */
.ace-body {
  flex: 1;
  display: flex;
  flex-direction: column;
  overflow: hidden;
}

.ace-body.ace-split {
  flex-direction: row;
}

.ace-editor-area {
  flex: 1;
  display: flex;
  flex-direction: column;
  min-height: 200px;
  position: relative;
}

.ace-editor-area.ace-side-by-side {
  width: 50%;
  flex: none;
}

.ace-editor-container {
  flex: 1;
  position: relative;
}

.ace-preview {
  flex: 1;
  min-height: 200px;
  padding: 20px;
  overflow-y: auto;
  background: #ffffff;
  border-left: 1px solid var(--border);
  border-top: 1px solid var(--border);
}

.ace-preview.ace-side-by-side {
  width: 50%;
  flex: none;
  border-top: none;
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
  }

  .tb-btn {
    min-width: 24px;
    height: 24px;
    font-size: 11px;
    padding: 0 4px;
  }

  .ace-body.ace-split {
    flex-direction: column;
  }

  .ace-editor-area.ace-side-by-side,
  .ace-preview.ace-side-by-side {
    width: 100%;
    height: 50%;
  }

  .status-cursor,
  .status-select {
    display: none;
  }
}
</style>