<template>
  <div class="markdown-view">
    <!-- 加载骨架屏 -->
    <div v-if="loading && !content" class="mdv-loading">
      <div class="skeleton-header">
        <div class="skeleton-line w-40"></div>
        <div class="skeleton-line w-80"></div>
      </div>
      <div class="skeleton-body">
        <div v-for="i in 8" :key="i" class="skeleton-line" :class="`w-${[100,85,90,70,95,60,88,75][i-1]}`"></div>
      </div>
    </div>

    <!-- 错误状态 -->
    <div v-else-if="error" class="mdv-error">
      <p>⚠️ {{ error }}</p>
      <button class="mdv-btn" @click="load">重试</button>
    </div>

    <!-- 内容区 -->
    <template v-else>
      <!-- 正文区域 -->
      <div class="mdv-content">
        <MarkdownViewer
          :key="viewerKey"
          :content="content"
          :math-mode="mathMode"
          :load-images="loadImages"
          :show-source="showSource"
          :highlight="highlight"
          :placeholder-images="!loadImages"
          trusted
        />
      </div>

      <!-- ========== 右下角浮动区域 ========== -->
      <div class="mdv-floatbar">
        <!-- 分页控件（仅多页时显示） -->
        <div v-if="showPagination" class="float-pagination">
          <button class="float-btn" :disabled="currentPage <= 1" @click="prevPage" title="上一页">‹</button>
          <button class="float-btn page-indicator" @click="showPageJump = !showPageJump" title="点击跳转页码">
            {{ currentPage }}/{{ totalPages }}
          </button>
          <button class="float-btn" :disabled="currentPage >= totalPages" @click="nextPage" title="下一页">›</button>
        </div>

        <!-- 主按钮 -->
        <div class="float-actions">
          <button class="float-btn main-btn" @click="tocOpen = !tocOpen" title="目录">
            <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
              <line x1="3" y1="6" x2="21" y2="6"/><line x1="3" y1="12" x2="21" y2="12"/><line x1="3" y1="18" x2="21" y2="18"/>
            </svg>
          </button>
          <button class="float-btn main-btn" @click="toolbarOpen = !toolbarOpen" title="工具栏">
            <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
              <circle cx="12" cy="12" r="3"/><path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 0 1 0 2.83 2 2 0 0 1-2.83 0l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 0 1-2 2 2 2 0 0 1-2-2v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 0 1-2.83 0 2 2 0 0 1 0-2.83l.06-.06A1.65 1.65 0 0 0 4.68 15a1.65 1.65 0 0 0-1.51-1H3a2 2 0 0 1-2-2 2 2 0 0 1 2-2h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 0 1 0-2.83 2 2 0 0 1 2.83 0l.06.06A1.65 1.65 0 0 0 9 4.68a1.65 1.65 0 0 0 1-1.51V3a2 2 0 0 1 2-2 2 2 0 0 1 2 2v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 0 1 2.83 0 2 2 0 0 1 0 2.83l-.06.06a1.65 1.65 0 0 0-.33 1.82V9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 0 1 2 2 2 2 0 0 1-2 2h-.09a1.65 1.65 0 0 0-1.51 1z"/>
            </svg>
          </button>
        </div>
      </div>

      <!-- ========== 页码跳转弹窗 ========== -->
      <div v-if="showPageJump" class="page-jump-overlay" @click.self="showPageJump = false">
        <div class="page-jump-panel">
          <label>跳转到第</label>
          <input v-model.number="jumpPageInput" type="number" :min="1" :max="totalPages"
            @keyup.enter="jumpToPage" @keyup.escape="showPageJump = false" ref="jumpInput" />
          <label>页（共 {{ totalPages }} 页）</label>
          <button class="mdv-btn" @click="jumpToPage">跳转</button>
        </div>
      </div>
    </template>

    <!-- ========== 目录抽屉 ========== -->
    <Transition name="drawer-slide">
      <div v-if="tocOpen" class="toc-overlay" @click.self="tocOpen = false">
        <div class="toc-drawer">
          <div class="toc-header">
            <h3>目录</h3>
            <button class="toc-close" @click="tocOpen = false">✕</button>
          </div>
          <div class="toc-items" ref="tocListEl">
            <div
              v-for="item in toc"
              :key="item.line"
              class="toc-item"
              :class="{
                'toc-l1': item.level === 1,
                'toc-l2': item.level === 2,
                'toc-l3': item.level >= 3,
                'toc-active': activeTocLine === item.line
              }"
              @click="jumpToToc(item)"
            >
              <span class="toc-dot"></span>
              <span class="toc-title">{{ item.title }}</span>
            </div>
            <div v-if="toc.length === 0" class="toc-empty">无标题</div>
          </div>
        </div>
      </div>
    </Transition>

    <!-- ========== 工具栏 Popover ========== -->
    <Teleport to="body">
      <Transition name="popover-fade">
        <div v-if="toolbarOpen" class="toolbar-overlay" @click.self="toolbarOpen = false">
          <div class="toolbar-popover">
            <div class="toolbar-item">
              <button class="tb-action" @click="goBack">← 返回</button>
            </div>
            <div class="toolbar-item">
              <button class="tb-action" @click="triggerSearch">🔍 搜索 (Ctrl+F)</button>
            </div>
            <hr class="tb-divider" />
            <div class="toolbar-item">
              <label class="tb-label">数学渲染器</label>
              <div class="tb-radio-group">
                <label :class="{ active: mathMode === 'none' }">
                  <input v-model="mathMode" type="radio" value="none" @change="reloadViewer" /> 不渲染
                </label>
                <label :class="{ active: mathMode === 'katex' }">
                  <input v-model="mathMode" type="radio" value="katex" @change="reloadViewer" /> KaTeX
                </label>
                <label :class="{ active: mathMode === 'mathjax' }">
                  <input v-model="mathMode" type="radio" value="mathjax" @change="reloadViewer" /> MathJax
                </label>
              </div>
            </div>
            <hr class="tb-divider" />
            <div class="toolbar-item">
              <label class="tb-checkbox">
                <input v-model="showSource" type="checkbox" />
                显示源码
              </label>
            </div>
            <div class="toolbar-item">
              <label class="tb-checkbox">
                <input v-model="loadImages" type="checkbox" @change="onLoadImagesChange" />
                加载全部图片
              </label>
            </div>
          </div>
        </div>
      </Transition>
    </Teleport>
  </div>
