import katex from 'katex'
import 'katex/dist/katex.min.css'

export type MathMode = 'none' | 'katex' | 'mathjax'

interface MathMatch {
  id: string
  latex: string
  displayMode: boolean
}

/** 提取 markdown 中的数学公式，用占位符替换 */
function extractMath(content: string): { text: string; matches: MathMatch[] } {
  const matches: MathMatch[] = []
  let counter = 0

  // 块级公式 $$...$$
  const blockReplaced = content.replace(
    /\$\$([\s\S]*?)\$\$/g,
    (_match, latex: string) => {
      const id = `__MATH_BLOCK_${counter++}__`
      matches.push({ id, latex: latex.trim(), displayMode: true })
      return id
    }
  )

  // 行内公式 $...$（避开已替换的占位符和 LaTeX 命令中的 $）
  const inlineReplaced = blockReplaced.replace(
    /(?<!\\)\$(?!\$)([^\n\$]+?)(?<!\\)\$(?!\$)/g,
    (_match, latex: string) => {
      const id = `__MATH_INLINE_${counter++}__`
      matches.push({ id, latex: latex.trim(), displayMode: false })
      return id
    }
  )

  return { text: inlineReplaced, matches }
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

/** 保持原样，不渲染 */
function insertMathNone(text: string, matches: MathMatch[]): string {
  let result = text
  for (const { id, latex, displayMode } of matches) {
    const raw = displayMode ? `$$${latex}$$` : `$${latex}$`
    result = result.replace(id, raw)
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
