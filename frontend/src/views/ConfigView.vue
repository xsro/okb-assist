<template>
  <div class="config-view">
    <h2>服务配置</h2>

    <div v-if="config" class="config-form">
      <BaseUrlSection v-model="config" />

      <MineruConfigSection
        ref="mineruSectionRef"
        :configs="config.mineru"
        @update="onConfigUpdate"
      />

      <OllamaConfigSection
        ref="ollamaSectionRef"
        :ollama="config.ollama"
        @update="onConfigUpdate"
      />

      <VectorDbConfigSection
        ref="vectorDbSectionRef"
        :dbs="config.vector_dbs"
        @update="onConfigUpdate"
      />

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
import { getServiceStatus } from '@/api/admin'
import { useToast } from '@/composables/useToast'
import type { ServiceConfig } from '@/types/config'
import BaseUrlSection from '@/components/config/BaseUrlSection.vue'
import MineruConfigSection from '@/components/config/MineruConfigSection.vue'
import OllamaConfigSection from '@/components/config/OllamaConfigSection.vue'
import VectorDbConfigSection from '@/components/config/VectorDbConfigSection.vue'

const { showSuccess, showError } = useToast()

const config = ref<ServiceConfig | null>(null)
const originalConfig = ref<ServiceConfig | null>(null)

const mineruSectionRef = ref<InstanceType<typeof MineruConfigSection> | null>(null)
const ollamaSectionRef = ref<InstanceType<typeof OllamaConfigSection> | null>(null)
const vectorDbSectionRef = ref<InstanceType<typeof VectorDbConfigSection> | null>(null)

function onConfigUpdate() {
  // 子组件已直接修改响应式对象，无需额外操作
}

async function load() {
  try {
    const raw = await getServiceConfig()
    if (!Array.isArray(raw.mineru)) {
      raw.mineru = [raw.mineru]
    }
    config.value = raw
    for (const db of config.value.vector_dbs || []) {
      if (!db.embedding) {
        db.embedding = { source: 'ollama', model: 'nomic-embed-text' }
      }
    }
    originalConfig.value = JSON.parse(JSON.stringify(config.value))
    await syncServiceStatus()
  } catch {
    showError('加载配置失败')
  }
}

async function syncServiceStatus() {
  try {
    const sv = await getServiceStatus()
    // MinerU
    const mineruItems = Array.isArray(sv.mineru) ? sv.mineru : [sv.mineru]
    for (let i = 0; i < mineruItems.length; i++) {
      const item = mineruItems[i]
      mineruSectionRef.value?.syncStatus(i, {
        status: item.status,
        detail: item.error || item.url || item.status || '-'
      })
    }
    // Ollama
    if (sv.ollama) {
      ollamaSectionRef.value?.syncStatus({
        status: sv.ollama.status,
        detail: sv.ollama.error || sv.ollama.url || sv.ollama.status || '-'
      })
    }
    // 向量库
    const dbs = sv.vector_dbs || []
    for (let i = 0; i < dbs.length; i++) {
      const db = dbs[i]
      vectorDbSectionRef.value?.syncStatus(i, {
        status: db.status,
        detail: db.error || db.url || db.status || '-'
      })
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
    await syncServiceStatus()
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
.action-buttons {
  display: flex;
  gap: 12px;
  margin-top: 24px;
}
.loading {
  text-align: center;
  padding: 60px 20px;
  color: var(--text-secondary);
}

@media (max-width: 768px) {
  .action-buttons {
    flex-direction: column;
    gap: 8px;
  }
  .action-buttons .btn {
    width: 100%;
  }
}
</style>