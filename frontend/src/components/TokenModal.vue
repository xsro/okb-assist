<template>
  <div v-if="visible" class="modal-overlay" @click.self="close">
    <div class="modal token-modal">
      <h3>{{ titleText }}</h3>

      <!-- 已登录：显示当前账号 -->
      <div v-if="isAuthenticated" class="current-account">
        <div class="account-main">
          <span class="account-name">{{ tokenStore.displayName || '已登录' }}</span>
          <span class="account-role" :class="'role-' + (tokenStore.role || '')">
            {{ tokenStore.roleLabel }}
          </span>
        </div>
        <div class="account-meta">
          <span>凭证：{{ viaLabel }}</span>
          <span v-if="tokenStore.user?.username">用户名：{{ tokenStore.user?.username }}</span>
        </div>
        <div v-if="tokenStore.mustSetPassword" class="account-warn">
          当前仍是引导凭证，请到「账户」页设置密码。
        </div>
      </div>

      <!-- Token 登录 / 切换账号 -->
      <div class="token-login-section">
        <p class="input-label">
          {{ isAuthenticated ? '切换到其他账号或令牌：' : '也可直接粘贴 Token 登录：' }}
        </p>
        <div class="token-input-row">
          <input
            v-model="inputToken"
            :type="showInputToken ? 'text' : 'password'"
            placeholder="okb_… / sess_…"
            @keyup.enter="submit"
            :disabled="verifying"
            ref="inputRef"
          />
          <button class="btn-icon" @click="showInputToken = !showInputToken" :title="showInputToken ? '隐藏' : '显示'">
            {{ showInputToken ? '🙈' : '👁️' }}
          </button>
        </div>
        <div v-if="errorMsg" class="error-msg">{{ errorMsg }}</div>
        <div v-if="verifying" class="verifying">正在验证…</div>
        <div class="token-actions">
          <button class="btn btn-sm" @click="submit" :disabled="verifying || !inputToken.trim()">
            {{ verifying ? '验证中…' : 'Token 登录' }}
          </button>
        </div>
      </div>

      <!-- 历史 token -->
      <div v-if="tokenHistory.length && !verifying" class="token-history">
        <div class="history-title">最近使用</div>
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

      <div class="modal-actions">
        <router-link
          v-if="isAuthenticated"
          class="btn btn-outline"
          to="/assist/account"
          @click="close"
        >
          账户设置
        </router-link>
        <button v-if="isAuthenticated" class="btn btn-outline-danger" @click="doLogout" :disabled="verifying">
          退出登录
        </button>
        <button class="btn" @click="close" :disabled="verifying">
          {{ isAuthenticated ? '关闭' : '去登录页' }}
        </button>
      </div>
    </div>
  </div>
</template>

<script setup lang="ts">
import { computed, ref } from 'vue'
import { useRouter } from 'vue-router'
import { useTokenStore } from '@/stores/token'

const tokenStore = useTokenStore()
const router = useRouter()

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
const inputRef = ref<HTMLInputElement>()

const isAuthenticated = computed(() => tokenStore.isAuthenticated && !!tokenStore.role)
const tokenHistory = computed(() => tokenStore.tokenHistory)

const titleText = computed(() => (isAuthenticated.value ? '账号' : 'Token 登录'))

const viaLabel = computed(() => {
  const map: Record<string, string> = {
    session: '账号密码（会话）',
    api_token: '访问令牌',
    local_subnet: '受信任子网'
  }
  return map[tokenStore.via] || '—'
})

function roleLabel(role: string): string {
  const labels: Record<string, string> = {
    admin: '管理员',
    'view-only': '只读',
    'view-upload': '查看+上传'
  }
  return labels[role] || role
}

function maskToken(token: string): string {
  if (!token) return ''
  if (token.length <= 12) return token.slice(0, 4) + '****'
  return token.slice(0, 6) + '****' + token.slice(-4)
}

async function submit() {
  const token = inputToken.value.trim()
  if (!token) return
  verifying.value = true
  errorMsg.value = ''
  try {
    const ok = await tokenStore.setToken(token)
    if (ok) {
      tokenStore.closeTokenModal()
      inputToken.value = ''
      router.push('/assist')
    } else {
      errorMsg.value = 'Token 无效或已被撤销'
    }
  } catch {
    errorMsg.value = 'Token 验证失败'
  } finally {
    verifying.value = false
  }
}

function selectHistory(token: string) {
  inputToken.value = token
  submit()
}

function removeHistory(token: string) {
  tokenStore.removeFromHistory(token)
}

async function doLogout() {
  await tokenStore.logout()
  inputToken.value = ''
  errorMsg.value = ''
  tokenStore.closeTokenModal()
  router.replace({ path: '/assist/login' })
}

function close() {
  if (verifying.value) return
  tokenStore.closeTokenModal()
  inputToken.value = ''
  errorMsg.value = ''
  // 未登录时「关闭」等价于去登录页
  if (!isAuthenticated.value) {
    router.push({ path: '/assist/login' })
  }
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
  flex-wrap: wrap;
}

.modal p {
  color: var(--text-secondary);
  font-size: 14px;
}

/* 当前账号 */
.current-account {
  padding: 12px 14px;
  background: var(--bg);
  border: 1px solid var(--border);
  border-radius: 8px;
  margin-bottom: 16px;
}

.account-main {
  display: flex;
  align-items: center;
  gap: 10px;
  flex-wrap: wrap;
}

.account-name {
  font-size: 15px;
  font-weight: 600;
  color: var(--text);
}

.account-role {
  padding: 2px 10px;
  border-radius: 4px;
  font-size: 12px;
  font-weight: 600;
}
.role-admin {
  background: #e3f2fd;
  color: #1565c0;
}
.role-view-only {
  background: #e8f5e9;
  color: #2e7d32;
}
.role-view-upload {
  background: #f3e5f5;
  color: #7b1fa2;
}

.account-meta {
  display: flex;
  flex-direction: column;
  gap: 2px;
  margin-top: 8px;
  font-size: 12px;
  color: var(--text-secondary);
}

.account-warn {
  margin-top: 10px;
  padding: 8px 10px;
  border-radius: 6px;
  background: var(--warning-bg);
  color: var(--warning-text);
  font-size: 12px;
}

.input-label {
  margin-top: 4px;
}

.token-input-row {
  display: flex;
  gap: 8px;
  align-items: center;
  margin-top: 8px;
}

.token-input-row input {
  flex: 1;
  padding: 10px 12px;
  border: 1px solid var(--border);
  border-radius: 6px;
  font-size: 14px;
}

.token-input-row input:focus {
  outline: none;
  border-color: var(--primary);
  box-shadow: 0 0 0 2px rgba(25, 118, 210, 0.15);
}

.token-actions {
  display: flex;
  justify-content: flex-end;
  margin-top: 8px;
}

.error-msg {
  color: var(--danger);
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
  padding: 8px 10px;
  font-size: 14px;
  line-height: 1;
  flex-shrink: 0;
}
.btn-icon:hover {
  background: var(--bg-secondary);
}

/* 历史记录 */
.token-history {
  margin-top: 16px;
  max-height: 200px;
  overflow-y: auto;
}

.history-title {
  font-size: 12px;
  color: var(--text-muted);
  margin-bottom: 6px;
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
  font-family: 'SF Mono', 'Cascadia Code', monospace;
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
  color: var(--danger);
}
</style>
