<template>
  <span
    class="badge"
    :class="[statusClass, { 'has-error': view.status === 'error' && view.detail }]"
    :title="view.detail || undefined"
  >
    <span class="files-text">{{ view.files }}</span>
    <span v-if="view.activity" class="activity-text">
      {{ view.activity }}<template v-if="view.progress !== null"> {{ Math.round(view.progress) }}%</template>
    </span>
    <span
      v-if="view.synced === false"
      class="unsynced-icon"
      title="文件事实尚未从磁盘同步，状态未知"
    >未同步</span>
    <span v-if="view.status === 'error'" class="error-icon" title="">⚠</span>
  </span>
</template>

<script setup lang="ts">
import { computed } from 'vue'
import type { Document } from '@/types/document'
import { deriveDocStatus } from '@/utils/docStatus'

/**
 * 两种用法：
 * 1. 传 `doc`：状态与文案由 `deriveDocStatus()` 前端推导（文献列表 / 详情页用这个）；
 * 2. 传 `status`：直接给枚举值（监控面板的流水线任务等场景用这个）。
 */
const props = defineProps<{
  doc?: Document | null
  status?: string
  status_message?: string | null
  synced?: boolean | null
}>()

const statusMap: Record<string, { label: string; class: string }> = {
  // 文献状态（粗粒度，决定配色）
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

const view = computed(() => {
  // 文献：主文字直接显示「实际存在的文件类型」（pdf|md|zip / 无文件 / 未知）
  if (props.doc) {
    const d = deriveDocStatus(props.doc)
    return {
      status: d.status,
      files: d.files,
      activity: d.activity,
      detail: d.detail,
      progress: d.progress,
      synced: d.synced,
      health: d.health,
      healthDetail: d.healthDetail
    }
  }
  // 其他场景（流水线任务状态等）：主文字就是状态文案
  const key = props.status || ''
  return {
    status: key,
    files: statusMap[key]?.label || key,
    activity: null as string | null,
    detail: props.status_message ?? null,
    progress: null as number | null,
    synced: props.synced ?? null,
    health: null as string | null,
    healthDetail: null as string | null
  }
})

const statusClass = computed(() => statusMap[view.value.status]?.class || 'badge-gray')
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

.pulse {
  animation: badge-pulse 1.6s ease-in-out infinite;
}

@keyframes badge-pulse {
  0%,
  100% {
    opacity: 1;
  }
  50% {
    opacity: 0.55;
  }
}

.files-text {
  font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
  letter-spacing: -0.2px;
}

.activity-text {
  font-variant-numeric: tabular-nums;
  opacity: 0.85;
  font-size: 10px;
  padding: 0 4px;
  border-radius: 6px;
  background: rgba(0, 0, 0, 0.06);
}

.unsynced-icon {
  font-size: 10px;
  padding: 0 4px;
  border-radius: 6px;
  background: rgba(0, 0, 0, 0.08);
  color: var(--text-secondary, #666);
}

.error-icon {
  font-size: 11px;
}
</style>
