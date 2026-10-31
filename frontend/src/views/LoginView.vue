<template>
  <div class="login-page">
    <div class="login-card">
      <div class="login-brand">
        <h1>OKB-Assist</h1>
        <p class="login-sub">本地学术文献库</p>
      </div>

      <!-- 未初始化：只能创建首个管理员 -->
      <template v-if="needSetup">
        <div class="notice notice-info">
          系统尚未初始化，请创建第一个管理员账号（之后可在「用户管理」里添加其他用户）。
        </div>
        <form @submit.prevent="doBootstrap">
          <div class="form-group">
            <label>管理员用户名</label>
            <input v-model="form.username" type="text" placeholder="admin" autocomplete="username" />
          </div>
          <div class="form-group">
            <label>密码（至少 8 位）</label>
            <input
              v-model="form.password"
              type="password"
              placeholder="设置密码"
              autocomplete="new-password"
            />
          </div>
          <div class="form-group">
            <label>确认密码</label>
            <input
              v-model="form.password2"
              type="password"
              placeholder="再输入一次"
              autocomplete="new-password"
            />
          </div>
          <div v-if="error" class="notice notice-error">{{ error }}</div>
          <button class="btn btn-block" type="submit" :disabled="busy">
            {{ busy ? '创建中…' : '创建管理员并登录' }}
          </button>
        </form>
      </template>

      <!-- 已初始化：登录 / 注册 / Token 登录 -->
      <template v-else>
        <div class="tabs">
          <button
            v-for="t in visibleTabs"
            :key="t.key"
            class="tab"
            :class="{ active: tab === t.key }"
            type="button"
            @click="switchTab(t.key)"
          >
            {{ t.label }}
          </button>
        </div>

        <!-- 账号登录 -->
        <form v-if="tab === 'login'" @submit.prevent="doLogin">
          <div class="form-group">
            <label>用户名</label>
            <input
              v-model="form.username"
              type="text"
              placeholder="用户名"
              autocomplete="username"
              autofocus
            />
          </div>
          <div class="form-group">
            <label>密码</label>
            <input
              v-model="form.password"
              type="password"
              placeholder="密码"
              autocomplete="current-password"
            />
          </div>
          <div v-if="error" class="notice notice-error">{{ error }}</div>
          <button class="btn btn-block" type="submit" :disabled="busy">
            {{ busy ? '登录中…' : '登录' }}
          </button>
        </form>

        <!-- 注册 -->
        <form v-else-if="tab === 'register'" @submit.prevent="doRegister">
          <div class="form-group">
            <label>用户名（3–32 位字母/数字/_.-）</label>
            <input v-model="form.username" type="text" autocomplete="username" />
          </div>
          <div class="form-group">
            <label>显示名（可选）</label>
            <input v-model="form.displayName" type="text" />
          </div>
          <div class="form-group">
            <label>密码（至少 8 位）</label>
            <input v-model="form.password" type="password" autocomplete="new-password" />
          </div>
          <div class="form-group">
            <label>确认密码</label>
            <input v-model="form.password2" type="password" autocomplete="new-password" />
          </div>
          <div v-if="status?.require_approval" class="hint">
            注册后需要管理员审核通过才能登录。
          </div>
          <div v-if="error" class="notice notice-error">{{ error }}</div>
          <div v-if="success" class="notice notice-success">{{ success }}</div>
          <button class="btn btn-block" type="submit" :disabled="busy">
            {{ busy ? '提交中…' : '注册' }}
          </button>
        </form>

        <!-- Token 登录 -->
        <form v-else @submit.prevent="doTokenLogin">
          <div class="notice notice-info">
            可粘贴<b>个人 API token</b>（<code>okb_…</code>）或已有会话 token。个人 token 可在登录后
            「账户」页签发。
          </div>
          <div class="form-group">
            <label>Token</label>
            <input v-model="form.token" type="password" placeholder="okb_… / sess_…" />
          </div>
          <div v-if="error" class="notice notice-error">{{ error }}</div>
          <button class="btn btn-block" type="submit" :disabled="busy">
            {{ busy ? '验证中…' : 'Token 登录' }}
          </button>
        </form>
      </template>

      <div class="login-footer">
        <span v-if="status">
          用户 {{ status.user_count }} 个<span v-if="status.pending_count">
            · 待审核 {{ status.pending_count }}</span
          >
        </span>
        <span v-if="locked">系统配置为需登录，登录后有效期 {{ status?.session_ttl_hours }} 小时</span>
      </div>
    </div>
  </div>
