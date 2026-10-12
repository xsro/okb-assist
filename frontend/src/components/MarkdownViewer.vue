<template>
  <div v-if="showSource" class="markdown-source">
    <pre><code>{{ combinedContent }}</code></pre>
  </div>
  <div v-else ref="viewerEl" class="markdown-viewer"></div>
</template>

<script setup lang="ts">
/**
 * MarkdownViewer — 高性能增量 Markdown 阅读器
 *
 * 核心优化：
 * 1. 接受 chunks 数组，每个 chunk 独立渲染后追加 DOM（不重绘已有内容）
 * 2. 公式「按需渲染」：占位符进入扩展视口并停留 ≥500ms 才用 KaTeX 渲染，
 *    首屏/新追加 chunk 中已可见的公式立即渲染（仍按帧分片），不阻塞主线程
 * 3. chunk.id 不变则不重新渲染
 * 4. 保留 content 属性兼容旧用法
 */

import { ref, computed, watch, nextTick, onMounted, onBeforeUnmount } from 'vue'
import { marked, Renderer } from 'marked'
import type { ChunkInfo } from '@/types/document'
import { extractMathPlaceholders, loadMathJax, type MathMode } from '@/utils/mathRenderer'

const props = withDefaults(defineProps<{
  content?: string
  chunks?: ChunkInfo[]
  mathMode?: MathMode
  loadImages?: boolean
  showSource?: boolean
  showLineNumbers?: boolean
  highlight?: string
  trusted?: boolean
  placeholderImages?: boolean
  /** 公式按需渲染（视口内停留后才渲染 KaTeX），默认开启 */
  lazyMath?: boolean
}>(), {
  // 注意：布尔 prop 缺省时 Vue 会按 Boolean 转换规则变成 false，必须显式给默认值
  lazyMath: true,
})

const viewerEl = ref<HTMLElement | null>(null)
const renderedChunkIds = ref(new Set<number>())

// ===== 合并后的全量内容（用于源码模式） =====
const combinedContent = computed(() => {
  if (props.content !== undefined) return props.content
  return (props.chunks || []).map(c => c.content).join('\n')
})

// ===== KaTeX 动态 import（只加载一次） =====
let katexRender: ((latex: string, opts: any) => string) | null = null
let katexLoading: Promise<void> | null = null
let katexFailed = false

function ensureKatex(): Promise<void> {
  if (katexRender) return Promise.resolve()
  if (!katexLoading) {
    katexLoading = (async () => {
      try {
        const mod = await import('katex')
        const k = mod.default || mod
        katexRender = (latex: string, opts: any) => k.renderToString(latex, opts)
      } catch {
        // KaTeX 加载失败，保留占位符（公式以原始 LaTeX 呈现）
        katexFailed = true
      }
    })()
  }
  return katexLoading
}

