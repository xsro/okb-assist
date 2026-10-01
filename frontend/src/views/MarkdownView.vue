<template>
  <div class="markdown-view">
    <!-- 加载骨架屏 -->
    <div v-if="loading && allChunks.length === 0" class="mdv-loading">
      <div class="skeleton-header">
        <div class="skeleton-line w-40"></div>
        <div class="skeleton-line w-80"></div>
      </div>
      <div class="skeleton-body">
        <div v-for="i in 8" :key="i" class="skeleton-line skeleton-w"></div>
      </div>
    </div>

    <!-- 错误状态 -->
    <div v-else-if="error" class="mdv-error">
      <p>⚠️ {{ error }}</p>
      <button class="mdv-btn" @click="load">重试</button>
    </div>

    <!-- 内容区 -->
    <template v-else>
      <div class="mdv-content">
        <MarkdownViewer
          :key="viewerKey"
          :chunks="visibleChunks"
          :math-mode="mathMode"
          :load-images="loadImages"
          :show-source="showSource"
          :highlight="highlight"
          :placeholder-images="!loadImages"
          trusted
        />
        <!-- 底部哨兵元素（用于 IntersectionObserver） -->
        <div ref="sentinelEl" class="mdv-sentinel">
          <div v-if="loadingMore" class="mdv-loading-more">
            <span class="loading-spinner"></span>
            <span>加载更多...</span>
          </div>
          <div v-else-if="allLoaded" class="mdv-end">
            — 已加载全部内容（共 {{ totalChunks }} 段，{{ totalLines }} 行） —
          </div>
        </div>
      </div>

      <!-- 右下角浮动区域 -->
      <div class="mdv-floatbar">
        <!-- 进度信息（替代分页） -->
        <div class="float-progress" @click="tocOpen = !tocOpen" title="点击打开目录">
          <span class="progress-text">{{ visibleChunks.length }}/{{ allChunks.length }} 段</span>
          <span class="progress-bar-track">
            <span class="progress-bar-fill" :style="{ width: progressPercent + '%' }"></span>
          </span>
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
    </template>

    <!-- 目录抽屉 -->
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

    <!-- 工具栏 Popover -->
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
 * MarkdownView — 沉浸式 Markdown 阅读器（滚动增量加载版）
 *
 * 与旧版区别：
 * - 去掉前端行数分页，改为后端按 heading 边界分块
 * - 初始加载 2 个 chunk，滚动到底部自动加载更多
 * - 每个 chunk 通过 MarkdownViewer 独立渲染并追加
 * - 公式通过 requestIdleCallback 异步渲染
 */
import { ref, computed, onMounted, onBeforeUnmount, watch, nextTick, defineAsyncComponent } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import { getChunks, getTOC } from '@/api/documents'
import { useToast } from '@/composables/useToast'
import { useRequireToken } from '@/composables/useRequireToken'
import type { TocItem, ChunkInfo } from '@/types/document'

const MarkdownViewer = defineAsyncComponent(() =>
  import('@/components/MarkdownViewer.vue')
)

const INITIAL_CHUNKS = 2  // 初始加载前 2 个 chunk

const route = useRoute()
const router = useRouter()
const { showError } = useToast()
const { requireToken } = useRequireToken()

// ── 状态 ──
const allChunks = ref<ChunkInfo[]>([])
const visibleCount = ref(INITIAL_CHUNKS)
const loading = ref(false)
const loadingMore = ref(false)
const error = ref('')
const highlight = ref('')
const viewerKey = ref(0)

// ── 元数据 ──
const totalLines = ref(0)

// ── 计算 ──
const visibleChunks = computed(() => allChunks.value.slice(0, visibleCount.value))
const totalChunks = computed(() => allChunks.value.length)
const allLoaded = computed(() => visibleCount.value >= allChunks.value.length)
const progressPercent = computed(() =>
  totalChunks.value > 0 ? Math.round((visibleCount.value / totalChunks.value) * 100) : 0
)

// ── 视图设置 ──
const mathMode = ref<'none' | 'katex' | 'mathjax'>('katex')
const loadImages = ref(false)
const showSource = ref(false)

// ── 浮动 UI ──
const tocOpen = ref(false)
const toolbarOpen = ref(false)
const sentinelEl = ref<HTMLElement | null>(null)
const tocListEl = ref<HTMLElement | null>(null)

// ── 目录 ──
const toc = ref<TocItem[]>([])
const activeTocLine = ref<number | null>(null)

// ── IntersectionObserver ──
let observer: IntersectionObserver | null = null

function setupObserver() {
  if (observer) observer.disconnect()
  if (!sentinelEl.value) return

  observer = new IntersectionObserver(
    (entries) => {
      for (const entry of entries) {
        if (entry.isIntersecting && !allLoaded.value && !loadingMore.value) {
          loadMoreChunks()
        }
      }
    },
    { rootMargin: '400px' } // 提前 400px 触发加载
  )
  observer.observe(sentinelEl.value)
}

