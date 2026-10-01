<template>
  <div v-if="visible" class="modal-overlay" @click.self="close">
    <div class="modal token-modal">
      <h3>{{ titleText }}</h3>

      <!-- 当前 Token（已登录时显示） -->
      <div v-if="isAuthenticated && !verifying" class="current-token-section">
        <div class="current-token-row">
          <code class="token-value">{{ tokenMasked ? maskToken(tokenStore.token) : tokenStore.token }}</code>
          <button class="btn-icon" @click="tokenMasked = !tokenMasked" :title="tokenMasked ? '显示' : '隐藏'">
            {{ tokenMasked ? '👁️' : '🙈' }}
          </button>
        </div>
        <div class="current-role" :class="'role-' + (tokenStore.role || '')">
          {{ roleLabel(tokenStore.role || '') }}
        </div>
      </div>

      <!-- 已保存的 token 历史 -->
      <div v-if="!verifying" class="token-history">
        <div
          v-for="(item, idx) in tokenHistory"
          :key="idx"
          class="history-item"
          @click="selectHistory(item.token)"
        >
          <code class="history-token">{{ maskToken(item.token) }}</code>
          <span class="history-role" :class="'role-' + item.role">{{ roleLabel(item.role) }}</span>
          <button class="history-remove" @click.stop="removeHistory(item.token)" title="从历史中移除">×</button>
        </div>
      </div>

      <!-- 输入新 token -->
      <p class="input-label">{{ isAuthenticated ? '切换到其他 token：' : '输入 token：' }}</p>
      <div class="token-input-row">
        <input
          v-model="inputToken"
          :type="showInputToken ? 'text' : 'password'"
          placeholder="输入 Token"
          @keyup.enter="submit"
          :disabled="verifying"
          ref="inputRef"
        />
        <button class="btn-icon" @click="showInputToken = !showInputToken" :title="showInputToken ? '隐藏' : '显示'">
          {{ showInputToken ? '🙈' : '👁️' }}
        </button>
      </div>

      <div v-if="errorMsg" class="error-msg">{{ errorMsg }}</div>
      <div v-if="verifying" class="verifying">正在验证 token 并获取权限...</div>

      <div class="modal-actions">
        <button class="btn" @click="submit" :disabled="verifying || !inputToken.trim()">
          {{ verifying ? '验证中...' : '确定' }}
        </button>
        <button v-if="isAuthenticated" class="btn btn-outline-danger" @click="logout" :disabled="verifying">登出</button>
        <button class="btn btn-outline" @click="close" :disabled="verifying">{{ isAuthenticated ? '关闭' : '取消' }}</button>
      </div>
    </div>
  </div>
</template>

<script setup lang="ts">
import { computed, ref } from 'vue'
import { useTokenStore } from '@/stores/token'

const tokenStore = useTokenStore()

const visible = computed({
  get: () => tokenStore.showTokenModal,
  set: (v) => {
    if (!v) tokenStore.closeTokenModal()
  }
})

const inputToken = ref('')
const verifying = ref(false)
const errorMsg = ref('')
const showInputToken = ref(false)
const tokenMasked = ref(true)
const inputRef = ref<HTMLInputElement>()

const isAuthenticated = computed(() => tokenStore.isAuthenticated)
const tokenHistory = computed(() => tokenStore.tokenHistory)

const titleText = computed(() => {
  if (verifying.value) return '验证中...'
  return isAuthenticated.value ? 'Token 管理' : '访问 Token'
})

function roleLabel(role: string): string {
  const labels: Record<string, string> = {
    'admin': '管理员',
    'view-only': '只读',
    'view-upload': '查看+上传'
  }
  return labels[role] || role
}

function maskToken(token: string): string {
  if (!token) return ''
  if (token.length <= 8) return token.slice(0, 4) + '****'
  return token.slice(0, 6) + '****' + token.slice(-4)
}

async function doLogin(token: string) {
  verifying.value = true
  errorMsg.value = ''
  try {
    await tokenStore.setToken(token)
    if (tokenStore.role) {
      tokenStore.closeTokenModal()
      inputToken.value = ''
    } else {
      errorMsg.value = 'token 验证失败，请检查后重试'
    }
  } catch {
    errorMsg.value = 'token 无效或请求失败'
  } finally {
    verifying.value = false
  }
}

