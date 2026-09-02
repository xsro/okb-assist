<template>
  <div class="toast-container">
    <div
      v-for="t in store.toasts"
      :key="t.id"
      class="toast"
      :class="t.type"
    >
      <button
        class="toast-close"
        type="button"
        aria-label="关闭"
        @click="store.remove(t.id)"
      >
        <span aria-hidden="true">×</span>
      </button>
      <span class="toast-message">{{ t.message }}</span>
      <span class="toast-countdown" :class="{ urgent: t.remaining <= 3 }">
        {{ t.remaining }}s
      </span>
    </div>
  </div>
</template>

<script setup lang="ts">
import { useToastStore } from '@/stores/toast'

const store = useToastStore()
</script>

<style scoped>
.toast-countdown {
  margin-left: 8px;
  font-size: 12px;
  font-weight: 600;
  color: #888;
  background: rgba(0, 0, 0, 0.06);
  border-radius: 8px;
  padding: 0 6px;
  line-height: 20px;
  white-space: nowrap;
  flex-shrink: 0;
  transition: color 0.3s, background 0.3s;
}
.toast-countdown.urgent {
  color: #e74c3c;
  background: rgba(231, 76, 60, 0.12);
  animation: pulse 1s ease-in-out infinite;
}
@keyframes pulse {
  0%, 100% { opacity: 1; }
  50% { opacity: 0.5; }
}
</style>