</template>

<script setup lang="ts">
import { computed, onMounted, reactive, ref } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import { useTokenStore } from '@/stores/token'
import { useToast } from '@/composables/useToast'

type Tab = 'login' | 'register' | 'token'

const store = useTokenStore()
const router = useRouter()
const route = useRoute()
const toast = useToast()

const tab = ref<Tab>('login')
const busy = ref(false)
const error = ref('')
const success = ref('')
const form = reactive({
  username: '',
  password: '',
  password2: '',
  displayName: '',
  token: ''
})

const status = computed(() => store.status)
/** 还没有任何管理员 → 只能初始化 */
const needSetup = computed(() => status.value !== null && !status.value.initialized)
const locked = computed(() => !!status.value?.initialized)

const visibleTabs = computed(() => {
  const tabs: { key: Tab; label: string }[] = [
    { key: 'login', label: '账号登录' },
    { key: 'token', label: 'Token 登录' }
  ]
  if (status.value?.allow_registration) {
    tabs.splice(1, 0, { key: 'register', label: '注册' })
  }
  return tabs
})

function switchTab(next: Tab) {
  tab.value = next
  error.value = ''
  success.value = ''
}

/** 回跳目标必须是站内绝对路径（防开放重定向） */
function safeRedirect(raw: unknown): string {
  if (typeof raw !== 'string' || !raw.startsWith('/') || raw.startsWith('//')) return '/assist'
  // 反斜杠、控制字符会被浏览器规范化成别的协议/主机
  if (raw.includes('\\') || /[\u0000-\u001f]/.test(raw)) return '/assist'
  return raw
}

/**
 * 登录后回跳。
 *
 * - SPA 路由（`/assist/...` 且有匹配的页面）→ 客户端路由跳转；
 * - 其余路径（例如 `system.json` 的 `static_dirs` 挂载的 `/static/...`）→ 整页跳转，
 *   凭证由登录/校验时下发的 `okb_static` 会话 cookie 携带（浏览器直接导航带不了 X-Token）。
 */
function redirectAfterLogin() {
  const target = safeRedirect(route.query.redirect)
  const resolved = router.resolve(target)
  if (resolved.matched.length > 0) {
    router.replace(target)
    return
  }
  window.location.replace(target)
}

function message(e: unknown): string {
  const err = e as { response?: { data?: { detail?: string } } }
  return err.response?.data?.detail || '请求失败，请稍后重试'
}

async function doLogin() {
  if (!form.username.trim() || !form.password) {
    error.value = '请填写用户名与密码'
    return
  }
  busy.value = true
  error.value = ''
  try {
    await store.login(form.username.trim(), form.password)
    toast.showSuccess('登录成功')
    redirectAfterLogin()
  } catch (e) {
    error.value = message(e)
  } finally {
    busy.value = false
  }
}

async function doRegister() {
  error.value = ''
  success.value = ''
  if (!form.username.trim() || form.password.length < 8) {
    error.value = '用户名不能为空，密码至少 8 位'
    return
  }
  if (form.password !== form.password2) {
    error.value = '两次输入的密码不一致'
    return
  }
  busy.value = true
  try {
    const res = await store.register(form.username.trim(), form.password, form.displayName.trim())
    success.value = res.detail || '注册成功'
    form.password = ''
    form.password2 = ''
  } catch (e) {
    error.value = message(e)
  } finally {
    busy.value = false
  }
}

