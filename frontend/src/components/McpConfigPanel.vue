<template>
  <div class="mcp-config-panel">
    <p class="hint">
      将以下配置复制到你的 MCP 客户端（CodeBuddy / Claude / Codex / Pi / OpenCode），
      即可使用 OKB-Assist 的工具与资源。
    </p>

    <!-- 当前凭证状态：只展示类型与权限，不展示任何明文凭证 -->
    <div class="cred-card">
      <div class="cred-row">
        <span class="cred-label">当前账号</span>
        <span class="cred-value">
          {{ accountLabel }}
          <span class="role-badge">{{ tokenStore.roleLabel }}</span>
        </span>
      </div>
      <div class="cred-row">
        <span class="cred-label">当前凭证</span>
        <span class="cred-value">{{ viaLabel }}</span>
      </div>
      <div class="cred-row">
        <span class="cred-label">MCP 权限</span>
        <span class="cred-value" :class="mcpReady ? 'ok' : 'warn'">
          {{ mcpReady ? '✅ 可用' : '⚠️ 不可用' }}<span class="cred-note">{{ mcpReason }}</span>
        </span>
      </div>
      <div class="cred-actions">
        <a class="btn btn-sm" :href="accountUrl">去账户页签发 MCP 令牌</a>
        <span class="cred-hint">MCP 令牌明文可随时在账户页查看，无需当场保存</span>
      </div>
    </div>

    <!-- 选择自己的 MCP 令牌（后端回呈明文）或手动粘贴 -->
    <div class="token-input">
      <label for="mcp-token-select">我的 MCP 令牌</label>
      <div class="token-input-row">
        <select id="mcp-token-select" v-model="selectedTokenId" @change="onSelectToken">
          <option value="">── 手动粘贴下面的令牌 ──</option>
          <option v-for="t in usableTokens" :key="t.id" :value="t.id">
            {{ t.label || '未命名' }} · {{ maskValue(t.token) }}
          </option>
        </select>
        <a class="btn btn-sm btn-outline" :href="accountUrl">签发新的 MCP 令牌</a>
      </div>
      <p v-if="!usableTokens.length" class="hint hint-sm">
        还没有可用的 MCP 令牌（或当前账号未开启 MCP 权限）。
        <a :href="accountUrl">去账户页签发 MCP 令牌</a>。
      </p>
      <p v-if="legacyTokens.length" class="hint hint-sm warn-text">
        有 {{ legacyTokens.length }} 个旧 MCP 令牌的明文不可恢复（签发时还没有该功能），请到账户页撤销后重新签发。
      </p>

      <label for="mcp-token-input">或手动粘贴令牌</label>
      <div class="token-input-row">
        <input
          id="mcp-token-input"
          v-model="mcpToken"
          type="text"
          placeholder="okb_… 或你在账户页自定义的令牌（仅用于本页生成配置）"
          autocomplete="off"
          spellcheck="false"
        />
        <button class="btn btn-sm" :disabled="testing" @click="testConnection">
          {{ testing ? '测试中…' : '测试连接' }}
        </button>
        <button v-if="mcpToken" class="btn btn-sm btn-outline" @click="clearToken">清除</button>
      </div>
      <label class="remember-line">
        <input v-model="remember" type="checkbox" @change="onRememberChange" />
        在这台机器的浏览器里记住（存入 localStorage，公共电脑请勿勾选）
      </label>
      <p v-if="!mcpToken" class="hint hint-sm warn-text">
        未选定令牌：下方配置里留的是占位符 <code>{{ TOKEN_PLACEHOLDER }}</code>，选好或粘贴后再复制才能直接使用。
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
            <code v-if="mcpToken" class="token-value">{{ maskedToken }}</code>
            <code v-else class="token-value muted-value">{{ TOKEN_PLACEHOLDER }}</code>
            <span class="cred-note">{{ mcpToken ? '（当前用于生成配置的 MCP 令牌）' : '（尚未选择/粘贴）' }}</span>
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
import { ref, computed, onMounted, watch } from 'vue'
import { useToast } from '@/composables/useToast'
import { useTokenStore } from '@/stores/token'
import { listMyTokens } from '@/api/auth'
import type { ApiTokenMeta } from '@/types/auth'

