<template>
  <div class="docs-view container">
    <div class="docs-head">
      <div class="docs-head-main">
        <h2>使用文档</h2>
        <p class="docs-sub">
          面向使用者的操作说明。部署、架构与设计文档见
          <a href="/document/" target="_blank" rel="noopener">文档站</a>。
        </p>
      </div>
      <div class="docs-head-actions">
        <a
          v-if="current"
          class="btn btn-sm btn-outline"
          :href="rawDocUrl(current.file)"
          target="_blank"
          rel="noopener"
        >原文 Markdown</a>
        <button v-if="isMobile" class="btn btn-sm btn-outline" @click="navOpen = !navOpen">
          {{ navOpen ? '收起目录' : '目录' }}
        </button>
      </div>
    </div>

    <div class="docs-layout">
      <!-- 左侧：文档目录 -->
      <aside v-show="!isMobile || navOpen" class="docs-nav">
        <div v-for="g in DOC_GROUPS" :key="g.title" class="docs-nav-group">
          <div class="docs-nav-title">{{ g.title }}</div>
          <router-link
            v-for="d in g.items"
            :key="d.slug"
            class="docs-nav-item"
            :class="{ active: d.slug === slug }"
            :to="`/assist/docs/${d.slug}`"
            @click="navOpen = false"
          >
            <span class="docs-nav-label">{{ d.title }}</span>
            <span class="docs-nav-desc">{{ d.desc }}</span>
          </router-link>
        </div>
        <div class="docs-nav-foot">
          找不到要的内容？
          <a href="/document/" target="_blank" rel="noopener">看开发文档 →</a>
        </div>
      </aside>

      <!-- 中间：正文（点击站内 .md 链接与页内锚点由 onClick 接管） -->
      <article class="docs-main" @click="onClick">
        <div v-if="loading" class="docs-hint">加载中…</div>
        <div v-else-if="error" class="docs-hint is-error">{{ error }}</div>
        <template v-else>
          <div ref="contentEl" class="docs-content">
            <MarkdownViewer :content="raw" :math-mode="'katex'" />
          </div>
          <nav class="docs-pager">
            <router-link v-if="prev" class="btn btn-sm btn-outline" :to="`/assist/docs/${prev.slug}`">
              ← {{ prev.title }}
            </router-link>
            <span v-else></span>
            <router-link v-if="next" class="btn btn-sm btn-outline" :to="`/assist/docs/${next.slug}`">
              {{ next.title }} →
            </router-link>
          </nav>
        </template>
      </article>

      <!-- 右侧：本页目录 -->
      <aside v-if="!isMobile && toc.length" class="docs-toc">
        <div class="docs-toc-title">本页目录</div>
        <button
          v-for="t in toc"
          :key="t.id"
          class="docs-toc-item"
          :class="`lv${t.level}`"
          @click="scrollToAnchor(t.id)"
        >{{ t.text }}</button>
      </aside>
    </div>
  </div>
</template>

<script setup lang="ts">
/**
 * DocsView — 使用文档阅读页（`/assist/docs/:slug?`）
 *
 * - 内容来自 `frontend/public/docs/*.md`，构建期以 `?raw` 打进包（见 `docs/userDocs.ts`）；
 * - 目录 / 顺序 / 标题来自 `DOC_GROUPS`，不在 Markdown 里写任何导航元数据；
 * - 站内 `.md` 互链会被拦截成路由跳转，`#锚点` 走平滑滚动，外链自动 `target=_blank`；
 * - 标题 id 由源码解析出的目录顺序分配（marked 不生成 id），因此 TOC 与正文标题一一对应。
 */
import { computed, nextTick, onBeforeUnmount, onMounted, ref, watch } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import MarkdownViewer from '@/components/MarkdownViewer.vue'
import { useResponsive } from '@/composables/useResponsive'
import {
  DOC_GROUPS,
  DOC_INDEX,
  findDoc,
  loadDocContent,
  rawDocUrl,
} from '@/docs/userDocs'

interface TocEntry {
  id: string
  text: string
  level: number
}

const route = useRoute()
const router = useRouter()
const { isMobile } = useResponsive()

const slug = computed(() => String(route.params.slug || DOC_INDEX[0]?.slug || ''))
const current = computed(() => findDoc(slug.value))