async function doBootstrap() {
  error.value = ''
  if (!form.password || form.password.length < 8) {
    error.value = '密码至少 8 位'
    return
  }
  if (form.password !== form.password2) {
    error.value = '两次输入的密码不一致'
    return
  }
  busy.value = true
  try {
    await store.bootstrap(form.username.trim() || 'admin', form.password)
    toast.showSuccess('管理员已创建')
    redirectAfterLogin()
  } catch (e) {
    error.value = message(e)
  } finally {
    busy.value = false
  }
}

async function doTokenLogin() {
  const token = form.token.trim()
  if (!token) {
    error.value = '请输入 Token'
    return
  }
  busy.value = true
  error.value = ''
  try {
    const ok = await store.setToken(token)
    if (!ok) {
      error.value = 'Token 无效'
      return
    }
    toast.showSuccess('Token 登录成功')
    redirectAfterLogin()
  } catch {
    error.value = 'Token 验证失败'
  } finally {
    busy.value = false
  }
}

onMounted(async () => {
  await store.refreshStatus()
  // 已有有效会话 → 直接进站
  if (store.token) {
    await store.restore()
    if (store.role) {
      redirectAfterLogin()
      return
    }
  }
  if (needSetup.value) tab.value = 'login'
  else if (status.value && !status.value.allow_registration) tab.value = 'login'
})
</script>

<style scoped>
.login-page {
  min-height: calc(100vh - 140px);
  display: flex;
  align-items: center;
  justify-content: center;
  padding: 24px 16px;
}
.login-card {
  width: 100%;
  max-width: 420px;
  background: var(--bg-white);
  border: 1px solid var(--border);
  border-radius: 12px;
  padding: 28px 24px;
  box-shadow: 0 8px 28px rgba(0, 0, 0, 0.06);
}
.login-brand {
  text-align: center;
  margin-bottom: 20px;
}
.login-brand h1 {
  font-size: 22px;
  font-weight: 700;
  color: var(--primary);
}
.login-sub {
  font-size: 13px;
  color: var(--text-secondary);
  margin-top: 4px;
}
.tabs {
  display: flex;
  gap: 4px;
  background: var(--bg);
  padding: 4px;
  border-radius: 8px;
  margin-bottom: 18px;
}
.tab {
  flex: 1;
  border: none;
  background: transparent;
  padding: 8px 4px;
  border-radius: 6px;
  font-size: 14px;
  color: var(--text-secondary);
  cursor: pointer;
  min-height: 36px;
}
.tab.active {
  background: var(--bg-white);
  color: var(--primary);
  font-weight: 600;
  box-shadow: 0 1px 4px rgba(0, 0, 0, 0.08);
}
.btn-block {
  width: 100%;
  margin-top: 4px;
}
.notice {
  font-size: 13px;
  padding: 10px 12px;
  border-radius: 6px;
  margin-bottom: 14px;
  line-height: 1.5;
}
.notice-info {
  background: var(--primary-light);
  color: var(--primary-dark);
}
.notice-error {
  background: var(--danger-bg);
  color: var(--danger-text);
}
.notice-success {
  background: var(--success-bg);
  color: var(--success-text);
}
.hint {
  font-size: 12px;
  color: var(--text-muted);
  margin-bottom: 12px;
}
.login-footer {
  margin-top: 18px;
  display: flex;
  flex-direction: column;
  gap: 4px;
  font-size: 12px;
  color: var(--text-muted);
  text-align: center;
}
.form-group input[type='password'] {
  width: 100%;
  padding: 10px 12px;
  border: 1px solid var(--border);
  border-radius: 6px;
  font-size: 14px;
}
.form-group input[type='password']:focus {
  outline: none;
  border-color: var(--primary);
  box-shadow: 0 0 0 2px rgba(25, 118, 210, 0.15);
}
</style>
