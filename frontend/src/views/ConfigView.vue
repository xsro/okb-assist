<template>
  <div class="config-view">
    <h2>服务配置</h2>

    <div v-if="config" class="config-form">
      <!-- Base URL -->
      <div class="section">
        <h3>部署地址</h3>
        <div class="form-group">
          <label>Base URL</label>
          <input v-model="config.base_url" type="text" placeholder="https://xsro20.xyz" />
        </div>
        <p class="hint">部署的基础地址，用于拼接文档链接（pdf_url / markdown_url / detail_url）。留空则使用相对路径。</p>
      </div>

      <!-- MinerU 多配置 -->
      <div class="section">
        <div class="section-header">
          <h3>MinerU</h3>
          <button class="btn btn-sm btn-outline" @click="addMineruConfig">添加配置</button>
        </div>
        <div v-for="(mu, idx) in config.mineru" :key="idx" class="mineru-card">
          <div class="mineru-card-header">
            <span class="mineru-index">配置 #{{ idx + 1 }}</span>
            <div class="mineru-actions">
              <button class="btn btn-sm btn-outline" @click="moveUp(idx)" :disabled="idx === 0">↑</button>
              <button class="btn btn-sm btn-outline" @click="moveDown(idx)" :disabled="idx === config.mineru.length - 1">↓</button>
              <button class="btn btn-sm btn-danger" @click="removeMineruConfig(idx)" :disabled="config.mineru.length <= 1">删除</button>
            </div>
          </div>
          <div class="form-row">
            <div class="form-group">
              <label>类型</label>
              <select v-model="mu.type">
                <option value="local">本地服务</option>
                <option value="official">官方精准解析 API</option>
              </select>
            </div>
            <div class="form-group">
              <label>URL</label>
              <input v-model="mu.url" type="text" :placeholder="mu.type === 'official' ? 'https://mineru.net' : 'http://127.0.0.1:8002'" />
            </div>
          </div>
          <div class="form-row">
            <div class="form-group">
              <label>API Key</label>
              <input v-model="mu.key" type="password" :placeholder="mu.type === 'official' ? 'sk-...' : '本地服务无需填写'" />
            </div>
            <div class="form-group" v-if="mu.type === 'official'">
              <label>模型版本</label>
              <select v-model="mu.model_version">
                <option value="vlm">vlm（推荐）</option>
                <option value="pipeline">pipeline</option>
                <option value="MinerU-HTML">MinerU-HTML</option>
              </select>
            </div>
          </div>
          <div class="form-row">
            <div class="form-group">
              <label>任务超时 (秒)</label>
              <input v-model.number="mu.task_timeout" type="number" />
            </div>
            <div class="form-group">
              <label>最大并发任务数</label>
              <input v-model.number="mu.max_tasks" type="number" />
            </div>
          </div>
          <div class="status-test-row">
            <span class="status-dot" :class="mineruStatusClass(idx)"></span>
            <span class="status-text">{{ mineruStatusText(idx) }}</span>
            <button class="btn btn-sm btn-outline" @click="testMineru(idx)" :disabled="testingMineru[idx]">
              {{ testingMineru[idx] ? '测试中...' : '测试' }}
            </button>
          </div>
        </div>
        <p class="hint">解析 PDF 时按顺序逐个尝试以上配置，直到成功。</p>
      </div>

      <!-- Ollama -->
      <div class="section">
        <div class="section-header">
          <h3>Ollama</h3>
        </div>
        <div class="form-group">
          <label>URL</label>
          <input v-model="config.ollama.url" type="text" />
        </div>
        <div class="form-group">
          <label>API Key</label>
          <input v-model="config.ollama.key" type="password" />
        </div>
        <div class="form-group">
          <label>模型</label>
          <input v-model="config.ollama.model" type="text" />
        </div>
        <div class="status-test-row">
          <span class="status-dot" :class="ollamaStatusClass"></span>
          <span class="status-text">{{ ollamaStatusText }}</span>
          <button class="btn btn-sm btn-outline" @click="testOllama" :disabled="testingOllama">
            {{ testingOllama ? '测试中...' : '测试' }}
          </button>
        </div>
      </div>

      <!-- 向量库 -->
      <div class="section">
        <div class="section-header">
          <h3>向量库</h3>
          <button class="btn btn-sm btn-outline" @click="addVectorDb">添加向量库</button>
        </div>
        <div v-for="(db, idx) in config.vector_dbs" :key="idx" class="vector-db-card">
          <div class="form-row">
            <div class="form-group">
              <label>ID</label>
              <input v-model="db.id" type="text" />
            </div>
            <div class="form-group">
              <label>名称</label>
              <input v-model="db.name" type="text" />
            </div>
            <div class="form-group">
              <label>类型</label>
              <select v-model="db.type">
                <option value="qdrant">Qdrant</option>
                <option value="milvus">Milvus</option>
                <option value="chroma">Chroma</option>
              </select>
            </div>
          </div>
          <div class="form-row">
            <div class="form-group">
              <label>URL</label>
              <input v-model="db.url" type="text" />
            </div>
            <div class="form-group">
              <label>集合名</label>
              <input v-model="db.collection" type="text" />
            </div>
          </div>
          <div class="form-row">
            <div class="form-group">
              <label>API Key</label>
              <input v-model="db.api_key" type="password" />
            </div>
            <div class="form-group checkbox-group">
              <label>
                <input v-model="db.enabled" type="checkbox" />
                启用
              </label>
            </div>
          </div>
          <div class="form-row">
            <div class="form-group">
              <label>嵌入源</label>
              <input v-model="db.embedding.source" type="text" />
            </div>
            <div class="form-group">
              <label>嵌入模型</label>
              <input v-model="db.embedding.model" type="text" />
            </div>
          </div>
          <div class="status-test-row">
            <span class="status-dot" :class="vectorDbStatusClass(idx)"></span>
            <span class="status-text">{{ vectorDbStatusText(idx) }}</span>
            <button class="btn btn-sm btn-outline" @click="testVectorDb(idx)" :disabled="testingVectorDb[idx]">
              {{ testingVectorDb[idx] ? '测试中...' : '测试' }}
            </button>
          </div>
          <button class="btn btn-sm btn-danger" @click="removeVectorDb(idx)">删除</button>
        </div>
      </div>

      <div class="action-buttons">
        <button class="btn" @click="save">保存</button>
        <button class="btn btn-outline" @click="reload">重载</button>
        <button class="btn btn-outline" @click="reset">重置</button>
      </div>
    </div>

    <div v-else class="loading">加载中...</div>
  </div>
