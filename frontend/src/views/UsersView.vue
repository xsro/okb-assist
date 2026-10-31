<template>
  <div class="users-view" :class="{ container: !embedded, embedded }">
    <div class="page-header" :class="{ 'no-title': embedded }">
      <h2 v-if="!embedded">用户管理</h2>
      <div class="header-actions">
        <button class="btn btn-sm btn-outline" @click="loadAll" :disabled="loading">刷新</button>
        <button class="btn btn-sm" @click="openCreate">新建用户</button>
      </div>
    </div>

    <!-- 待审核 -->
    <section v-if="pendingUsers.length" class="section pending-section">
      <h3>待审核（{{ pendingUsers.length }}）</h3>
      <div v-for="u in pendingUsers" :key="u.id" class="pending-row">
        <div class="pending-info">
          <strong>{{ u.username }}</strong>
          <span v-if="u.display_name" class="muted">{{ u.display_name }}</span>
          <span class="badge badge-info">{{ roleLabel(u.role) }}</span>
          <span class="muted">注册于 {{ formatTime(u.created_at) }}</span>
        </div>
        <div class="pending-actions">
          <button class="btn btn-sm" @click="approve(u)">通过</button>
          <button class="btn btn-sm btn-outline-danger" @click="remove(u)">拒绝（删除）</button>
        </div>
      </div>
    </section>

    <!-- 用户表 -->
    <section class="section">
      <h3>全部用户（{{ users.length }}）</h3>
      <div class="table-wrap">
        <table class="user-table">
          <thead>
            <tr>
              <th>用户名</th>
              <th>显示名</th>
              <th>角色</th>
              <th>状态</th>
              <th>MCP</th>
              <th>密码</th>
              <th>最后登录</th>
              <th class="col-actions">操作</th>
            </tr>
          </thead>
          <tbody>
            <tr v-for="u in users" :key="u.id">
              <td class="mono">{{ u.username }}</td>
              <td>{{ u.display_name || '—' }}</td>
              <td>
                <select
                  :value="u.role"
                  :disabled="busyId === u.id"
                  @change="changeRole(u, ($event.target as HTMLSelectElement).value)"
                >
                  <option value="admin">管理员</option>
                  <option value="view-only">只读</option>
                  <option value="view-upload">查看+上传</option>
                </select>
              </td>
              <td>
                <span class="badge" :class="statusClass(u.status)">{{ statusLabel(u.status) }}</span>
              </td>
              <td>
                <input
                  type="checkbox"
                  :checked="u.mcp"
                  :disabled="busyId === u.id"
                  @change="toggleMcp(u, ($event.target as HTMLInputElement).checked)"
                />
              </td>
              <td>
                <span class="badge" :class="u.has_password ? 'badge-success' : 'badge-warning'">
                  {{ u.has_password ? '已设置' : '未设置' }}
                </span>
              </td>
              <td class="nowrap">{{ formatTime(u.last_login_at) }}</td>
              <td class="col-actions">
                <button class="btn btn-sm btn-outline" @click="openReset(u)">重置密码</button>
                <button class="btn btn-sm btn-outline" @click="openTokens(u)">
                  令牌{{ u.api_tokens.length ? `(${u.api_tokens.length})` : '' }}
                </button>
                <button
                  class="btn btn-sm btn-outline-danger"
                  :disabled="busyId === u.id"
                  @click="remove(u)"
                >
                  删除
                </button>
              </td>
            </tr>
            <tr v-if="!users.length">
              <td colspan="8" class="empty-hint">暂无用户</td>
            </tr>
          </tbody>
        </table>
      </div>
    </section>

    <!-- 注册策略 -->
    <section class="section">
      <h3>注册与会话策略</h3>
      <div class="settings-grid">
        <label class="setting">
          <input v-model="settings.allow_registration" type="checkbox" />
          <span>开放自助注册</span>
        </label>
        <label class="setting">
          <input v-model="settings.require_approval" type="checkbox" />
          <span>注册后需管理员审核</span>
        </label>
        <div class="setting">
          <span>注册默认角色</span>
          <select v-model="settings.registration_role">
            <option value="view-only">只读</option>
            <option value="view-upload">查看+上传</option>
          </select>
        </div>
        <div class="setting">
          <span>会话有效期（小时）</span>
          <input v-model.number="settings.session_ttl_hours" type="number" min="1" max="8760" />
        </div>
      </div>
      <div class="settings-actions">
        <button class="btn btn-sm" :disabled="settingsBusy" @click="saveSettings">
          {{ settingsBusy ? '保存中…' : '保存策略' }}
        </button>
        <span class="form-hint">会话为滑动续期：连续空闲超过该时长才需重新登录。</span>
      </div>
    </section>

    <!-- 新建用户对话框 -->
    <div v-if="createVisible" class="modal-overlay" @click.self="createVisible = false">
      <div class="modal">
        <h3>新建用户</h3>
        <div class="form-group">
          <label>用户名（3–32 位字母/数字/_.-）</label>
          <input v-model="createForm.username" type="text" />
        </div>
        <div class="form-group">
          <label>显示名（可选）</label>
          <input v-model="createForm.display_name" type="text" />
        </div>
        <div class="form-group">
          <label>初始密码（留空则只能 Token 登录，需后续重置密码）</label>
          <input v-model="createForm.password" type="password" autocomplete="new-password" />
        </div>
        <div class="form-group">
          <label>角色</label>
          <select v-model="createForm.role">
            <option value="view-only">只读</option>
            <option value="view-upload">查看+上传</option>
            <option value="admin">管理员</option>
          </select>
        </div>
        <div class="form-group">
          <label>状态</label>
          <select v-model="createForm.status">
            <option value="active">启用</option>
            <option value="pending">待审核</option>
            <option value="disabled">禁用</option>
          </select>
        </div>
        <label class="checkbox">
          <input v-model="createForm.mcp" type="checkbox" />
          允许访问 MCP
        </label>
        <div class="modal-actions">
          <button class="btn" :disabled="createBusy" @click="submitCreate">
            {{ createBusy ? '创建中…' : '创建' }}
          </button>
          <button class="btn btn-outline" @click="createVisible = false">取消</button>
        </div>
      </div>
    </div>

    <!-- 重置密码对话框 -->
    <div v-if="resetTarget" class="modal-overlay" @click.self="resetTarget = null">
      <div class="modal">
        <h3>重置密码：{{ resetTarget.username }}</h3>
        <div class="form-group">
          <label>新密码（至少 8 位）</label>
          <input v-model="resetPassword" type="password" autocomplete="new-password" />
        </div>
        <label class="checkbox">
          <input v-model="resetRevokeTokens" type="checkbox" />
          同时撤销该用户的全部访问令牌
        </label>
        <p class="form-hint">重置后该用户的所有登录会话会被立即撤销。</p>
        <div class="modal-actions">
          <button class="btn" :disabled="resetBusy" @click="submitReset">
            {{ resetBusy ? '提交中…' : '确定' }}
          </button>
          <button class="btn btn-outline" @click="resetTarget = null">取消</button>
        </div>
      </div>
    </div>

    <!-- 令牌管理对话框 -->
    <div v-if="tokenTarget" class="modal-overlay" @click.self="tokenTarget = null">
      <div class="modal modal-wide">
        <h3>访问令牌：{{ tokenTarget.username }}</h3>
        <div class="token-create">
          <input v-model="tokenLabel" type="text" placeholder="备注（如 zotero）" />
          <select v-model="tokenScope">
            <option value="api">API（/assist/api/*）</option>
            <option value="mcp" :disabled="!tokenTarget.mcp">MCP（/assist/mcp）</option>
          </select>
          <input
            v-model="tokenPlain"
            type="text"
            placeholder="自定义令牌（留空自动生成）"
            autocomplete="off"
            spellcheck="false"
          />
          <button class="btn btn-sm" :disabled="tokenBusy" @click="createToken">签发</button>
        </div>
        <p class="form-hint">
          API 与 MCP 令牌互不通用：API 令牌调 <code>/assist/api/*</code>，MCP 令牌只能调 MCP。
          自定义明文至少 8 位、仅可见 ASCII，且不能与其他账号重复。
        </p>
        <p v-if="!tokenTarget.mcp" class="form-hint">该用户未开启 MCP，无法签发 MCP 令牌。</p>

        <div v-if="issuedToken" class="issued-token">
          <div class="issued-token-label">新令牌（只显示这一次）</div>
          <code class="issued-token-value">{{ issuedToken }}</code>
          <div class="issued-token-actions">
            <button class="btn btn-sm" @click="copyToken">复制</button>
            <button class="btn btn-sm btn-outline" @click="issuedToken = ''">我已保存</button>
          </div>
        </div>

        <table v-if="tokenTarget.api_tokens.length" class="token-table">
          <thead>
            <tr>
              <th>备注</th>
              <th>权限</th>
              <th>创建时间</th>
              <th></th>
            </tr>
          </thead>
          <tbody>
            <tr v-for="t in tokenTarget.api_tokens" :key="t.id">
              <td>{{ t.label || '—' }}</td>
              <td>{{ t.scope === 'mcp' ? 'MCP' : 'API' }}</td>
              <td>{{ formatTime(t.created_at) }}</td>
              <td class="row-actions">
                <button class="btn btn-sm btn-outline-danger" @click="revokeToken(t.id)">撤销</button>
              </td>
            </tr>
          </tbody>
        </table>
        <p v-else class="empty-hint">该用户还没有访问令牌</p>

        <div class="modal-actions">
          <button class="btn btn-outline" @click="tokenTarget = null">关闭</button>
        </div>
      </div>
    </div>

    <ConfirmDialog ref="confirmRef" />
  </div>
</template>

<script setup lang="ts">
import { computed, onMounted, reactive, ref } from 'vue'
import {
  approveUser,
  createUser,
  deleteUser,
  getUsersSettings,
  issueUserToken,
  listUsers,
  resetUserPassword,
  revokeUserToken,
  updateUser,
  updateUsersSettings
} from '@/api/users'
import ConfirmDialog from '@/components/ConfirmDialog.vue'
import { useToast } from '@/composables/useToast'
import { useTokenStore } from '@/stores/token'
import type { Role, TokenScope, User, UserStatus, UsersSettings } from '@/types/auth'

withDefaults(defineProps<{ embedded?: boolean }>(), { embedded: false })

const toast = useToast()
const tokenStore = useTokenStore()

const users = ref<User[]>([])
const loading = ref(false)
const busyId = ref('')

const settings = reactive<UsersSettings>({
  allow_registration: true,
  require_approval: true,
  registration_role: 'view-only',
  session_ttl_hours: 168
})
const settingsBusy = ref(false)

// 新建用户
const createVisible = ref(false)
const createBusy = ref(false)
const createForm = reactive({
  username: '',
  display_name: '',
  password: '',
  role: 'view-only' as Role,
  status: 'active' as UserStatus,
  mcp: false
})

// 重置密码
const resetTarget = ref<User | null>(null)
const resetPassword = ref('')
const resetRevokeTokens = ref(false)
const resetBusy = ref(false)

// 令牌
const tokenTarget = ref<User | null>(null)
const tokenLabel = ref('')
const tokenScope = ref<TokenScope>('api')
const tokenPlain = ref('')
const tokenBusy = ref(false)
const issuedToken = ref('')

const pendingUsers = computed(() => users.value.filter((u) => u.status === 'pending'))

function roleLabel(role: Role): string {
  return { admin: '管理员', 'view-only': '只读', 'view-upload': '查看+上传' }[role] || role
}
function statusLabel(status: UserStatus): string {
  return { pending: '待审核', active: '启用', disabled: '禁用' }[status] || status
}
function statusClass(status: UserStatus): string {
  return status === 'active' ? 'badge-success' : status === 'pending' ? 'badge-warning' : 'badge-gray'
}
function formatTime(v?: string | null): string {
  if (!v) return '—'
  const d = new Date(v)
  return Number.isNaN(d.getTime()) ? v : d.toLocaleString()
}
function errDetail(e: unknown, fallback: string): string {
  const err = e as { response?: { data?: { detail?: string } } }
  return err.response?.data?.detail || fallback
}

async function loadAll() {
  loading.value = true
  try {
    const res = await listUsers()
    users.value = res.users || []
  } catch (e) {
    toast.showError(errDetail(e, '加载用户失败'))
  } finally {
    loading.value = false
  }
  try {
    const s = await getUsersSettings()
    Object.assign(settings, s)
  } catch {
    /* 忽略：设置区保持默认值 */
  }
}

