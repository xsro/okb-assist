<template>
  <div class="section">
    <div class="section-header">
      <h3>MinerU</h3>
      <button class="btn btn-sm btn-outline" @click="add">添加配置</button>
    </div>
    <div v-for="(mu, idx) in configs" :key="idx" class="mineru-card">
      <!-- 折叠头：始终显示关键信息 -->
      <div class="card-header-collapsible" @click="toggle(idx)">
        <div class="card-summary">
          <span class="collapse-arrow" :class="{ expanded: expanded[idx] }">▶</span>
          <span class="mineru-index">{{ mu.name || `配置 #${idx + 1}` }}</span>
          <span class="mineru-type-badge" :class="mu.type">{{ typeLabel(mu.type) }}</span>
          <span class="mineru-url-preview">{{ mu.url }}</span>
          <span class="status-dot-small" :class="getStatusClass(idx)" :title="getStatusText(idx)"></span>
          <span
            class="active-radio"
            :class="{ active: isActive(mu) }"
            :title="isActive(mu) ? '当前使用的配置' : '点击设为当前配置'"
            @click.stop="setActive(mu)"
          >
            <span v-if="isActive(mu)" class="radio-dot active">●</span>
            <span v-else class="radio-dot">○</span>
          </span>
        </div>
        <button class="btn btn-sm btn-ghost" @click.stop="toggle(idx)">
          {{ expanded[idx] ? '收起' : '展开' }}
        </button>
      </div>

      <!-- 折叠体：展开后显示完整编辑表单 -->
      <div v-show="expanded[idx]" class="card-body-collapsible">
        <div class="form-row">
          <div class="form-group">
            <label>名称</label>
            <input v-model="mu.name" type="text" placeholder="例如：官方云 API" @input="emitUpdate" />
          </div>
          <div class="form-group">
            <label>类型</label>
            <select v-model="mu.type" @change="emitUpdate">
              <option value="local">本地服务 (local)</option>
              <option value="official">官方精准解析 API (official)</option>
              <option value="official-lightweight">官方 Agent 轻量 API (official-lightweight)</option>
            </select>
          </div>
        </div>
        <div class="form-row">
          <div class="form-group">
            <label>URL</label>
            <input v-model="mu.url" type="text" :placeholder="mu.type === 'local' ? 'http://127.0.0.1:8000' : 'https://mineru.net'" @input="emitUpdate" />
          </div>
          <div class="form-group">
            <label>Token</label>
            <input v-model="mu.token" type="text" :placeholder="mu.type === 'official' ? 'sk-...' : '无需填写'" @input="emitUpdate" />
          </div>
        </div>
        <div class="form-row">
          <div class="form-group">
            <label>解析档位</label>
            <select v-model="mu.tier" @change="emitUpdate">
              <option value="vlm">vlm（VLM 高精度，推荐）</option>
              <option value="pipeline">pipeline（管线）</option>
              <option value="standard">standard（标准）</option>
              <option value="advanced">advanced（进阶）</option>
              <option value="basic">basic（基础）</option>
              <option value="flash">flash（快速）</option>
            </select>
          </div>
          <div class="form-group">
            <label>任务超时 (秒)</label>
            <input v-model.number="mu.task_timeout" type="number" @input="emitUpdate" />
          </div>
        </div>
        <div class="form-row">
          <div class="form-group">
            <label>最大并发任务数</label>
            <input v-model.number="mu.max_tasks" type="number" @input="emitUpdate" />
          </div>
          </div>
        <div class="status-test-row">
          <span class="status-dot" :class="getStatusClass(idx)"></span>
          <span class="status-text">{{ getStatusText(idx) }}</span>
          <button class="btn btn-sm btn-outline" @click="test(idx)" :disabled="testing[idx]">
            {{ testing[idx] ? '测试中...' : '测试连接' }}
          </button>
        </div>
        <div class="card-actions">
          <button class="btn btn-sm btn-outline" @click="moveUp(idx)" :disabled="idx === 0">上移</button>
          <button class="btn btn-sm btn-outline" @click="moveDown(idx)" :disabled="idx === configs.length - 1">下移</button>
          <button class="btn btn-sm btn-danger" @click="remove(idx)" :disabled="configs.length <= 1">删除</button>
        </div>
      </div>
    </div>
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
  activeKey?: string
}>()

