<template>
  <div class="error-boundary-wrap">
    <!-- 错误降级 UI -->
    <div v-if="caughtError" class="error-fallback">
      <svg class="error-icon" width="48" height="48" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round">
        <circle cx="12" cy="12" r="10"></circle>
        <line x1="12" y1="8" x2="12" y2="12"></line>
        <line x1="12" y1="16" x2="12.01" y2="16"></line>
      </svg>
      <h3>组件渲染出错</h3>
      <p class="error-message">{{ caughtError }}</p>
      <div class="error-actions">
        <button class="btn" @click="retry">重试</button>
        <button class="btn btn-outline" @click="dismiss">关闭此区域</button>
      </div>
    </div>

    <!-- 正常内容 -->
    <template v-else>
      <slot />
    </template>

    <!-- 错误报告浮动按钮（未确认错误数 > 0 时显示） -->
    <div
      v-if="!caughtError && store.unacknowledgedCount > 0"
      class="error-badge"
      @click="showPanel = !showPanel"
      title="查看错误详情"
    >
      <span class="error-badge-dot"></span>
      <span class="error-badge-count">{{ store.unacknowledgedCount }}</span>
    </div>

    <!-- 错误详情面板 -->
    <Teleport to="body">
      <div v-if="showPanel" class="error-panel-overlay" @click.self="showPanel = false">
        <div class="error-panel">
          <div class="error-panel-header">
            <h3>错误日志</h3>
            <div class="error-panel-actions">
              <button class="btn btn-sm btn-outline" @click="store.acknowledgeAll()">全部确认</button>
              <button class="btn btn-sm btn-outline" @click="store.clearAcknowledged()">清除已确认</button>
              <button class="btn btn-sm btn-close" @click="showPanel = false">✕</button>
            </div>
          </div>
          <div class="error-panel-body">
            <div v-if="store.errors.length === 0" class="error-panel-empty">暂无错误记录</div>
            <div
              v-for="err in store.errors"
              :key="err.id"
              class="error-item"
              :class="{ 'error-item-new': !err.acknowledged }"
            >
              <div class="error-item-header">
                <span class="error-badge-source">{{ err.source }}</span>
                <span class="error-item-time">{{ formatTime(err.timestamp) }}</span>
                <button
                  v-if="!err.acknowledged"
                  class="btn btn-sm btn-outline"
                  @click="store.acknowledge(err.id)"
                >确认</button>
              </div>
              <p class="error-item-message">{{ err.message }}</p>
              <pre v-if="err.stack" class="error-item-stack">{{ err.stack }}</pre>
            </div>
          </div>
        </div>
      </div>
    </Teleport>
  </div>
</template>

<script setup lang="ts">
import { ref, onErrorCaptured, type ComponentPublicInstance } from 'vue'
import { useErrorStore } from '@/stores/error'

const store = useErrorStore()

/** 当前 ErrorBoundary 是否已降级 */
const caughtError = ref<string | null>(null)
const showPanel = ref(false)

onErrorCaptured((err: unknown, instance: ComponentPublicInstance | null, info: string) => {
  const message = err instanceof Error ? err.message : String(err)

  // 存入全局错误日志
  store.add({
    message,
    source: 'component',
    component: instance?.$options?.name || instance?.$el?.tagName || '(unknown)',
    stack: err instanceof Error ? err.stack : undefined
  })

  // 阻止错误继续向上冒泡（避免触发全局 errorHandler 的重复记录）
  // 但如果是致命错误仍允许传播
  if (!caughtError.value) {
    caughtError.value = `${message}（${info}）`
  }

  // 返回 false 阻止错误继续传播
  return false
})

function retry() {
  caughtError.value = null
}

function dismiss() {
  caughtError.value = null
}

function formatTime(date: Date): string {
  const pad = (n: number) => String(n).padStart(2, '0')
  return `${pad(date.getHours())}:${pad(date.getMinutes())}:${pad(date.getSeconds())}`
}
</script>

<style scoped>
.error-fallback {
  padding: 40px 24px;
  text-align: center;
  background: var(--bg-white);
  border: 1px solid var(--danger);
  border-radius: 8px;
  margin: 16px 0;
}

