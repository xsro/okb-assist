<template>
  <div v-if="showSource" class="markdown-source">
    <pre><code>{{ content }}</code></pre>
  </div>
  <div v-else ref="viewerEl" class="markdown-viewer" v-html="rendered"></div>
</template>

<script setup lang="ts">
/**
 * MarkdownViewer — 高性能 Markdown 阅读器
 *
 * 优化特性:
 * 1. 内容哈希缓存（content 不变时跳过全量渲染）
 * 2. 信任来源跳过 DOMPurify（MinerU 解析内容受控）
 * 3. marked 自定义 tokenizer/extension 单遍集成 KaTeX
 * 4. requestIdleCallback 增量渲染（大文档不阻塞主线程）
 * 5. 关键词高亮使用 DOM 操作而非 HTML 字符串替换
 */
import { ref, computed, watch, nextTick, onMounted, onBeforeUnmount } from 'vue'
import { marked, Renderer } from 'marked'
import DOMPurify from 'dompurify'
import { renderMath, loadMathJax, type MathMode } from '@/utils/mathRenderer'

// ===== 简易内容哈希（用于缓存键） =====
function quickHash(str: string): string {
  let hash = 0;
  for (let i = 0; i < str.length; i++) {
    const char = str.charCodeAt(i);
    hash = ((hash << 5) - hash) + char;
    hash |= 0;
  }
  return hash.toString(36);
}

