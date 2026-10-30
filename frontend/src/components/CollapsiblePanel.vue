<template>
  <div class="collapsible-panel" :class="{ 'is-open': open }">
    <button
      type="button"
      class="panel-header"
      :aria-expanded="open"
      @click="toggle"
    >
      <span class="panel-chevron" :class="{ 'is-open': open }">▶</span>
      <span class="panel-title">{{ title }}</span>
      <span v-if="badge" class="panel-badge">{{ badge }}</span>
      <span v-if="hint" class="panel-hint">{{ hint }}</span>
    </button>
    <div v-if="open" class="panel-body">
      <div v-if="loading" class="panel-state">加载中…</div>
      <div v-else-if="error" class="panel-state is-error">{{ error }}</div>
      <slot v-else />
    </div>
  </div>
</template>

<script setup lang="ts">
/**
 * 通用折叠面板：默认收起，首次展开时触发 `open` 事件（供懒加载使用）。
 */
import { ref } from 'vue'

const props = withDefaults(
  defineProps<{
    title: string
    /** 标题右侧的短标签（如“页数 17”“已保存”） */
    badge?: string
    /** 标题右侧的次要说明 */
    hint?: string
    /** 初始是否展开 */
    defaultOpen?: boolean
    loading?: boolean
    error?: string
  }>(),
  {
    badge: '',
    hint: '',
    defaultOpen: false,
    loading: false,
    error: ''
  }
)

const emit = defineEmits<{
  (e: 'open'): void
}>()

const open = ref(props.defaultOpen)
const hasEmitted = ref(false)

function toggle() {
  open.value = !open.value
  if (open.value && !hasEmitted.value) {
    hasEmitted.value = true
    emit('open')
  }
}
</script>

<style scoped>
.collapsible-panel {
  border: 1px solid var(--border);
  border-radius: 6px;
  background: var(--bg-white);
  overflow: hidden;
}

.collapsible-panel + .collapsible-panel {
  margin-top: 10px;
}

.panel-header {
  display: flex;
  align-items: center;
  gap: 8px;
  width: 100%;
  padding: 12px 16px;
  border: none;
  background: var(--bg-table-header);
  color: var(--text);
  font-size: 14px;
  font-weight: 600;
  text-align: left;
  cursor: pointer;
  min-height: var(--touch-min);
}

.panel-header:hover {
  background: var(--bg-hover);
}

.panel-chevron {
  display: inline-block;
  font-size: 10px;
  color: var(--text-muted);
  transition: transform 0.15s;
}

.panel-chevron.is-open {
  transform: rotate(90deg);
}

.panel-title {
  flex-shrink: 0;
}

.panel-badge {
  padding: 1px 8px;
  border-radius: 10px;
  background: var(--primary-light);
  color: var(--primary-dark);
  font-size: 12px;
  font-weight: 500;
  white-space: nowrap;
}

.panel-hint {
  color: var(--text-muted);
  font-size: 12px;
  font-weight: 400;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.panel-body {
  padding: 14px 16px;
  border-top: 1px solid var(--border);
}

.panel-state {
  color: var(--text-secondary);
  font-size: 13px;
  padding: 4px 0;
}

.panel-state.is-error {
  color: var(--danger-text);
}

@media (max-width: 768px) {
  .panel-header {
    padding: 12px;
  }

  .panel-body {
    padding: 12px;
  }
}
</style>