const raw = ref('')
const loading = ref(false)
const error = ref('')
const navOpen = ref(false)
const contentEl = ref<HTMLElement | null>(null)

/** 源码里的全部标题（h1–h4），用于分配 id 与生成右侧目录 */
const headings = ref<TocEntry[]>([])
const toc = computed(() => headings.value.filter((h) => h.level >= 2 && h.level <= 3))

const index = computed(() => DOC_INDEX.findIndex((d) => d.slug === slug.value))
const prev = computed(() => (index.value > 0 ? DOC_INDEX[index.value - 1] : null))
const next = computed(() =>
  index.value >= 0 && index.value < DOC_INDEX.length - 1 ? DOC_INDEX[index.value + 1] : null
)

// ── Markdown → 目录（跳过代码块里的 `#` 注释） ──────────────
function stripInline(text: string): string {
  return text
    .replace(/!?\[([^\]]*)\]\([^)]*\)/g, '$1')
    .replace(/`([^`]*)`/g, '$1')
    .replace(/\*\*(.+?)\*\*/g, '$1')
    .replace(/\*(.+?)\*/g, '$1')
    // 不要动 `_`：工具名大多是 `get_document_info` 这种下划线命名，当强调语法会被删掉
    .replace(/\s+/g, ' ')
    .trim()
}

function slugify(text: string): string {
  return text
    .toLowerCase()
    .replace(/[^\p{L}\p{N}\s-]/gu, '')
    .trim()
    .replace(/\s+/g, '-')
}

function extractHeadings(md: string): TocEntry[] {
  const out: TocEntry[] = []
  const used = new Map<string, number>()
  let inFence = false
  for (const line of md.split('\n')) {
    if (/^\s*(```|~~~)/.test(line)) {
      inFence = !inFence
      continue
    }
    if (inFence) continue
    const m = /^(#{1,4})\s+(.*)$/.exec(line)
    if (!m) continue
    const text = stripInline(m[2])
    if (!text) continue
    const base = slugify(text) || `section-${out.length + 1}`
    const dup = used.get(base) || 0
    used.set(base, dup + 1)
    out.push({ id: dup ? `${base}-${dup}` : base, text, level: m[1].length })
  }
  return out
}

/** 给渲染出来的标题补 id（marked 不生成 id），并给外链加 target=_blank */
function decorate() {
  const root = contentEl.value
  if (!root) return
  const domHeadings = root.querySelectorAll<HTMLElement>('h1, h2, h3, h4')
  domHeadings.forEach((el, i) => {
    const entry = headings.value[i]
    if (entry && !el.id) el.id = entry.id
  })
  // 宽表格套一层滚动容器：既能「内容装得下就换行铺满」，也能「min-content 超宽时内部滚动」，
  // 而不是把整页撑宽（`display: table` 自身不能滚动）。
  root.querySelectorAll<HTMLTableElement>('table').forEach((t) => {
    if (t.parentElement?.classList.contains('docs-table-scroll')) return
    const wrap = document.createElement('div')
    wrap.className = 'docs-table-scroll'
    t.parentNode?.insertBefore(wrap, t)
    wrap.appendChild(t)
  })
  root.querySelectorAll<HTMLAnchorElement>('a[href]').forEach((a) => {
    const href = a.getAttribute('href') || ''
    if (/^https?:\/\//i.test(href)) {
      a.setAttribute('target', '_blank')
      a.setAttribute('rel', 'noopener noreferrer')
    }
  })
}

// 正文由 MarkdownViewer 在自己挂载后异步写入（首帧要等 KaTeX 动态 import），
// 所以用 MutationObserver 在内容真正出现后再补 id，而不是猜 nextTick 次数。
let observer: MutationObserver | null = null
let decoratePending = false
function scheduleDecorate() {
  if (decoratePending) return
  decoratePending = true
  requestAnimationFrame(() => {
    decoratePending = false
    decorate()
  })
}

onMounted(() => {
  observer = new MutationObserver(scheduleDecorate)
  if (contentEl.value) observer.observe(contentEl.value, { childList: true, subtree: true })
})
onBeforeUnmount(() => {
  observer?.disconnect()
  observer = null
})

