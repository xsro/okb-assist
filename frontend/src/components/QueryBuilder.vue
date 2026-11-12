<template>
  <div v-if="visible" class="qb-overlay" @click.self="close">
    <div class="qb-modal">
      <div class="qb-header">
        <h3>🔧 高级搜索（Web of Science 风格）</h3>
        <button class="qb-close" @click="close" title="关闭">×</button>
      </div>

      <!-- 选项卡 -->
      <div class="qb-tabs">
        <button
          class="qb-tab"
          :class="{ active: activeTab === 'search' }"
          @click="activeTab = 'search'"
        >
          <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
            <circle cx="11" cy="11" r="8"/><line x1="21" y1="21" x2="16.65" y2="16.65"/>
          </svg>
          检索式
        </button>
        <button
          class="qb-tab"
          :class="{ active: activeTab === 'columns' }"
          @click="activeTab = 'columns'"
        >
          <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
            <rect x="3" y="3" width="18" height="18" rx="2" ry="2"/>
            <line x1="3" y1="9" x2="21" y2="9"/>
            <line x1="9" y1="3" x2="9" y2="21"/>
            <line x1="15" y1="3" x2="15" y2="21"/>
          </svg>
          列管理
        </button>
      </div>

      <!-- 检索式面板 -->
      <div v-show="activeTab === 'search'" class="qb-body">
        <!-- 检索式编辑框 -->
        <div class="qb-section">
          <label class="qb-label">检索式</label>
          <textarea
            ref="editorRef"
            v-model="draft"
            class="qb-editor"
            :class="{ 'is-invalid': !!error }"
            rows="4"
            spellcheck="false"
            placeholder='TS=(transformer OR "deep learning") AND PY=2020-2023'
            @keydown.ctrl.enter.prevent="apply"
            @keydown.meta.enter.prevent="apply"
          ></textarea>
          <div class="qb-status" :class="error ? 'is-error' : 'is-ok'">
            <template v-if="!draft.trim()">空检索式：应用后显示全部文献</template>
            <template v-else-if="error">
              ⚠ {{ error.message }}
              <span v-if="error.position !== undefined" class="qb-status-pos">
                （第 {{ error.position + 1 }} 个字符附近）
              </span>
            </template>
            <template v-else>
              ✓ 语法正确 · 共 {{ parsed.conditions.length }} 个条件
              <span v-if="parsed.conditions.length === 0" class="qb-status-pos">
                （只有排序指令）
              </span>
            </template>
          </div>
        </div>

        <!-- 字段标签 -->
        <div class="qb-section">
          <label class="qb-label">插入字段标签</label>
          <div class="qb-chips">
            <button
              v-for="f in WOS_FIELD_PALETTE"
              :key="f.tag"
              class="qb-ins"
              :title="`${f.label}：${f.hint}`"
              @click="insertText(`${f.tag}=`)"
            >{{ f.tag }}</button>
          </div>
        </div>

        <!-- 运算符 / 通配符 -->
        <div class="qb-section">
          <label class="qb-label">布尔运算符与通配符</label>
          <div class="qb-chips">
            <button
              v-for="op in WOS_OPERATORS"
              :key="op"
              class="qb-ins is-op"
              :title="op === 'AND' ? '并且' : op === 'OR' ? '或者' : '排除'"
              @click="insertText(op)"
            >{{ op }}</button>
            <button class="qb-ins" title="括号分组（优先级）" @click="insertText('( ')">(</button>
            <button class="qb-ins" title="括号分组（优先级）" @click="insertText(' )')">)</button>
            <button class="qb-ins is-wild" title="任意字符（0 或多个）" @click="insertText('*')">*</button>
            <button class="qb-ins is-wild" title="单个字符" @click="insertText('?')">?</button>
            <button class="qb-ins" title="给选中的文本包上引号（含空格的值必须加引号）" @click="quoteSelection">“ ”</button>
          </div>
        </div>

        <!-- 已识别条件 -->
        <div v-if="parsed.conditions.length" class="qb-section">
          <label class="qb-label">已识别条件</label>
          <div class="qb-cond-list">
            <span
              v-for="(c, i) in parsed.conditions"
              :key="i"
              class="qb-cond"
              :class="{ 'is-negated': c.negated }"
            >
              <b>{{ c.tag }}</b>: {{ c.value }}
            </span>
          </div>
        </div>

        <!-- 示例 -->
        <div class="qb-section">
          <label class="qb-label">示例</label>
          <div class="qb-examples">
            <button
              v-for="ex in WOS_EXAMPLES"
              :key="ex.query"
              class="qb-example"
              :title="ex.query"
              @click="draft = ex.query"
            >{{ ex.label }}</button>
          </div>
        </div>

        <!-- 语法说明 -->
        <div class="qb-section">
          <label class="qb-label">语法说明</label>
          <table class="qb-help">
            <tbody>
              <tr>
                <td class="qb-help-key">TAG=值</td>
                <td>字段检索；值含空格时加引号：<code>SO="IEEE TRANSACTIONS"</code></td>
              </tr>
              <tr>
                <td class="qb-help-key">AND / OR / NOT</td>
                <td>布尔运算，<code>AND</code> 优先级高于 <code>OR</code>；相邻条件默认 AND</td>
              </tr>
              <tr>
                <td class="qb-help-key">( )</td>
                <td>分组改变优先级：<code>TI=(a OR b) AND PY=2023</code></td>
              </tr>
              <tr>
                <td class="qb-help-key">* ? $</td>
                <td><code>*</code> 任意字符、<code>?</code> 单个字符、<code>$</code> 0/1 个字符；不含通配符时按「包含」匹配</td>
              </tr>
              <tr>
                <td class="qb-help-key">PY=2020-2023</td>
                <td>数值范围，也可写 <code>PY&gt;2020</code> / <code>PY&gt;=2020</code> / <code>PY&lt;=2023</code></td>
              </tr>
              <tr>
                <td class="qb-help-key">SORT=PY ORDER=desc</td>
                <td>排序（不参与筛选）；<code>SORT</code> 支持 PY / TI / AU / SO / DT / id / updated_at</td>
              </tr>
              <tr>
                <td class="qb-help-key">-条件</td>
                <td>旧写法否定：<code>-PY:2020</code> 等价 <code>NOT PY=2020</code></td>
              </tr>
            </tbody>
          </table>
          <p class="qb-hint">
            旧语法 <code>field:value</code>（如 <code>year:&gt;2020</code>、<code>type:article</code>）与
            <code>TAG=value</code> 等价，历史链接与 MCP 调用不受影响；<code>NEAR</code> / <code>SAME</code> 目前按 AND 处理。
          </p>
        </div>
      </div>

      <!-- 列管理面板 -->
      <div v-show="activeTab === 'columns'" class="qb-body qb-body-columns">
        <div class="qb-section">
          <label class="qb-label">表格显示列</label>
          <p class="qb-hint">勾选需要在文献列表中显示的列</p>
          <div class="qb-col-list">
            <div
              v-for="col in allColumns"
              :key="col.key"
              class="qb-col-item"
              :class="{ 'qb-col-disabled': col.always }"
            >
              <label>
                <input
                  type="checkbox"
                  :checked="visibleColumns[col.key]"
                  :disabled="col.always"
                  @change="toggleColumn(col.key)"
                />
                <span class="qb-col-label">{{ col.label }}</span>
                <span v-if="col.always" class="qb-col-always">始终显示</span>
              </label>
            </div>
          </div>
        </div>
      </div>

      <div class="qb-footer">
        <template v-if="activeTab === 'search'">
          <button class="btn btn-primary" :disabled="!!error" @click="apply">✅ 应用搜索</button>
          <button class="btn btn-outline" @click="close">取消</button>
        </template>
        <button v-else class="btn btn-primary" @click="close">✅ 完成</button>
      </div>
    </div>
  </div>
