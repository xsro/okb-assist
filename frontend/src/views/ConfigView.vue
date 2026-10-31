<template>
  <div class="config-view">
    <div class="page-head">
      <h2>配置</h2>
      <div class="config-tabs">
        <button
          class="config-tab"
          :class="{ active: activeTab === 'service' }"
          @click="switchTab('service')"
        >
          服务配置
        </button>
        <button
          class="config-tab"
          :class="{ active: activeTab === 'users' }"
          @click="switchTab('users')"
        >
          用户管理
        </button>
      </div>
    </div>

    <div v-show="activeTab === 'service'">
    <div v-if="config" class="config-form">
      <BaseUrlSection v-model="config" />

      <MineruConfigSection
        ref="mineruSectionRef"
        :configs="config.mineru"
        :active-key="config.active_mineru"
        @update="onConfigUpdate"
        @update:active-key="onActiveMineruKeyChange"
      />

      <LlmConfigSection
        :models="config.llm.models"
        @update="onConfigUpdate"
      />

      <LlmExtractConfigSection
        ref="llmExtractSectionRef"
        :extract="config.llm_extract"
        :models="config.llm.models"
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

    <!-- 用户管理（原独立页面，现并入配置页的 tab） -->
    <UsersView v-if="activeTab === 'users'" embedded />
  </div>
</template>

<script setup lang="ts">
import { computed, onMounted, ref, watch } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import { getServiceConfig, updateServiceConfig, reloadConfig } from '@/api/config'
import { getServiceStatus } from '@/api/admin'
import { useToast } from '@/composables/useToast'
import type { LlmModelEntry, ServiceConfig } from '@/types/config'
import BaseUrlSection from '@/components/config/BaseUrlSection.vue'
import MineruConfigSection from '@/components/config/MineruConfigSection.vue'
import LlmConfigSection from '@/components/config/LlmConfigSection.vue'
import LlmExtractConfigSection from '@/components/config/LlmExtractConfigSection.vue'
import VectorDbConfigSection from '@/components/config/VectorDbConfigSection.vue'
import UsersView from '@/views/UsersView.vue'

const { showSuccess, showError } = useToast()

const config = ref<ServiceConfig | null>(null)
const originalConfig = ref<ServiceConfig | null>(null)

const mineruSectionRef = ref<InstanceType<typeof MineruConfigSection> | null>(null)
const llmExtractSectionRef = ref<InstanceType<typeof LlmExtractConfigSection> | null>(null)
const vectorDbSectionRef = ref<InstanceType<typeof VectorDbConfigSection> | null>(null)

function onConfigUpdate() {
  // 子组件已直接修改响应式对象，无需额外操作
}

function onActiveMineruKeyChange(key: string) {
  if (config.value) {
    config.value.active_mineru = key
  }
}

