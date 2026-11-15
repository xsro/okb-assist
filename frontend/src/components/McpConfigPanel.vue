<template>
  <div class="mcp-config-panel">
    <p class="hint">
      将以下配置复制到你的 MCP 客户端（CodeBuddy / Claude / Codex / Pi / OpenCode），
      即可使用 OKB-Assist 的工具与资源。
    </p>

    <!-- 当前账号状态：只展示类型与权限，不展示任何明文凭证 -->
    <div class="cred-card">
      <div class="cred-row">
        <span class="cred-label">当前账号</span>
        <span class="cred-value">
          {{ accountLabel }}
          <span class="role-badge">{{ tokenStore.roleLabel }}</span>
        </span>
      </div>
      <div class="cred-row">
        <span class="cred-label">当前登录方式</span>
        <span class="cred-value">{{ viaLabel }}</span>
      </div>
      <div class="cred-row">
        <span class="cred-label">MCP 权限</span>
        <span class="cred-value" :class="mcpReady ? 'ok' : 'warn'">
          {{ mcpReady ? '✅ 可用' : '⚠️ 不可用' }}<span class="cred-note">{{ mcpReason }}</span>
        </span>
      </div>
    </div>

    <!-- 我的 MCP 令牌：在本页直接创建 / 查看 / 撤销 / 选用 -->
    <section class="token-section">
      <div class="token-head">
        <h4>我的 MCP 令牌</h4>
        <span class="token-count">{{ mcpTokens.length }} 个</span>
      </div>

      <!-- 账号没有 MCP 权限：只能提示找管理员 -->
      <div v-if="!tokenStore.user?.mcp" class="notice notice-warn">
        当前账号未开启「MCP 访问」，无法创建 MCP 令牌。请联系管理员在
        <router-link to="/assist/config?tab=users">用户管理</router-link>中开启。
      </div>

      <template v-else>
        <!-- 还没有令牌：明确提醒先创建 -->
        <div v-if="!mcpTokens.length" class="notice notice-info">
          你还没有 MCP 令牌。创建一把后，下方参考配置里就能直接写入它的明文，复制即可使用。
        </div>

        <div class="token-create">
          <input
            v-model="newLabel"
            type="text"
            placeholder="备注，如 codebuddy / claude / pi"
            autocomplete="off"
          />
          <input
            v-model="newPlain"
            type="text"
            placeholder="自定义令牌（可选，留空自动生成 okb_…）"
            autocomplete="off"
            spellcheck="false"
          />
          <button class="btn btn-sm" :disabled="creating" @click="createToken">
            {{ creating ? '创建中…' : '创建 MCP 令牌' }}
          </button>
        </div>

        <div v-if="mcpTokens.length" class="token-table-wrap">
          <table class="token-table">
            <thead>
              <tr>
                <th>备注</th>
                <th>令牌明文</th>
                <th>创建时间</th>
                <th>最后使用</th>
                <th></th>
              </tr>
            </thead>
            <tbody>
              <tr v-for="t in mcpTokens" :key="t.id">
                <td class="label-cell">
                  {{ t.label || '—' }}
                  <span v-if="selectedTokenId === t.id" class="in-use">配置中</span>
                  <span v-else-if="!t.token" class="in-use warn">旧令牌</span>
                </td>
                <td class="token-cell">
                  <template v-if="t.token">
                    <code class="token-value">{{ revealed[t.id] ? t.token : maskValue(t.token) }}</code>
                    <button class="btn btn-sm btn-outline" @click="toggleReveal(t.id)">
                      {{ revealed[t.id] ? '隐藏' : '显示' }}
                    </button>
                    <button class="btn btn-sm" @click="copyValue(t.token)">复制</button>
                  </template>
                  <span v-else class="empty-hint muted">
                    旧令牌明文不可恢复，请撤销后重新创建
                  </span>
                </td>
                <td>{{ formatTime(t.created_at) }}</td>
                <td>{{ formatTime(t.last_used_at) }}</td>
                <td class="row-actions">
                  <button class="btn btn-sm btn-outline-danger" @click="revokeToken(t)">撤销</button>
                </td>
              </tr>
            </tbody>
          </table>
        </div>

        <p v-if="legacyTokens.length" class="hint hint-sm warn-text">
          有 {{ legacyTokens.length }} 个旧 MCP 令牌的明文不可恢复（创建时还没有该功能），
          无法写入配置，请撤销后重新创建。
        </p>
      </template>
    </section>

    <!-- 选用哪个令牌写进参考配置 -->
    <div class="token-input">
      <label for="mcp-token-select">写入下方参考配置的令牌</label>
      <div class="token-input-row">
        <select id="mcp-token-select" v-model="selectedTokenId" @change="onSelectToken">
          <option value="">── 暂不写入（配置里保留占位符）──</option>
          <option v-for="t in plainTokens" :key="t.id" :value="t.id">
            {{ t.label || '未命名' }} · {{ maskValue(t.token) }}
          </option>
        </select>
        <button
          class="btn btn-sm btn-outline"
          :disabled="testing || !selectedToken"
          @click="testConnection"
        >
          {{ testing ? '测试中…' : '测试连接' }}
        </button>
      </div>
      <p v-if="!plainTokens.length" class="hint hint-sm warn-text">
        还没有可用的 MCP 令牌：先在上方创建一把，选中后配置里才会带上明文。
      </p>
      <p v-else-if="!selectedToken" class="hint hint-sm warn-text">
        未选择令牌：下方配置里留的是占位符 <code>{{ TOKEN_PLACEHOLDER }}</code>，选中令牌后再复制才能直接使用。
      </p>
    </div>

    <div class="client-tabs">
      <button
        v-for="c in clients"
        :key="c.key"
        class="tab"
        :class="{ active: activeClient === c.key }"
        @click="activeClient = c.key"
      >
        {{ c.label }}
      </button>
    </div>

    <p class="config-path">
      配置文件位置：<code>{{ configPath }}</code>
    </p>

    <div class="config-output">
      <pre><code>{{ clientConfig }}</code></pre>
      <button class="btn btn-sm" @click="copy">复制</button>
    </div>

    <div class="common-config">
      <h4>通用配置</h4>
      <table class="info-table">
        <tr><th>MCP URL</th><td><code>{{ mcpUrl }}</code></td></tr>
        <tr><th>传输方式</th><td>Streamable HTTP（<code>Authorization: Bearer &lt;令牌&gt;</code>）</td></tr>
        <tr>
          <th>用于配置的令牌</th>
          <td class="token-cell">
            <code v-if="selectedToken" class="token-value">{{ maskedToken }}</code>
            <code v-else class="token-value muted-value">{{ TOKEN_PLACEHOLDER }}</code>
            <span class="cred-note">
              {{ selectedToken ? `（${selectedToken.label || '未命名'}，明文可在上方随时查看）` : '（尚未选择）' }}
            </span>
          </td>
        </tr>
      </table>
      <p class="hint hint-sm">
        令牌权限需为 <strong>MCP</strong>（<code>scope=mcp</code>）：权限为 API 的令牌调 MCP 会被拒（401）。
        另外账号本身也要开启「MCP 访问」，否则会返回
        <code>Unauthorized: MCP access not granted</code>；没有权限请联系管理员在
        <router-link to="/assist/config?tab=users">用户管理</router-link>里开启。
      </p>
    </div>
  </div>