onMounted(async () => {
  await loadAll()
})

function openCreate() {
  createForm.username = ''
  createForm.display_name = ''
  createForm.password = ''
  createForm.role = 'view-only'
  createForm.status = 'active'
  createForm.mcp = false
  createVisible.value = true
}

async function submitCreate() {
  createBusy.value = true
  try {
    await createUser({
      username: createForm.username.trim(),
      password: createForm.password || undefined,
      display_name: createForm.display_name.trim(),
      role: createForm.role,
      status: createForm.status,
      mcp: createForm.mcp
    })
    toast.showSuccess('用户已创建')
    createVisible.value = false
    await loadAll()
  } catch (e) {
    toast.showError(errDetail(e, '创建失败'))
  } finally {
    createBusy.value = false
  }
}

async function patch(u: User, patchBody: Parameters<typeof updateUser>[1]) {
  busyId.value = u.id
  try {
    const res = await updateUser(u.id, patchBody)
    const idx = users.value.findIndex((x) => x.id === u.id)
    if (idx >= 0) users.value[idx] = res.user
    toast.showSuccess('已更新')
  } catch (e) {
    toast.showError(errDetail(e, '更新失败'))
    await loadAll()
  } finally {
    busyId.value = ''
  }
}

async function changeRole(u: User, role: string) {
  await patch(u, { role: role as Role })
}