const { showSuccess, showError } = useToast()
const tokenStore = useTokenStore()

const baseUrl = ref('')

// 【重要】页面只展示 **MCP 专属令牌**（`scope=mcp`），它们由后端回呈明文，可直接选进配置：
// - `sess_…` 是整站登录态，抄进 MCP 客户端等于把账号搬运出去，且进程重启即失效；
// - `scope=api` 的个人令牌在 MCP 上必然 401（`Principal.mcp = 用户.mcp && scope == mcp`）；
// - `scope=mcp` 的令牌无法登录前端，只用于 MCP，所以它的明文在这里暴露面可控。
const TOKEN_PLACEHOLDER = '<在此粘贴你的 MCP 令牌>'

/** 用户粘贴/选中的 MCP 令牌（**仅本页内存**，刷新即清空；勾选「记住」才落 localStorage） */
const mcpToken = ref('')
const remember = ref(false)
const testing = ref(false)

/** 账户页签发的 MCP 令牌（含明文，供下拉选择） */
const mcpTokens = ref<ApiTokenMeta[]>([])
const selectedTokenId = ref('')

/** 明文可用的令牌（改版前签发的 MCP 令牌只有哈希，无法回呈） */
const usableTokens = computed(() => mcpTokens.value.filter((t) => !!t.token))
const legacyTokens = computed(() => mcpTokens.value.filter((t) => !t.token))

/** 账户页直达地址（带 scope=mcp 预选） */
const accountUrl = computed(() => {
  const origin = baseUrl.value || window.location.origin
  return `${origin}/assist/account?scope=mcp`
})

function maskValue(v?: string | null): string {
  if (!v) return ''
  return v.length <= 4 ? '••••' : `••••${v.slice(-4)}`
}

/** 从下拉选中令牌 → 填入配置 */
function onSelectToken() {
  const picked = usableTokens.value.find((t) => t.id === selectedTokenId.value)
  mcpToken.value = picked?.token || ''
}

/** 记住令牌的 localStorage 键（按用户隔离，避免多账号互相覆盖） */
const rememberKey = computed(() => `okb_mcp_config_token:${tokenStore.user?.id || 'anon'}`)

const accountLabel = computed(() => {
  return tokenStore.displayName || tokenStore.user?.username || '未登录'
})

/** 当前凭证类型（只给类型，不给内容） */
const viaLabel = computed(() => {
  const labels: Record<string, string> = {
    session: '会话令牌（密码登录，重启服务即失效）',
    api_token: '个人访问令牌（权限 API）',
    local_subnet: '受信任子网（免凭证）'
  }
  return labels[tokenStore.via] || '未知'
})

/** 当前凭证是否真能调 MCP：取后端算好的最终值（用户 mcp 开关 × 令牌 scope） */
const mcpReady = computed(() => tokenStore.mcp === true)

const mcpReason = computed(() => {
  if (mcpReady.value) return ''
  if (tokenStore.via === 'api_token') return '：当前令牌 scope=api，需另签发 scope=mcp 的令牌'
  if (tokenStore.user && tokenStore.user.mcp !== true) return '：账号未开启「MCP 访问」，请联系管理员'
  return ''
})

/** 表格里只显示尾 4 位 */
const maskedToken = computed(() => {
  const t = mcpToken.value
  if (!t) return ''
  return t.length <= 4 ? '••••' : `••••••••${t.slice(-4)}`
})

function onRememberChange() {
  if (remember.value && mcpToken.value) {
    localStorage.setItem(rememberKey.value, mcpToken.value)
  } else if (!remember.value) {
    localStorage.removeItem(rememberKey.value)
  }
}

