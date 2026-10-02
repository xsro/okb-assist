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
 * 2. 公式渲染通过 requestIdleCallback 分片完成，不阻塞主线程
 * 3. chunk.id 不变则不重新渲染
 * 4. 保留 content 属性兼容旧用法
 */

import { ref, computed, watch, nextTick, onMounted, onBeforeUnmount } from 'vue'
import { marked, Renderer } from 'marked'
import type { ChunkInfo } from '@/types/document'
import { extractMathPlaceholders, loadMathJax, type MathMode } from '@/utils/mathRenderer'

const props = defineProps<{
  content?: string
  chunks?: ChunkInfo[]
  mathMode?: MathMode
  loadImages?: boolean
  showSource?: boolean
  showLineNumbers?: boolean
  highlight?: string
  trusted?: boolean
  placeholderImages?: boolean
}>()

const viewerEl = ref<HTMLElement | null>(null)
const renderedChunkIds = ref(new Set<number>())

// ===== 合并后的全量内容（用于源码模式） =====
const combinedContent = computed(() => {
  if (props.content !== undefined) return props.content
  return (props.chunks || []).map(c => c.content).join('\n')
})

// ===== KaTeX 动态 import（只加载一次） =====
let katexRender: ((latex: string, opts: any) => string) | null = null

async function ensureKatex() {
  if (katexRender) return
  try {
    const mod = await import('katex')
    const k = mod.default || mod
    katexRender = (latex: string, opts: any) => k.renderToString(latex, opts)
  } catch {
    // KaTeX 加载失败，保留占位符
  }
}

// ===== 渲染单个 chunk =====
function renderChunk(content: string, chunkId: number, startLine?: number) {
  if (!viewerEl.value) return

  const mode = props.mathMode || 'katex'
  const loadImages = props.loadImages ?? true
  const trusted = props.trusted !== false

  // ── Phase 1: 提取公式占位符 + marked 解析（同步，快） ──
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

  const raw = marked.parse(text, {
    async: false,
    gfm: true,
    breaks: true,
    renderer,
  }) as string

  // 将公式占位符转换为 DOM 中的 data-math-id span
  let html = raw
  // 占位符显示为原始公式文本，但仍可通过 data-math-id 找到
  for (const m of matches) {
    const escapedLatex = m.latex
      .replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;')
    const display = m.displayMode
    const placeholderContent = display
      ? `<span class="math-raw">$$${escapedLatex}$$</span>`
      : `<span class="math-raw">$${escapedLatex}$</span>`
    // 先将占位符替换为 span
    const placeholder = `<span class="math-placeholder" data-math-id="${m.id}" data-math-latex="${escapedLatex}" data-math-display="${display ? 'true' : 'false'}">${placeholderContent}</span>`
    html = html.replaceAll(m.id, placeholder)
  }

  // 信任模式净化
  if (!trusted) {
    html = html.replace(/<script[\s\S]*?<\/script>/gi, '<!-- script removed -->')
  }

  // ── Phase 2: 插入 DOM ──
  const container = document.createElement('div')
  container.dataset.chunkId = String(chunkId)
  container.innerHTML = html
  
  // 如果启用了行号，为每个直接子元素添加 data-line 属性
  if (props.showLineNumbers && startLine !== undefined) {
    const children = container.children
    for (let i = 0; i < children.length; i++) {
      children[i].setAttribute('data-line', String(startLine + i + 1))
    }
  }
  viewerEl.value.appendChild(container)

  renderedChunkIds.value.add(chunkId)

  // ── Phase 3: 异步渲染公式 ──
  if (mode === 'katex' && matches.length > 0) {
    scheduleFormulaRender(container)
  }
}

// ===== Idle Callback 公式渲染 =====
interface PendingFormula {
  el: HTMLElement | null
  id: string
  latex: string
  displayMode: boolean
}

const pendingQueue: PendingFormula[] = []
let schedulerActive = false

function scheduleFormulaRender(container: HTMLElement) {
  const placeholders = container.querySelectorAll<HTMLElement>('.math-placeholder[data-math-id]')
  for (const el of placeholders) {
    pendingQueue.push({
      el,
      id: el.dataset.mathId || '',
      latex: el.dataset.mathLatex || '',
      displayMode: el.dataset.mathDisplay === 'true',
    })
  }

  if (!schedulerActive) {
    schedulerActive = true
    requestIdleCallback(processFormulas, { timeout: 3000 })
  }
}

function processFormulas(deadline: IdleDeadline) {
  const BUDGET_MS = 8
  let rendered = 0

  while (pendingQueue.length > 0 && deadline.timeRemaining() > BUDGET_MS) {
    const item = pendingQueue.shift()!
    if (!item.el || !item.el.isConnected) continue // DOM 中已不存在

    if (!katexRender) {
      // KaTeX 未就绪，跳过（保留占位符）
      continue
    }

    try {
      const html = katexRender(item.latex, {
        throwOnError: false,
        displayMode: item.displayMode,
      })
      item.el.outerHTML = html
      rendered++
    } catch {
      // 渲染失败，保留原始公式文本
      const el = item.el
      el.outerHTML = `<span class="math-fallback">${item.displayMode ? '$$' : '$'}${escapeAttr(item.latex)}${item.displayMode ? '$$' : '$'}</span>`
    }
  }

  if (pendingQueue.length > 0) {
    requestIdleCallback(processFormulas, { timeout: 3000 })
  } else {
    schedulerActive = false
  }
}

function escapeAttr(s: string): string {
  return s.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;')
}

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
  renderChunk(content, 0)
})

// ===== 高亮 =====
watch(() => props.highlight, (kw) => {
  if (!viewerEl.value) return
  clearHighlight()
  if (kw?.trim()) nextTick(() => applyHighlight(kw.trim()))
}, { immediate: true })

// ===== MathJax =====
watch(() => [props.content, props.chunks, props.mathMode], async () => {
  if (props.mathMode !== 'mathjax' || !viewerEl.value) return
  await loadMathJax()
  ;(window as any).MathJax?.typesetPromise?.([viewerEl.value])
}, { immediate: true })

// ===== 生命周期 =====
onMounted(async () => {
  await ensureKatex()
  viewerEl.value?.addEventListener('click', onViewerClick)
  // 处理初始 chunks（watch 可能在 mounted 前触发）
  if (props.chunks && viewerEl.value) {
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

onBeforeUnmount(() => {
  viewerEl.value?.removeEventListener('click', onViewerClick)
  pendingQueue.length = 0
  schedulerActive = false
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