async function toggleMcp(u: User, mcp: boolean) {
  await patch(u, { mcp })
}

async function approve(u: User) {
  busyId.value = u.id
  try {
    const res = await approveUser(u.id)
    const idx = users.value.findIndex((x) => x.id === u.id)
    if (idx >= 0) users.value[idx] = res.user
    toast.showSuccess(`${u.username} 已通过审核`)
  } catch (e) {
    toast.showError(errDetail(e, '审核失败'))
  } finally {
    busyId.value = ''
  }
}

async function remove(u: User) {
  const ok = await confirmRef.value?.show(
    '删除用户',
    `确定删除用户 ${u.username}？其会话与访问令牌会立即失效，此操作不可撤销。`
  )
  if (!ok) return
  busyId.value = u.id
  try {
    await deleteUser(u.id)
    toast.showSuccess('用户已删除')
    if (u.id === tokenStore.user?.id) {
      toast.showInfo('你删除的是当前登录账号，请重新登录')
    }
    await loadAll()
  } catch (e) {
    toast.showError(errDetail(e, '删除失败'))
  } finally {
    busyId.value = ''
  }
}

function openReset(u: User) {
  resetTarget.value = u
  resetPassword.value = ''
  resetRevokeTokens.value = false
}

async function submitReset() {
  if (!resetTarget.value) return
  if (resetPassword.value.length < 8) {
    toast.showError('新密码至少 8 位')
    return
  }
  resetBusy.value = true
  try {
    const res = await resetUserPassword(
      resetTarget.value.id,
      resetPassword.value,
      resetRevokeTokens.value
    )
    toast.showSuccess(
      `密码已重置（撤销会话 ${res.revoked_sessions} 个` +
        (res.revoked_tokens ? `、令牌 ${res.revoked_tokens} 个` : '') +
        '）'
    )
    resetTarget.value = null
    await loadAll()
  } catch (e) {
    toast.showError(errDetail(e, '重置失败'))
  } finally {
    resetBusy.value = false
  }
}