.error-icon {
  color: var(--danger);
  margin-bottom: 12px;
}

.error-fallback h3 {
  font-size: 16px;
  color: var(--danger);
  margin-bottom: 8px;
}

.error-message {
  font-size: 13px;
  color: var(--text-secondary);
  margin-bottom: 16px;
  word-break: break-word;
  max-width: 480px;
  margin-left: auto;
  margin-right: auto;
}

.error-actions {
  display: flex;
  gap: 8px;
  justify-content: center;
}

/* ── 浮动错误徽章 ──────────────────────────────────── */
.error-badge {
  position: fixed;
  bottom: 24px;
  right: 24px;
  z-index: 9997;
  width: 44px;
  height: 44px;
  border-radius: 50%;
  background: var(--danger);
  color: #fff;
  display: flex;
  align-items: center;
  justify-content: center;
  cursor: pointer;
  box-shadow: 0 2px 12px rgba(244, 67, 54, 0.4);
  transition: transform 0.15s;
}

.error-badge:hover {
  transform: scale(1.1);
}

.error-badge-dot {
  display: none;
}

.error-badge-count {
  font-size: 14px;
  font-weight: 700;
}

/* ── 错误详情面板 ──────────────────────────────────── */
.error-panel-overlay {
  position: fixed;
  inset: 0;
  background: rgba(0, 0, 0, 0.4);
  z-index: 9999;
  display: flex;
  align-items: center;
  justify-content: center;
}

.error-panel {
  background: var(--bg-white);
  border-radius: 8px;
  width: 560px;
  max-width: 92vw;
  max-height: 80vh;
  display: flex;
  flex-direction: column;
  box-shadow: 0 8px 32px rgba(0, 0, 0, 0.18);
}

.error-panel-header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 16px 20px;
  border-bottom: 1px solid var(--border);
  flex-shrink: 0;
}

.error-panel-header h3 {
  font-size: 16px;
  margin: 0;
}

.error-panel-actions {
  display: flex;
  gap: 6px;
  align-items: center;
}

.btn-close {
  background: transparent;
  border: 1px solid var(--border);
  border-radius: 4px;
  font-size: 16px;
  color: var(--text-secondary);
  cursor: pointer;
  padding: 4px 8px;
  line-height: 1;
}

.btn-close:hover {
  background: var(--bg);
}

.error-panel-body {
  flex: 1;
  overflow-y: auto;
  padding: 12px 20px;
}

.error-panel-empty {
  text-align: center;
  padding: 40px 0;
  color: var(--text-secondary);
  font-size: 14px;
}

.error-item {
  padding: 12px;
  margin-bottom: 8px;
  background: var(--bg);
  border-radius: 6px;
  border-left: 3px solid var(--border);
}

.error-item-new {
  border-left-color: var(--danger);
  background: var(--danger-bg);
}

.error-item-header {
  display: flex;
  align-items: center;
  gap: 8px;
  margin-bottom: 6px;
}

.error-badge-source {
  display: inline-block;
  padding: 1px 6px;
  border-radius: 4px;
  font-size: 11px;
  font-weight: 600;
  background: var(--bg-muted);
  color: var(--text-secondary);
  text-transform: uppercase;
}

.error-item-time {
  font-size: 11px;
  color: var(--text-muted);
  flex: 1;
}

.error-item-message {
  font-size: 13px;
  color: var(--text);
  word-break: break-word;
}

.error-item-stack {
  margin-top: 8px;
  padding: 8px;
  background: #1e1e1e;
  color: #d4d4d4;
  border-radius: 4px;
  font-size: 11px;
  overflow-x: auto;
  max-height: 200px;
  font-family: 'SF Mono', 'Consolas', monospace;
}

@media (max-width: 480px) {
  .error-panel {
    width: 96vw;
    max-height: 90vh;
  }
  .error-panel-header {
    flex-direction: column;
    align-items: flex-start;
    gap: 8px;
  }
  .error-panel-actions {
    width: 100%;
    flex-wrap: wrap;
  }
  .error-item-stack {
    font-size: 10px;
  }
  .error-badge {
    bottom: 16px;
    right: 16px;
  }
}
</style>