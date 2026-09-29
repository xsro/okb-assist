<template>
  <div class="skeleton-table" :class="{ 'skeleton-card': card }">
    <div v-for="n in rows" :key="n" class="skeleton-row" :style="{ gridTemplateColumns: columns }">
      <div
        v-for="(col, ci) in columnsArr"
        :key="ci"
        class="sk"
        :style="{ width: col.width || '100%' }"
      ></div>
    </div>
  </div>
</template>

<script setup lang="ts">
import { computed } from 'vue'

const props = withDefaults(defineProps<{
  rows?: number
  columns?: string | string[]
  card?: boolean
}>(), {
  rows: 5,
  columns: '60px 1fr 180px 80px 80px 120px 80px',
  card: false
})

const columnsArr = computed(() => {
  const cols = Array.isArray(props.columns) ? props.columns : props.columns.split(/\s+/)
  return cols.map((c) => {
    if (c.endsWith('px')) return { width: c, flex: 'none' }
    if (c.endsWith('%')) return { width: c, flex: 'none' }
    if (c === '1fr' || c === 'auto') return { width: '100%', flex: '1' }
    return { width: c, flex: 'none' }
  })
})
</script>

<style scoped>
.skeleton-table {
  background: var(--bg-white);
  border: 1px solid var(--border);
  border-radius: 8px;
  overflow: hidden;
}
.skeleton-row {
  display: grid;
  gap: 8px;
  padding: 14px 16px;
  border-bottom: 1px solid var(--border);
  align-items: center;
}
.skeleton-row:last-child {
  border-bottom: none;
}

@keyframes shimmer {
  0% { background-position: -200px 0; }
  100% { background-position: calc(200px + 100%) 0; }
}

.sk {
  height: 14px;
  border-radius: 4px;
  background: linear-gradient(90deg, var(--bg) 25%, var(--bg-muted) 50%, var(--bg) 75%);
  background-size: 200px 100%;
  animation: shimmer 1.5s infinite ease-in-out;
}

.skeleton-card .skeleton-row {
  grid-template-columns: 1fr;
  padding: 16px;
}

.skeleton-card .sk {
  height: 16px;
}
</style>