async function submit() {
  const token = inputToken.value.trim()
  if (!token) return
  await doLogin(token)
}

function selectHistory(token: string) {
  doLogin(token)
}

function removeHistory(token: string) {
  tokenStore.removeFromHistory(token)
}

function logout() {
  tokenStore.clearToken()
  inputToken.value = ''
  errorMsg.value = ''
  tokenMasked.value = true
}

function close() {
  if (verifying.value) return
  tokenStore.closeTokenModal()
  inputToken.value = ''
  errorMsg.value = ''
  tokenMasked.value = true
}
</script>

<style scoped>
.token-modal {
  max-width: 480px;
}

.modal-actions {
  display: flex;
  gap: 8px;
  justify-content: flex-end;
  margin-top: 20px;
}

.modal p {
  color: var(--text-secondary);
  font-size: 14px;
}

/* 当前 Token */
.current-token-section {
  display: flex;
  align-items: center;
  gap: 10px;
  padding: 12px 16px;
  background: var(--bg);
  border: 1px solid var(--border);
  border-radius: 8px;
  margin-bottom: 16px;
}

.current-token-row {
  display: flex;
  align-items: center;
  gap: 6px;
  flex: 1;
  min-width: 0;
}

.token-value {
  flex: 1;
  font-family: 'SF Mono', 'Cascadia Code', 'Fira Code', monospace;
  font-size: 13px;
  color: var(--text);
  word-break: break-all;
  padding: 4px 8px;
  background: var(--bg-white);
  border-radius: 4px;
  border: 1px solid var(--border);
}

.current-role {
  flex-shrink: 0;
  padding: 2px 10px;
  border-radius: 4px;
  font-size: 12px;
  font-weight: 600;
  white-space: nowrap;
}
.current-role.role-admin {
  background: #e3f2fd;
  color: #1565c0;
}
.current-role.role-view-only {
  background: #e8f5e9;
  color: #2e7d32;
}
.current-role.role-view-upload {
  background: #f3e5f5;
  color: #7b1fa2;
}

.input-label {
  margin-top: 8px;
}

.token-input-row {
  display: flex;
  gap: 8px;
  align-items: center;
  margin-top: 8px;
}

.token-input-row input {
  flex: 1;
}

.error-msg {
  color: #f44336;
  font-size: 13px;
  margin-top: 8px;
}

.verifying {
  color: var(--text-secondary);
  font-size: 13px;
  margin-top: 8px;
}

.btn-icon {
  background: none;
  border: 1px solid var(--border);
  border-radius: 6px;
  cursor: pointer;
  padding: 6px 10px;
  font-size: 14px;
  line-height: 1;
  flex-shrink: 0;
}
.btn-icon:hover {
  background: var(--bg-secondary);
}

/* 历史记录 */
.token-history {
  margin-bottom: 12px;
  max-height: 200px;
  overflow-y: auto;
}

.history-item {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 8px 10px;
  border: 1px solid var(--border);
  border-radius: 6px;
  margin-bottom: 6px;
  cursor: pointer;
  transition: background 0.1s;
  background: var(--bg-white);
}
.history-item:hover {
  background: var(--bg-secondary);
  border-color: var(--primary);
}

.history-token {
  font-family: 'SF Mono', 'Cascadia Code', 'Fira Code', monospace;
  font-size: 12px;
  color: var(--text);
  flex: 1;
  word-break: break-all;
}

.history-role {
  display: inline-block;
  padding: 1px 6px;
  border-radius: 3px;
  font-size: 10px;
  font-weight: 600;
  white-space: nowrap;
  flex-shrink: 0;
}
.history-role.role-admin {
  background: #e3f2fd;
  color: #1565c0;
}
.history-role.role-view-only {
  background: #e8f5e9;
  color: #2e7d32;
}
.history-role.role-view-upload {
  background: #f3e5f5;
  color: #7b1fa2;
}

.history-remove {
  background: none;
  border: none;
  color: #999;
  font-size: 18px;
  cursor: pointer;
  padding: 0 4px;
  line-height: 1;
  flex-shrink: 0;
}
.history-remove:hover {
  color: #f44336;
}
</style>