async function openTokens(u: User) {
  tokenTarget.value = u
  tokenLabel.value = ''
  tokenScope.value = 'api'
  tokenPlain.value = ''
  issuedToken.value = ''
}

async function createToken() {
  if (!tokenTarget.value) return
  tokenBusy.value = true
  try {
    const custom = tokenPlain.value.trim()
    const res = await issueUserToken(tokenTarget.value.id, {
      label: tokenLabel.value.trim() || 'token',
      scope: tokenScope.value,
      token: custom || undefined
    })
    issuedToken.value = res.token
    tokenLabel.value = ''
    tokenPlain.value = ''
    await loadAll()
    // 同步刷新弹窗里的用户对象
    const fresh = users.value.find((x) => x.id === tokenTarget.value?.id)
    if (fresh) tokenTarget.value = fresh
    toast.showSuccess('令牌已签发，请立即保存')
  } catch (e) {
    toast.showError(errDetail(e, '签发失败'))
  } finally {
    tokenBusy.value = false
  }
}

async function revokeToken(tokenId: string) {
  if (!tokenTarget.value) return
  try {
    await revokeUserToken(tokenTarget.value.id, tokenId)
    await loadAll()
    const fresh = users.value.find((x) => x.id === tokenTarget.value?.id)
    if (fresh) tokenTarget.value = fresh
    toast.showSuccess('令牌已撤销')
  } catch (e) {
    toast.showError(errDetail(e, '撤销失败'))
  }
}

