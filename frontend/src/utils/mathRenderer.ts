import katex from 'katex'
import 'katex/dist/katex.min.css'

export type MathMode = 'none' | 'katex' | 'mathjax'

export interface MathMatch {
  id: string
  latex: string
  displayMode: boolean
  /** 被占位符折叠掉的换行数（多行块级公式会跨多行），用于把行号还原回原始 md 文件 */
  lineSpan?: number
  /**
   * 公式起始行在传入内容中的行号（0-based）。块级公式可能跨多行，
   * 结束行 = line + lineSpan，供「公式渲染失败 → 去编辑这段源码」定位。
   */
  line?: number
}

/** 公式匹配：块级 `$$…$$` 优先于行内 `$…$`（与原来的两段替换语义一致） */
const MATH_RE = /\$\$([\s\S]*?)\$\$|(?<!\\)\$(?!\$)([^\n$]+?)(?<!\\)\$(?!\$)/g

/** 统计 content 中 offset 之前的换行数（回调按 offset 递增调用，整体线性） */
function makeLineCounter(content: string): (offset: number) => number {
  let scanned = 0
  let line = 0
  return (offset: number) => {
    while (scanned < offset) {
      if (content.charCodeAt(scanned) === 10) line++
      scanned++
    }
    return line
  }
}

/** 统一提取公式（单次扫描，同时记录每个公式的起始行） */
function replaceMath(
  content: string,
  makeId: (displayMode: boolean, index: number) => string,
  startIndex = 0
): { text: string; matches: MathMatch[] } {
  const matches: MathMatch[] = []
  let counter = startIndex
  const lineAt = makeLineCounter(content)

  const text = content.replace(
    MATH_RE,
    (
      _whole: string,
      blockLatex: string | undefined,
      inlineLatex: string | undefined,
      offset: number
    ) => {
      const displayMode = blockLatex !== undefined
      const latex = (displayMode ? blockLatex : inlineLatex) ?? ''
      const id = makeId(displayMode, counter++)
      matches.push({
        id,
        latex: latex.trim(),
        displayMode,
        lineSpan: displayMode ? (latex.match(/\n/g) || []).length : 0,
        line: lineAt(offset),
      })
      return id
    }
  )

  return { text, matches }
}

/** 提取 markdown 中的数学公式，用占位符替换 */
function extractMath(content: string): { text: string; matches: MathMatch[] } {
  return replaceMath(
    content,
    (displayMode, index) => (displayMode ? `__MATH_BLOCK_${index}__` : `__MATH_INLINE_${index}__`)
  )
}

/** 将 KaTeX 渲染后的 HTML 插回 markdown */
function insertMathKatex(text: string, matches: MathMatch[]): string {
  let result = text
  for (const { id, latex, displayMode } of matches) {
    try {
      const html = katex.renderToString(latex, {
        throwOnError: false,
        displayMode
      })
      result = result.replace(id, html)
    } catch {
      result = result.replace(id, displayMode ? `$$${latex}$$` : `$${latex}$`)
    }
  }
  return result
}

/** 为 MathJax 生成可识别的包裹标签 */
function insertMathJax(text: string, matches: MathMatch[]): string {
  let result = text
  for (const { id, latex, displayMode } of matches) {
    const escaped = latex.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;')
    const tag = displayMode
      ? `<div class="mathjax-block">\\[${escaped}\\]</div>`
      : `<span class="mathjax-inline">\\(${escaped}\\)</span>`
    result = result.replace(id, tag)
  }
  return result
}

/** 公式以代码框显示（none 模式） */
function insertMathNone(text: string, matches: MathMatch[]): string {
  let result = text
  for (const { id, latex, displayMode } of matches) {
    const escaped = latex.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;')
    const html = displayMode
      ? `<pre class="math-codeblock"><code>$$${escaped}$$</code></pre>`
      : `<code class="math-inline-code">$${escaped}$</code>`
    result = result.replace(id, html)
  }
  return result
}

/**
 * 根据模式渲染 markdown 中的数学公式。
 * 先提取公式、再用 marked 渲染 markdown，最后插回公式 HTML。
 */
export function renderMath(content: string, mode: MathMode): string {
  const { text, matches } = extractMath(content)

  switch (mode) {
    case 'katex':
      return insertMathKatex(text, matches)
    case 'mathjax':
      return insertMathJax(text, matches)
    case 'none':
    default:
      return insertMathNone(text, matches)
  }
}

/**
 * 提取公式并生成含占位符的文本。
 * 使用 @@ 作为占位符定界符（避免 marked 解析 __ 为强调语法）。
 * 返回的 text 中包含 @@MATH_BLOCK_N@@ / @@MATH_INLINE_N@@ 占位符，
 * matches 数组中保存每个公式的 id、LaTeX 源码和展示模式。
 */
export function extractMathPlaceholders(content: string): { text: string; matches: MathMatch[] } {
  // 使用 @@ 作为占位符定界符（避免 marked 解析 __ 为强调语法）。
  // 返回的 text 中包含 @@MATH_BLOCK_N@@ / @@MATH_INLINE_N@@ 占位符，
  // matches 数组中保存每个公式的 id、LaTeX 源码、展示模式与起始行号。
  return replaceMath(
    content,
    (displayMode, index) =>
      displayMode ? `@@MATH_BLOCK_${index}@@` : `@@MATH_INLINE_${index}@@`
  )
}

/** 动态加载 MathJax CDN */
export function loadMathJax(): Promise<void> {
  if ((window as any).MathJax) {
    return Promise.resolve()
  }

  return new Promise((resolve) => {
    const script = document.createElement('script')
    script.src =
      'https://cdn.jsdelivr.net/npm/mathjax@3.2.2/es5/tex-svg.min.js'
    script.async = true
    script.onload = () => {
      ;(window as any).MathJax = {
        tex: {
          inlineMath: [['\\(', '\\)']],
          displayMath: [['\\[', '\\]']]
        },
        svg: { fontCache: 'global' },
        startup: {
          pageReady: () => resolve()
        }
      }
      resolve()
    }
    script.onerror = () => resolve()
    document.head.appendChild(script)
  })
}
