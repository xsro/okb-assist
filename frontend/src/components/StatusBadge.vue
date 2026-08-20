<template>
  <span class="badge" :class="statusClass">
    {{ statusLabel }}
  </span>
</template>

<script setup lang="ts">
import { computed } from 'vue'

const props = defineProps<{ status: string }>()

const statusMap: Record<string, { label: string; class: string }> = {
  // 文档状态
  uploaded: { label: '已上传', class: 'badge-gray' },
  parsing: { label: '解析中', class: 'badge-warning pulse' },
  markdown_done: { label: '解析完成', class: 'badge-info' },
  extracting: { label: '提取中', class: 'badge-warning pulse' },
  meta_done: { label: '提取完成', class: 'badge-info' },
  indexing: { label: '索引中', class: 'badge-warning pulse' },
  indexed: { label: '已索引', class: 'badge-success' },
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