function clearToken() {
  mcpToken.value = ''
  localStorage.removeItem(rememberKey.value)
}

// 勾选「记住」时保留令牌变更；手动改动了粘贴框与所选令牌不一致时退回「手动粘贴」
watch(mcpToken, (v) => {
  if (selectedTokenId.value) {
    const picked = usableTokens.value.find((t) => t.id === selectedTokenId.value)
    if (!picked || (picked.token || '') !== v) selectedTokenId.value = ''
  }
  if (!remember.value) return
  if (v) localStorage.setItem(rememberKey.value, v)
  else localStorage.removeItem(rememberKey.value)
})

const mcpUrl = computed(() => {
  const origin = baseUrl.value || window.location.origin
  return `${origin}/assist/mcp/stream`
})

/** 用于生成配置的令牌：未粘贴时用占位符（避免误导为「已经能用了」） */
const configToken = computed(() => mcpToken.value.trim() || TOKEN_PLACEHOLDER)

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

async function load() {
  // 不需要走后端 API，直接使用当前页面 origin
  baseUrl.value = window.location.origin

  await loadMcpTokens()

  // 只恢复用户主动勾选「记住」的 MCP 令牌（绝不回写登录凭证）
  const saved = localStorage.getItem(rememberKey.value)
  if (saved) {
    mcpToken.value = saved
    remember.value = true
    const matched = usableTokens.value.find((t) => t.token === saved)
    if (matched) selectedTokenId.value = matched.id
    return
  }
  // 没有记住的令牌时，自动选中第一个可用的 MCP 令牌（明文由后端回呈）
  const first = usableTokens.value[0]
  if (first) {
    selectedTokenId.value = first.id
    mcpToken.value = first.token || ''
  }
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

function copy() {
  if (!mcpToken.value) {
    // 占位符也可以复制（方便先搭好配置、稍后再替换），但不报成功误导用户
    navigator.clipboard.writeText(clientConfig.value)
    showSuccess('已复制（令牌仍为占位符，粘贴令牌后需重新复制）')
    return
  }
  navigator.clipboard.writeText(clientConfig.value)
  showSuccess('已复制到剪贴板')
}

/**
 * 用粘贴的令牌向上后端发一个 JSON-RPC `initialize`，验证令牌与 MCP 权限。
 * 直接用 fetch：axios 实例会自动带 `X-Token`（当前登录凭证），会混淆被测令牌。
 */
async function testConnection() {
  const token = mcpToken.value.trim()
  if (!token) {
    showError('请先粘贴你的 MCP 令牌')
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
        showError('认证失败：账号或该令牌未开启 MCP 权限（需 scope=mcp 且账号 MCP 开关已开）')
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

onMounted(() => {
  load()
})
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
  margin-bottom: 16px;
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
.cred-actions {
  display: flex;
  gap: 12px;
  align-items: center;
  flex-wrap: wrap;
  margin-top: 10px;
  padding-top: 10px;
  border-top: 1px dashed var(--border);
}
.cred-hint {
  font-size: 12px;
  color: var(--text-secondary);
}

/* 令牌输入 */
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
.token-input-row input {
  flex: 1;
  min-width: 200px;
  padding: 8px 10px;
  border: 1px solid var(--border);
  border-radius: 6px;
  font-family: 'SF Mono', 'Cascadia Code', 'Fira Code', monospace;
  font-size: 12px;
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
.token-input > label:not(:first-of-type) {
  margin-top: 14px;
}
.remember-line {
  display: flex;
  gap: 6px;
  align-items: center;
  margin-top: 8px;
  font-size: 12px;
  color: var(--text-secondary);
  cursor: pointer;
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

  .token-input-row {
    flex-direction: column;
    align-items: stretch;
  }

  .token-input-row input {
    min-width: 0;
    width: 100%;
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