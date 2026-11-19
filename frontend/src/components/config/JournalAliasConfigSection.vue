<template>
  <div class="section">
    <h3>期刊别名</h3>

    <p class="hint">
      把常用缩写（<code>TRO</code>、<code>TAC</code>…）映射到一组期刊全名：检索时写
      <code>SO=TRO</code> 或 <code>journal=TRO</code> 就能一次命中这些期刊在库里的所有写法（大小写不敏感，
      如 <code>IEEE Transactions on Robotics</code> 与 <code>IEEE TRANSACTIONS ON ROBOTICS</code>）。
      一个别名可以写<b>多个</b>期刊名（每行一个），用于旧刊名或另一种拼写；匹配按<b>期刊全名精确</b>进行，
      不做片段模糊。别名表只影响检索过滤，不会修改库里已存的期刊名。
    </p>

    <div class="alias-table">
      <div class="alias-head">
        <span>别名</span>
        <span>对应的期刊名（每行一个）</span>
        <span></span>
      </div>

      <div
        v-for="(item, i) in items"
        :key="i"
        class="alias-row"
        :class="{ invalid: rowProblem(i) }"
      >
        <input
          v-model="item.alias"
          class="alias-input"
          placeholder="TRO"
          spellcheck="false"
          @input="touch"
        />
        <textarea
          class="journal-input"
          rows="2"
          spellcheck="false"
          placeholder="IEEE Transactions on Robotics"
          :value="item.journals.join('\n')"
          @input="onJournalsInput(i, $event)"
        ></textarea>
        <button class="btn btn-outline btn-sm" title="删除该别名" @click="remove(i)">删除</button>
      </div>

      <p v-if="!items.length" class="empty">还没有别名，点下面的「新增别名」开始。</p>
    </div>

    <div class="alias-actions">
      <button class="btn btn-outline btn-sm" @click="add">+ 新增别名</button>
      <span class="counter">{{ items.length }} 条</span>
    </div>

    <ul v-if="problems.length" class="problems">
      <li v-for="(p, i) in problems" :key="i">⚠ {{ p }}</li>
    </ul>

    <p class="hint">
      保存后立即生效（写进 <code>config.json</code> 的 <code>journal_aliases</code> 段）。别名重复、
      别名为空、或某个别名一个期刊名都没写时，保存会被服务端拒绝并给出提示。
    </p>
  </div>
</template>

<script setup lang="ts">
import { computed } from 'vue'
import type { JournalAliasItem, ServiceConfig } from '@/types/config'

const props = defineProps<{
  modelValue: ServiceConfig
}>()

const emit = defineEmits<{
  (e: 'update:modelValue', val: ServiceConfig): void
}>()

/** 确保 `journal_aliases.items` 存在（老配置 / 首次加载） */
const items = computed<JournalAliasItem[]>(() => {
  if (!props.modelValue.journal_aliases) {
    props.modelValue.journal_aliases = { version: 1, items: [] }
  }
  const sec = props.modelValue.journal_aliases
  if (!Array.isArray(sec.items)) {
    sec.items = []
  }
  return sec.items
})

function commit() {
  emit('update:modelValue', props.modelValue)
}

function touch() {
  commit()
}

function add() {
  items.value.push({ alias: '', journals: [] })
  commit()
}

function remove(i: number) {
  items.value.splice(i, 1)
  commit()
}

/** 期刊名一行一个：去首尾空白、丢空行、忽略大小写去重 */
function onJournalsInput(i: number, ev: Event) {
  const raw = (ev.target as HTMLTextAreaElement).value
  const seen: string[] = []
  const journals: string[] = []
  for (const line of raw.split('\n')) {
    const name = line.trim().replace(/\s+/g, ' ')
    if (!name) continue
    const key = name.toLowerCase()
    if (seen.includes(key)) continue
    seen.push(key)
    journals.push(name)
  }
  const item = items.value[i]
  if (item) {
    item.journals = journals
    commit()
  }
  // 让 textarea 立刻反映规范化后的内容（例如删掉空行）
  ;(ev.target as HTMLTextAreaElement).value = journals.join('\n')
}

/** 与服务端 `services::journal_alias` 一致的归一化：折叠空白 + 小写 */
function normalizeKey(s: string): string {
  return s.split(/\s+/).filter(Boolean).join(' ').toLowerCase()
}

/** 单行问题（返回空串表示没问题） */
function rowProblem(i: number): string {
  const item = items.value[i]
  if (!item) return ''
  const alias = (item.alias || '').trim()
  if (!alias) return '别名为空'
  if (alias.length > 64) return '别名超过 64 个字符'
  if (!item.journals.length) return '至少写一个期刊名'
  const dup = items.value.some(
    (other, j) => j < i && normalizeKey(other.alias || '') === normalizeKey(alias)
  )
  if (dup) return '别名重复'
  for (const name of item.journals) {
    if (name.length > 200) return '期刊名超过 200 个字符'
  }
  return ''
}

const problems = computed<string[]>(() => {
  const out: string[] = []
  items.value.forEach((item, i) => {
    const p = rowProblem(i)
    if (p) out.push(`第 ${i + 1} 条（${(item.alias || '').trim() || '空'}）：${p}`)
  })
  return out
})

defineExpose({ problems })
</script>

<style scoped>
.alias-table {
  margin-top: 10px;
  border: 1px solid var(--border-color);
  border-radius: 8px;
  overflow: hidden;
}

.alias-head,
.alias-row {
  display: grid;
  grid-template-columns: 160px minmax(0, 1fr) 72px;
  gap: 10px;
  padding: 8px 10px;
  align-items: start;
}

.alias-head {
  background: var(--bg-tertiary, #f4f5f7);
  font-size: 12px;
  font-weight: 600;
  color: var(--text-secondary);
}

.alias-row + .alias-row {
  border-top: 1px solid var(--border-color);
}

.alias-row.invalid {
  background: rgba(229, 62, 62, 0.06);
}

.alias-input,
.journal-input {
  width: 100%;
  min-width: 0;
  font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
  font-size: 13px;
  padding: 6px 8px;
  border: 1px solid var(--border-color);
  border-radius: 6px;
  background: var(--bg-primary, #fff);
  color: inherit;
}

.alias-input:focus,
.journal-input:focus {
  outline: none;
  border-color: var(--primary-color);
}

.journal-input {
  resize: vertical;
  line-height: 1.45;
}

.empty {
  margin: 0;
  padding: 14px 10px;
  font-size: 13px;
  color: var(--text-secondary);
}

.alias-actions {
  display: flex;
  align-items: center;
  gap: 10px;
  margin-top: 10px;
}

.counter {
  font-size: 12px;
  color: var(--text-secondary);
}

.problems {
  margin: 8px 0 0;
  padding-left: 18px;
  font-size: 12px;
  line-height: 1.6;
  color: #c53030;
}

.hint {
  font-size: 12px;
  color: var(--text-secondary);
  margin: 8px 0 0;
  line-height: 1.6;
}

.hint code {
  padding: 1px 4px;
  border-radius: 4px;
  background: var(--bg-tertiary, #f4f5f7);
  font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
}

.btn-sm {
  padding: 4px 10px;
  font-size: 12px;
}

@media (max-width: 768px) {
  .alias-head {
    display: none;
  }

  .alias-row {
    grid-template-columns: minmax(0, 1fr);
    gap: 6px;
  }
}
</style>
