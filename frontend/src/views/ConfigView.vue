<template>
  <div class="config-view">
    <h2>服务配置</h2>

    <div v-if="config" class="config-form">
      <!-- MinerU -->
      <div class="section">
        <h3>MinerU</h3>
        <div class="form-group">
          <label>URL</label>
          <input v-model="config.mineru.url" type="text" />
        </div>
        <div class="form-group">
          <label>API Key</label>
          <input v-model="config.mineru.key" type="password" />
        </div>
        <div class="form-group">
          <label>任务超时 (秒)</label>
          <input v-model.number="config.mineru.task_timeout" type="number" />
        </div>
      </div>

      <!-- Ollama -->
      <div class="section">
        <h3>Ollama</h3>
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
import { ref, onMounted } from 'vue'
import { getServiceConfig, updateServiceConfig, reloadConfig } from '@/api/config'
import { useToast } from '@/composables/useToast'
import type { ServiceConfig, VectorDbConfig } from '@/types/config'

const { showSuccess, showError } = useToast()

const config = ref<ServiceConfig | null>(null)
const originalConfig = ref<ServiceConfig | null>(null)

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
    config.value = await getServiceConfig()
    // 确保每个向量库都有嵌入字段
    for (const db of config.value.vector_dbs || []) {
      if (!db.embedding) {
        db.embedding = { source: 'ollama', model: 'nomic-embed-text' }
      }
    }
    originalConfig.value = JSON.parse(JSON.stringify(config.value))
  } catch {
    showError('加载配置失败')
  }
}

async function save() {
  try {
    await updateServiceConfig(config.value!)
    originalConfig.value = JSON.parse(JSON.stringify(config.value))
    showSuccess('配置已保存')
  } catch {
    showError('保存失败')
  }
}

async function reload() {
  try {
    await reloadConfig()
    showSuccess('已重载配置')
  } catch {
    showError('重载失败')
  }
}

function reset() {
  config.value = JSON.parse(JSON.stringify(originalConfig.value))
}

function addVectorDb() {
  config.value?.vector_dbs.push(defaultVectorDb())
}

function removeVectorDb(idx: number) {
  config.value?.vector_dbs.splice(idx, 1)
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
</style>