const emit = defineEmits<{
  (e: 'update'): void
  (e: 'update:activeKey', key: string): void
}>()

function emitUpdate() {
  emit('update')
}

// 折叠状态：每个配置独立
const expanded = reactive<Record<number, boolean>>({})

function toggle(idx: number) {
  expanded[idx] = !expanded[idx]
}

function typeLabel(type: string): string {
  const map: Record<string, string> = {
    local: '本地',
    official: '精准',
    'official-lightweight': '轻量'
  }
  return map[type] || type
}

// 测试状态
const testing = reactive<Record<number, boolean>>({})
const statuses = reactive<Record<number, { status: string; detail: string }>>({})

function defaultMineruConfig(): MinerUConfig {
  return {
    type: 'local',
    url: 'http://127.0.0.1:8000',
    token: '',
    name: '',
    task_timeout: 300,
    tier: 'standard',
    max_tasks: 3
  }
}

function add() {
  props.configs.push(defaultMineruConfig())
  expanded[props.configs.length - 1] = true  // 新配置自动展开
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

function isActive(mu: MinerUConfig): boolean {
  return props.activeKey === mu.name
}

function setActive(mu: MinerUConfig) {
  if (mu.name) {
    emit('update:activeKey', mu.name)
  }
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

function syncStatus(idx: number, status: { status: string; detail: string }) {
  statuses[idx] = status
}

defineExpose({ syncStatus })
</script>

<style scoped>
.mineru-card {
  margin-bottom: 8px;
  background: var(--bg-white);
  border: 1px solid var(--border);
  border-radius: 8px;
  overflow: hidden;
}

/* 折叠头 */
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
.collapse-arrow {
  font-size: 10px;
  transition: transform 0.2s;
  flex-shrink: 0;
  color: var(--text-secondary);
}
.collapse-arrow.expanded {
  transform: rotate(90deg);
}
.mineru-index {
  font-weight: 600;
  font-size: 14px;
  white-space: nowrap;
}
.mineru-type-badge {
  font-size: 11px;
  padding: 1px 6px;
  border-radius: 4px;
  white-space: nowrap;
  flex-shrink: 0;
}
.mineru-type-badge.local { background: #e8f5e9; color: #2e7d32; }
.mineru-type-badge.official { background: #e3f2fd; color: #1565c0; }
.mineru-type-badge.official-lightweight { background: #fff3e0; color: #e65100; }
.mineru-url-preview {
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
.active-radio {
  cursor: pointer;
  flex-shrink: 0;
  display: flex;
  align-items: center;
  line-height: 1;
}
.radio-dot {
  font-size: 14px;
  color: var(--text-secondary, #999);
  transition: color 0.15s;
}
.radio-dot.active {
  color: var(--primary, #1976d2);
}
.active-radio:hover .radio-dot {
  color: var(--primary, #1976d2);
}
.active-radio.active {
  cursor: default;
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

/* 折叠体 */
.card-body-collapsible {
  padding: 14px;
  border-top: 1px solid var(--border);
}
.card-body-collapsible .form-row {
  margin-bottom: 10px;
}
.card-actions {
  display: flex;
  gap: 6px;
  margin-top: 12px;
  padding-top: 12px;
  border-top: 1px dashed var(--border);
}

/* 原有的垂直布局保持 */
.form-row { display: flex; gap: 12px; }
.form-group { flex: 1; }
.form-group label { display: block; font-size: 12px; margin-bottom: 4px; color: var(--text-secondary); }
.form-group input, .form-group select { width: 100%; box-sizing: border-box; }
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

.hint { display: none; }

@media (max-width: 768px) {
  .card-summary { flex-wrap: wrap; gap: 4px; }
  .mineru-url-preview { width: 100%; order: 10; }
  .card-body-collapsible .form-row { flex-direction: column; gap: 8px; }
  .card-actions { flex-wrap: wrap; }
  .card-actions button { flex: 1; min-width: 60px; }
  .status-test-row { flex-wrap: wrap; }
}
</style>