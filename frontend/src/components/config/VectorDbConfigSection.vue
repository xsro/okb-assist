<template>
  <div class="section">
    <div class="section-header">
      <h3>向量库</h3>
      <button class="btn btn-sm btn-outline" @click="add">添加向量库</button>
    </div>
    <div v-for="(db, idx) in dbs" :key="idx" class="vector-db-card">
      <!-- 折叠头 -->
      <div class="card-header-collapsible" @click="toggle(idx)">
        <div class="card-summary">
          <span class="collapse-arrow" :class="{ expanded: expanded[idx] }">▶</span>
          <span class="db-name">{{ db.name || db.id || `向量库 #${idx + 1}` }}</span>
          <span class="db-type-badge" :class="db.type">{{ db.type }}</span>
          <span class="db-url-preview">{{ db.url }}</span>
          <span class="status-dot-small" :class="getStatusClass(idx)" :title="getStatusText(idx)"></span>
          <span class="enabled-badge" :class="db.enabled !== false ? 'on' : 'off'">
            {{ db.enabled !== false ? '启用' : '禁用' }}
          </span>
        </div>
        <button class="btn btn-sm btn-ghost" @click.stop="toggle(idx)">
          {{ expanded[idx] ? '收起' : '展开' }}
        </button>
      </div>

      <!-- 折叠体 -->
      <div v-show="expanded[idx]" class="card-body-collapsible">
        <div class="form-row">
          <div class="form-group">
            <label>ID</label>
            <input v-model="db.id" type="text" @input="emitUpdate" />
          </div>
          <div class="form-group">
            <label>名称</label>
            <input v-model="db.name" type="text" @input="emitUpdate" />
          </div>
          <div class="form-group">
            <label>类型</label>
            <select v-model="db.type" @change="emitUpdate">
              <option value="qdrant">Qdrant</option>
              <option value="milvus">Milvus</option>
              <option value="chroma">Chroma</option>
            </select>
          </div>
        </div>
        <div class="form-row">
          <div class="form-group">
            <label>URL</label>
            <input v-model="db.url" type="text" @input="emitUpdate" />
          </div>
          <div class="form-group">
            <label>集合名</label>
            <input v-model="db.collection" type="text" @input="emitUpdate" />
          </div>
        </div>
        <div class="form-row">
          <div class="form-group">
            <label>API Key</label>
            <div class="input-with-toggle">
              <input v-model="db.api_key" :type="keyVisible[idx] ? 'text' : 'password'" @input="emitUpdate" />
              <button class="toggle-visibility" type="button" @click="toggleKey(idx)" :title="keyVisible[idx] ? '隐藏 Key' : '显示 Key'">
                <svg v-if="keyVisible[idx]" width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M17.94 17.94A10.07 10.07 0 0 1 12 20c-7 0-11-8-11-8a18.45 18.45 0 0 1 5.06-5.94M9.9 4.24A9.12 9.12 0 0 1 12 4c7 0 11 8 11 8a18.5 18.5 0 0 1-2.16 3.19m-6.72-1.07a3 3 0 1 1-4.24-4.24"/><line x1="1" y1="1" x2="23" y2="23"/></svg>
                <svg v-else width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M1 12s4-8 11-8 11 8 11 8-4 8-11 8-11-8-11-8z"/><circle cx="12" cy="12" r="3"/></svg>
              </button>
            </div>
          </div>
          <div class="form-group checkbox-group">
            <label>
              <input v-model="db.enabled" type="checkbox" @change="emitUpdate" />
              启用
            </label>
          </div>
        </div>
        <div class="form-row">
          <div class="form-group">
            <label>嵌入源</label>
            <input v-model="db.embedding.source" type="text" @input="emitUpdate" />
          </div>
          <div class="form-group">
            <label>嵌入模型</label>
            <input v-model="db.embedding.model" type="text" @input="emitUpdate" />
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
          <button class="btn btn-sm btn-danger" @click="remove(idx)">删除</button>
        </div>
      </div>
    </div>
  </div>
