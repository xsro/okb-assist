<template>
  <div class="section">
    <div class="ollama-card">
      <div class="card-header-collapsible" @click="expanded = !expanded">
        <div class="card-summary">
          <span class="collapse-arrow" :class="{ expanded }">▶</span>
          <h3>Ollama</h3>
          <span class="ollama-url-preview">{{ ollama.url }}</span>
          <span class="status-dot-small" :class="statusClass" :title="statusText"></span>
        </div>
        <button class="btn btn-sm btn-ghost" @click.stop="expanded = !expanded">
          {{ expanded ? '收起' : '展开' }}
        </button>
      </div>
      <div v-show="expanded" class="card-body-collapsible">
        <div class="form-group">
          <label>URL</label>
          <input v-model="ollama.url" type="text" @input="emitUpdate" />
        </div>
        <div class="form-group">
          <label>API Key</label>
          <div class="input-with-toggle">
            <input v-model="ollama.key" :type="keyVisible ? 'text' : 'password'" @input="emitUpdate" />
            <button class="toggle-visibility" type="button" @click="keyVisible = !keyVisible" :title="keyVisible ? '隐藏 Key' : '显示 Key'">
              <svg v-if="keyVisible" width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M17.94 17.94A10.07 10.07 0 0 1 12 20c-7 0-11-8-11-8a18.45 18.45 0 0 1 5.06-5.94M9.9 4.24A9.12 9.12 0 0 1 12 4c7 0 11 8 11 8a18.5 18.5 0 0 1-2.16 3.19m-6.72-1.07a3 3 0 1 1-4.24-4.24"/><line x1="1" y1="1" x2="23" y2="23"/></svg>
              <svg v-else width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M1 12s4-8 11-8 11 8 11 8-4 8-11 8-11-8-11-8z"/><circle cx="12" cy="12" r="3"/></svg>
            </button>
          </div>
        </div>
        <div class="form-group">
          <label>模型</label>
          <input v-model="ollama.model" type="text" @input="emitUpdate" />
        </div>
        <div class="status-test-row">
          <span class="status-dot" :class="statusClass"></span>
          <span class="status-text">{{ statusText }}</span>
          <button class="btn btn-sm btn-outline" @click="test" :disabled="testing">
            {{ testing ? '测试中...' : '测试' }}
          </button>
        </div>
      </div>
    </div>
  </div>
</template>

<script setup lang="ts">
import { ref, computed } from 'vue'
import type { OllamaConfig } from '@/types/config'
import { testService } from '@/api/config'
import { useToast } from '@/composables/useToast'

const { showError, showToast } = useToast()

const props = defineProps<{
  ollama: OllamaConfig
}>()

const emit = defineEmits<{
  (e: 'update'): void
}>()

function emitUpdate() {
  emit('update')
}

const expanded = ref(false)
const testing = ref(false)
const localStatus = ref<{ status: string; detail: string } | null>(null)
const keyVisible = ref(false)

const statusClass = computed(() => {
  const s = localStatus.value?.status
  if (!s) return ''
  if (s === 'connected') return 'ok'
  if (s === 'disabled' || s === 'not_configured') return 'warning'
  return 'error'
})

const statusText = computed(() => {
  const s = localStatus.value
  if (!s) return '未测试'
  const map: Record<string, string> = {
    connected: '正常',
    disabled: '已禁用',
    not_configured: '未配置',
    disconnected: '无法连接',
    error: '连接错误'
  }
  return map[s.status] || s.status
})

async function test() {
  testing.value = true
  try {
    const res = await testService({
      service_type: 'ollama',
      url: props.ollama.url,
      model: props.ollama.model
    })
    localStatus.value = { status: res.status, detail: res.detail }
    showToast(res.detail, res.status === 'connected' ? 'success' : 'error')
  } catch {
    showError('连接测试失败')
  } finally {
    testing.value = false
  }
}

function syncStatus(status: { status: string; detail: string }) {
  localStatus.value = status
}

defineExpose({ syncStatus })
</script>

<style scoped>
.ollama-card {
  margin-bottom: 8px;
  background: var(--bg-white);
  border: 1px solid var(--border);
  border-radius: 8px;
  overflow: hidden;
}

.card-header-collapsible {
  display: flex;
  justify-content: space-between;
  align-items: center;
  padding: 10px 14px;
  cursor: pointer;
  user-select: none;
  transition: background 0.15s;
}
.card-header-collapsible:hover {
  background: var(--bg-hover, rgba(0,0,0,0.03));
}
.card-summary {
  display: flex;
  align-items: center;
  gap: 8px;
  flex: 1;
  min-width: 0;
  overflow: hidden;
}
.card-summary h3 {
  margin: 0;
  font-size: 14px;
  font-weight: 600;
  white-space: nowrap;
}
.collapse-arrow {
  font-size: 10px;
  transition: transform 0.2s;
  flex-shrink: 0;
  color: var(--text-secondary);
}
.collapse-arrow.expanded {
  transform: rotate(90deg);
}
.ollama-url-preview {
  font-size: 12px;
  color: var(--text-secondary);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
  flex: 1;
  min-width: 0;
}
.status-dot-small {
  display: inline-block;
  width: 6px;
  height: 6px;
  border-radius: 50%;
  flex-shrink: 0;
}
.status-dot-small.ok { background: var(--success); }
.status-dot-small.error { background: var(--danger); }
.status-dot-small.warning { background: var(--warning); }

.card-body-collapsible {
  padding: 14px;
  border-top: 1px solid var(--border);
}

.btn-ghost {
  background: transparent;
  border: 1px solid transparent;
  color: var(--text-secondary);
  font-size: 12px;
  padding: 4px 10px;
}
.btn-ghost:hover {
  border-color: var(--border);
  background: var(--bg-hover, rgba(0,0,0,0.03));
}

.status-test-row {
  display: flex;
  align-items: center;
  gap: 8px;
  margin-top: 12px;
  padding-top: 12px;
  border-top: 1px dashed var(--border);
}
.status-dot {
  display: inline-block;
  width: 8px;
  height: 8px;
  border-radius: 50%;
  flex-shrink: 0;
}
.status-dot.ok { background: var(--success); }
.status-dot.error { background: var(--danger); }
.status-dot.warning { background: var(--warning); }
.status-text {
  font-size: 12px;
  color: var(--text-secondary);
  flex: 1;
}
.status-test-row .btn {
  margin-left: auto;
}

.input-with-toggle {
  display: flex;
  align-items: stretch;
  gap: 0;
}
.input-with-toggle input {
  flex: 1;
  border-top-right-radius: 0;
  border-bottom-right-radius: 0;
}
.toggle-visibility {
  display: flex;
  align-items: center;
  justify-content: center;
  padding: 0 10px;
  background: var(--bg-white);
  border: 1px solid var(--border);
  border-left: none;
  border-radius: 0 6px 6px 0;
  cursor: pointer;
  color: var(--text-secondary);
  min-width: 36px;
  transition: background 0.15s, color 0.15s;
}
.toggle-visibility:hover {
  background: var(--bg-hover, rgba(0,0,0,0.03));
  color: var(--text-primary);
}
</style>