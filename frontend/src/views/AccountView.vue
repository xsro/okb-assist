<template>
  <div class="account-view container">
    <h2 class="page-title">账户</h2>

    <!-- 需强制改密提示 -->
    <div v-if="store.mustSetPassword" class="banner banner-warning">
      当前账号使用的是引导凭证，请尽快设置密码（设置后该引导 token 会自动失效）。
    </div>

    <!-- 基本信息 -->
    <section class="section">
      <h3>基本信息</h3>
      <table class="info-table">
        <tbody>
          <tr>
            <th>用户名</th>
            <td>{{ store.user?.username || '—' }}</td>
          </tr>
          <tr>
            <th>显示名</th>
            <td>{{ store.user?.display_name || '—' }}</td>
          </tr>
          <tr>
            <th>角色</th>
            <td><span class="badge badge-info">{{ store.roleLabel }}</span></td>
          </tr>
          <tr>
            <th>MCP 访问</th>
            <td>
              <span class="badge" :class="store.canMcp ? 'badge-success' : 'badge-gray'">
                {{ store.canMcp ? '已开启' : '未开启' }}
              </span>
            </td>
          </tr>
          <tr>
            <th>登录方式</th>
            <td>{{ viaLabel }}</td>
          </tr>
          <tr>
            <th>上次登录</th>
            <td>{{ formatTime(store.user?.last_login_at) }}</td>
          </tr>
        </tbody>
      </table>
    </section>

    <!-- 修改密码 -->
    <section class="section">
      <h3>修改密码</h3>
      <form class="pwd-form" @submit.prevent="submitPassword">
        <div class="form-group">
          <label>当前密码{{ store.user?.has_password ? '（必填）' : '（尚无密码，可留空）' }}</label>
          <input v-model="pwd.old" type="password" autocomplete="current-password" />
        </div>
        <div class="form-group">
          <label>新密码（至少 8 位）</label>
          <input v-model="pwd.next" type="password" autocomplete="new-password" />
        </div>
        <div class="form-group">
          <label>确认新密码</label>
          <input v-model="pwd.next2" type="password" autocomplete="new-password" />
        </div>
        <div class="form-actions">
          <button class="btn" type="submit" :disabled="pwdBusy">
            {{ pwdBusy ? '提交中…' : '更新密码' }}
          </button>
          <span class="form-hint">更新后所有已登录会话会被撤销，需要用新密码重新登录。</span>
        </div>
      </form>
    </section>

    <!-- 个人访问令牌 -->
    <section class="section">
      <h3>个人访问令牌（Zotero / MCP 客户端）</h3>
      <p class="section-hint">
        令牌以 <code>okb_…</code> 开头，可用于 <code>X-Token</code> 头或
        <code>Authorization: Bearer</code>（MCP）。明文只在创建时显示一次。
      </p>

      <div class="token-create">
        <div class="form-group inline">
          <label>备注</label>
          <input v-model="newToken.label" type="text" placeholder="如 zotero / obsidian" />
        </div>
        <label class="checkbox">
          <input v-model="newToken.mcp" type="checkbox" :disabled="!store.user?.mcp" />
          允许访问 MCP
        </label>
        <button class="btn btn-sm" :disabled="tokenBusy" @click="createToken">
          {{ tokenBusy ? '创建中…' : '创建令牌' }}
        </button>
      </div>
      <p v-if="!store.user?.mcp" class="section-hint muted">
        当前账号未获得 MCP 权限，如需在 MCP 客户端里查询文献库，请联系管理员在用户管理中开启。
      </p>

      <div v-if="issuedToken" class="issued-token">
        <div class="issued-token-label">新令牌（请立即复制保存，关闭后无法再查看）</div>
        <code class="issued-token-value">{{ issuedToken }}</code>
        <div class="issued-token-actions">
          <button class="btn btn-sm" @click="copyToken">复制</button>
          <button class="btn btn-sm btn-outline" @click="issuedToken = ''">我已保存</button>
        </div>
      </div>

      <table v-if="tokens.length" class="token-table">
        <thead>
          <tr>
            <th>备注</th>
            <th>MCP</th>
            <th>创建时间</th>
            <th>最后使用</th>
            <th></th>
          </tr>
        </thead>
        <tbody>
          <tr v-for="t in tokens" :key="t.id">
            <td>{{ t.label || '—' }}</td>
            <td>{{ t.mcp ? '是' : '否' }}</td>
            <td>{{ formatTime(t.created_at) }}</td>
            <td>{{ formatTime(t.last_used_at) }}</td>
            <td class="row-actions">
              <button class="btn btn-sm btn-outline-danger" @click="revokeToken(t.id)">撤销</button>
            </td>
          </tr>
        </tbody>
      </table>
      <p v-else class="empty-hint">还没有个人令牌</p>
    </section>
  </div>
</template>

<script setup lang="ts">
import { computed, onMounted, reactive, ref } from 'vue'
import { useRouter } from 'vue-router'
import { issueMyToken, listMyTokens, revokeMyToken } from '@/api/auth'
import { useTokenStore } from '@/stores/token'
import { useToast } from '@/composables/useToast'
import type { ApiTokenMeta } from '@/types/auth'

const store = useTokenStore()
const router = useRouter()
const toast = useToast()