async function load() {
  try {
    const raw = await getServiceConfig()
    if (!Array.isArray(raw.mineru)) {
      raw.mineru = [raw.mineru]
    }
    config.value = raw
    // 兼容旧配置：llm 模型池、抽取模型、各向量库 embedding 缺失字段时按默认值补齐
    const rawLlm = raw.llm as { models?: LlmModelEntry[] } & Partial<LlmModelEntry>
    if (!rawLlm || typeof rawLlm !== 'object') {
      raw.llm = { models: [] }
    } else if (!Array.isArray(rawLlm.models)) {
      // 单对象 llm（{protocol, url, key, model}）→ 模型池单项
      raw.llm = rawLlm.model !== undefined || rawLlm.url !== undefined
        ? { models: [{ name: 'default', protocol: rawLlm.protocol || 'ollama', url: rawLlm.url || '', key: rawLlm.key || '', model: rawLlm.model || '' }] }
        : { models: [] }
    }
    for (const model of config.value.llm.models) {
      model.name = model.name || ''
      model.protocol = model.protocol || 'ollama'
      model.url = model.url || ''
      model.key = model.key || ''
      model.model = model.model || ''
    }
    if (!config.value.llm_extract || typeof config.value.llm_extract !== 'object') {
      config.value.llm_extract = { from: '', protocol: '', url: '', key: '', model: '', prompt: '', max_input_chars: 6000, max_tokens: 2048, overwrite: true }
    } else {
      config.value.llm_extract.from = config.value.llm_extract.from || ''
      config.value.llm_extract.protocol = config.value.llm_extract.protocol || ''
      config.value.llm_extract.url = config.value.llm_extract.url || ''
      config.value.llm_extract.key = config.value.llm_extract.key || ''
      config.value.llm_extract.model = config.value.llm_extract.model || ''
      config.value.llm_extract.prompt = config.value.llm_extract.prompt || ''
      config.value.llm_extract.max_input_chars = config.value.llm_extract.max_input_chars || 6000
      config.value.llm_extract.max_tokens = config.value.llm_extract.max_tokens || 2048
      config.value.llm_extract.overwrite = config.value.llm_extract.overwrite !== false
    }
    for (const db of config.value.vector_dbs || []) {
      if (!db.embedding || typeof db.embedding !== 'object') {
        db.embedding = { protocol: 'ollama', url: '', key: '', model: 'nomic-embed-text' }
      } else {
        db.embedding.protocol = db.embedding.protocol || 'ollama'
        db.embedding.url = db.embedding.url || ''
        db.embedding.key = db.embedding.key || ''
        db.embedding.model = db.embedding.model || 'nomic-embed-text'
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
    // 信息补全模型（服务状态探测的就是它）
    if (sv.llm) {
      llmExtractSectionRef.value?.syncStatus({
        status: sv.llm.status,
        detail: sv.llm.error || sv.llm.url || sv.llm.status || '-'
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

// ── 页内 tab（服务配置 / 用户管理） ──
//
// 用户管理原先是独立路由 `/assist/users`，现并入配置页：
// - 选中状态与 `?tab=users` 同步，便于深链与刷新保持；
// - 用 v-show 保留服务配置的编辑状态，用户管理面板懒挂载（切到才请求数据）。

type ConfigTab = 'service' | 'users'

const route = useRoute()
const router = useRouter()

function tabFromQuery(): ConfigTab {
  return route.query.tab === 'users' ? 'users' : 'service'
}

const activeTab = ref<ConfigTab>(tabFromQuery())

function switchTab(next: ConfigTab) {
  if (activeTab.value === next) return
  activeTab.value = next
  const query = { ...route.query }
  if (next === 'users') {
    query.tab = 'users'
  } else {
    delete query.tab
  }
  void router.replace({ query })
}

// 外部通过链接切换 tab（如从 MCP 配置页跳「用户管理」）时同步
watch(
  () => route.query.tab,
  () => {
    activeTab.value = tabFromQuery()
  }
)

onMounted(() => {
  load()
})
</script>

<style scoped>
.page-head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
  flex-wrap: wrap;
  margin-bottom: 16px;
}
.page-head h2 {
  font-size: 20px;
  font-weight: 600;
}
.config-tabs {
  display: inline-flex;
  gap: 4px;
  background: var(--bg);
  padding: 4px;
  border-radius: 8px;
}
.config-tab {
  border: none;
  background: transparent;
  padding: 7px 16px;
  border-radius: 6px;
  font-size: 14px;
  color: var(--text-secondary);
  cursor: pointer;
  min-height: 34px;
}
.config-tab:hover {
  color: var(--text);
}
.config-tab.active {
  background: var(--bg-white);
  color: var(--primary);
  font-weight: 600;
  box-shadow: 0 1px 4px rgba(0, 0, 0, 0.08);
}

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

/* ── 权限管理 ── */
.section-desc {
  color: var(--text-secondary);
  font-size: 13px;
  margin-top: -8px;
  margin-bottom: 16px;
}

.perm-select {
  padding: 6px 10px;
  border: 1px solid var(--border);
  border-radius: 6px;
  font-size: 13px;
  background: var(--bg-white);
  color: var(--text);
  outline: none;
  cursor: pointer;
  min-width: 140px;
}
.perm-select:focus {
  border-color: var(--primary);
}

.perm-token-role {
  display: inline-block;
  padding: 2px 8px;
  border-radius: 4px;
  font-size: 11px;
  font-weight: 600;
  white-space: nowrap;
  flex-shrink: 0;
}
.perm-token-role.role-view-only {
  background: #e3f2fd;
  color: #1565c0;
}
.perm-token-role.role-view-upload {
  background: #e8f5e9;
  color: #2e7d32;
}

.perm-token-list {
  display: flex;
  flex-direction: column;
  gap: 6px;
  margin-bottom: 12px;
}

.perm-token-row {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 8px;
  padding: 6px 10px;
  background: var(--bg-white);
  border: 1px solid var(--border);
  border-radius: 6px;
}

.perm-token-value {
  font-family: 'SF Mono', 'Cascadia Code', 'Fira Code', monospace;
  font-size: 12px;
  color: var(--text);
  word-break: break-all;
  flex: 1;
}

.btn-outline-danger {
  color: #f44336;
  border-color: #f44336;
  background: transparent;
  white-space: nowrap;
}
.btn-outline-danger:hover {
  background: #f44336;
  color: #fff;
}

.perm-add-row {
  display: flex;
  gap: 8px;
}

.perm-input {
  flex: 1;
  padding: 6px 10px;
  border: 1px solid var(--border);
  border-radius: 6px;
  font-size: 13px;
  font-family: 'SF Mono', 'Cascadia Code', 'Fira Code', monospace;
  outline: none;
}
.perm-input:focus {
  border-color: var(--primary);
  box-shadow: 0 0 0 2px rgba(25, 118, 210, 0.12);
}

.perm-mcp-label {
  display: flex;
  align-items: center;
  gap: 4px;
  font-size: 13px;
  color: var(--text);
  white-space: nowrap;
  cursor: pointer;
}

.perm-mcp-label input[type="checkbox"] {
  width: 16px;
  height: 16px;
  cursor: pointer;
}

.perm-token-mcp-badge {
  display: inline-block;
  padding: 2px 6px;
  border-radius: 4px;
  font-size: 10px;
  font-weight: 700;
  background: #fff3e0;
  color: #e65100;
  flex-shrink: 0;
  letter-spacing: 0.5px;
}

.error-message {
  color: #f44336;
  font-size: 13px;
  margin-top: 8px;
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