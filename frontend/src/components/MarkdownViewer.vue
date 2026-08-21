<template>
  <div class="markdown-viewer" v-html="rendered"></div>
</template>

<script setup lang="ts">
import { computed, watch } from 'vue'
import { marked, Renderer } from 'marked'
import DOMPurify from 'dompurify'
import { renderMath, loadMathJax, type MathMode } from '@/utils/mathRenderer'

const props = defineProps<{
  content: string
  mathMode?: MathMode
  loadImages?: boolean
}>()

const rendered = computed(() => {
  const mode = props.mathMode || 'katex'
  const loadImages = props.loadImages ?? true

  // 1. 先渲染数学公式（提取公式 → 渲染 markdown → 插回公式 HTML）
  const withMath = renderMath(props.content, mode)

  // 2. 自定义图片渲染器：关闭时不加载图片，用占位符显示
  const renderer=new Renderer();
  const old_image_renderer=renderer.image;
  renderer.image=e=>{
      if (loadImages) {
        return old_image_renderer.call(renderer, e)
      }
      const {title, text}=e;
      const label = text || title || '图片'
      return `<span class="image-placeholder">[${label}]</span>`
    }

  // 3. marked 解析 markdown
  const raw = marked.parse(withMath, {
    async: false,
    gfm: true,
    breaks: true,
    renderer
  }) as string

  // 4. DOMPurify 净化，同时保留 KaTeX/MathJax 所需标签/属性
  return DOMPurify.sanitize(raw, {
    ADD_TAGS: ['math', 'mrow', 'mi', 'mo', 'mn', 'msup', 'msub', 'mfrac', 'svg', 'path'],
    ADD_ATTR: ['xmlns', 'viewBox', 'preserveAspectRatio', 'stroke-linecap', 'stroke-linejoin', 'stroke-width', 'fill', 'd', 'aria-hidden', 'display']
  })
})

// MathJax 模式需要动态加载脚本并在内容更新后触发排版
watch(
  () => [props.content, props.mathMode, props.loadImages],
  async () => {
    if (props.mathMode !== 'mathjax') return
    await loadMathJax()
    const mj = (window as any).MathJax
    if (mj && mj.typesetPromise) {
      mj.typesetPromise()
    }
  },
  { immediate: true }
)
</script>

<style scoped>
.image-placeholder {
  display: inline-block;
  padding: 4px 8px;
  background: var(--bg);
  border: 1px dashed var(--border);
  border-radius: 4px;
  color: var(--text-secondary);
  font-size: 13px;
  line-height: 1.4;
}
</style>