</template>

<script setup lang="ts">
import { ref, onMounted, reactive, computed } from 'vue'
import { getServiceConfig, updateServiceConfig, reloadConfig, testService } from '@/api/config'
import { getServiceStatus } from '@/api/admin'
import { useToast } from '@/composables/useToast'
import type { ServiceConfig, VectorDbConfig, MinerUConfig } from '@/types/config'

const { showSuccess, showError, showToast } = useToast()

const config = ref<ServiceConfig | null>(null)
const originalConfig = ref<ServiceConfig | null>(null)

// 测试状态
const testingMineru = ref<Record<number, boolean>>({})
const testingOllama = ref(false)
const testingVectorDb = ref<Record<number, boolean>>({})

// 各服务状态
const mineruStatus = ref<Record<number, { status: string; detail: string }>>({})
const ollamaStatus = ref<{ status: string; detail: string } | null>(null)
const vectorDbStatus = ref<Record<number, { status: string; detail: string }>>({})

function defaultMineruConfig(): MinerUConfig {
  return {
    type: 'local',
    url: 'http://127.0.0.1:8002',
    key: 'key',
    task_timeout: 300,
    model_version: 'vlm',
    max_tasks: 3
  }
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

async function load() {
  try {
    const raw = await getServiceConfig()
    // 确保 mineru 是数组
    if (!Array.isArray(raw.mineru)) {
      raw.mineru = [raw.mineru]
    }
    config.value = raw
    // 确保每个向量库都有嵌入字段
    for (const db of config.value.vector_dbs || []) {
      if (!db.embedding) {
        db.embedding = { source: 'ollama', model: 'nomic-embed-text' }
      }
    }
    originalConfig.value = JSON.parse(JSON.stringify(config.value))

    // 加载服务状态
    await loadServiceStatus()
  } catch {
    showError('加载配置失败')
  }
}

async function loadServiceStatus() {
  try {
    const sv = await getServiceStatus()
    // MinerU 状态
    const mineruItems = Array.isArray(sv.mineru) ? sv.mineru : [sv.mineru]
    for (let i = 0; i < mineruItems.length; i++) {
      const item = mineruItems[i]
      mineruStatus.value[i] = {
        status: item.status,
        detail: item.error || item.url || item.status || '-'
      }
    }
    // Ollama 状态
    if (sv.ollama) {
      ollamaStatus.value = {
        status: sv.ollama.status,
        detail: sv.ollama.error || sv.ollama.url || sv.ollama.status || '-'
      }
    }
    // 向量库状态
    const dbs = sv.vector_dbs || []
    for (let i = 0; i < dbs.length; i++) {
      const db = dbs[i]
      vectorDbStatus.value[i] = {
        status: db.status,
        detail: db.error || db.url || db.status || '-'
      }
    }
  } catch {
    // 加载状态失败不影响配置编辑
  }
}

async function save() {
  try {
    await updateServiceConfig(config.value!)
    originalConfig.value = JSON.parse(JSON.stringify(config.value))
    showSuccess('配置已保存')
    // 保存后刷新状态
    await loadServiceStatus()
  } catch {
    showError('保存失败')
  }
}

async function reload() {
  try {
    await reloadConfig()
    showSuccess('已重载配置')
    await load()
  } catch {
    showError('重载失败')
  }
}

function reset() {
  config.value = JSON.parse(JSON.stringify(originalConfig.value))
}

function addMineruConfig() {
  config.value?.mineru.push(defaultMineruConfig())
}

function removeMineruConfig(idx: number) {
  if ((config.value?.mineru.length ?? 0) <= 1) return
  config.value?.mineru.splice(idx, 1)
}

function moveUp(idx: number) {
  if (idx <= 0 || !config.value) return
  const arr = config.value.mineru
  const tmp = arr[idx]
  arr[idx] = arr[idx - 1]
  arr[idx - 1] = tmp
}

function moveDown(idx: number) {
  if (!config.value || idx >= config.value.mineru.length - 1) return
  const arr = config.value.mineru
  const tmp = arr[idx]
  arr[idx] = arr[idx + 1]
  arr[idx + 1] = tmp
}

function addVectorDb() {
  config.value?.vector_dbs.push(defaultVectorDb())
}

function removeVectorDb(idx: number) {
  config.value?.vector_dbs.splice(idx, 1)
}

// ── 连接测试 ──

async function testMineru(idx: number) {
  const mu = config.value?.mineru[idx]
  if (!mu) return
  testingMineru.value[idx] = true
  try {
    const res = await testService({
      service_type: 'mineru',
      url: mu.url,
      key: mu.type // 后端将 key 字段用作 mineru type
    })
    mineruStatus.value[idx] = { status: res.status, detail: res.detail }
    showToast(res.detail, res.status === 'connected' ? 'success' : 'error')
  } catch {
    showError('连接测试失败')
  } finally {
    testingMineru.value[idx] = false
  }
}

async function testOllama() {
  if (!config.value) return
  testingOllama.value = true
  try {
    const res = await testService({
      service_type: 'ollama',
      url: config.value.ollama.url,
      model: config.value.ollama.model
    })
    ollamaStatus.value = { status: res.status, detail: res.detail }
    showToast(res.detail, res.status === 'connected' ? 'success' : 'error')
  } catch {
    showError('连接测试失败')
  } finally {
    testingOllama.value = false
  }
}

async function testVectorDb(idx: number) {
  const db = config.value?.vector_dbs[idx]
  if (!db) return
  testingVectorDb.value[idx] = true
  try {
    const res = await testService({
      service_type: db.type,
      url: db.url,
      collection: db.collection
    })
    vectorDbStatus.value[idx] = { status: res.status, detail: res.detail }
    showToast(res.detail, res.status === 'connected' ? 'success' : 'error')
  } catch {
    showError('连接测试失败')
  } finally {
    testingVectorDb.value[idx] = false
  }
}

// ── 状态显示辅助函数 ──

function mineruStatusClass(idx: number): string {
  const s = mineruStatus.value[idx]?.status
  if (!s) return ''
  if (s === 'connected') return 'ok'
  if (s === 'disabled' || s === 'not_configured') return 'warning'
  return 'error'
}

function mineruStatusText(idx: number): string {
  const s = mineruStatus.value[idx]
  if (!s) return '未测试'
  if (s.status === 'connected') return '正常'
  if (s.status === 'disabled') return '已禁用'
  if (s.status === 'not_configured') return '未配置'
  if (s.status === 'disconnected') return '无法连接'
  if (s.status === 'error') return '连接错误'
  return s.status
}

const ollamaStatusClass = computed(() => {
  const s = ollamaStatus.value?.status
  if (!s) return ''
  if (s === 'connected') return 'ok'
  if (s === 'disabled' || s === 'not_configured') return 'warning'
  return 'error'
})

const ollamaStatusText = computed(() => {
  const s = ollamaStatus.value
  if (!s) return '未测试'
  if (s.status === 'connected') return '正常'
  if (s.status === 'disabled') return '已禁用'
  if (s.status === 'not_configured') return '未配置'
  if (s.status === 'disconnected') return '无法连接'
  if (s.status === 'error') return '连接错误'
  return s.status
})

function vectorDbStatusClass(idx: number): string {
  const s = vectorDbStatus.value[idx]?.status
  if (!s) return ''
  if (s === 'connected') return 'ok'
  if (s === 'disabled') return 'warning'
  if (s === 'not_configured') return 'warning'
  if (s === 'unsupported') return 'warning'
  return 'error'
}

function vectorDbStatusText(idx: number): string {
  const s = vectorDbStatus.value[idx]
  if (!s) return '未测试'
  if (s.status === 'connected') return '正常'
  if (s.status === 'disabled') return '已禁用'
  if (s.status === 'not_configured') return '未配置'
  if (s.status === 'unsupported') return '暂不支持'
  if (s.status === 'disconnected') return '无法连接'
  if (s.status === 'error') return '连接错误'
  return s.status
}

onMounted(load)
</script>

<style scoped>
.config-form .section {
  margin-bottom: 16px;
}
.section-header {
  display: flex;
  justify-content: space-between;
  align-items: center;
  margin-bottom: 12px;
}
.section-header h3 {
  margin: 0;
}
.mineru-card {
  padding: 16px;
  margin-bottom: 12px;
  background: #fff;
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
  color: var(--text-secondary, #888);
  margin: 8px 0 0;
}
.vector-db-card {
  padding: 16px;
  margin-bottom: 16px;
  background: #fff;
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
.status-dot.ok { background: var(--success, #16a34a); }
.status-dot.error { background: var(--danger, #dc2626); }
.status-dot.warning { background: var(--warning, #f59e0b); }
.status-dot.empty { background: var(--text-secondary, #999); }
.status-text {
  font-size: 12px;
  color: var(--text-secondary, #666);
  flex: 1;
}
.status-test-row .btn {
  margin-left: auto;
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

  .vector-db-card .form-group input,
  .vector-db-card .form-group select {
    font-size: 14px;
  }

  .action-buttons {
    flex-direction: column;
    gap: 8px;
  }

  .action-buttons .btn {
    width: 100%;
  }

  .status-test-row {
    flex-wrap: wrap;
    gap: 6px;
  }

  .status-test-row .btn {
    margin-left: 0;
    width: 100%;
  }
}

@media (max-width: 480px) {
  .vector-db-card {
    padding: 10px;
  }

  .vector-db-card .form-group label {
    font-size: 12px;
  }

  .status-text {
    font-size: 11px;
  }
}
</style>