async function copyToken() {
  try {
    await navigator.clipboard.writeText(issuedToken.value)
    toast.showSuccess('已复制')
  } catch {
    toast.showError('复制失败，请手动选择复制')
  }
}

async function saveSettings() {
  settingsBusy.value = true
  try {
    const res = await updateUsersSettings({ ...settings })
    Object.assign(settings, res)
    toast.showSuccess('策略已保存')
  } catch (e) {
    toast.showError(errDetail(e, '保存失败'))
  } finally {
    settingsBusy.value = false
  }
}

const confirmRef = ref<InstanceType<typeof ConfirmDialog> | null>(null)
</script>

<style scoped>
.page-header.no-title {
  justify-content: flex-end;
}
.page-header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
  margin-bottom: 16px;
  flex-wrap: wrap;
}
.page-header h2 {
  font-size: 20px;
  font-weight: 600;
}
.header-actions {
  display: flex;
  gap: 8px;
}
.table-wrap {
  overflow-x: auto;
}
.user-table {
  width: 100%;
  border-collapse: collapse;
  font-size: 13px;
  min-width: 860px;
}
.user-table th,
.user-table td {
  text-align: left;
  padding: 8px 10px;
  border-bottom: 1px solid var(--border);
  vertical-align: middle;
}
.user-table th {
  background: var(--bg-table-header);
  color: var(--text-secondary);
  font-weight: 500;
  white-space: nowrap;
}
.user-table select,
.user-table input[type='number'],
.settings-grid input[type='number'],
.settings-grid select,
.token-create input[type='text'],
.modal .form-group input,
.modal .form-group select {
  padding: 6px 8px;
  border: 1px solid var(--border);
  border-radius: 5px;
  font-size: 13px;
  background: var(--bg-white);
}
.col-actions {
  white-space: nowrap;
  text-align: right;
}
.col-actions .btn + .btn {
  margin-left: 6px;
}
.mono {
  font-family: 'SF Mono', 'Cascadia Code', monospace;
}
.nowrap {
  white-space: nowrap;
}
.muted {
  color: var(--text-muted);
  font-size: 12px;
}
.pending-section {
  border-color: var(--warning);
  background: #fffdf7;
}
.pending-row {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
  padding: 10px 0;
  border-bottom: 1px dashed var(--border);
  flex-wrap: wrap;
}
.pending-row:last-child {
  border-bottom: none;
}
.pending-info {
  display: flex;
  align-items: center;
  gap: 10px;
  flex-wrap: wrap;
  font-size: 14px;
}
.pending-actions {
  display: flex;
  gap: 8px;
}
.settings-grid {
  display: grid;
  grid-template-columns: repeat(auto-fit, minmax(220px, 1fr));
  gap: 12px 20px;
}
.setting {
  display: flex;
  align-items: center;
  gap: 8px;
  font-size: 14px;
  color: var(--text);
}
.setting.checkbox {
  cursor: pointer;
}
.settings-actions {
  display: flex;
  align-items: center;
  gap: 12px;
  margin-top: 16px;
  flex-wrap: wrap;
}
.form-hint {
  font-size: 12px;
  color: var(--text-muted);
}
.checkbox {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  font-size: 13px;
  color: var(--text-secondary);
  min-height: 32px;
}
.checkbox + .checkbox {
  margin-top: 8px;
}
.modal .checkbox {
  display: flex;
}
.modal-wide {
  max-width: 720px;
}
.token-create {
  display: flex;
  align-items: center;
  gap: 10px;
  margin-bottom: 12px;
  flex-wrap: wrap;
}
.token-create input[type='text'] {
  flex: 1;
  min-width: 160px;
}
.issued-token {
  background: var(--success-bg);
  border: 1px solid #c8e6c9;
  border-radius: 8px;
  padding: 10px 12px;
  margin-bottom: 12px;
}
.issued-token-label {
  font-size: 12px;
  font-weight: 600;
  color: var(--success-text);
  margin-bottom: 6px;
}
.issued-token-value {
  display: block;
  word-break: break-all;
  font-family: 'SF Mono', 'Cascadia Code', monospace;
  font-size: 13px;
  background: var(--bg-white);
  border: 1px solid #c8e6c9;
  border-radius: 4px;
  padding: 8px 10px;
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
  text-align: center;
}
</style>
