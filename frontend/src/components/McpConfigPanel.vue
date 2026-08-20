<template>
  <div class="mcp-config-panel">
    <p class="hint">
      将以下配置复制到你的 MCP 客户端（CodeBuddy / Claude / Codex），
      即可使用 OKB-Assist 的工具与资源。
    </p>

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
        <tr><th>MCP Token</th><td><code>{{ mcpToken }}</code></td></tr>
      </table>
    </div>
  </div>
</template>

<script setup lang="ts">
import { ref, computed, onMounted } from 'vue'
import { getSystemConfig } from '@/api/config'
import { useToast } from '@/composables/useToast'

const { showSuccess } = useToast()

const mcpUrl = computed(() => `${window.location.origin}/assist/mcp/stream`)
const mcpToken = ref('change-me')

const clients = [
  { key: 'codebuddy', label: 'CodeBuddy' },
  { key: 'claude', label: 'Claude Desktop' },
  { key: 'codex', label: 'Codex' }
]
const activeClient = ref('codebuddy')

const configPath = computed(() => {
  const paths: Record<string, string> = {
    codebuddy: '~/.codebuddy/mcp.json（具体路径以 CodeBuddy 设置为准）',
    claude: 'macOS: ~/Library/Application Support/Claude/claude_desktop_config.json；Linux: ~/.config/claude-desktop/config.json',
    codex: '~/.codex/config.toml（或 ~/.config/codex/config.toml）'
  }
  return paths[activeClient.value] || ''
})

const clientConfig = computed(() => {
  const configs: Record<string, string> = {
    codebuddy: `{
  "mcpServers": {
    "okb-assist": {
      "type": "sse",
      "url": "${mcpUrl.value}",
      "headers": {
        "Authorization": "Bearer ${mcpToken.value}"
      }
    }
  }
}`,
    claude: `{
  "mcpServers": {
    "okb-assist": {
      "command": "npx",
      "args": ["-y", "@modelcontextprotocol/server-sse"],
      "env": {
        "MCP_URL": "${mcpUrl.value}",
        "MCP_TOKEN": "${mcpToken.value}"
      }
    }
  }
}`,
    codex: `# Add to your Codex config.toml:
[mcp_servers.okb-assist]
url = "${mcpUrl.value}"
token = "${mcpToken.value}"`
  }
  return configs[activeClient.value] || ''
})

async function load() {
  try {
    const sys = await getSystemConfig()
    mcpToken.value = sys.mcp_token || 'change-me'
  } catch { /* ignore */ }
}

function copy() {
  navigator.clipboard.writeText(clientConfig.value)
  showSuccess('已复制到剪贴板')
}

onMounted(load)
</script>

<style scoped>
.hint {
  color: var(--text-secondary);
  margin-bottom: 16px;
  line-height: 1.6;
}
.client-tabs {
  display: flex;
  gap: 4px;
  margin-bottom: 16px;
  border-bottom: 1px solid var(--border);
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
</style>