</template>

<script setup lang="ts">
import { reactive } from 'vue'
import type { VectorDbConfig } from '@/types/config'
import { testService } from '@/api/config'
import { useToast } from '@/composables/useToast'

const { showError, showToast } = useToast()

const props = defineProps<{
  dbs: VectorDbConfig[]
}>()

const emit = defineEmits<{
  (e: 'update'): void
}>()

function emitUpdate() {
  emit('update')
}

// 折叠状态
const expanded = reactive<Record<number, boolean>>({})

function toggle(idx: number) {
  expanded[idx] = !expanded[idx]
}

const testing = reactive<Record<number, boolean>>({})
const statuses = reactive<Record<number, { status: string; detail: string }>>({})
const keyVisible = reactive<Record<number, boolean>>({})

function toggleKey(idx: number) {
  keyVisible[idx] = !keyVisible[idx]
}

function defaultVectorDb(): VectorDbConfig {
  return {
    id: '',
    name: '',
    type: 'qdrant',
    enabled: true,
    url: '',
    collection: 'documents',
    api_key: '',
    embedding: { source: 'ollama', model: 'nomic-embed-text' }
  }
}

function add() {
  props.dbs.push(defaultVectorDb())
  expanded[props.dbs.length - 1] = true
  emitUpdate()
}

function remove(idx: number) {
  props.dbs.splice(idx, 1)
  emitUpdate()
}

async function test(idx: number) {
  const db = props.dbs[idx]
  if (!db) return
  testing[idx] = true
  try {
    const res = await testService({
      service_type: db.type,
      url: db.url,
      collection: db.collection
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
  if (s === 'disabled' || s === 'not_configured' || s === 'unsupported') return 'warning'
  return 'error'
}

function getStatusText(idx: number): string {
  const s = statuses[idx]
  if (!s) return '未测试'
  const map: Record<string, string> = {
    connected: '正常',
    disabled: '已禁用',
    not_configured: '未配置',
    unsupported: '暂不支持',
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
.vector-db-card {
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
.db-name {
  font-weight: 600;
  font-size: 14px;
  white-space: nowrap;
}
.db-type-badge {
  font-size: 11px;
  padding: 1px 6px;
  border-radius: 4px;
  white-space: nowrap;
  flex-shrink: 0;
}
.db-type-badge.qdrant { background: #e3f2fd; color: #1565c0; }
.db-type-badge.milvus { background: #f3e5f5; color: #7b1fa2; }
.db-type-badge.chroma { background: #e8f5e9; color: #2e7d32; }
.db-url-preview {
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
.enabled-badge {
  font-size: 11px;
  padding: 1px 6px;
  border-radius: 4px;
  flex-shrink: 0;
}
.enabled-badge.on { background: #e8f5e9; color: #2e7d32; }
.enabled-badge.off { background: #fbe9e7; color: #bf360c; }

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

.form-row { display: flex; gap: 12px; }
.form-group { flex: 1; }
.form-group label { display: block; font-size: 12px; margin-bottom: 4px; color: var(--text-secondary); }
.form-group input, .form-group select { width: 100%; box-sizing: border-box; }
.checkbox-group { display: flex; align-items: center; padding-top: 18px; }
.checkbox-group label { display: flex; align-items: center; gap: 6px; cursor: pointer; }
.checkbox-group input[type="checkbox"] { width: auto; }

.status-test-row {
  display: flex;
  align-items: center;
  gap: 8px;
  margin-top: 12px;
  padding-top: 12px;
  border-top: 1px dashed var(--border);
  margin-bottom: 0;
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

@media (max-width: 768px) {
  .card-summary { flex-wrap: wrap; gap: 4px; }
  .db-url-preview { width: 100%; order: 10; }
  .card-body-collapsible .form-row { flex-direction: column; gap: 8px; }
  .card-actions { flex-wrap: wrap; }
  .card-actions button { flex: 1; }
  .status-test-row { flex-wrap: wrap; }
}
</style>