</template>

<script setup lang="ts">
import { ref, computed, onMounted } from 'vue'
import { useToast } from '@/composables/useToast'
import { useTokenStore } from '@/stores/token'
import { issueMyToken, listMyTokens, revokeMyToken } from '@/api/auth'
import type { ApiTokenMeta } from '@/types/auth'

const { showSuccess, showError } = useToast()
const tokenStore = useTokenStore()

const baseUrl = ref('')

/**
 * 页面只处理 **MCP 专属令牌**（`scope=mcp`），它们由后端回呈明文：
 * - 会话 token（`sess_…`）与 `scope=api` 的个人令牌都不能用于 MCP，也不再让用户粘贴；
 * - 明文因此只在本页从「我的 MCP 令牌」里选择，不会再要求用户手动输入。
 */
const TOKEN_PLACEHOLDER = '<选择或创建 MCP 令牌后重新复制>'

/** 账户页签发的 MCP 令牌（含明文，供选择/显示） */
const mcpTokens = ref<ApiTokenMeta[]>([])
/** 当前写入参考配置的令牌 ID（空 = 不写入，保留占位符） */
const selectedTokenId = ref('')
/** 表格里已展开明文的令牌 */
const revealed = ref<Record<string, boolean>>({})
/** 创建表单 */
const creating = ref(false)
const newLabel = ref('')
const newPlain = ref('')
const testing = ref(false)