async function loadMoreChunks() {
  if (loadingMore.value || allLoaded.value) return
  loadingMore.value = true
  // 每次加载 3 个 chunk
  const nextCount = Math.min(visibleCount.value + 3, allChunks.value.length)
  visibleCount.value = nextCount
  loadingMore.value = false
  // 调整 observer（sentinel 可能因 DOM 变化需要重新绑定）
  await nextTick()
  setupObserver()
}

// ===== 加载 =====

async function load() {
  const id = parseInt(route.params.id as string)
  if (!requireToken()) return

  loading.value = true
  error.value = ''
  highlight.value = (route.query.highlight as string) || ''

  try {
    const [chunkRes, tocRes] = await Promise.all([
      getChunks(id),
      getTOC(id).catch(() => null)
    ])

    allChunks.value = chunkRes.chunks
    totalLines.value = chunkRes.total_lines
    visibleCount.value = Math.min(INITIAL_CHUNKS, chunkRes.chunks.length)

    if (tocRes) {
      toc.value = tocRes.toc
    }

    await nextTick()
    setupObserver()
  } catch (err: any) {
    error.value = err?.message || '加载失败'
    showError(error.value)
  } finally {
    loading.value = false
  }
}

// ===== 目录跳转 =====

function jumpToToc(item: TocItem) {
  // 找到 item.line 所在的 chunk
  const chunkIdx = allChunks.value.findIndex(c =>
    item.line >= c.start_line && item.line <= c.end_line
  )
  if (chunkIdx >= 0) {
    // 确保该 chunk 已加载
    if (chunkIdx + 1 > visibleCount.value) {
      visibleCount.value = Math.min(chunkIdx + 3, allChunks.value.length)
      nextTick(() => {
        scrollToHeading(item.title)
      })
    } else {
      scrollToHeading(item.title)
    }
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
      activeTocLine.value = title
      return
    }
  }
}

// ===== 工具栏操作 =====

function goBack() {
  router.back()
}

function triggerSearch() {
  if (window.find) {
    window.find('')
  }
  document.dispatchEvent(new KeyboardEvent('keydown', {
    key: 'f', ctrlKey: true, metaKey: true, bubbles: true
  }))
  toolbarOpen.value = false
}

function reloadViewer() {
  viewerKey.value++
  toolbarOpen.value = false
}

function onLoadImagesChange() {
  viewerKey.value++
  toolbarOpen.value = false
}

// ===== 监听路由变化 =====

watch(() => route.params.id, () => {
  allChunks.value = []
  visibleCount.value = INITIAL_CHUNKS
  viewerKey.value++
  error.value = ''
  load()
})

// ===== 生命周期 =====

onMounted(load)

onBeforeUnmount(() => {
  if (observer) observer.disconnect()
})
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
  padding: 40px 48px 120px;
  max-width: 900px;
  margin: 0 auto;
  width: 100%;
  box-sizing: border-box;
  line-height: 1.8;
}

/* ── 底部哨兵/加载提示 ── */
.mdv-sentinel {
  min-height: 1px;
  padding: 20px 0;
  text-align: center;
}

.mdv-loading-more {
  display: flex;
  align-items: center;
  justify-content: center;
  gap: 8px;
  color: var(--text-tertiary);
  font-size: 14px;
}

.loading-spinner {
  display: inline-block;
  width: 16px;
  height: 16px;
  border: 2px solid var(--border);
  border-top-color: var(--primary);
  border-radius: 50%;
  animation: spin 0.8s linear infinite;
}

@keyframes spin {
  to { transform: rotate(360deg); }
}