</template>

<script setup lang="ts">
/**
 * MarkdownView — 沉浸式 Markdown 阅读器
 *
 * 特性：
 * - 全屏宽阅读，无顶部工具栏
 * - 右下角浮动：翻页 + 目录 + 工具栏
 * - 目录由后端 API 提供，支持跨页跳转
 * - 图片默认占位符，点击加载单张
 * - 前端分页计算（后端只做行切片）
 */
import { ref, computed, onMounted, watch, nextTick, defineAsyncComponent } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import { getMarkdown, getTOC } from '@/api/documents'
import { useToast } from '@/composables/useToast'
import { useRequireToken } from '@/composables/useRequireToken'
import type { TocItem, MarkdownResponse } from '@/types/document'

// 懒加载 MarkdownViewer
const MarkdownViewer = defineAsyncComponent(() =>
  import('@/components/MarkdownViewer.vue')
)

const LINES_PER_PAGE = 5000

const route = useRoute()
const router = useRouter()
const { showError } = useToast()
const { requireToken } = useRequireToken()

// ── 内容状态 ──
const content = ref('')
const loading = ref(false)
const error = ref('')
const highlight = ref('')
const viewerKey = ref(0)

// ── 元数据 ──
const totalLength = ref(0)
const totalLines = ref(0)
const currentPage = ref(1)
const linesReturned = ref(0)

// ── 计算属性 ──
const totalPages = computed(() => Math.max(1, Math.ceil(totalLines.value / LINES_PER_PAGE)))
const showPagination = computed(() => totalPages.value > 1)