/** 明文可用的令牌（改版前签发的 MCP 令牌只有哈希，无法回呈） */
const plainTokens = computed(() => mcpTokens.value.filter((t) => !!t.token))
const legacyTokens = computed(() => mcpTokens.value.filter((t) => !t.token))
const selectedToken = computed(
  () => plainTokens.value.find((t) => t.id === selectedTokenId.value) || null
)

/** 选中令牌的记忆键（只存 ID，不存明文；按用户隔离） */
const selectedKey = computed(() => `okb_mcp_selected_token:${tokenStore.user?.id || 'anon'}`)

const accountLabel = computed(() => {
  return tokenStore.displayName || tokenStore.user?.username || '未登录'
})

/** 当前登录凭证类型（只给类型，不给内容） */
const viaLabel = computed(() => {
  const labels: Record<string, string> = {
    session: '账号密码（会话登录）',
    api_token: '个人访问令牌（权限 API）',
    local_subnet: '受信任子网（免凭证）'
  }
  return labels[tokenStore.via] || '未知'
})

/** 当前凭证是否真能调 MCP：取后端算好的最终值（用户 mcp 开关 × 令牌 scope） */
const mcpReady = computed(() => tokenStore.mcp === true)

const mcpReason = computed(() => {
  if (mcpReady.value) return ''
  if (tokenStore.via === 'api_token') return '：当前登录用的是 scope=api 的令牌，与 MCP 无关（本页令牌不受影响）'
  if (tokenStore.user && tokenStore.user.mcp !== true) return '：账号未开启「MCP 访问」，请联系管理员'
  return ''
})

const mcpUrl = computed(() => {
  const origin = baseUrl.value || window.location.origin
  return `${origin}/assist/mcp/stream`
})

/** 用于生成配置的令牌：未选定时用占位符（避免误导为「已经能用了」） */
const configToken = computed(() => selectedToken.value?.token || TOKEN_PLACEHOLDER)

/** 表格/信息表里只显示尾 4 位 */
const maskedToken = computed(() => maskValue(selectedToken.value?.token))

const clients = [
  { key: 'codebuddy', label: 'CodeBuddy' },
  { key: 'claude', label: 'Claude Desktop' },
  { key: 'codex', label: 'Codex' },
  { key: 'pi', label: 'Pi Agent' },
  { key: 'opencode', label: 'OpenCode' }
]
const activeClient = ref('codebuddy')

const configPath = computed(() => {
  const paths: Record<string, string> = {
    codebuddy: '~/.codebuddy/.mcp.json（具体路径以 CodeBuddy 设置为准，https://www.codebuddy.ai/docs/zh/cli/mcp#%E9%85%8D%E7%BD%AE%E6%96%87%E4%BB%B6）',
    claude: 'macOS: ~/Library/Application Support/Claude/claude_desktop_config.json；Linux: ~/.config/claude-desktop/config.json',
    codex: '~/.codex/config.toml（或 ~/.config/codex/config.toml）',
    pi: '~/.config/mcp/mcp.json（或项目级 .mcp.json）',
    opencode: '~/.config/opencode/opencode.json（或项目级 opencode.json）'
  }
  return paths[activeClient.value] || ''
})

const clientConfig = computed(() => {
  const token = configToken.value
  const configs: Record<string, string> = {
    codebuddy: `{
  "mcpServers": {
    "okb-assist": {
      "type": "http",
      "url": "${mcpUrl.value}",
      "headers": {
        "Authorization": "Bearer ${token}"
      }
    }
  }
}`,
    claude: `{
  "mcpServers": {
    "okb-assist": {
      "type": "http",
      "url": "${mcpUrl.value}",
      "headers": {
        "Authorization": "Bearer ${token}"
      }
    }
  }
}`,
    codex: `# Add to your Codex config.toml:
[mcp_servers.okb-assist]
url = "${mcpUrl.value}"
http_headers = {
    Authorization = "Bearer ${token}"
}
`,
    pi: `{
  "mcpServers": {
    "okb-assist": {
      "url": "${mcpUrl.value}",
      "headers": {
        "Authorization": "Bearer ${token}"
      }
    }
  }
}`,
    opencode: `{
  "mcp": {
    "okb-assist": {
      "type": "remote",
      "url": "${mcpUrl.value}",
      "headers": {
        "Authorization": "Bearer ${token}"
      }
    }
  }
}`
  }
  return configs[activeClient.value] || ''
})

onMounted(() => {
  load()
})

