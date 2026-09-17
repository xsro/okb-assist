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

/* 移动端 Toast 适配 */
@media (max-width: 768px) {
  .toast-container {
    top: 56px;
    right: 0;
    left: 0;
    padding: 0 12px;
  }

  .toast {
    min-width: 0;
    width: 100%;
    margin: 0;
    padding: 12px 16px 12px 44px;
    font-size: 14px;
  }

  .toast-close {
    width: 36px;
    height: 36px;
  }
}

@media (max-width: 480px) {
  .toast {
    padding: 10px 14px 10px 40px;
    font-size: 13px;
  }
}
</style>