// contentEl 在 `v-else` 分支里，切换文档 / 切回正文时元素会被重建 → 重新挂观察
watch(contentEl, (el) => {
  observer?.disconnect()
  if (el && observer) {
    observer.observe(el, { childList: true, subtree: true })
    scheduleDecorate()
  }
})

function scrollToAnchor(id: string) {
  const el = document.getElementById(id)
  if (!el) return
  el.scrollIntoView({ behavior: 'smooth', block: 'start' })
}

// ── 站内链接拦截 ─────────────────────────────────────────
function onClick(e: MouseEvent) {
  const anchor = (e.target as HTMLElement | null)?.closest?.('a')
  if (!anchor) return
  const href = anchor.getAttribute('href') || ''
  if (!href) return

  // 页内锚点
  if (href.startsWith('#')) {
    e.preventDefault()
    scrollToAnchor(decodeURIComponent(href.slice(1)))
    return
  }

  // 站内 .md 互链：`./x.md`、`x.md`、`/assist/docs/x.md`
  const m = /^(?:\.\/|\/)?(?:assist\/)?(?:docs\/)?([\w.-]+)\.md(?:#(.*))?$/.exec(href)
  if (m) {
    e.preventDefault()
    router.push({ path: `/assist/docs/${m[1]}`, hash: m[2] ? `#${m[2]}` : '' })
  }
}

// ── 加载文档 ─────────────────────────────────────────────
async function loadDoc(s: string) {
  const doc = findDoc(s)
  if (!doc) {
    error.value = `找不到文档「${s}」，请从左侧目录选择。`
    raw.value = ''
    headings.value = []
    return
  }
  loading.value = true
  error.value = ''
  try {
    const text = await loadDocContent(doc.file)
    headings.value = extractHeadings(text)
    raw.value = text
    document.title = `${doc.title} - OKB-Assist`
  } catch (err) {
    error.value = `加载文档失败：${err instanceof Error ? err.message : String(err)}`
    raw.value = ''
    headings.value = []
  } finally {
    loading.value = false
  }

  await nextTick()
  const hash = route.hash ? decodeURIComponent(route.hash.slice(1)) : ''
  if (hash) {
    // 内容渲染是异步的，稍等再滚到锚点
    window.setTimeout(() => scrollToAnchor(hash), 80)
  } else {
    window.scrollTo({ top: 0 })
  }
}

watch(slug, (s) => void loadDoc(s), { immediate: true })
</script>

<style scoped>
.docs-view {
  padding-bottom: 32px;
}
.docs-head {
  display: flex;
  align-items: flex-start;
  justify-content: space-between;
  gap: 12px;
  flex-wrap: wrap;
  margin-bottom: 16px;
}
.docs-head h2 {
  font-size: 20px;
  font-weight: 600;
}
.docs-sub {
  margin-top: 4px;
  font-size: 13px;
  color: var(--text-secondary);
}
.docs-head-actions {
  display: flex;
  gap: 8px;
  flex-wrap: wrap;
}
.docs-layout {
  display: grid;
  grid-template-columns: 210px minmax(0, 1fr) 190px;
  gap: 20px;
  align-items: start;
}
/* 学习点：grid 项默认 min-width:auto，正文里的宽表格会把整页撑开 */
.docs-layout > * {
  min-width: 0;
}
.docs-nav {
  position: sticky;
  top: 68px; /* 顶部导航（sticky，56px）下方 */
  max-height: calc(100vh - 88px);
  overflow-y: auto;
  background: var(--bg-white);
  border: 1px solid var(--border);
  border-radius: 8px;
  padding: 10px;
}
.docs-nav-group + .docs-nav-group {
  margin-top: 12px;
}
.docs-nav-title {
  font-size: 12px;
  color: var(--text-muted);
  padding: 4px 6px;
}
.docs-nav-item {
  display: block;
  padding: 7px 8px;
  border-radius: 6px;
  color: var(--text);
  text-decoration: none;
  border-left: 3px solid transparent;
}
.docs-nav-item:hover {
  background: var(--bg);
}
.docs-nav-item.active {
  background: var(--primary-light);
  border-left-color: var(--primary);
  color: var(--primary-dark);
}
.docs-nav-label {
  display: block;
  font-size: 14px;
  font-weight: 500;
}
.docs-nav-desc {
  display: block;
  margin-top: 2px;
  font-size: 11px;
  line-height: 1.4;
  color: var(--text-muted);
}
.docs-nav-foot {
  margin-top: 14px;
  padding: 8px 6px 2px;
  border-top: 1px solid var(--border);
  font-size: 12px;
  color: var(--text-muted);
  line-height: 1.6;
}
.docs-main {
  background: var(--bg-white);
  border: 1px solid var(--border);
  border-radius: 8px;
  padding: 18px 22px 22px;
  min-height: 320px;
}
.docs-hint {
  color: var(--text-muted);
  font-size: 13px;
  padding: 24px 0;
  text-align: center;
}
.docs-hint.is-error {
  color: var(--danger, #c62828);
}
.docs-content :deep(.docs-table-scroll) {
  overflow-x: auto;
  margin: 12px 0;
}
.docs-content :deep(table) {
  /* 覆盖 MarkdownViewer 的 `display:block` + `main.scss` 的 `white-space:nowrap`：
     文档页优先「装得下、能换行」，而不是像读论文那样横向滚动 */
  display: table;
  width: 100%;
  table-layout: auto;
  border-collapse: collapse;
  font-size: 13px;
  margin: 0;
}
.docs-content :deep(th),
.docs-content :deep(td) {
  border: 1px solid var(--border);
  padding: 6px 10px;
  text-align: left;
  vertical-align: top;
  white-space: normal;
  /* 用 overflow-wrap 而不是 word-break：后者会把 `grep_search` 这种标识符
     的 min-content 算成 1 个字符，导致第一列被挤成竖排 */
  overflow-wrap: break-word;
}
.docs-content :deep(th) {
  background: var(--bg-table-header, var(--bg));
  font-weight: 500;
}
/* 表格里的行内代码（工具名、字段名）缩小一档：否则自动布局会把首列压到
   「一个字符的宽度」，`grep_search` 会被折断成 `grep_se / arch` */
.docs-content :deep(td code),
.docs-content :deep(th code) {
  font-size: 12px;
  padding: 1px 5px;
  white-space: nowrap;
}
.docs-content :deep(pre) {
  overflow-x: auto;
}
.docs-content :deep(code) {
  word-break: break-word;
}
.docs-content :deep(h1),
.docs-content :deep(h2),
.docs-content :deep(h3),
.docs-content :deep(h4) {
  /* 锚点跳转时给 sticky 顶部导航留出空间 */
  scroll-margin-top: 72px;
}
.docs-content :deep(h1) {
  font-size: 22px;
  margin-bottom: 12px;
}
.docs-content :deep(h2) {
  font-size: 17px;
  margin: 22px 0 8px;
  padding-top: 6px;
  border-top: 1px solid var(--border);
}
.docs-content :deep(h3) {
  font-size: 15px;
  margin: 16px 0 6px;
}
.docs-pager {
  display: flex;
  justify-content: space-between;
  gap: 8px;
  margin-top: 24px;
  padding-top: 14px;
  border-top: 1px solid var(--border);
}
.docs-toc {
  position: sticky;
  top: 68px;
  max-height: calc(100vh - 88px);
  overflow-y: auto;
  font-size: 12px;
  padding: 4px 0;
}
.docs-toc-title {
  font-size: 12px;
  color: var(--text-muted);
  margin-bottom: 6px;
  padding-left: 8px;
}
.docs-toc-item {
  display: block;
  width: 100%;
  text-align: left;
  border: none;
  background: transparent;
  color: var(--text-secondary);
  font-size: 12px;
  line-height: 1.5;
  padding: 4px 8px;
  border-left: 2px solid var(--border);
  cursor: pointer;
}
.docs-toc-item:hover {
  color: var(--primary);
  border-left-color: var(--primary);
}
.docs-toc-item.lv3 {
  padding-left: 18px;
  color: var(--text-muted);
}

@media (max-width: 1200px) {
  .docs-layout {
    grid-template-columns: 190px minmax(0, 1fr);
  }
  .docs-toc {
    display: none;
  }
}

@media (max-width: 768px) {
  .docs-layout {
    grid-template-columns: minmax(0, 1fr);
    gap: 12px;
  }
  .docs-nav {
    position: static;
    max-height: none;
  }
  .docs-main {
    padding: 14px 14px 18px;
  }
}
</style>
