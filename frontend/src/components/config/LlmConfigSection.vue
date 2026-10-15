<template>
  <div class="section">
    <div class="section-header">
      <h3>LLM 模型池</h3>
      <button class="btn btn-sm btn-outline" @click="add">添加模型</button>
    </div>
    <p class="section-desc">
      预留：后续问答系统将从这里的模型中选择；当前只有「信息补全」使用的模型会真正参与处理。
    </p>
    <div v-for="(model, idx) in models" :key="idx" class="llm-card">
      <div class="card-header-collapsible" @click="toggle(idx)">
        <div class="card-summary">
          <span class="collapse-arrow" :class="{ expanded: expanded[idx] }">▶</span>
          <span class="llm-name">{{ model.name || `模型 #${idx + 1}` }}</span>
          <span class="protocol-badge">{{ protocolLabel(model.protocol) }}</span>
          <span class="llm-model-preview">{{ model.model }}</span>
          <span class="llm-url-preview">{{ model.url }}</span>
        </div>
        <button class="btn btn-sm btn-ghost" @click.stop="toggle(idx)">
          {{ expanded[idx] ? '收起' : '展开' }}
        </button>
      </div>

      <div v-show="expanded[idx]" class="card-body-collapsible">
        <div class="form-row">
          <div class="form-group">
            <label>名称</label>
            <input v-model="model.name" type="text" placeholder="例如：本地 Ollama" @input="emitUpdate" />
          </div>
          <div class="form-group">
            <label>协议</label>
            <select v-model="model.protocol" @change="emitUpdate">
              <option value="ollama">Ollama（原生协议）</option>
              <option value="openai">OpenAI（兼容协议）</option>
            </select>
          </div>
        </div>
        <div class="form-row">
          <div class="form-group">
            <label>URL</label>
            <input v-model="model.url" type="text" placeholder="http://127.0.0.1:11434" @input="emitUpdate" />
          </div>
          <div class="form-group">
            <label>模型</label>
            <input v-model="model.model" type="text" placeholder="qwen3.5:9b" @input="emitUpdate" />
          </div>
        </div>
        <div class="form-row">
          <div class="form-group">
            <label>API Key</label>
            <div class="input-with-toggle">
              <input v-model="model.key" :type="keyVisible[idx] ? 'text' : 'password'" placeholder="无需鉴权可留空" @input="emitUpdate" />
              <button class="toggle-visibility" type="button" @click="toggleKey(idx)" :title="keyVisible[idx] ? '隐藏 Key' : '显示 Key'">
                <svg v-if="keyVisible[idx]" width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M17.94 17.94A10.07 10.07 0 0 1 12 20c-7 0-11-8-11-8a18.45 18.45 0 0 1 5.06-5.94M9.9 4.24A9.12 9.12 0 0 1 12 4c7 0 11 8 11 8a18.5 18.5 0 0 1-2.16 3.19m-6.72-1.07a3 3 0 1 1-4.24-4.24"/><line x1="1" y1="1" x2="23" y2="23"/></svg>
              <svg v-else width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M1 12s4-8 11-8 11 8 11 8-4 8-11 8-11-8-11-8z"/><circle cx="12" cy="12" r="3"/></svg>
              </button>
            </div>
          </div>
          <div class="form-group">
            <label>连接测试</label>
            <button class="btn btn-sm btn-outline btn-block" @click="test(idx)" :disabled="testing[idx]">
              {{ testing[idx] ? '测试中...' : '测试该模型' }}
            </button>
          </div>
        </div>
        <div v-if="statuses[idx]" class="test-result" :class="statuses[idx].status">
          <span class="status-dot-small" :class="statusClass(statuses[idx].status)"></span>
          <span>{{ statuses[idx].detail }}</span>
        </div>
        <div class="card-actions">
          <button class="btn btn-sm btn-outline" @click="moveUp(idx)" :disabled="idx === 0">上移</button>
          <button class="btn btn-sm btn-outline" @click="moveDown(idx)" :disabled="idx === models.length - 1">下移</button>
          <button class="btn btn-sm btn-danger" @click="remove(idx)" :disabled="models.length <= 1">删除</button>
        </div>
      </div>
    </div>
  </div>