// ===== 渲染单个 chunk =====
function renderChunk(content: string, chunkId: number, startLine?: number) {
  if (!viewerEl.value) return

  const mode = props.mathMode || 'katex'
  const loadImages = props.loadImages ?? true
  const trusted = props.trusted !== false

  // ── Phase 1: 提取公式占位符 ──
  const { text, matches } = extractMathPlaceholders(content)

  const renderer = new Renderer()
  const usePlaceholder = !loadImages && props.placeholderImages !== false

  renderer.image = (e) => {
    const { href, title, text: altText } = e
    if (loadImages) return Renderer.prototype.image.call(renderer, e)
    const url = href || ''
    const alt = (altText || title || '').replace(/[&<>"']/g, (ch: string) =>
      ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' })[ch] || ch)
    if (usePlaceholder) {
      const viewUrl = url.replace(/[&<>"']/g, (ch: string) =>
        ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' })[ch] || ch)
      return `<span class="img-placeholder" data-src="${viewUrl}">`
        + `<span class="ip-icon">🖼</span>`
        + (alt ? `<span class="ip-alt">${alt}</span>` : '')
        + `<a class="ip-view-btn" href="${viewUrl}" target="_blank" rel="noopener"`
        + ` onclick="event.stopPropagation();">查看图片</a>`
        + `<button class="ip-load-btn" data-load-img>点击加载</button></span>`
    }
    return Renderer.prototype.image.call(renderer, e)
  }

  const parseOpts = { async: false as const, gfm: true, breaks: true, renderer }

  // ── Phase 2: marked 解析（全量） ──
  const raw = marked.parse(text, parseOpts) as string

  // 替换公式占位符 + 净化
  let html = replacePlaceholders(raw, matches, trusted)

  const container = document.createElement('div')
  container.dataset.chunkId = String(chunkId)
  container.innerHTML = html

  // 如果启用了行号，为每个子元素分配对应的 markdown 行号
  // 策略：扫描原始 markdown 内容，跳过空行，为每个渲染出的块元素分配行号
  if (props.showLineNumbers && startLine !== undefined) {
    const children = container.children
    const contentLines = text.split('\n')
    let linePtr = 0
    let childIdx = 0

    while (childIdx < children.length && linePtr < contentLines.length) {
      // 跳过空行
      while (linePtr < contentLines.length && contentLines[linePtr].trim() === '') {
        linePtr++
      }
      if (linePtr >= contentLines.length) break

      children[childIdx].setAttribute('data-line', String(startLine + linePtr + 1))

      // 该块元素可能占了多行，跳过直到遇到空行或文件末尾
      linePtr++
      while (linePtr < contentLines.length && contentLines[linePtr].trim() !== '') {
        linePtr++
      }
      childIdx++
    }
  }

  viewerEl.value.appendChild(container)
  renderedChunkIds.value.add(chunkId)

  // ── Phase 3: 公式渲染调度 ──
  if (matches.length > 0) {
    if (mode === 'katex') {
      scheduleFormulaRender(container)
    } else if (mode === 'mathjax') {
      typesetMathJaxChunk(container)
    }
  }
}

function replacePlaceholderIds(html: string, matches: { id: string; latex: string; displayMode: boolean }[]): string {
  let result = html
  for (const m of matches) {
    const escapedLatex = m.latex
      .replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;')
    const placeholderHtml = `<span class=\"math-placeholder\" data-math-id=\"${m.id}\" data-math-latex=\"${escapedLatex}\" data-math-display=\"${m.displayMode ? 'true' : 'false'}\">`
      + `<span class=\"math-raw\">${m.displayMode ? '$$' : '$'}${escapedLatex}${m.displayMode ? '$$' : '$'}</span></span>`
    result = result.replaceAll(m.id, placeholderHtml)
  }
  return result
}

function replacePlaceholders(html: string, matches: { id: string; latex: string; displayMode: boolean }[], trusted: boolean): string {
  let result = html
  for (const m of matches) {
    const escapedLatex = m.latex
      .replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;')
    const placeholderContent = m.displayMode
      ? `<span class=\"math-raw\">$$${escapedLatex}$$</span>`
      : `<span class=\"math-raw\">$${escapedLatex}$</span>`
    const placeholder = `<span class=\"math-placeholder\" data-math-id=\"${m.id}\" data-math-latex=\"${escapedLatex}\" data-math-display=\"${m.displayMode ? 'true' : 'false'}\">${placeholderContent}</span>`
    result = result.replaceAll(m.id, placeholder)
  }
  if (!trusted) {
    result = result.replace(/<script[\s\S]*?<\/script>/gi, '<!-- script removed -->')
  }
  return result
}

// ===== 公式渲染调度（视口惰性 + 空闲分片） =====
// 策略：
//   - 占位符首次被观察且已在视口内（含 rootMargin 预取区）→ 立即入队渲染；
//   - 已经在视口外、后续因滚动进入视口 → 停留满 MATH_DWELL_MS 才渲染；
//   - 快速划过（未满停留时间）或重新离开视口 → 取消渲染；
//   - 关闭惰性（props.lazyMath === false）→ 回退到全量空闲队列。
const MATH_DWELL_MS = 500
const MATH_ROOT_MARGIN = '400px 0px'

interface PendingFormula {
  el: HTMLElement
  latex: string
  displayMode: boolean
}

const pendingQueue: PendingFormula[] = []
let schedulerActive = false
let mathObserver: IntersectionObserver | null = null
const mathTimers = new Map<HTMLElement, ReturnType<typeof setTimeout>>()
// 已经历过首个观察回调的元素（首个回调中可见的公式免停留）
let mathSeen = new WeakSet<Element>()

/** 空闲回调（无 requestIdleCallback 时退化为 setTimeout） */
const idleCallback: (cb: (deadline: IdleDeadline) => void, opts?: { timeout: number }) => number =
  typeof requestIdleCallback === 'function'
    ? requestIdleCallback
    : ((cb) => setTimeout(
        () => cb({ didTimeout: true, timeRemaining: () => 8 } as IdleDeadline),
        1
      ) as unknown as number)

function lazyMathEnabled(): boolean {
  return props.lazyMath === true && typeof IntersectionObserver !== 'undefined'
}

function scheduleFormulaRender(container: HTMLElement) {
  const placeholders = Array.from(
    container.querySelectorAll<HTMLElement>('.math-placeholder[data-math-id]')
  )
  if (placeholders.length === 0) return

  const observer = lazyMathEnabled() ? ensureMathObserver() : null
  if (!observer) {
    // 不支持 IntersectionObserver 或已关闭惰性渲染：全量空闲渲染
    for (const el of placeholders) enqueueMath(el)
    return
  }
  for (const el of placeholders) observer.observe(el)
}

function ensureMathObserver(): IntersectionObserver {
  if (mathObserver) return mathObserver
  mathObserver = new IntersectionObserver((entries) => {
    for (const entry of entries) {
      const el = entry.target as HTMLElement
      const firstSeen = !mathSeen.has(el)
      mathSeen.add(el)

      if (!entry.isIntersecting) {
        // 离开视口：取消停留计时，并撤销已入队的渲染
        cancelMathTimer(el)
        removePendingMath(el)
        continue
      }

      // 首次出现即已在视口内（首屏 / 刚追加的 chunk）→ 不等停留
      if (firstSeen) {
        enqueueMath(el)
        continue
      }

      if (mathTimers.has(el)) continue
      mathTimers.set(el, setTimeout(() => {
        mathTimers.delete(el)
        enqueueMath(el)
      }, MATH_DWELL_MS))
    }
  }, { rootMargin: MATH_ROOT_MARGIN })
  return mathObserver
}

function cancelMathTimer(el: HTMLElement) {
  const timer = mathTimers.get(el)
  if (timer !== undefined) {
    clearTimeout(timer)
    mathTimers.delete(el)
  }
}

function removePendingMath(el: HTMLElement) {
  const idx = pendingQueue.findIndex(item => item.el === el)
  if (idx >= 0) pendingQueue.splice(idx, 1)
}

function kickScheduler() {
  if (schedulerActive || pendingQueue.length === 0) return
  schedulerActive = true
  idleCallback(processFormulas, { timeout: 3000 })
}

function enqueueMath(el: HTMLElement) {
  if (!el.isConnected) return
  if (pendingQueue.some(item => item.el === el)) return
  pendingQueue.push({
    el,
    latex: el.dataset.mathLatex || '',
    displayMode: el.dataset.mathDisplay === 'true',
  })
  kickScheduler()
}

function processFormulas(deadline: IdleDeadline) {
  const BUDGET_MS = 8

  while (pendingQueue.length > 0 && deadline.timeRemaining() > BUDGET_MS) {
    const item = pendingQueue.shift()!
    if (!item.el.isConnected) continue // DOM 中已不存在

    if (!katexRender) {
      // KaTeX 尚未就绪：放回队列，等加载完成后再继续
      pendingQueue.unshift(item)
      schedulerActive = false
      if (katexFailed) {
        pendingQueue.length = 0 // 加载失败：保留原始 LaTeX，不再重试
        return
      }
      ensureKatex().then(kickScheduler)
      return
    }

    renderMathElement(item.el, item.latex, item.displayMode)
  }

  if (pendingQueue.length > 0) {
    idleCallback(processFormulas, { timeout: 3000 })
  } else {
    schedulerActive = false
  }
}

/** 将单个占位符替换为 KaTeX 输出 */
function renderMathElement(
  el: HTMLElement,
  latex: string,
  displayMode: boolean,
  compensateScroll = true
) {
  if (!el.isConnected || !katexRender) return

  const beforeRect = el.getBoundingClientRect()
  const parent = el.parentElement
  const nextSibling = el.nextSibling

  let html: string
  try {
    html = katexRender(latex, { throwOnError: false, displayMode })
  } catch {
    // 渲染失败，回退为原始公式文本
    el.outerHTML = `<span class="math-fallback">${displayMode ? '$$' : '$'}${escapeAttr(latex)}${displayMode ? '$$' : '$'}</span>`
    return
  }

  el.outerHTML = html

  // 滚动补偿：已滚出视口上方的公式渲染后高度变化会把正文推下去。
  // 若浏览器自身的 scroll anchoring 已处理，实测高度差会接近 0，不会重复补偿。
  if (!compensateScroll || !parent || beforeRect.bottom > 0) return
  const inserted = (nextSibling ? nextSibling.previousSibling : parent.lastChild) as HTMLElement | null
  if (!inserted || typeof inserted.getBoundingClientRect !== 'function') return
  const delta = inserted.getBoundingClientRect().bottom - beforeRect.bottom
  if (Math.abs(delta) > 1) window.scrollBy(0, delta)
}

function escapeAttr(s: string): string {
  return s.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;')
}

/** 渲染指定容器内的全部公式（同步） */
function renderMathInContainer(container: HTMLElement) {
  if (!katexRender) return
  const placeholders = Array.from(
    container.querySelectorAll<HTMLElement>('.math-placeholder[data-math-id]')
  )
  for (const el of placeholders) {
    cancelMathTimer(el)
    removePendingMath(el)
    renderMathElement(el, el.dataset.mathLatex || '', el.dataset.mathDisplay === 'true', false)
  }
}

/**
 * 强制渲染位于 el 之前的全部公式（同步）。
 * 用于目录/搜索跳转：只处理可能把落点推下去的公式，避免整段同步渲染。
 */
function forceRenderMathBefore(el: Element | null) {
  const root = viewerEl.value
  if (!root || !el || !katexRender) return
  const placeholders = Array.from(
    root.querySelectorAll<HTMLElement>('.math-placeholder[data-math-id]')
  )
  for (const ph of placeholders) {
    if (!(el.compareDocumentPosition(ph) & Node.DOCUMENT_POSITION_PRECEDING)) continue
    cancelMathTimer(ph)
    removePendingMath(ph)
    renderMathElement(ph, ph.dataset.mathLatex || '', ph.dataset.mathDisplay === 'true', false)
  }
}

/** 强制渲染全部公式（打印 / 用户主动触发） */
async function renderAllMath() {
  await ensureKatex()
  clearMathTimers()
  pendingQueue.length = 0
  const root = viewerEl.value
  if (root) renderMathInContainer(root)
}

function clearMathTimers() {
  for (const timer of mathTimers.values()) clearTimeout(timer)
  mathTimers.clear()
}

/** 清空公式调度状态（DOM 重建时调用） */
function resetMathRuntime() {
  pendingQueue.length = 0
  schedulerActive = false
  clearMathTimers()
  mathObserver?.disconnect()
  mathObserver = null
  mathSeen = new WeakSet<Element>()
  mathjaxQueue.length = 0
}

// ===== MathJax（按 chunk 增量排版，不做惰性） =====
const mathjaxQueue: HTMLElement[] = []
let mathjaxRunning = false

function typesetMathJaxChunk(container: HTMLElement) {
  if (props.mathMode !== 'mathjax') return
  mathjaxQueue.push(container)
  void runMathJaxQueue()
}

async function runMathJaxQueue() {
  if (mathjaxRunning || mathjaxQueue.length === 0) return
  mathjaxRunning = true
  try {
    await loadMathJax()
    const mj = (window as any).MathJax
    while (mathjaxQueue.length > 0 && props.mathMode === 'mathjax') {
      const batch = mathjaxQueue.splice(0, mathjaxQueue.length)
      await mj?.typesetPromise?.(batch)
    }
  } finally {
    mathjaxRunning = false
  }
}

defineExpose({ renderAllMath, forceRenderMathBefore })

// ===== 关键词高亮（DOM 操作，保留 KaTeX 结果） =====
function applyHighlight(keyword: string) {
  if (!viewerEl.value || !keyword) return
  try {
    const escaped = keyword.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')
    const regex = new RegExp(`(${escaped})`, 'gi')
    const walker = document.createTreeWalker(viewerEl.value, NodeFilter.SHOW_TEXT, null, false)
    const targets: Text[] = []
    let node: Text | null
    while ((node = walker.nextNode() as Text | null)) {
      if (node.textContent && regex.test(node.textContent)) {
        targets.push(node)
      }
      regex.lastIndex = 0
    }
    for (const textNode of targets) {
      const text = textNode.textContent || ''
      regex.lastIndex = 0
      let lastIdx = 0
      let match
      const frag = document.createDocumentFragment()
      while ((match = regex.exec(text)) !== null) {
        if (match.index > lastIdx) {
          frag.appendChild(document.createTextNode(text.slice(lastIdx, match.index)))
        }
        const mark = document.createElement('mark')
        mark.className = 'search-highlight'
        mark.textContent = match[1]
        frag.appendChild(mark)
        lastIdx = regex.lastIndex
      }
      if (lastIdx < text.length) {
        frag.appendChild(document.createTextNode(text.slice(lastIdx)))
      }
      textNode.parentNode?.replaceChild(frag, textNode)
    }
  } catch { /* ignore */ }
}

function clearHighlight() {
  if (!viewerEl.value) return
  viewerEl.value.querySelectorAll('.search-highlight').forEach(el => {
    const parent = el.parentNode
    if (parent) {
      parent.replaceChild(document.createTextNode(el.textContent || ''), el)
      parent.normalize()
    }
  })
}

// ===== 图片点击加载 =====
function onViewerClick(e: MouseEvent) {
  const btn = (e.target as HTMLElement).closest('[data-load-img]') as HTMLElement | null
  if (!btn) return
  const ph = btn.closest('.img-placeholder') as HTMLElement | null
  if (!ph?.dataset.src) return
  const img = document.createElement('img')
  img.src = ph.dataset.src
  img.alt = ph.querySelector('.ip-alt')?.textContent || ''
  img.style.maxWidth = '100%'
  ph.parentNode?.replaceChild(img, ph)
}

// ===== 监听 chunks（增量追加） =====
watch(() => props.chunks, (chunks) => {
  if (!chunks || !viewerEl.value) return
  for (const chunk of chunks) {
    if (!renderedChunkIds.value.has(chunk.id)) {
      renderChunk(chunk.content, chunk.id, chunk.start_line)
    }
  }
}, { immediate: true, deep: false })

// ===== 兼容旧的 content 属性 =====
watch(() => props.content, (content) => {
  if (content === undefined || !viewerEl.value) return
  viewerEl.value.innerHTML = ''
  renderedChunkIds.value.clear()
  resetMathRuntime()
  renderChunk(content, 0)
})

// ===== 源码/渲染视图切换后重建内容（v-if 会销毁 viewer 容器） =====
watch(() => props.showSource, (show) => {
  if (show) return
  nextTick(() => {
    if (!viewerEl.value) return
    renderedChunkIds.value = new Set<number>()
    viewerEl.value.innerHTML = ''
    resetMathRuntime()
    for (const chunk of props.chunks || []) {
      renderChunk(chunk.content, chunk.id, chunk.start_line)
    }
    if (props.content !== undefined) renderChunk(props.content, 0)
    const kw = props.highlight?.trim()
    if (kw) nextTick(() => applyHighlight(kw))
  })
})

// ===== 高亮 =====
watch(() => props.highlight, (kw) => {
  if (!viewerEl.value) return
  clearHighlight()
  if (kw?.trim()) nextTick(() => applyHighlight(kw.trim()))
}, { immediate: true })

// ===== MathJax：切换离开时丢弃未排版的队列 =====
watch(() => props.mathMode, (mode) => {
  if (mode !== 'mathjax') mathjaxQueue.length = 0
})

// ===== 关闭「公式按需渲染」时，把已存在的占位符补渲染 =====
watch(() => props.lazyMath, (lazy) => {
  if (lazy !== true && props.mathMode === 'katex') void renderAllMath()
})

// ===== 生命周期 =====
onMounted(async () => {
  await ensureKatex()
  viewerEl.value?.addEventListener('click', onViewerClick)
  // 打印前强制渲染全部公式，避免打印出未渲染的 LaTeX 源码
  window.addEventListener('beforeprint', onBeforePrint)
  // 处理初始内容：props.content（编辑器预览）或 props.chunks（阅读器）
  // 注意：watch 回调非 immediate，首次渲染必须在这里完成
  if (props.content !== undefined) {
    if (!renderedChunkIds.value.has(0)) renderChunk(props.content, 0)
  } else if (props.chunks && viewerEl.value) {
    for (const chunk of props.chunks) {
      if (!renderedChunkIds.value.has(chunk.id)) {
        renderChunk(chunk.content, chunk.id, chunk.start_line)
      }
    }
  }
  // 初始渲染完成后，应用高亮
  const kw = props.highlight?.trim()
  if (kw && viewerEl.value) {
    nextTick(() => applyHighlight(kw))
  }
})

function onBeforePrint() {
  if (props.mathMode !== 'katex') return
  void renderAllMath()
}

onBeforeUnmount(() => {
  viewerEl.value?.removeEventListener('click', onViewerClick)
  window.removeEventListener('beforeprint', onBeforePrint)
  resetMathRuntime()
})
</script>

<style scoped>
.markdown-viewer :deep(img) {
  display: block;
  margin: 12px auto;
  max-width: 100%;
  height: auto;
  border-radius: 4px;
}

/* ===== 公式占位符 ===== */
.markdown-viewer :deep(.math-placeholder) {
  display: inline;
}

.markdown-viewer :deep(.math-placeholder[data-math-display="true"]) {
  display: block;
  margin: 12px 0;
}

.markdown-raw {
  background: #f5f5f5;
  border: 1px solid #e0e0e0;
  border-radius: 3px;
  padding: 1px 4px;
  font-family: 'SF Mono', 'Fira Code', 'Consolas', monospace;
  font-size: 0.85em;
  color: #666;
}

.math-placeholder[data-math-display="true"] .math-raw {
  display: block;
  padding: 8px 12px;
  overflow-x: auto;
}

.math-fallback {
  background: #f5f5f5;
  border: 1px solid #e0e0e0;
  border-radius: 3px;
  padding: 1px 4px;
  font-family: 'SF Mono', 'Fira Code', 'Consolas', monospace;
  font-size: 0.85em;
  color: #c00;
}

/* ===== 图片占位符 ===== */
.img-placeholder {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  padding: 8px 12px;
  background: #fff;
  border: 2px solid #000;
  border-radius: 4px;
  color: var(--text-secondary);
  font-size: 13px;
  line-height: 1.6;
  cursor: default;
  max-width: 100%;
  overflow: hidden;
}

.ip-icon { flex-shrink: 0; font-size: 16px; }
.ip-alt { flex-shrink: 0; font-weight: 500; color: var(--text); max-width: 180px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; margin-right: 4px; }
.ip-alt:empty { display: none; }

.ip-view-btn, .ip-load-btn {
  display: inline-flex;
  align-items: center;
  padding: 3px 10px;
  border: 1px solid #000;
  border-radius: 3px;
  font-size: 12px;
  font-family: inherit;
  cursor: pointer;
  text-decoration: none;
  line-height: 1.6;
  white-space: nowrap;
  background: #fff;
  color: #000;
  transition: all 0.12s;
}

.ip-view-btn:hover, .ip-load-btn:hover { background: #000; color: #fff; }

.markdown-viewer :deep(.search-highlight) {
  background: #ffeb3b;
  padding: 0 2px;
  border-radius: 2px;
}

/* ===== 行号 ===== */
.markdown-viewer :deep([data-line]) {
  position: relative;
  padding-left: 56px;
  min-height: 1.4em;
}

.markdown-viewer :deep([data-line]::before) {
  content: attr(data-line);
  position: absolute;
  left: 0;
  top: 0;
  width: 44px;
  text-align: right;
  padding-right: 8px;
  font-size: 12px;
  line-height: inherit;
  color: #bbb;
  font-family: 'SF Mono', 'Fira Code', 'Consolas', monospace;
  user-select: none;
  pointer-events: none;
  border-right: 1px solid #eee;
  white-space: nowrap;
  overflow: hidden;
}

/* ===== 公式溢出滚动 ===== */
.markdown-viewer :deep(.katex-display) {
  max-width: 100%;
  overflow-x: auto;
  overflow-y: hidden;
  padding: 4px 0;
  /* 滚动条不占宽度 */
  scrollbar-width: thin;
}

.markdown-viewer :deep(.katex) {
  max-width: 100%;
  overflow-x: auto;
  overflow-y: hidden;
}

/* ===== 长链接折行 ===== */
.markdown-viewer :deep(a) {
  word-break: break-all;
  overflow-wrap: break-word;
}

/* ===== 图片、代码块、表格溢出 ===== */
.markdown-viewer :deep(pre) {
  overflow-x: auto;
  max-width: 100%;
}

.markdown-viewer :deep(table) {
  display: block;
  max-width: 100%;
  overflow-x: auto;
}

.markdown-source {
  background: #f8f9fa;
  border: 1px solid var(--border);
  border-radius: 6px;
  padding: 16px;
  overflow-x: auto;
}

.markdown-source pre { margin: 0; white-space: pre-wrap; word-break: break-all; }
.markdown-source code {
  font-family: 'SF Mono', 'Fira Code', 'Consolas', monospace;
  font-size: 13px;
  line-height: 1.6;
  color: #333;
}
</style>