</template>

<script setup lang="ts">
import { ref, computed, nextTick } from 'vue'
import {
  parseWosQuery,
  WOS_FIELD_PALETTE,
  WOS_OPERATORS,
  WOS_EXAMPLES
} from '@/utils/wosQuery'

interface ColumnDef {
  key: string
  label: string
  always?: boolean
}

defineProps<{
  allColumns: ColumnDef[]
  visibleColumns: Record<string, boolean>
  toggleColumn: (key: string) => void
}>()

const activeTab = ref<'search' | 'columns'>('search')
const visible = ref(false)
/** 检索式草稿（唯一状态，不再维护一堆表单字段） */
const draft = ref('')
const editorRef = ref<HTMLTextAreaElement | null>(null)

/** 实时校验：解析结果 / 错误（后端仍会再校验一次） */
const parsed = computed(() => parseWosQuery(draft.value))
const error = computed(() => (parsed.value.ok ? undefined : parsed.value.error))

let resolveFn: ((query: string | null) => void) | null = null

/** 在光标处插入文本（必要时自动补空格） */
function insertText(text: string) {
  const el = editorRef.value
  if (!el) {
    draft.value += text
    return
  }
  const start = el.selectionStart ?? draft.value.length
  const end = el.selectionEnd ?? start
  const before = draft.value.slice(0, start)
  const selected = draft.value.slice(start, end)
  const after = draft.value.slice(end)

  const padBefore = before && !/[\s(]$/.test(before) ? ' ' : ''
  const padAfter = after && !/^[\s)]/.test(after) ? ' ' : ''
  // 有选中内容时不覆盖它：插入点放在选区之前
  draft.value = `${before}${padBefore}${text}${padAfter}${selected}${after}`

  const caret = (before + padBefore + text).length
  nextTick(() => {
    el.focus()
    el.setSelectionRange(caret, caret)
  })
}

