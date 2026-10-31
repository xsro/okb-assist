<template>
  <div v-if="show" class="modal-overlay" @click.self="close">
    <div class="modal token-modal">
      <h3>账号</h3>

      <!-- 当前账号 -->
      <div class="current-account">
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

      <!-- 切换账号走「退出登录 → 登录页选择账号」，这里不再直接换号 -->
      <p class="logout-hint">
        需要换账号？点击下方「退出登录」，登录页会列出本机登录过的账号（仍需输入密码），也可以登录新账号。
      </p>

      <div class="modal-actions">
        <router-link class="btn btn-outline" to="/assist/account" @click="close">
          账户设置
        </router-link>
        <button class="btn btn-outline-danger" @click="doLogout">退出登录</button>
        <button class="btn" @click="close">关闭</button>
      </div>
    </div>
  </div>
</template>

<script setup lang="ts">
import { computed } from 'vue'
import { useRouter } from 'vue-router'
import { useTokenStore } from '@/stores/token'

const tokenStore = useTokenStore()
const router = useRouter()

/** 只作为已登录账号面板使用：未登录时统一走登录页 */
const show = computed(() => tokenStore.showTokenModal && tokenStore.isAuthenticated)

const viaLabel = computed(() => {
  const map: Record<string, string> = {
    session: '账号密码（会话）',
    api_token: '访问令牌',
    local_subnet: '受信任子网'
  }
  return map[tokenStore.via] || '—'
})

async function doLogout() {
  tokenStore.closeTokenModal()
  await tokenStore.logout()
  router.replace({ path: '/assist/login' })
}

function close() {
  tokenStore.closeTokenModal()
}
</script>

<style scoped>
.token-modal {
  max-width: 460px;
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

.logout-hint {
  margin-top: 14px;
  font-size: 12px;
  line-height: 1.6;
  color: var(--text-muted);
}
</style>