async function load() {
  // 不需要走后端 API 取地址，直接用当前页面 origin
  baseUrl.value = window.location.origin
  await loadMcpTokens()
  restoreSelection()
}

/** 拉取自己的 MCP 令牌（`GET /assist/api/auth/tokens`，MCP 条目带明文字段） */
async function loadMcpTokens() {
  try {
    const res = await listMyTokens()
    mcpTokens.value = (res.tokens || []).filter((t) => t.scope === 'mcp')
  } catch {
    mcpTokens.value = []
  }
}

/** 恢复上次选中的令牌；没有记录则默认选第一个明文可用的 */
function restoreSelection() {
  const saved = localStorage.getItem(selectedKey.value) || ''
  if (saved && plainTokens.value.some((t) => t.id === saved)) {
    selectedTokenId.value = saved
    return
  }
  selectedTokenId.value = plainTokens.value[0]?.id || ''
  persistSelection()
}

function onSelectToken() {
  persistSelection()
}

function persistSelection() {
  if (selectedTokenId.value) localStorage.setItem(selectedKey.value, selectedTokenId.value)
  else localStorage.removeItem(selectedKey.value)
}

function maskValue(v?: string | null): string {
  if (!v) return ''
  return v.length <= 4 ? '••••' : `••••${v.slice(-4)}`
}

function formatTime(v?: string | null): string {
  if (!v) return '—'
  const d = new Date(v)
  if (Number.isNaN(d.getTime())) return v
  return d.toLocaleString()
}

function toggleReveal(id: string) {
  revealed.value[id] = !revealed.value[id]
}

async function copyValue(v?: string | null) {
  if (!v) return
  try {
    await navigator.clipboard.writeText(v)
    showSuccess('已复制到剪贴板')
  } catch {
    showError('复制失败，请手动选择复制')
  }
}

/** 在本页直接创建 MCP 令牌（明文由后端回呈，创建后自动选进配置） */
async function createToken() {
  creating.value = true
  try {
    const res = await issueMyToken({
      label: newLabel.value.trim() || 'mcp',
      scope: 'mcp',
      token: newPlain.value.trim() || undefined
    })
    showSuccess('MCP 令牌已创建并选入配置')
    newLabel.value = ''
    newPlain.value = ''
    await loadMcpTokens()
    const created = mcpTokens.value.find((t) => t.id === res.meta?.id) || plainTokens.value[0]
    selectedTokenId.value = created?.id || ''
    persistSelection()
  } catch (e) {
    const err = e as { response?: { data?: { detail?: string } } }
    showError(err.response?.data?.detail || '创建失败')
  } finally {
    creating.value = false
  }
}

async function revokeToken(t: ApiTokenMeta) {
  if (!window.confirm(`确定撤销令牌「${t.label || '未命名'}」？使用它的 MCP 客户端会立即失效。`)) {
    return
  }
  try {
    await revokeMyToken(t.id)
    showSuccess('令牌已撤销')
    delete revealed.value[t.id]
    if (selectedTokenId.value === t.id) selectedTokenId.value = ''
    await loadMcpTokens()
    if (!plainTokens.value.some((x) => x.id === selectedTokenId.value)) {
      selectedTokenId.value = plainTokens.value[0]?.id || ''
    }
    persistSelection()
  } catch {
    showError('撤销失败')
  }
}

function copy() {
  if (!selectedToken.value) {
    // 占位符也可以复制（方便先搭好配置、稍后再选令牌），但不报成功误导用户
    navigator.clipboard.writeText(clientConfig.value)
    showSuccess('已复制（令牌仍为占位符，选好令牌后需重新复制）')
    return
  }
  navigator.clipboard.writeText(clientConfig.value)
  showSuccess('已复制到剪贴板')
}

/**
 * 用选中的令牌向后端发一个 JSON-RPC `initialize`，验证令牌与 MCP 权限。
 * 直接用 fetch：axios 实例会自动带 `X-Token`（当前登录凭证），会混淆被测令牌。
 */
