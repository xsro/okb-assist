<template>
  <div class="section">
    <div class="section-header">
      <h3>向量库</h3>
      <button class="btn btn-sm btn-outline" @click="add">添加向量库</button>
    </div>
    <div v-for="(db, idx) in dbs" :key="idx" class="vector-db-card">
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
          <input v-model="db.api_key" type="password" @input="emitUpdate" />
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
          {{ testing[idx] ? '测试中...' : '测试' }}
        </button>
      </div>
      <button class="btn btn-sm btn-danger" @click="remove(idx)">删除</button>
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

const testing = reactive<Record<number, boolean>>({})
const statuses = reactive<Record<number, { status: string; detail: string }>>({})

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
  padding: 16px;
  margin-bottom: 16px;
  background: var(--bg-white);
  border: 1px solid var(--border);
  border-radius: 8px;
}
.checkbox-group {
  display: flex;
  align-items: center;
}
.checkbox-group label {
  display: flex;
  align-items: center;
  gap: 6px;
  cursor: pointer;
}
.checkbox-group input {
  width: auto;
}
.status-test-row {
  display: flex;
  align-items: center;
  gap: 8px;
  margin-top: 12px;
  padding-top: 12px;
  border-top: 1px dashed var(--border);
  margin-bottom: 8px;
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

@media (max-width: 768px) {
  .vector-db-card {
    padding: 12px;
  }
  .vector-db-card .form-row {
    flex-direction: column;
    gap: 0;
  }
  .vector-db-card .form-group {
    width: 100%;
  }
  .status-test-row {
    flex-wrap: wrap;
    gap: 6px;
  }
}
</style>