/** 给选中的文本包上双引号（含空格的值必须加引号） */
function quoteSelection() {
  const el = editorRef.value
  if (!el) return
  const start = el.selectionStart ?? 0
  const end = el.selectionEnd ?? 0
  if (start === end) {
    insertText('""')
    return
  }
  const selected = draft.value.slice(start, end).replace(/^"|"$/g, '')
  const before = draft.value.slice(0, start)
  const after = draft.value.slice(end)
  draft.value = `${before}"${selected}"${after}`
  const caret = start + selected.length + 2
  nextTick(() => {
    el.focus()
    el.setSelectionRange(caret, caret)
  })
}

/** 打开构建器，可传入初始检索式。返回检索式或 null（取消时）。 */
function show(initialQuery = ''): Promise<string | null> {
  draft.value = (initialQuery || '').trim()
  activeTab.value = 'search'
  visible.value = true
  nextTick(() => {
    const el = editorRef.value
    if (!el) return
    el.focus()
    el.setSelectionRange(draft.value.length, draft.value.length)
  })
  return new Promise((resolve) => {
    resolveFn = resolve
  })
}

function close() {
  visible.value = false
  resolveFn?.(null)
}

function apply() {
  if (error.value) return
  visible.value = false
  resolveFn?.(draft.value.trim())
}

defineExpose({ show })
</script>

<style scoped>
/* ── 遮罩层 ──────────────────────────────────────────── */
.qb-overlay {
  position: fixed;
  inset: 0;
  z-index: 1000;
  background: rgba(0, 0, 0, 0.45);
  display: flex;
  align-items: center;
  justify-content: center;
  padding: 20px;
}