// ── 视图设置 ──
const mathMode = ref<'none' | 'katex' | 'mathjax'>('katex')
const loadImages = ref(false)
const showSource = ref(false)

// ── 浮动 UI ──
const tocOpen = ref(false)
const toolbarOpen = ref(false)
const showPageJump = ref(false)
const jumpPageInput = ref(1)
const jumpInput = ref<HTMLInputElement | null>(null)
const tocListEl = ref<HTMLElement | null>(null)

// ── 目录 ──
const toc = ref<TocItem[]>([])
const activeTocLine = ref<number | null>(null)

// ===== 工具函数 =====

/** 加载某页内容 */
async function loadPage(page: number) {
  const id = parseInt(route.params.id as string)
  loading.value = true
  error.value = ''
  try {
    const res = await getMarkdown(id, {
      line_start: (page - 1) * LINES_PER_PAGE,
      line_count: LINES_PER_PAGE
    })
    applyContent(res)
    currentPage.value = page
  } catch (err: any) {
    error.value = err?.message || '加载失败'
    showError(error.value)
  } finally {
    loading.value = false
  }
}

/** 应用 API 响应到状态 */
function applyContent(res: MarkdownResponse) {
  content.value = res.content
  totalLength.value = res.total_length
  totalLines.value = res.total_lines
  linesReturned.value = res.lines_returned
}

/** 首次加载（第 1 页 + 目录） */
async function load() {
  const id = parseInt(route.params.id as string)
  if (!requireToken()) return

  loading.value = true
  error.value = ''
  currentPage.value = 1

  try {
    const [res, tocRes] = await Promise.all([
      getMarkdown(id, { line_start: 0, line_count: LINES_PER_PAGE }),
      getTOC(id).catch(() => null) // TOC 失败不阻塞正文
    ])
    applyContent(res)
    if (tocRes) {
      toc.value = tocRes.toc
    }
    await nextTick()
  } catch (err: any) {
    error.value = err?.message || '加载失败'
    showError(error.value)
  } finally {
    loading.value = false
  }
}

// ===== 分页操作 =====

function nextPage() {
  if (currentPage.value < totalPages.value) {
    loadPage(currentPage.value + 1)
  }
}

function prevPage() {
  if (currentPage.value > 1) {
    loadPage(currentPage.value - 1)
  }
}

function jumpToPage() {
  const p = Math.max(1, Math.min(totalPages.value, jumpPageInput.value || 1))
  if (p !== currentPage.value) {
    loadPage(p)
  }
  showPageJump.value = false
}

// ===== 目录跳转 =====

async function jumpToToc(item: TocItem) {
  const targetPage = Math.floor(item.line / LINES_PER_PAGE) + 1
  const curLineStart = (currentPage.value - 1) * LINES_PER_PAGE
  const curLineEnd = curLineStart + linesReturned.value

  if (item.line >= curLineStart && item.line < curLineEnd) {
    // 同页：直接滚动
    scrollToHeading(item.title)
  } else {
    // 跨页：加载目标页后再滚动
    await loadPage(targetPage)
    await nextTick()
    scrollToHeading(item.title)
  }
  tocOpen.value = false
}

function scrollToHeading(title: string) {
  const viewer = document.querySelector('.markdown-viewer')
  if (!viewer) return
  const headings = viewer.querySelectorAll('h1, h2, h3, h4, h5, h6')
  for (const h of headings) {
    if (h.textContent?.trim() === title) {
      h.scrollIntoView({ behavior: 'smooth', block: 'start' })
      activeTocLine.value = title // 简化为标题文字作为标识
      return
    }
  }
}

// ===== 工具栏操作 =====

function goBack() {
  router.back()
}

function triggerSearch() {
  // 浏览器原生查找
  if (window.find) {
    // 大多数浏览器
    window.find('')
  }
  // 备用：dispatch Ctrl+F
  document.dispatchEvent(new KeyboardEvent('keydown', {
    key: 'f',
    ctrlKey: true,
    metaKey: true,
    bubbles: true
  }))
  toolbarOpen.value = false
}