</template>

<script setup lang="ts">
import { reactive } from 'vue'
import type { LlmModelEntry } from '@/types/config'
import { testService } from '@/api/config'
import { useToast } from '@/composables/useToast'

const { showError, showToast } = useToast()

const props = defineProps<{
  models: LlmModelEntry[]
}>()

const emit = defineEmits<{
  (e: 'update'): void
}>()

function emitUpdate() {
  emit('update')
}

const expanded = reactive<Record<number, boolean>>({})
const testing = reactive<Record<number, boolean>>({})
const keyVisible = reactive<Record<number, boolean>>({})
const statuses = reactive<Record<number, { status: string; detail: string }>>({})

function toggle(idx: number) {
  expanded[idx] = !expanded[idx]
}

function toggleKey(idx: number) {
  keyVisible[idx] = !keyVisible[idx]
}

function protocolLabel(protocol: string): string {
  return protocol === 'openai' ? 'OpenAI' : 'Ollama'
}

function statusClass(status: string): string {
  if (status === 'connected') return 'ok'
  if (status === 'unsupported' || status === 'not_configured') return 'warning'
  return 'error'
}

function defaultModel(): LlmModelEntry {
  return { name: '', protocol: 'ollama', url: 'http://127.0.0.1:11434', key: '', model: '' }
}

function add() {
  props.models.push(defaultModel())
  expanded[props.models.length - 1] = true
  emitUpdate()
}

function remove(idx: number) {
  props.models.splice(idx, 1)
  emitUpdate()
}

function moveUp(idx: number) {
  if (idx === 0) return
  const [item] = props.models.splice(idx, 1)
  props.models.splice(idx - 1, 0, item)
  emitUpdate()
}

function moveDown(idx: number) {
  if (idx >= props.models.length - 1) return
  const [item] = props.models.splice(idx, 1)
  props.models.splice(idx + 1, 0, item)
  emitUpdate()
}

async function test(idx: number) {
  const model = props.models[idx]
  if (!model) return
  testing[idx] = true
  try {
    const res = await testService({
      service_type: 'llm',
      protocol: model.protocol,
      url: model.url,
      key: model.key,
      model: model.model
    })
    statuses[idx] = { status: res.status, detail: res.detail }
    showToast(res.detail, res.status === 'connected' ? 'success' : 'error')
  } catch {
    showError('连接测试失败')
  } finally {
    testing[idx] = false
  }
}

defineExpose({ syncStatus: (idx: number, status: { status: string; detail: string }) => { statuses[idx] = status } })
</script>

<style scoped>
.section-desc {
  margin: 0 0 10px;
  font-size: 12px;
  color: var(--text-secondary);
}

.llm-card {
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
.llm-name {
  font-size: 13px;
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
.protocol-badge {
  font-size: 11px;
  padding: 1px 6px;
  border-radius: 4px;
  border: 1px solid var(--border);
  color: var(--text-secondary);
  white-space: nowrap;
  flex-shrink: 0;
}
.llm-model-preview {
  font-size: 12px;
  color: var(--text-primary);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}
.llm-url-preview {
  font-size: 12px;
  color: var(--text-secondary);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
  flex: 1;
  min-width: 0;
}

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
.btn-block {
  width: 100%;
}

.test-result {
  display: flex;
  align-items: center;
  gap: 6px;
  margin-top: 10px;
  padding: 6px 10px;
  border-radius: 6px;
  font-size: 12px;
  background: var(--bg-hover, rgba(0,0,0,0.03));
  color: var(--text-secondary);
  word-break: break-all;
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