async function testConnection() {
  const token = selectedToken.value?.token
  if (!token) {
    showError('请先创建并选择一个 MCP 令牌')
    return
  }
  testing.value = true
  try {
    const res = await fetch(mcpUrl.value, {
      method: 'POST',
      headers: {
        'Content-Type': 'application/json',
        Accept: 'application/json, text/event-stream',
        Authorization: `Bearer ${token}`
      },
      body: JSON.stringify({
        jsonrpc: '2.0',
        id: 1,
        method: 'initialize',
        params: {
          protocolVersion: '2024-11-05',
          capabilities: {},
          clientInfo: { name: 'okb-assist-web', version: '0.1.0' }
        }
      })
    })

    if (res.status === 401) {
      let detail = ''
      try {
        const body = await res.json()
        detail = body?.error?.message || ''
      } catch {
        /* 忽略：非 JSON 响应 */
      }
      if (detail.includes('MCP access not granted')) {
        showError('认证失败：账号未开启 MCP 权限（需管理员在用户管理里开启）')
      } else {
        showError('认证失败（401）：令牌无效或已撤销')
      }
      return
    }
    if (!res.ok) {
      showError(`连接失败（HTTP ${res.status}）`)
      return
    }
    let serverName = 'OKB-Assist'
    try {
      const body = await res.json()
      serverName = body?.result?.serverInfo?.name || serverName
    } catch {
      /* 忽略：非 JSON 响应 */
    }
    showSuccess(`连接成功：${serverName} 已接受该令牌`)
  } catch (e) {
    showError(`连接失败：${e instanceof Error ? e.message : String(e)}`)
  } finally {
    testing.value = false
  }
}
</script>

<style scoped>
.hint {
  color: var(--text-secondary);
  margin-bottom: 16px;
  line-height: 1.6;
}
.hint-sm {
  margin: 12px 0 0;
  font-size: 13px;
}
.hint-sm code {
  color: var(--primary);
}
/* 凭证状态卡片：只报类型和权限，不展示任何明文凭证 */
.cred-card {
  border: 1px solid var(--border);
  border-radius: 8px;
  padding: 12px 16px;
  margin-bottom: 20px;
  background: #fafafa;
}
.cred-row {
  display: flex;
  gap: 12px;
  align-items: baseline;
  padding: 4px 0;
  font-size: 13px;
  flex-wrap: wrap;
}
.cred-label {
  width: 88px;
  flex-shrink: 0;
  color: var(--text-secondary);
}
.cred-value {
  flex: 1;
  min-width: 0;
  word-break: break-all;
}
.cred-value.ok {
  color: #1a7f37;
}
.cred-value.warn {
  color: #b54708;
}
.cred-note {
  margin-left: 6px;
  font-size: 12px;
  color: var(--text-secondary);
}
.role-badge {
  margin-left: 6px;
  padding: 1px 8px;
  border-radius: 10px;
  background: var(--bg);
  border: 1px solid var(--border);
  font-size: 12px;
  color: var(--text-secondary);
}