function reloadViewer() {
  viewerKey.value++
  // 关闭工具栏
  toolbarOpen.value = false
}

function onLoadImagesChange() {
  // 切换后重新渲染 MarkdownViewer
  viewerKey.value++
  toolbarOpen.value = false
}

// ===== 生命周期 =====

watch(() => route.params.id, () => {
  currentPage.value = 1
  viewerKey.value++
  error.value = ''
  load()
})

watch(() => route.query.highlight, (val) => {
  highlight.value = (val as string) || ''
}, { immediate: true })

onMounted(load)
</script>

<style scoped>
/* ==========================================
   MarkdownView 全屏沉浸阅读器
   ========================================== */

.markdown-view {
  display: flex;
  flex-direction: column;
  min-height: calc(100vh - 60px);
  position: relative;
  background: #fff;
}

/* ── 内容区 ── */
.mdv-content {
  flex: 1;
  padding: 40px 48px;
  max-width: 900px;
  margin: 0 auto;
  width: 100%;
  box-sizing: border-box;
  line-height: 1.8;
}

.mdv-content :deep(img) {
  max-width: 100%;
  height: auto;
  border-radius: 4px;
}

.mdv-content :deep(pre) {
  background: #f5f5f5;
  padding: 16px;
  border-radius: 8px;
  overflow-x: auto;
  font-size: 14px;
  line-height: 1.6;
}

/* ── 右下角浮动栏 ── */
.mdv-floatbar {
  position: fixed;
  bottom: 24px;
  right: 24px;
  display: flex;
  align-items: center;
  gap: 8px;
  z-index: 100;
}

.float-pagination {
  display: flex;
  align-items: center;
  gap: 4px;
  background: rgba(255,255,255,0.95);
  backdrop-filter: blur(8px);
  border: 1px solid var(--border);
  border-radius: 12px;
  padding: 4px;
  box-shadow: 0 2px 12px rgba(0,0,0,0.1);
}

.float-actions {
  display: flex;
  gap: 4px;
  background: rgba(255,255,255,0.95);
  backdrop-filter: blur(8px);
  border: 1px solid var(--border);
  border-radius: 12px;
  padding: 4px;
  box-shadow: 0 2px 12px rgba(0,0,0,0.1);
}

.float-btn {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  min-width: 36px;
  height: 36px;
  padding: 0 8px;
  border: none;
  border-radius: 8px;
  background: transparent;
  color: var(--text-secondary);
  cursor: pointer;
  font-size: 14px;
  font-family: inherit;
  transition: all 0.15s;
  user-select: none;
}

.float-btn:hover:not(:disabled) {
  background: var(--bg-secondary);
  color: var(--text);
}

.float-btn:active:not(:disabled) {
  transform: scale(0.95);
}

.float-btn:disabled {
  opacity: 0.3;
  cursor: not-allowed;
}

.float-btn.main-btn {
  min-width: 38px;
  height: 38px;
}

.page-indicator {
  font-size: 13px;
  font-weight: 600;
  min-width: 44px;
  padding: 0 6px;
  color: var(--primary);
  cursor: pointer;
}

/* ── 页码跳转弹窗 ── */
.page-jump-overlay {
  position: fixed;
  inset: 0;
  background: rgba(0,0,0,0.3);
  display: flex;
  align-items: center;
  justify-content: center;
  z-index: 200;
}

.page-jump-panel {
  background: #fff;
  border: 1px solid var(--border);
  border-radius: 12px;
  padding: 20px 24px;
  box-shadow: 0 4px 24px rgba(0,0,0,0.15);
  display: flex;
  align-items: center;
  gap: 8px;
  font-size: 14px;
}

.page-jump-panel input {
  width: 70px;
  padding: 6px 8px;
  border: 1px solid var(--border);
  border-radius: 6px;
  font-size: 14px;
  text-align: center;
}

