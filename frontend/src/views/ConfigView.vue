<template>
  <div class="config-view">
    <h2>服务配置</h2>

    <div v-if="config" class="config-form">
      <BaseUrlSection v-model="config" />

      <MineruConfigSection
        ref="mineruSectionRef"
        :configs="config.mineru"
        :active-key="config.active_mineru"
        @update="onConfigUpdate"
        @update:active-key="onActiveMineruKeyChange"
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

      <!-- 权限管理 -->
      <div class="section">
        <h3>权限管理</h3>
        <p class="section-desc">管理角色 token，支持 view-only、view-upload</p>

        <div v-if="permEntries.length > 0" class="perm-token-list">
          <div v-for="(entry, idx) in permEntries" :key="idx" class="perm-token-row">
            <code class="perm-token-value">{{ entry.token }}</code>
            <span class="perm-token-role" :class="'role-' + entry.role">{{ roleLabel(entry.role) }}</span>
            <button class="btn btn-sm btn-outline-danger" @click="removeToken(entry.token)" :disabled="savingPerm">删除</button>
          </div>
        </div>
        <div v-else class="empty-hint">暂无权限 token</div>

        <div class="perm-add-row">
          <select v-model="newPermRole" class="perm-select">
            <option value="view-only">view-only（只读）</option>
            <option value="view-upload">view-upload（查看+上传）</option>
          </select>
          <input
            v-model="newPermToken"
            type="text"
            :placeholder="'输入 ' + newPermRole + ' token'"
            class="perm-input"
            @keyup.enter="addToken"
          />
          <button class="btn btn-sm" @click="addToken" :disabled="savingPerm || !newPermToken.trim()">
            添加
          </button>
        </div>

        <div v-if="permError" class="error-message">{{ permError }}</div>
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
import { ref, computed, onMounted } from 'vue'
import { getServiceConfig, updateServiceConfig, reloadConfig } from '@/api/config'
import { getServiceStatus, getPermissionTokens, addPermissionToken, deletePermissionToken } from '@/api/admin'
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

// ── 权限管理 ──

interface PermEntry { token: string; role: string }

const permTokens = ref<Record<string, string>>({})
const newPermToken = ref('')
const newPermRole = ref('view-only')
const savingPerm = ref(false)
const permError = ref('')

const permEntries = computed<PermEntry[]>(() => {
  return Object.entries(permTokens.value).map(([token, role]) => ({ token, role }))
})

function roleLabel(role: string): string {
  const labels: Record<string, string> = {
    'view-only': '只读',
    'view-upload': '查看+上传'
  }
  return labels[role] || role
}

async function loadPermTokens() {
  try {
    const res = await getPermissionTokens()
    permTokens.value = (res.permissions || {}) as Record<string, string>
  } catch {
    // 静默忽略
  }
}

async function addToken() {
  const token = newPermToken.value.trim()
  if (!token) return
  savingPerm.value = true
  permError.value = ''
  try {
    const res = await addPermissionToken(newPermRole.value, token)
    permTokens.value = (res.permissions || {}) as Record<string, string>
    newPermToken.value = ''
  } catch {
    permError.value = '添加失败，请重试'
  } finally {
    savingPerm.value = false
  }
}

async function removeToken(token: string) {
  const role = permTokens.value[token]
  if (!role) return
  if (!confirm(`确定删除 token: ${token}（${roleLabel(role)}）？`)) return
  savingPerm.value = true
  permError.value = ''
  try {
    const res = await deletePermissionToken(role, token)
    permTokens.value = (res.permissions || {}) as Record<string, string>
  } catch {
    permError.value = '删除失败，请重试'
  } finally {
    savingPerm.value = false
  }
}

onMounted(() => {
  load()
  loadPermTokens()
})
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