/* ── 模态框 ──────────────────────────────────────────── */
.qb-modal {
  background: var(--bg-white, #fff);
  border-radius: 12px;
  width: 100%;
  max-width: 680px;
  max-height: calc(100vh - 40px);
  display: flex;
  flex-direction: column;
  box-shadow: 0 16px 48px rgba(0, 0, 0, 0.2);
  overflow: hidden;
}

.qb-header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 16px 20px 12px;
  border-bottom: 1px solid var(--border, #e0e0e0);
}

.qb-header h3 {
  margin: 0;
  font-size: 16px;
  font-weight: 600;
}

.qb-close {
  background: none;
  border: none;
  font-size: 22px;
  color: var(--text-secondary, #666);
  cursor: pointer;
  padding: 0 4px;
  line-height: 1;
  border-radius: 4px;
}
.qb-close:hover {
  background: var(--bg, #f5f5f5);
  color: var(--text, #333);
}

/* ── 选项卡 ──────────────────────────────────────────── */
.qb-tabs {
  display: flex;
  border-bottom: 1px solid var(--border, #e0e0e0);
  padding: 0 20px;
  gap: 0;
}

.qb-tab {
  display: flex;
  align-items: center;
  gap: 6px;
  padding: 10px 16px;
  border: none;
  border-bottom: 2px solid transparent;
  background: transparent;
  font-size: 14px;
  font-family: inherit;
  color: var(--text-secondary, #666);
  cursor: pointer;
  transition: all 0.15s;
  margin-bottom: -1px;
  white-space: nowrap;
}
.qb-tab:hover {
  color: var(--text, #333);
  background: var(--bg, #f5f5f5);
}
.qb-tab.active {
  color: var(--primary, #1976d2);
  border-bottom-color: var(--primary, #1976d2);
  font-weight: 500;
}
.qb-tab svg {
  flex-shrink: 0;
}

/* ── 面板 ────────────────────────────────────────────── */
.qb-body {
  padding: 16px 20px;
  overflow-y: auto;
  flex: 1;
  display: flex;
  flex-direction: column;
  gap: 14px;
}

.qb-body-columns {
  min-height: 200px;
}

.qb-section {
  display: flex;
  flex-direction: column;
  gap: 6px;
}

.qb-label {
  font-size: 12px;
  font-weight: 600;
  color: var(--text-secondary, #666);
  text-transform: uppercase;
  letter-spacing: 0.3px;
}

.qb-hint {
  font-size: 12px;
  color: var(--text-secondary, #666);
  margin: 0;
  line-height: 1.6;
}
.qb-hint code,
.qb-help code {
  padding: 1px 4px;
  border-radius: 3px;
  background: var(--bg, #f5f5f5);
  font-size: 12px;
}

/* ── 检索式编辑框 ────────────────────────────────────── */
.qb-editor {
  width: 100%;
  box-sizing: border-box;
  padding: 10px 12px;
  border: 1px solid var(--border, #e0e0e0);
  border-radius: 6px;
  font-family: 'SF Mono', 'Fira Code', 'Cascadia Code', Consolas, monospace;
  font-size: 13px;
  line-height: 1.6;
  color: var(--text, #333);
  background: var(--bg-white, #fff);
  resize: vertical;
  outline: none;
  transition: border-color 0.15s;
}
.qb-editor:focus {
  border-color: var(--primary, #1976d2);
  box-shadow: 0 0 0 2px rgba(25, 118, 210, 0.1);
}
.qb-editor.is-invalid {
  border-color: var(--danger, #f44336);
}
.qb-editor.is-invalid:focus {
  box-shadow: 0 0 0 2px rgba(244, 67, 54, 0.12);
}

.qb-status {
  font-size: 12px;
  line-height: 1.6;
}
.qb-status.is-ok {
  color: #2e7d32;
}
.qb-status.is-error {
  color: #c62828;
}
.qb-status-pos {
  color: var(--text-secondary, #666);
}

/* ── 标签 / 运算符按钮 ───────────────────────────────── */
.qb-chips {
  display: flex;
  flex-wrap: wrap;
  gap: 6px;
}

.qb-ins {
  padding: 4px 10px;
  border: 1px solid var(--border, #e0e0e0);
  border-radius: 6px;
  background: var(--bg-white, #fff);
  color: var(--text, #333);
  font-family: 'SF Mono', 'Fira Code', Consolas, monospace;
  font-size: 12px;
  cursor: pointer;
  transition: all 0.12s;
  min-height: 28px;
}
.qb-ins:hover {
  border-color: var(--primary, #1976d2);
  color: var(--primary, #1976d2);
  background: var(--primary-light, #e3f2fd);
}
.qb-ins.is-op {
  color: #e65100;
  font-weight: 700;
}
.qb-ins.is-wild {
  color: #b8860b;
  font-weight: 700;
}

/* ── 已识别条件 ──────────────────────────────────────── */
.qb-cond-list {
  display: flex;
  flex-wrap: wrap;
  gap: 6px;
}
.qb-cond {
  padding: 3px 9px;
  border-radius: 12px;
  background: var(--primary-light, #e3f2fd);
  border: 1px solid var(--primary, #1976d2);
  color: var(--primary, #1976d2);
  font-size: 12px;
  word-break: break-all;
}
.qb-cond.is-negated {
  background: rgba(244, 67, 54, 0.08);
  border-color: var(--danger, #f44336);
  color: #c62828;
}

/* ── 示例 ────────────────────────────────────────────── */
.qb-examples {
  display: flex;
  flex-wrap: wrap;
  gap: 6px;
}
.qb-example {
  padding: 4px 10px;
  border: 1px dashed var(--border, #e0e0e0);
  border-radius: 6px;
  background: transparent;
  color: var(--text-secondary, #666);
  font-size: 12px;
  cursor: pointer;
}
.qb-example:hover {
  border-color: var(--primary, #1976d2);
  color: var(--primary, #1976d2);
}

/* ── 语法说明 ────────────────────────────────────────── */
.qb-help {
  width: 100%;
  border-collapse: collapse;
  font-size: 12px;
}
.qb-help td {
  padding: 5px 8px;
  border-bottom: 1px solid var(--border, #f0f0f0);
  vertical-align: top;
  color: var(--text-secondary, #666);
  line-height: 1.6;
}
.qb-help-key {
  white-space: nowrap;
  font-family: 'SF Mono', 'Fira Code', Consolas, monospace;
  color: var(--text, #333) !important;
  font-weight: 600;
  width: 1%;
}

/* ── 列管理 ──────────────────────────────────────────── */
.qb-col-list {
  display: flex;
  flex-direction: column;
  gap: 2px;
}

.qb-col-item {
  padding: 4px 0;
}
.qb-col-item label {
  display: flex;
  align-items: center;
  gap: 8px;
  font-size: 14px;
  cursor: pointer;
  padding: 6px 10px;
  border-radius: 6px;
  color: var(--text, #333);
  transition: background 0.12s;
  user-select: none;
}
.qb-col-item label:hover {
  background: var(--bg, #f5f5f5);
}
.qb-col-item input[type="checkbox"] {
  accent-color: var(--primary, #1976d2);
  cursor: pointer;
}

.qb-col-disabled {
  opacity: 0.5;
  pointer-events: none;
}

.qb-col-label {
  flex: 1;
}

.qb-col-always {
  font-size: 11px;
  color: var(--text-tertiary, #999);
  font-style: italic;
}

/* ── 按钮栏 ──────────────────────────────────────────── */
.qb-footer {
  display: flex;
  align-items: center;
  justify-content: flex-end;
  gap: 8px;
  padding: 12px 20px 16px;
  border-top: 1px solid var(--border, #e0e0e0);
}

/* ── 响应式 ──────────────────────────────────────────── */
@media (max-width: 480px) {
  .qb-modal {
    max-height: calc(100vh - 20px);
  }
  .qb-overlay {
    padding: 10px;
  }
  .qb-help-key {
    white-space: normal;
  }
}
</style>