.mdv-btn {
  padding: 6px 16px;
  border: 1px solid var(--border);
  border-radius: 6px;
  background: var(--primary);
  color: #fff;
  cursor: pointer;
  font-size: 14px;
  transition: opacity 0.15s;
}

.mdv-btn:hover {
  opacity: 0.9;
}

/* ── 骨架屏 ── */
.mdv-loading {
  padding: 60px 48px;
  max-width: 900px;
  margin: 0 auto;
  width: 100%;
  box-sizing: border-box;
}

.skeleton-header {
  margin-bottom: 32px;
}

.skeleton-body {
  display: flex;
  flex-direction: column;
  gap: 14px;
}

.skeleton-line {
  height: 16px;
  background: linear-gradient(90deg, #eee 25%, #f5f5f5 50%, #eee 75%);
  background-size: 200% 100%;
  animation: skeleton-shimmer 1.5s infinite;
  border-radius: 4px;
}

.skeleton-line.w-40 { width: 40%; }
.skeleton-line.w-60 { width: 60%; }
.skeleton-line.w-70 { width: 70%; }
.skeleton-line.w-75 { width: 75%; }
.skeleton-line.w-80 { width: 80%; }
.skeleton-line.w-85 { width: 85%; }
.skeleton-line.w-88 { width: 88%; }
.skeleton-line.w-90 { width: 90%; }
.skeleton-line.w-95 { width: 95%; }
.skeleton-line.w-100 { width: 100%; }

@keyframes skeleton-shimmer {
  0% { background-position: 200% 0; }
  100% { background-position: -200% 0; }
}

/* ── 错误状态 ── */
.mdv-error {
  text-align: center;
  padding: 80px 20px;
  color: var(--text-secondary);
}

.mdv-error p {
  margin-bottom: 16px;
  font-size: 16px;
}

/* ==========================================
   目录抽屉（右侧滑入）
   ========================================== */

.toc-overlay {
  position: fixed;
  inset: 0;
  background: rgba(0,0,0,0.3);
  z-index: 500;
  display: flex;
  justify-content: flex-end;
}

.toc-drawer {
  width: 320px;
  max-width: 85vw;
  height: 100%;
  background: #fff;
  box-shadow: -4px 0 24px rgba(0,0,0,0.12);
  display: flex;
  flex-direction: column;
  overflow: hidden;
}

.toc-header {
  display: flex;
  justify-content: space-between;
  align-items: center;
  padding: 20px 20px 12px;
  border-bottom: 1px solid var(--border);
  flex-shrink: 0;
}

.toc-header h3 {
  margin: 0;
  font-size: 16px;
  font-weight: 600;
}

.toc-close {
  background: none;
  border: none;
  font-size: 20px;
  cursor: pointer;
  color: var(--text-secondary);
  padding: 4px 8px;
  border-radius: 4px;
}

.toc-close:hover {
  background: var(--bg-secondary);
}

.toc-items {
  flex: 1;
  overflow-y: auto;
  padding: 8px 0;
}

.toc-item {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 8px 20px;
  cursor: pointer;
  transition: background 0.12s;
  border-left: 3px solid transparent;
  font-size: 14px;
  line-height: 1.4;
  color: var(--text-secondary);
}

.toc-item:hover {
  background: var(--bg-secondary);
  color: var(--text);
}

.toc-item.toc-active {
  border-left-color: var(--primary);
  background: #f0f4ff;
  color: var(--primary);
  font-weight: 500;
}

.toc-item.toc-l1 {
  padding-left: 20px;
  font-weight: 500;
}

.toc-item.toc-l2 {
  padding-left: 36px;
  font-size: 13px;
}

.toc-item.toc-l3 {
  padding-left: 52px;
  font-size: 13px;
  color: var(--text-tertiary);
}

.toc-dot {
  width: 4px;
  height: 4px;
  border-radius: 50%;
  background: currentColor;
  flex-shrink: 0;
}

.toc-title {
  flex: 1;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.toc-empty {
  padding: 40px 20px;
  text-align: center;
  color: var(--text-tertiary);
  font-size: 14px;
}

/* 抽屉动画 */
.drawer-slide-enter-active,
.drawer-slide-leave-active {
  transition: all 0.25s ease;
}

.drawer-slide-enter-from,
.drawer-slide-leave-to {
  opacity: 0;
}

.drawer-slide-enter-from .toc-drawer,
.drawer-slide-leave-to .toc-drawer {
  transform: translateX(100%);
}

/* ==========================================
   工具栏 Popover
   ========================================== */

.toolbar-overlay {
  position: fixed;
  inset: 0;
  background: rgba(0,0,0,0.2);
  z-index: 500;
  display: flex;
  justify-content: flex-end;
  align-items: flex-end;
  padding: 24px;
}

.toolbar-popover {
  background: #fff;
  border: 1px solid var(--border);
  border-radius: 12px;
  box-shadow: 0 4px 24px rgba(0,0,0,0.12);
  padding: 12px;
  min-width: 220px;
}

.toolbar-item {
  padding: 4px 0;
}

.tb-action {
  display: block;
  width: 100%;
  padding: 8px 12px;
  border: none;
  border-radius: 8px;
  background: transparent;
  cursor: pointer;
  font-size: 14px;
  text-align: left;
  color: var(--text);
  transition: background 0.12s;
  font-family: inherit;
}

.tb-action:hover {
  background: var(--bg-secondary);
}

.tb-divider {
  border: none;
  border-top: 1px solid var(--border);
  margin: 6px 0;
}

.tb-label {
  display: block;
  font-size: 12px;
  font-weight: 600;
  color: var(--text-tertiary);
  text-transform: uppercase;
  letter-spacing: 0.5px;
  margin-bottom: 6px;
  padding: 0 12px;
}

.tb-radio-group {
  display: flex;
  flex-direction: column;
  gap: 2px;
  padding: 0 12px;
}

.tb-radio-group label {
  display: flex;
  align-items: center;
  gap: 6px;
  font-size: 14px;
  padding: 6px 8px;
  border-radius: 6px;
  cursor: pointer;
  color: var(--text-secondary);
  transition: all 0.12s;
}

.tb-radio-group label:hover {
  background: var(--bg-secondary);
  color: var(--text);
}

.tb-radio-group label.active {
  color: var(--primary);
  font-weight: 500;
}

.tb-radio-group input[type="radio"] {
  accent-color: var(--primary);
}

.tb-checkbox {
  display: flex;
  align-items: center;
  gap: 8px;
  font-size: 14px;
  padding: 6px 12px;
  border-radius: 6px;
  cursor: pointer;
  color: var(--text-secondary);
  transition: all 0.12s;
}

.tb-checkbox:hover {
  background: var(--bg-secondary);
  color: var(--text);
}

.tb-checkbox input[type="checkbox"] {
  accent-color: var(--primary);
}

/* Popover 动画 */
.popover-fade-enter-active,
.popover-fade-leave-active {
  transition: opacity 0.15s;
}

.popover-fade-enter-from,
.popover-fade-leave-to {
  opacity: 0;
}

/* ==========================================
   响应式
   ========================================== */

@media (max-width: 768px) {
  .mdv-content {
    padding: 20px 20px 100px;
  }

  .mdv-floatbar {
    bottom: 16px;
    right: 16px;
    gap: 6px;
  }

  .float-btn {
    min-width: 40px;
    height: 40px;
    font-size: 14px;
  }

  .float-btn.main-btn {
    min-width: 42px;
    height: 42px;
  }

  .page-indicator {
    font-size: 13px;
    min-width: 48px;
  }

  .toc-drawer {
    width: 280px;
  }

  .toolbar-overlay {
    padding: 12px;
  }

  .toolbar-popover {
    min-width: 200px;
  }
}

@media (max-width: 480px) {
  .mdv-content {
    padding: 16px 14px 90px;
  }

  .mdv-floatbar {
    bottom: 12px;
    right: 12px;
  }

  .float-btn {
    min-width: 44px;
    height: 44px;
  }
}
</style>