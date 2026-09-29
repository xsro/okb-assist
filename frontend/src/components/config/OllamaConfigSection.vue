<template>
  <div class="section">
    <div class="section-header">
      <h3>Ollama</h3>
    </div>
    <div class="form-group">
      <label>URL</label>
      <input v-model="ollama.url" type="text" @input="emitUpdate" />
    </div>
    <div class="form-group">
      <label>API Key</label>
      <input v-model="ollama.key" type="password" @input="emitUpdate" />
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

const testing = ref(false)
const localStatus = ref<{ status: string; detail: string } | null>(null)

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
</style>