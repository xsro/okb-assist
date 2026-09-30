<template>
  <span
    class="badge"
    :class="[statusClass, { 'has-error': props.status === 'error' && props.status_message }]"
    :title="props.status === 'error' && props.status_message ? props.status_message : undefined"
  >
    {{ statusLabel }}
    <span v-if="props.status === 'error' && props.status_message" class="error-icon" title="">⚠</span>
  </span>
</template>

<script setup lang="ts">
import { computed } from 'vue'

const props = defineProps<{
  status: string
  status_message?: string | null
}>()

const statusMap: Record<string, { label: string; class: string }> = {
  // 文档状态
  uploaded: { label: '已上传', class: 'badge-gray' },
  parsing: { label: '解析中', class: 'badge-warning pulse' },
  markdown_done: { label: '解析完成', class: 'badge-info' },
  error: { label: '错误', class: 'badge-error' },
  // 索引状态
  not_indexed: { label: '未索引', class: 'badge-gray' },
  // 流水线阶段
  parse: { label: '解析', class: 'badge-info' },
  extract: { label: '提取', class: 'badge-info' },
  index: { label: '索引', class: 'badge-info' },
  // 批次状态
  running: { label: '运行中', class: 'badge-info pulse' },
  paused: { label: '已暂停', class: 'badge-warning' },
  completed: { label: '已完成', class: 'badge-success' }
}

const statusLabel = computed(
  () => statusMap[props.status]?.label || props.status
)
const statusClass = computed(
  () => statusMap[props.status]?.class || 'badge-gray'
)
</script>

<style scoped>
.badge {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  padding: 2px 10px;
  border-radius: 12px;
  font-size: 12px;
  font-weight: 500;
  line-height: 1.6;
}

.badge-gray {
  background: var(--bg-muted);
  color: var(--text-secondary);
}

.badge-info {
  background: var(--primary-light);
  color: var(--primary);
}

.badge-warning {
  background: var(--warning-bg);
  color: var(--warning-text);
}

.badge-error {
  background: var(--danger-bg);
  color: var(--danger-text);
}

.badge-success {
  background: var(--success-bg);
  color: var(--success-text);
}

.error-icon {
  font-size: 13px;
  cursor: help;
}

.has-error {
  cursor: help;
  position: relative;
}

@keyframes pulse {
  0%, 100% { opacity: 1; }
  50% { opacity: 0.6; }
}

.pulse {
  animation: pulse 1.5s ease-in-out infinite;
}
</style>