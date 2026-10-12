<template>
  <div class="formula-editor-view">
    <!-- 顶部控制栏 -->
    <div class="formula-controls">
      <!-- 渲染引擎选择 -->
      <div class="engine-selector">
        <label class="engine-radio">
          <input type="radio" v-model="activeEngine" value="katex" />
          <span class="engine-label">KaTeX 渲染</span>
        </label>
        <label class="engine-radio">
          <input type="radio" v-model="activeEngine" value="mathjax" />
          <span class="engine-label">MathJax 渲染</span>
        </label>
      </div>

      <!-- 公式模式切换 -->
      <div class="mode-selector">
        <button
          class="mode-btn"
          :class="{ active: formulaMode === 'inline' }"
          @click="formulaMode = 'inline'"
        >
          行内模式
        </button>
        <button
          class="mode-btn"
          :class="{ active: formulaMode === 'display' }"
          @click="formulaMode = 'display'"
        >
          行间模式
        </button>
      </div>

      <button class="clear-btn" @click="clearContent" title="清空">✕ 清空</button>
    </div>

    <!-- 上半：编辑器 -->
    <div class="formula-editor">
      <AceEditor
        v-model="formulaContent"
        mode="text"
        theme="monokai"
        :showToolbar="false"
        :showPreviewControls="false"
      />
    </div>

    <!-- 下半：实时渲染 -->
    <div class="formula-preview">
      <MarkdownViewer
        :content="previewContent"
        :mathMode="activeEngine"
        :loadImages="false"
      />
    </div>
  </div>
</template>

<script setup lang="ts">
import { ref, computed } from 'vue'
import AceEditor from '@/components/AceEditor.vue'
import MarkdownViewer from '@/components/MarkdownViewer.vue'
import type { MathMode } from '@/utils/mathRenderer'

const activeEngine = ref<'katex' | 'mathjax'>('katex')
const formulaMode = ref<'inline' | 'display'>('display')
const formulaContent = ref(`\\sum_{i=1}^{n} i = \\frac{n(n+1)}{2}

\\int_{0}^{\\infty} e^{-x^2} dx = \\frac{\\sqrt{\\pi}}{2}

E = mc^2
`)

/** 根据模式包裹公式分隔符 */
const previewContent = computed(() => {
  const text = formulaContent.value.trim()
  if (!text) return ''

  // 按空行分割为多个公式，每个分别包裹
  const blocks = text.split(/\n\s*\n/).filter(b => b.trim())
  if (blocks.length === 0) return ''

  const wrapped = blocks.map(b => b.trim()).join('\n\n')
  if (formulaMode.value === 'inline') {
    return wrapped.split('\n\n').map(b => `$${b}$`).join('\n\n')
  } else {
    return wrapped.split('\n\n').map(b => `$$${b}$$`).join('\n\n')
  }
})

function clearContent() {
  formulaContent.value = ''
}
</script>

<style scoped>
.formula-editor-view {
  display: flex;
  flex-direction: column;
  height: calc(100vh - 140px);
  gap: 10px;
  padding: 16px;
}

/* ── 顶部控制栏 ─────────────────────────────────── */
.formula-controls {
  display: flex;
  align-items: center;
  gap: 16px;
  flex-wrap: wrap;
  flex-shrink: 0;
  padding: 8px 0;
}

.engine-selector {
  display: flex;
  gap: 20px;
}

.engine-radio {
  display: flex;
  align-items: center;
  gap: 6px;
  cursor: pointer;
  font-size: 14px;
  color: var(--text-secondary);
  user-select: none;
}

.engine-radio input[type="radio"] {
  appearance: none;
  width: 16px;
  height: 16px;
  border: 2px solid var(--border);
  border-radius: 50%;
  cursor: pointer;
  position: relative;
  outline: none;
  transition: all 0.15s;
}

.engine-radio input[type="radio"]::after {
  content: "";
  position: absolute;
  top: 50%;
  left: 50%;
  width: 8px;
  height: 8px;
  background: var(--primary);
  border-radius: 50%;
  transform: translate(-50%, -50%) scale(0);
  transition: transform 0.15s;
}

.engine-radio input[type="radio"]:checked {
  border-color: var(--primary);
}

.engine-radio input[type="radio"]:checked::after {
  transform: translate(-50%, -50%) scale(1);
}

.engine-radio:has(input:checked) .engine-label {
  color: var(--primary);
  font-weight: 600;
}

/* ── 公式模式切换 ───────────────────────────────── */
.mode-selector {
  display: flex;
  border: 1px solid var(--border);
  border-radius: 6px;
  overflow: hidden;
}

.mode-btn {
  padding: 8px 18px;
  border: none;
  background: transparent;
  cursor: pointer;
  font-size: 14px;
  color: var(--text-secondary);
  transition: all 0.15s;
}

.mode-btn.active {
  background: var(--primary);
  color: #fff;
}

.mode-btn:not(.active):hover {
  background: var(--bg-hover);
}

.clear-btn {
  margin-left: auto;
  padding: 8px 16px;
  border: 1px solid var(--border);
  border-radius: 6px;
  background: transparent;
  cursor: pointer;
  font-size: 14px;
  color: var(--text-secondary);
  transition: all 0.15s;
}

.clear-btn:hover {
  background: var(--danger-bg);
  color: var(--danger);
}

/* ── 编辑器 / 预览分区 ──────────────────────────── */
.formula-editor {
  flex: 1;
  min-height: 0;
  overflow: hidden;
}

.formula-preview {
  flex: 1;
  min-height: 0;
  overflow: hidden;
  border: 1px solid var(--border);
  border-radius: 8px;
  background: #fff;
  padding: 20px;
}

/* ── 移动端响应式 ──────────────────────────────────── */
@media (max-width: 768px) {
  .formula-editor-view {
    height: calc(100vh - 120px);
    padding: 10px;
    gap: 8px;
  }

  .formula-controls {
    gap: 10px;
    padding: 6px 0;
    flex-direction: column;
    align-items: stretch;
  }

  .engine-selector {
    gap: 16px;
    justify-content: center;
  }

  .engine-radio {
    font-size: 13px;
  }

  .mode-btn {
    padding: 8px 14px;
    font-size: 13px;
  }

  .clear-btn {
    margin-left: 0;
    width: 100%;
  }
}

@media (max-width: 480px) {
  .formula-editor-view {
    height: calc(100vh - 100px);
    padding: 8px;
    gap: 6px;
  }

  .formula-controls {
    gap: 8px;
  }

  .engine-radio {
    font-size: 12px;
    gap: 4px;
  }

  .engine-radio input[type="radio"] {
    width: 14px;
    height: 14px;
  }

  .mode-btn {
    padding: 7px 12px;
    font-size: 12px;
  }

  .formula-preview {
    padding: 12px;
  }
}
</style>