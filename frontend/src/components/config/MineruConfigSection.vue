<template>
  <div class="section">
    <div class="section-header">
      <h3>MinerU</h3>
      <button class="btn btn-sm btn-outline" @click="add">添加配置</button>
    </div>
    <div v-for="(mu, idx) in configs" :key="idx" class="mineru-card">
      <div class="mineru-card-header">
        <span class="mineru-index">配置 #{{ idx + 1 }}</span>
        <div class="mineru-actions">
          <button class="btn btn-sm btn-outline" @click="moveUp(idx)" :disabled="idx === 0">↑</button>
          <button class="btn btn-sm btn-outline" @click="moveDown(idx)" :disabled="idx === configs.length - 1">↓</button>
          <button class="btn btn-sm btn-danger" @click="remove(idx)" :disabled="configs.length <= 1">删除</button>
        </div>
      </div>
      <div class="form-row">
        <div class="form-group">
          <label>类型</label>
          <select v-model="mu.type" @change="emitUpdate">
            <option value="local">本地服务</option>
            <option value="official">官方精准解析 API</option>
          </select>
        </div>
        <div class="form-group">
          <label>URL</label>
          <input v-model="mu.url" type="text" :placeholder="mu.type === 'official' ? 'https://mineru.net' : 'http://127.0.0.1:8000'" @input="emitUpdate" />
        </div>
      </div>
      <div class="form-row">
        <div class="form-group">
          <label>API Key</label>
          <input v-model="mu.key" type="password" :placeholder="mu.type === 'official' ? 'sk-...' : '本地服务无需填写'" @input="emitUpdate" />
        </div>
        <div class="form-group">
          <label>解析档位</label>
          <select v-model="mu.tier" @change="emitUpdate">
            <option value="standard">standard（VLM，推荐）</option>
            <option value="advanced">advanced（VLM 进阶）</option>
            <option value="basic">basic（基础小模型）</option>
            <option value="flash">flash（快速模式）</option>
          </select>
        </div>
      </div>
      <div class="form-row">
        <div class="form-group">
          <label>任务超时 (秒)</label>
          <input v-model.number="mu.task_timeout" type="number" @input="emitUpdate" />
        </div>
        <div class="form-group">
          <label>最大并发任务数</label>
          <input v-model.number="mu.max_tasks" type="number" @input="emitUpdate" />
        </div>
      </div>
      <div class="status-test-row">
        <span class="status-dot" :class="getStatusClass(idx)"></span>
        <span class="status-text">{{ getStatusText(idx) }}</span>
        <button class="btn btn-sm btn-outline" @click="test(idx)" :disabled="testing[idx]">
          {{ testing[idx] ? '测试中...' : '测试' }}
        </button>
      </div>
    </div>
    <p class="hint">解析 PDF 时按顺序逐个尝试以上配置，直到成功。</p>
  </div>
</template>

<script setup lang="ts">
import { reactive } from 'vue'
import type { MinerUConfig } from '@/types/config'
import { testService } from '@/api/config'
import { useToast } from '@/composables/useToast'

const { showError, showToast } = useToast()

const props = defineProps<{
  configs: MinerUConfig[]
}>()

const emit = defineEmits<{
  (e: 'update'): void
}>()

function emitUpdate() {
  emit('update')
}

// 测试状态
const testing = reactive<Record<number, boolean>>({})
const statuses = reactive<Record<number, { status: string; detail: string }>>({})

function defaultMineruConfig(): MinerUConfig {
  return {
    type: 'local',
    url: 'http://127.0.0.1:8000',
    key: 'key',
    task_timeout: 300,
    tier: 'standard',
    max_tasks: 3
  }
}

function add() {
  props.configs.push(defaultMineruConfig())
  emitUpdate()
}

function remove(idx: number) {
  if (props.configs.length <= 1) return
  props.configs.splice(idx, 1)
  emitUpdate()
}

function moveUp(idx: number) {
  if (idx <= 0) return
  const arr = props.configs
  ;[arr[idx], arr[idx - 1]] = [arr[idx - 1], arr[idx]]
  emitUpdate()
}

function moveDown(idx: number) {
  if (idx >= props.configs.length - 1) return
  const arr = props.configs
  ;[arr[idx], arr[idx + 1]] = [arr[idx + 1], arr[idx]]
  emitUpdate()
}

async function test(idx: number) {
  const mu = props.configs[idx]
  if (!mu) return
  testing[idx] = true
  try {
    const res = await testService({
      service_type: 'mineru',
      url: mu.url,
      key: mu.type
    })
    statuses[idx] = { status: res.status, detail: res.detail }
    showToast(res.detail, res.status === 'connected' ? 'success' : 'error')
  } catch {
    showError('连接测试失败')
  } finally {
    testing[idx] = false
  }
}

function getStatusClass(idx: number): string {
  const s = statuses[idx]?.status
  if (!s) return ''
  if (s === 'connected') return 'ok'
  if (s === 'disabled' || s === 'not_configured') return 'warning'
  return 'error'
}

function getStatusText(idx: number): string {
  const s = statuses[idx]
  if (!s) return '未测试'
  const map: Record<string, string> = {
    connected: '正常',
    disabled: '已禁用',
    not_configured: '未配置',
    disconnected: '无法连接',
    error: '连接错误'
  }
  return map[s.status] || s.status
}

// 从父组件同步外部状态（来自 loadServiceStatus）
function syncStatus(idx: number, status: { status: string; detail: string }) {
  statuses[idx] = status
}

defineExpose({ syncStatus })
</script>

<style scoped>
.mineru-card {
  padding: 16px;
  margin-bottom: 12px;
  background: var(--bg-white);
  border: 1px solid var(--border);
  border-radius: 8px;
}
.mineru-card-header {
  display: flex;
  justify-content: space-between;
  align-items: center;
  margin-bottom: 12px;
}
.mineru-index {
  font-weight: 600;
  font-size: 14px;
}
.mineru-actions {
  display: flex;
  gap: 6px;
}
.mineru-actions button {
  width: 28px;
  height: 28px;
  padding: 0;
  font-size: 14px;
  line-height: 1;
}
.hint {
  font-size: 12px;
  color: var(--text-secondary);
  margin: 8px 0 0;
}

/* 状态测试行 */
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

@media (max-width: 768px) {
  .status-test-row {
    flex-wrap: wrap;
    gap: 6px;
  }
  .status-test-row .btn {
    margin-left: 0;
  }
}
</style>