/* 我的 MCP 令牌 */
.token-section {
  margin-bottom: 20px;
  padding: 14px 16px;
  border: 1px solid var(--border);
  border-radius: 8px;
}
.token-head {
  display: flex;
  gap: 10px;
  align-items: baseline;
  margin-bottom: 12px;
}
.token-head h4 {
  margin: 0;
}
.token-count {
  font-size: 12px;
  color: var(--text-secondary);
}
.notice {
  padding: 10px 12px;
  border-radius: 6px;
  font-size: 13px;
  line-height: 1.6;
  margin-bottom: 12px;
}
.notice-info {
  background: var(--primary-bg, #eef4ff);
  color: var(--text-primary);
  border: 1px solid var(--border);
}
.notice-warn {
  background: var(--warning-bg, #fff7e6);
  color: #b54708;
  border: 1px solid #ffd8a8;
}
.token-create {
  display: flex;
  gap: 8px;
  align-items: center;
  flex-wrap: wrap;
  margin-bottom: 12px;
}
.token-create input {
  flex: 1;
  min-width: 160px;
  padding: 8px 10px;
  border: 1px solid var(--border);
  border-radius: 6px;
  font-size: 13px;
}
.token-table-wrap {
  overflow-x: auto;
  min-width: 0;
}
.token-table {
  width: 100%;
  min-width: 620px;
  border-collapse: collapse;
  font-size: 13px;
}
.token-table th,
.token-table td {
  padding: 8px 10px;
  border-bottom: 1px solid var(--border);
  text-align: left;
  vertical-align: middle;
}
.token-table th {
  color: var(--text-secondary);
  background: #f7f7f7;
  font-weight: 600;
}
.label-cell {
  white-space: nowrap;
}
.in-use {
  margin-left: 6px;
  padding: 0 6px;
  border-radius: 8px;
  background: var(--primary-bg, #eef4ff);
  color: var(--primary);
  font-size: 11px;
}
.in-use.warn {
  background: var(--warning-bg, #fff7e6);
  color: #b54708;
}
.empty-hint {
  color: var(--text-secondary);
  font-size: 12px;
}
.muted {
  color: var(--text-secondary);
}
.row-actions {
  white-space: nowrap;
}

/* 令牌选择 */
.token-input {
  margin-bottom: 20px;
}
.token-input > label {
  display: block;
  margin-bottom: 6px;
  font-size: 13px;
  font-weight: 600;
}
.token-input-row {
  display: flex;
  gap: 8px;
  align-items: center;
  flex-wrap: wrap;
}
.token-input-row select {
  flex: 1;
  min-width: 200px;
  padding: 8px 10px;
  border: 1px solid var(--border);
  border-radius: 6px;
  font-size: 13px;
  background: var(--bg-card, #fff);
}
.warn-text {
  color: #b54708;
}
.muted-value {
  color: var(--text-secondary);
}

.client-tabs {
  display: flex;
  gap: 4px;
  margin-bottom: 16px;
  border-bottom: 1px solid var(--border);
  overflow-x: auto;
  -webkit-overflow-scrolling: touch;
  flex-wrap: nowrap;
}

.client-tabs::-webkit-scrollbar {
  display: none;
}

.tab {
  padding: 10px 20px;
  border: none;
  background: transparent;
  cursor: pointer;
  font-size: 14px;
  color: var(--text-secondary);
  border-bottom: 2px solid transparent;
  transition: all 0.15s;
  white-space: nowrap;
  flex-shrink: 0;
}
.tab.active {
  color: var(--primary);
  border-bottom-color: var(--primary);
}
.config-path {
  margin-bottom: 12px;
  font-size: 13px;
  color: var(--text-secondary);
}
.config-path code {
  color: var(--primary);
  word-break: break-all;
}
.config-output {
  display: flex;
  gap: 12px;
  align-items: flex-start;
  margin-bottom: 24px;
}
.config-output pre {
  flex: 1;
  min-width: 0;
  background: #1e1e1e;
  color: #d4d4d4;
  padding: 16px;
  border-radius: 8px;
  overflow-x: auto;
  font-size: 13px;
  line-height: 1.6;
}
.common-config h4 {
  margin-bottom: 12px;
}
.info-table {
  width: 100%;
  border-collapse: collapse;
}
.info-table th,
.info-table td {
  padding: 10px 16px;
  border-bottom: 1px solid var(--border);
  text-align: left;
}
.info-table th {
  width: 120px;
  color: var(--text-secondary);
  background: #f7f7f7;
}

.token-cell {
  display: flex;
  gap: 8px;
  align-items: center;
  flex-wrap: wrap;
}

.token-value {
  font-family: 'SF Mono', 'Cascadia Code', 'Fira Code', monospace;
  font-size: 12px;
  word-break: break-all;
  user-select: all;
  flex: 1;
}

@media (max-width: 768px) {
  .config-output {
    flex-direction: column;
    gap: 12px;
  }

  .config-output pre {
    width: 100%;
    font-size: 12px;
    padding: 12px;
  }

  .config-output button {
    width: 100%;
  }

  .info-table th,
  .info-table td {
    padding: 8px 12px;
    font-size: 13px;
  }

  .info-table th {
    width: 100px;
  }

  .info-table td {
    word-break: break-all;
  }

  .token-cell {
    flex-direction: column;
    align-items: stretch;
  }

  .token-cell button {
    width: 100%;
  }

  .cred-card {
    padding: 10px 12px;
  }

  .cred-row {
    flex-direction: column;
    gap: 2px;
  }

  .cred-label {
    width: auto;
    font-size: 12px;
  }

  .token-create {
    flex-direction: column;
    align-items: stretch;
  }

  .token-create input {
    min-width: 0;
    width: 100%;
  }

  .token-create button {
    width: 100%;
  }

  .token-input-row {
    flex-direction: column;
    align-items: stretch;
  }

  .token-input-row select {
    min-width: 0;
    width: 100%;
  }

  .token-input-row button {
    width: 100%;
  }
}

@media (max-width: 480px) {
  .config-output pre {
    font-size: 11px;
    padding: 10px;
  }

  .info-table th,
  .info-table td {
    padding: 6px 10px;
    font-size: 12px;
  }
}
</style>