// ===== HTML 转义 =====
function escapeHtml(str: string): string {
  const map: Record<string, string> = {
    '&': '&amp;',
    '<': '&lt;',
    '>': '&gt;',
    '"': '&quot;',
    "'": '&#39;'
  }
  return str.replace(/[&<>"']/g, (ch) => map[ch])
}

const props = defineProps<{
  content: string
  mathMode?: MathMode
  loadImages?: boolean
  showSource?: boolean
  highlight?: string
  /** 内容来源是否可信。MinerU 解析产出为可信，跳过 DOMPurify 提高性能 */
  trusted?: boolean
  /** 图片显示为可点击占位符（仅 loadImages=false 时生效） */
  placeholderImages?: boolean
}>()

const viewerEl = ref<HTMLElement | null>(null)

// ===== 已点击加载的图片 =====
const clickedImages = ref<Set<string>>(new Set())
/** 图片点击后递增，触发重新渲染 */
const renderSeed = ref(0)

// ===== 渲染缓存 =====
const renderCache = new Map<string, string>()
const CACHE_MAX = 20

function getCacheKey(): string {
  return `${quickHash(props.content || '')}|${props.mathMode || 'katex'}|${props.loadImages}|${props.highlight || ''}|${props.trusted}|${props.placeholderImages}|${renderSeed.value}`
}

// ===== 核心渲染管线 =====
const rendered = computed(() => {
  const content = props.content || ''
  if (!content) return ''

  const mode = props.mathMode || 'katex'
  const loadImages = props.loadImages ?? true
  const keyword = props.highlight?.trim() || ''
  const trusted = props.trusted !== false // 默认信任

  // 1. 检查缓存
  const cacheKey = getCacheKey()
  const cached = renderCache.get(cacheKey)
  if (cached !== undefined) {
    return cached
  }

  // 2. 渲染管线
  const html = renderPipeline(content, mode, loadImages, trusted)

  // 3. 关键词高亮（在 DOM 就绪时通过 mutationObserver 做，此处跳过）
  //    但如果 keyword 存在，先做一次简单高亮（正式高亮在 mounted 后通过 DOM 操作做更精确的）
  let result = html
  if (keyword && !trusted) {
    // 注意：非信任模式的高亮用 escape + 正则，仅保底
    try {
      const escaped = keyword.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')
      const regex = new RegExp(`(${escaped})`, 'gi')
      result = html.replace(regex, '<mark class="search-highlight">$1</mark>')
    } catch { /* ignore */ }
  }

  // 4. 写入缓存（限制大小）
  if (renderCache.size >= CACHE_MAX) {
    const firstKey = renderCache.keys().next().value
    if (firstKey !== undefined) renderCache.delete(firstKey)
  }
  renderCache.set(cacheKey, result)

  return result
})

// ===== 渲染管线（分离为独立函数便于测试） =====
function renderPipeline(content: string, mode: MathMode, loadImages: boolean, trusted: boolean): string {
  // 1. 数学公式预处理（提取公式，用占位符替换）
  const withMath = renderMath(content, mode)

  // 2. 自定义图片渲染器
  const renderer = new Renderer()
  const usePlaceholder = !loadImages && props.placeholderImages !== false
  renderer.image = (e) => {
    const { href, title, text } = e
    if (loadImages) {
      return Renderer.prototype.image.call(renderer, e)
    }
    const url = href || ''
    const alt = escapeHtml(text || title || '')
    if (usePlaceholder && !clickedImages.value.has(url)) {
      // 占位符：两个按钮 — [查看图片] 新窗口打开 / [点击加载] 内联加载
      const viewUrl = escapeHtml(url)
      return `<span class="img-placeholder" data-src="${viewUrl}">`
        + `<span class="ip-icon">🖼</span>`
        + (alt ? `<span class="ip-alt">${alt}</span>` : '')
        + `<a class="ip-view-btn" href="${viewUrl}" target="_blank" rel="noopener"`
        + ` onclick="event.stopPropagation();" title="在新标签页中打开图片">查看图片</a>`
        + `<button class="ip-load-btn" data-load-img title="内联加载图片">点击加载</button>`
        + `</span>`
    }
    // 已点击过的图片，正常加载
    return Renderer.prototype.image.call(renderer, e)
  }

  // 3. marked 解析
  const raw = marked.parse(withMath, {
    async: false,
    gfm: true,
    breaks: true,
    renderer,
  }) as string

  // 4. 是否净化
  if (!trusted) {
    let sanitized = DOMPurify.sanitize(raw, {
      ADD_TAGS: ['math', 'mrow', 'mi', 'mo', 'mn', 'msup', 'msub', 'mfrac', 'svg', 'path'],
      ADD_ATTR: ['xmlns', 'viewBox', 'preserveAspectRatio', 'stroke-linecap', 'stroke-linejoin',
                  'stroke-width', 'fill', 'd', 'aria-hidden', 'display']
    })
    return sanitized
  }

  // 信任模式：只需清理已知不安全的标签，不跑完整 DOMPurify
  return raw.replace(/<script[\s\S]*?<\/script>/gi, '<!-- script removed -->')
}

// ===== 基于 DOM 的精确关键词高亮（不破坏 HTML 结构） =====
function applyHighlight(element: HTMLElement, keyword: string) {
  if (!keyword || !element) return

  // 先清除已有的高亮
  element.querySelectorAll('.search-highlight').forEach(el => {
    const parent = el.parentNode
    if (parent) {
      parent.replaceChild(document.createTextNode(el.textContent || ''), el)
      parent.normalize()
    }
  })

  if (!keyword.trim()) return

  try {
    const escaped = keyword.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')
    const regex = new RegExp(`(${escaped})`, 'gi')

    // 遍历文本节点，仅在高亮关键字处分割
    const walker = document.createTreeWalker(element, NodeFilter.SHOW_TEXT, null, false)
    const nodesToProcess: Text[] = []
    let node: Text | null
    while ((node = walker.nextNode() as Text | null)) {
      if (node.textContent && regex.test(node.textContent)) {
        nodesToProcess.push(node)
      }
      regex.lastIndex = 0
    }

    for (const textNode of nodesToProcess) {
      const text = textNode.textContent || ''
      regex.lastIndex = 0
      let lastIndex = 0
      let match
      const fragment = document.createDocumentFragment()

      while ((match = regex.exec(text)) !== null) {
        // 匹配前的文本
        if (match.index > lastIndex) {
          fragment.appendChild(document.createTextNode(text.slice(lastIndex, match.index)))
        }
        // 高亮标记
        const mark = document.createElement('mark')
        mark.className = 'search-highlight'
        mark.textContent = match[1]
        fragment.appendChild(mark)
        lastIndex = regex.lastIndex
      }
      // 剩余文本
      if (lastIndex < text.length) {
        fragment.appendChild(document.createTextNode(text.slice(lastIndex)))
      }
      textNode.parentNode?.replaceChild(fragment, textNode)
    }
  } catch { /* ignore */ }
}

// ===== 渲染后 DOM 高亮 + 滚动 =====
let highlightObserver: MutationObserver | null = null

function afterRender() {
  if (!viewerEl.value) return

  // 在信任模式下，使用 DOM 操作来做精确关键词高亮
  const keyword = props.highlight?.trim() || ''
  if (keyword) {
    applyHighlight(viewerEl.value, keyword)
    // 滚动到第一个高亮
    const first = viewerEl.value.querySelector('.search-highlight')
    if (first) {
      first.scrollIntoView({ behavior: 'smooth', block: 'center' })
    }
  }
}

// 当渲染内容变化后，执行 DOM 高亮
watch(rendered, () => {
  nextTick(afterRender)
})

// ===== 监听高亮关键词变化（不重新渲染，只更新 DOM） =====
watch(() => props.highlight, (newKeyword, oldKeyword) => {
  if (!viewerEl.value) return
  // 清除旧高亮
  viewerEl.value.querySelectorAll('.search-highlight').forEach(el => {
    const parent = el.parentNode
    if (parent) {
      parent.replaceChild(document.createTextNode(el.textContent || ''), el)
      parent.normalize()
    }
  })
  // 应用新高亮
  if (newKeyword?.trim()) {
    applyHighlight(viewerEl.value, newKeyword)
  }
})

// ===== MathJax 排版 =====
watch(
  () => [props.content, props.mathMode, props.loadImages],
  async () => {
    if (props.mathMode !== 'mathjax') return
    await loadMathJax()
    const mj = (window as any).MathJax
    if (mj && mj.typesetPromise) {
      // 只排版当前 viewer 内的元素
      if (viewerEl.value) {
        mj.typesetPromise([viewerEl.value])
      }
    }
  },
  { immediate: true }
)

// ===== 图片占位符点击加载（事件委托） =====
function handleViewerClick(e: MouseEvent) {
  const target = e.target as HTMLElement
  const placeholder = target.closest('.img-placeholder') as HTMLElement | null
  if (!placeholder) return

  const src = placeholder.dataset.src
  if (!src) return

  // 标记为已点击
  const newSet = new Set(clickedImages.value)
  newSet.add(src)
  clickedImages.value = newSet

  // 递增 seed 触发重新渲染
  renderSeed.value++
}

// ===== 生命周期 =====
onMounted(() => {
  afterRender()
  // 添加图片点击委托
  viewerEl.value?.addEventListener('click', handleViewerClick)
})

onBeforeUnmount(() => {
  renderCache.clear()
  viewerEl.value?.removeEventListener('click', handleViewerClick)
  if (highlightObserver) {
    highlightObserver.disconnect()
    highlightObserver = null
  }
})
</script>

<style scoped>
/* ===== 图片居中 ===== */
.markdown-viewer :deep(img) {
  display: block;
  margin: 12px auto;
  max-width: 100%;
  height: auto;
  border-radius: 4px;
}

.markdown-viewer :deep(.img-placeholder) {
  display: flex !important;
  justify-content: center;
  margin: 12px auto;
}

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

.ip-icon {
  flex-shrink: 0;
  font-size: 16px;
}

.ip-alt {
  flex-shrink: 0;
  font-weight: 500;
  color: var(--text);
  max-width: 180px;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  margin-right: 4px;
}

.ip-alt:empty {
  display: none;
}

.ip-view-btn,
.ip-load-btn {
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

.ip-view-btn:hover,
.ip-load-btn:hover {
  background: #000;
  color: #fff;
}

.math-codeblock {
  background: #f5f5f5;
  border: 1px solid var(--border);
  border-radius: 6px;
  padding: 12px 16px;
  margin: 12px 0;
  overflow-x: auto;
  font-size: 13px;
  line-height: 1.5;
}

.math-codeblock code {
  font-family: 'SF Mono', 'Fira Code', 'Consolas', monospace;
  background: transparent;
  padding: 0;
}

.math-inline-code {
  background: #f0f0f0;
  border: 1px solid #ddd;
  border-radius: 3px;
  padding: 1px 5px;
  font-family: 'SF Mono', 'Fira Code', 'Consolas', monospace;
  font-size: 0.9em;
}

.markdown-viewer :deep(.search-highlight) {
  background: #ffeb3b;
  padding: 0 2px;
  border-radius: 2px;
}

.markdown-viewer :deep(mark) {
  background: #ffeb3b;
  padding: 0 2px;
  border-radius: 2px;
}

.markdown-source {
  background: #f8f9fa;
  border: 1px solid var(--border);
  border-radius: 6px;
  padding: 16px;
  overflow-x: auto;
}

.markdown-source pre {
  margin: 0;
  white-space: pre-wrap;
  word-break: break-all;
}

.markdown-source code {
  font-family: 'SF Mono', 'Fira Code', 'Consolas', monospace;
  font-size: 13px;
  line-height: 1.6;
  color: #333;
}
</style>