.mdv-end {
  color: var(--text-tertiary);
  font-size: 13px;
  padding: 10px;
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

.float-progress {
  display: flex;
  align-items: center;
  gap: 6px;
  background: rgba(255,255,255,0.95);
  backdrop-filter: blur(8px);
  border: 1px solid var(--border);
  border-radius: 12px;
  padding: 6px 12px;
  box-shadow: 0 2px 12px rgba(0,0,0,0.1);
  cursor: pointer;
  transition: background 0.15s;
  user-select: none;
}

.float-progress:hover {
  background: rgba(245,245,245,0.95);
}

.progress-text {
  font-size: 12px;
  font-weight: 600;
  color: var(--text-secondary);
  white-space: nowrap;
}

.progress-bar-track {
  display: inline-block;
  width: 50px;
  height: 4px;
  background: var(--border);
  border-radius: 2px;
  overflow: hidden;
}

.progress-bar-fill {
  display: block;
  height: 100%;
  background: var(--primary);
  border-radius: 2px;
  transition: width 0.3s ease;
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

.float-btn:hover { background: var(--bg-secondary); color: var(--text); }
.float-btn:active { transform: scale(0.95); }

.float-btn.main-btn { min-width: 38px; height: 38px; }

/* ── 骨架屏 ── */
.mdv-loading {
  padding: 60px 48px;
  max-width: 900px;
  margin: 0 auto;
  width: 100%;
  box-sizing: border-box;
}

.skeleton-header { margin-bottom: 32px; }

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

.skeleton-w { width: 100%; }
.skeleton-w:nth-child(2n) { width: 85%; }
.skeleton-w:nth-child(3n) { width: 70%; }
.skeleton-w:nth-child(5n) { width: 90%; }

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
.mdv-error p { margin-bottom: 16px; font-size: 16px; }

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
.mdv-btn:hover { opacity: 0.9; }

/* ==========================================
   目录抽屉
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

.toc-header h3 { margin: 0; font-size: 16px; font-weight: 600; }

.toc-close {
  background: none; border: none; font-size: 20px; cursor: pointer;
  color: var(--text-secondary); padding: 4px 8px; border-radius: 4px;
}
.toc-close:hover { background: var(--bg-secondary); }

.toc-items { flex: 1; overflow-y: auto; padding: 8px 0; }

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

.toc-item:hover { background: var(--bg-secondary); color: var(--text); }
.toc-item.toc-active { border-left-color: var(--primary); background: #f0f4ff; color: var(--primary); font-weight: 500; }
.toc-item.toc-l1 { padding-left: 20px; font-weight: 500; }
.toc-item.toc-l2 { padding-left: 36px; font-size: 13px; }
.toc-item.toc-l3 { padding-left: 52px; font-size: 13px; color: var(--text-tertiary); }

.toc-dot {
  width: 4px; height: 4px; border-radius: 50%; background: currentColor; flex-shrink: 0;
}

.toc-title {
  flex: 1; overflow: hidden; text-overflow: ellipsis; white-space: nowrap;
}

.toc-empty { padding: 40px 20px; text-align: center; color: var(--text-tertiary); font-size: 14px; }

.drawer-slide-enter-active, .drawer-slide-leave-active { transition: all 0.25s ease; }
.drawer-slide-enter-from, .drawer-slide-leave-to { opacity: 0; }
.drawer-slide-enter-from .toc-drawer, .drawer-slide-leave-to .toc-drawer { transform: translateX(100%); }

/* ==========================================
   工具栏 Popover
   ========================================== */

.toolbar-overlay {
  position: fixed; inset: 0; background: rgba(0,0,0,0.2); z-index: 500;
  display: flex; justify-content: flex-end; align-items: flex-end; padding: 24px;
}

.toolbar-popover {
  background: #fff; border: 1px solid var(--border); border-radius: 12px;
  box-shadow: 0 4px 24px rgba(0,0,0,0.12); padding: 12px; min-width: 220px;
}

.toolbar-item { padding: 4px 0; }

.tb-action {
  display: block; width: 100%; padding: 8px 12px; border: none; border-radius: 8px;
  background: transparent; cursor: pointer; font-size: 14px; text-align: left;
  color: var(--text); transition: background 0.12s; font-family: inherit;
}
.tb-action:hover { background: var(--bg-secondary); }

.tb-divider { border: none; border-top: 1px solid var(--border); margin: 6px 0; }

.tb-label {
  display: block; font-size: 12px; font-weight: 600; color: var(--text-tertiary);
  text-transform: uppercase; letter-spacing: 0.5px; margin-bottom: 6px; padding: 0 12px;
}

.tb-radio-group {
  display: flex; flex-direction: column; gap: 2px; padding: 0 12px;
}

.tb-radio-group label {
  display: flex; align-items: center; gap: 6px; font-size: 14px; padding: 6px 8px;
  border-radius: 6px; cursor: pointer; color: var(--text-secondary); transition: all 0.12s;
}
.tb-radio-group label:hover { background: var(--bg-secondary); color: var(--text); }
.tb-radio-group label.active { color: var(--primary); font-weight: 500; }
.tb-radio-group input[type="radio"] { accent-color: var(--primary); }

.tb-checkbox {
  display: flex; align-items: center; gap: 8px; font-size: 14px; padding: 6px 12px;
  border-radius: 6px; cursor: pointer; color: var(--text-secondary); transition: all 0.12s;
}
.tb-checkbox:hover { background: var(--bg-secondary); color: var(--text); }
.tb-checkbox input[type="checkbox"] { accent-color: var(--primary); }

.popover-fade-enter-active, .popover-fade-leave-active { transition: opacity 0.15s; }
.popover-fade-enter-from, .popover-fade-leave-to { opacity: 0; }

/* ==========================================
   响应式
   ========================================== */

@media (max-width: 768px) {
  .mdv-content { padding: 20px 20px 100px; }
  .mdv-floatbar { bottom: 16px; right: 12px; gap: 6px; flex-direction: column-reverse; }
}

@media (max-width: 480px) {
  .mdv-content { padding: 16px 14px 90px; }
  .mdv-floatbar { bottom: 12px; right: 8px; }
  .float-progress { padding: 4px 10px; }
  .progress-bar-track { width: 40px; }
}
</style>