const tokens = ref<ApiTokenMeta[]>([])
const tokenBusy = ref(false)
const pwdBusy = ref(false)
const issuedToken = ref('')
const newToken = reactive({ label: '', mcp: false })
const pwd = reactive({ old: '', next: '', next2: '' })

const viaLabel = computed(() => {
  const map: Record<string, string> = {
    session: '账号密码（会话）',
    api_token: '访问令牌',
    local_subnet: '受信任子网'
  }
  return map[store.via] || '—'
})

onMounted(async () => {
  await store.refreshMe()
  await loadTokens()
  // 默认勾选 MCP（若账号有权限）
  newToken.mcp = !!store.user?.mcp
})

async function loadTokens() {
  try {
    const res = await listMyTokens()
    tokens.value = res.tokens || []
  } catch {
    tokens.value = []
  }
}

function formatTime(v?: string | null): string {
  if (!v) return '—'
  const d = new Date(v)
  if (Number.isNaN(d.getTime())) return v
  return d.toLocaleString()
}

async function createToken() {
  tokenBusy.value = true
  try {
    const res = await issueMyToken(newToken.label.trim() || 'personal', newToken.mcp)
    issuedToken.value = res.token
    newToken.label = ''
    toast.showSuccess('令牌已创建，请立即保存')
    await loadTokens()
  } catch (e) {
    const err = e as { response?: { data?: { detail?: string } } }
    toast.showError(err.response?.data?.detail || '创建失败')
  } finally {
    tokenBusy.value = false
  }
}

async function copyToken() {
  try {
    await navigator.clipboard.writeText(issuedToken.value)
    toast.showSuccess('已复制到剪贴板')
  } catch {
    toast.showError('复制失败，请手动选择复制')
  }
}

async function revokeToken(id: string) {
  try {
    await revokeMyToken(id)
    toast.showSuccess('令牌已撤销')
    await loadTokens()
  } catch {
    toast.showError('撤销失败')
  }
}

async function submitPassword() {
  if (pwd.next.length < 8) {
    toast.showError('新密码至少 8 位')
    return
  }
  if (pwd.next !== pwd.next2) {
    toast.showError('两次输入的新密码不一致')
    return
  }
  pwdBusy.value = true
  try {
    await store.changePassword(pwd.next, pwd.old)
    toast.showSuccess('密码已更新，请重新登录')
    store.clearToken()
    router.replace({ path: '/assist/login', query: { redirect: '/assist' } })
  } catch (e) {
    const err = e as { response?: { data?: { detail?: string } } }
    toast.showError(err.response?.data?.detail || '修改失败')
  } finally {
    pwdBusy.value = false
  }
}
</script>

<style scoped>
.page-title {
  font-size: 20px;
  font-weight: 600;
  margin-bottom: 16px;
}
.banner {
  padding: 10px 14px;
  border-radius: 6px;
  font-size: 13px;
  margin-bottom: 16px;
}
.banner-warning {
  background: var(--warning-bg);
  color: var(--warning-text);
}
.info-table {
  width: 100%;
  border-collapse: collapse;
  font-size: 14px;
}
.info-table th,
.info-table td {
  text-align: left;
  padding: 8px 10px;
  border-bottom: 1px solid var(--border);
}
.info-table th {
  width: 120px;
  color: var(--text-secondary);
  font-weight: 500;
  background: var(--bg-table-header);
}
.pwd-form {
  max-width: 420px;
}
.form-actions {
  display: flex;
  align-items: center;
  gap: 12px;
  flex-wrap: wrap;
}
.form-hint {
  font-size: 12px;
  color: var(--text-muted);
}
.section-hint {
  font-size: 13px;
  color: var(--text-secondary);
  margin-bottom: 12px;
}
.section-hint.muted {
  color: var(--text-muted);
}
.token-create {
  display: flex;
  gap: 12px;
  align-items: flex-end;
  flex-wrap: wrap;
  margin-bottom: 16px;
}
.form-group.inline {
  margin-bottom: 0;
  min-width: 220px;
}
.checkbox {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  font-size: 13px;
  color: var(--text-secondary);
  min-height: 36px;
}
.issued-token {
  background: var(--success-bg);
  border: 1px solid #c8e6c9;
  border-radius: 8px;
  padding: 12px 14px;
  margin-bottom: 16px;
}
.issued-token-label {
  font-size: 12px;
  color: var(--success-text);
  font-weight: 600;
  margin-bottom: 6px;
}
.issued-token-value {
  display: block;
  word-break: break-all;
  font-family: 'SF Mono', 'Cascadia Code', monospace;
  font-size: 13px;
  background: var(--bg-white);
  padding: 8px 10px;
  border-radius: 4px;
  border: 1px solid #c8e6c9;
}
.issued-token-actions {
  display: flex;
  gap: 8px;
  margin-top: 8px;
}
.token-table {
  width: 100%;
  border-collapse: collapse;
  font-size: 13px;
}
.token-table th,
.token-table td {
  text-align: left;
  padding: 8px 10px;
  border-bottom: 1px solid var(--border);
}
.token-table th {
  background: var(--bg-table-header);
  color: var(--text-secondary);
  font-weight: 500;
}
.row-actions {
  text-align: right;
}
.empty-hint {
  font-size: 13px;
  color: var(--text-muted);
  padding: 12px 0;
}
</style>
