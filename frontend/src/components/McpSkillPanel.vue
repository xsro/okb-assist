<template>
  <div class="mcp-skill-panel">
    <p class="hint">
      将以下技能文件保存为 SKILL.md，让 AI 助手学会通过 curl / PowerShell 调用 OKB-Assist MCP API。
    </p>

    <div class="skill-tabs">
      <button
        v-for="s in skills"
        :key="s.key"
        class="tab"
        :class="{ active: activeSkill === s.key }"
        @click="activeSkill = s.key"
      >
        {{ s.label }}
      </button>
    </div>

    <div class="skill-actions">
      <button class="btn btn-sm btn-outline" @click="copySkill">复制</button>
      <button class="btn btn-sm btn-outline" @click="downloadSkill">下载</button>
    </div>

    <pre class="skill-content"><code>{{ skillContent }}</code></pre>
  </div>
</template>

<script setup lang="ts">
import { ref, computed, onMounted } from 'vue'
import { getSystemConfig, getServiceConfig } from '@/api/config'
import { useToast } from '@/composables/useToast'

const { showSuccess } = useToast()

const baseUrl = ref('')
const mcpToken = ref('change-me')

const skills = [
  { key: 'curl', label: 'curl (Shell)' },
  { key: 'powershell', label: 'PowerShell' }
]
const activeSkill = ref('curl')

const mcpUrl = computed(() => {
  const origin = baseUrl.value || window.location.origin
  return origin + '/assist/mcp/stream'
})

function getCurlSkill(): string {
  const url = mcpUrl.value
  const token = mcpToken.value
  return '# OKB-Assist MCP API \u2014 curl 技能\n\n' +
    '> 适用场景：在 Shell / 脚本中通过 HTTP 调用 OKB-Assist 的 MCP 工具。\n\n' +
    '## 前置条件\n\n' +
    '- OKB-Assist 后端正在运行（默认 http://localhost:5001）\n' +
    '- 已知 MCP Token（system.json 中的 mcp_token 字段，默认为 change-me 时禁用认证）\n\n' +
    '## 环境变量\n\n' +
    'bash\n' +
    'export MCP_URL="' + url + '"\n' +
    'export MCP_TOKEN="' + token + '"\n' +
    '\n\n' +
    '## 初始化会话\n\n' +
    'MCP 使用 Streamable HTTP 传输，首次请求需发送 initialize：\n\n' +
    'bash\n' +
    'curl -s -X POST "${MCP_URL}" \\\n' +
    '  -H "Content-Type: application/json" \\\n' +
    '  -H "Authorization: Bearer ${MCP_TOKEN}" \\\n' +
    '  -d \x27{\n' +
    '    "jsonrpc": "2.0",\n' +
    '    "id": 1,\n' +
    '    "method": "initialize",\n' +
    '    "params": {\n' +
    '      "protocolVersion": "2024-11-05",\n' +
    '      "clientInfo": { "name": "curl-skill", "version": "1.0.0" },\n' +
    '      "capabilities": {}\n' +
    '    }\n' +
    '  }\x27\n' +
    '\n\n' +
    '> 响应中包含 mcp-session-id header，后续请求需携带。\n\n' +
    '## 列出可用工具\n\n' +
    'bash\n' +
    'curl -s -X POST "${MCP_URL}" \\\n' +
    '  -H "Content-Type: application/json" \\\n' +
    '  -H "Authorization: Bearer ${MCP_TOKEN}" \\\n' +
    '  -H "mcp-session-id: ${SESSION_ID}" \\\n' +
    '  -d \x27{\n' +
    '    "jsonrpc": "2.0",\n' +
    '    "id": 2,\n' +
    '    "method": "tools/list"\n' +
    '  }\x27 | jq .\n' +
    '\n\n' +
    '## 调用工具示例\n\n' +
    '### grep_search（全文搜索）\n\n' +
    'bash\n' +
    'curl -s -X POST "${MCP_URL}" \\\n' +
    '  -H "Content-Type: application/json" \\\n' +
    '  -H "Authorization: Bearer ${MCP_TOKEN}" \\\n' +
    '  -H "mcp-session-id: ${SESSION_ID}" \\\n' +
    '  -d \x27{\n' +
    '    "jsonrpc": "2.0",\n' +
    '    "id": 3,\n' +
    '    "method": "tools/call",\n' +
    '    "params": {\n' +
    '      "name": "grep_search",\n' +
    '      "arguments": {\n' +
    '        "query": "transformer",\n' +
    '        "limit": 10,\n' +
    '        "context": 2\n' +
    '      }\n' +
    '    }\n' +
    '  }\x27 | jq .\n' +
    '\n\n' +
    '### search_info（搜索文献元数据）\n\n' +
    'bash\n' +
    'curl -s -X POST "${MCP_URL}" \\\n' +
    '  -H "Content-Type: application/json" \\\n' +
    '  -H "Authorization: Bearer ${MCP_TOKEN}" \\\n' +
    '  -H "mcp-session-id: ${SESSION_ID}" \\\n' +
    '  -d \x27{\n' +
    '    "jsonrpc": "2.0",\n' +
    '    "id": 4,\n' +
    '    "method": "tools/call",\n' +
    '    "params": {\n' +
    '      "name": "search_info",\n' +
    '      "arguments": {\n' +
    '        "query": "attention",\n' +
    '        "limit": 5,\n' +
    '        "year_from": 2017\n' +
    '      }\n' +
    '    }\n' +
    '  }\x27 | jq .\n' +
    '\n\n' +
    '### read_markdown（读取文献 Markdown）\n\n' +
    'bash\n' +
    'curl -s -X POST "${MCP_URL}" \\\n' +
    '  -H "Content-Type: application/json" \\\n' +
    '  -H "Authorization: Bearer ${MCP_TOKEN}" \\\n' +
    '  -H "mcp-session-id: ${SESSION_ID}" \\\n' +
    '  -d \x27{\n' +
    '    "jsonrpc": "2.0",\n' +
    '    "id": 5,\n' +
    '    "method": "tools/call",\n' +
    '    "params": {\n' +
    '      "name": "read_markdown",\n' +
    '      "arguments": {\n' +
    '        "id": 42,\n' +
    '        "page": 1,\n' +
    '        "page_size": 5000\n' +
    '      }\n' +
    '    }\n' +
    '  }\x27 | jq .\n' +
    '\n\n' +
    '### get_document_info（获取文献详情）\n\n' +
    'bash\n' +
    'curl -s -X POST "${MCP_URL}" \\\n' +
    '  -H "Content-Type: application/json" \\\n' +
    '  -H "Authorization: Bearer ${MCP_TOKEN}" \\\n' +
    '  -H "mcp-session-id: ${SESSION_ID}" \\\n' +
    '  -d \x27{\n' +
    '    "jsonrpc": "2.0",\n' +
    '    "id": 6,\n' +
    '    "method": "tools/call",\n' +
    '    "params": {\n' +
    '      "name": "get_document_info",\n' +
    '      "arguments": { "id": 42 }\n' +
    '    }\n' +
    '  }\x27 | jq .\n' +
    '\n\n' +
    '### get_stats（获取统计信息）\n\n' +
    'bash\n' +
    'curl -s -X POST "${MCP_URL}" \\\n' +
    '  -H "Content-Type: application/json" \\\n' +
    '  -H "Authorization: Bearer ${MCP_TOKEN}" \\\n' +
    '  -H "mcp-session-id: ${SESSION_ID}" \\\n' +
    '  -d \x27{\n' +
    '    "jsonrpc": "2.0",\n' +
    '    "id": 7,\n' +
    '    "method": "tools/call",\n' +
    '    "params": {\n' +
    '      "name": "get_stats",\n' +
    '      "arguments": {}\n' +
    '    }\n' +
    '  }\x27 | jq .\n' +
    '\n\n' +
    '## 完整会话示例\n\n' +
    'bash\n' +
    '#!/bin/bash\n' +
    'set -euo pipefail\n\n' +
    'MCP_URL="${MCP_URL:-http://localhost:5001/assist/mcp/stream}"\n' +
    'MCP_TOKEN="${MCP_TOKEN:-' + token + '}"\n\n' +
    '# 1. 初始化\n' +
    'INIT_RESP=$(curl -s -X POST "${MCP_URL}" \\\n' +
    '  -H "Content-Type: application/json" \\\n' +
    '  -H "Authorization: Bearer ${MCP_TOKEN}" \\\n' +
    '  -d \x27{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","clientInfo":{"name":"curl-skill","version":"1.0.0"},"capabilities":{}}}\x27)\n\n' +
    'echo "${INIT_RESP}" | jq .\n\n' +
    '# 提取 session ID\n' +
    'SESSION_ID=$(echo "${INIT_RESP}" | jq -r \x27.headers."mcp-session-id" // empty\x27)\n' +
    'if [ -z "${SESSION_ID}" ]; then\n' +
    '  echo "Failed to get session ID" >&2\n' +
    '  exit 1\n' +
    'fi\n\n' +
    '# 2. 列出工具\n' +
    'curl -s -X POST "${MCP_URL}" \\\n' +
    '  -H "Content-Type: application/json" \\\n' +
    '  -H "Authorization: Bearer ${MCP_TOKEN}" \\\n' +
    '  -H "mcp-session-id: ${SESSION_ID}" \\\n' +
    '  -d \x27{"jsonrpc":"2.0","id":2,"method":"tools/list"}\x27 | jq .\n\n' +
    '# 3. 调用 grep_search\n' +
    'curl -s -X POST "${MCP_URL}" \\\n' +
    '  -H "Content-Type: application/json" \\\n' +
    '  -H "Authorization: Bearer ${MCP_TOKEN}" \\\n' +
    '  -H "mcp-session-id: ${SESSION_ID}" \\\n' +
    '  -d \x27{\n' +
    '    "jsonrpc": "2.0",\n' +
    '    "id": 3,\n' +
    '    "method": "tools/call",\n' +
    '    "params": {\n' +
    '      "name": "grep_search",\n' +
    '      "arguments": { "query": "transformer", "limit": 5 }\n' +
    '    }\n' +
    '  }\x27 | jq .\n' +
    '\n\n' +
    '## 错误处理\n\n' +
    '工具调用失败时返回统一错误格式：\n' +
    'json\n' +
    '{ "error": { "code": "invalid_argument", "message": "..." } }\n' +
    '\n\n' +
    '常见错误码：\n' +
    '- invalid_argument：参数错误\n' +
    '- not_found：文档不存在\n' +
    '- unauthorized：Token 无效或缺失\n'
}

function getPowerShellSkill(): string {
  const url = mcpUrl.value
  const token = mcpToken.value
  return '# OKB-Assist MCP API \u2014 PowerShell 技能\n\n' +
    '> 适用场景：在 Windows PowerShell 中通过 HTTP 调用 OKB-Assist 的 MCP 工具。\n\n' +
    '## 前置条件\n\n' +
    '- OKB-Assist 后端正在运行（默认 http://localhost:5001）\n' +
    '- 已知 MCP Token（system.json 中的 mcp_token 字段，默认为 change-me 时禁用认证）\n\n' +
    '## 环境变量\n\n' +
    'powershell\n' +
    '$env:MCP_URL = "' + url + '"\n' +
    '$env:MCP_TOKEN = "' + token + '"\n' +
    '\n\n' +
    '## 初始化会话\n\n' +
    'MCP 使用 Streamable HTTP 传输，首次请求需发送 initialize：\n\n' +
    'powershell\n' +
    '$initBody = @{\n' +
    '    jsonrpc = "2.0"\n' +
    '    id = 1\n' +
    '    method = "initialize"\n' +
    '    params = @{\n' +
    '        protocolVersion = "2024-11-05"\n' +
    '        clientInfo = @{ name = "powershell-skill"; version = "1.0.0" }\n' +
    '        capabilities = @{}\n' +
    '    }\n' +
    '} | ConvertTo-Json -Depth 10\n\n' +
    '$initResp = Invoke-RestMethod -Uri $env:MCP_URL `\n' +
    '    -Method Post `\n' +
    '    -ContentType "application/json" `\n' +
    '    -Headers @{ Authorization = "Bearer $env:MCP_TOKEN" } `\n' +
    '    -Body $initBody\n\n' +
    '$initResp | Format-List\n' +
    '\n\n' +
    '> 响应 header 中的 mcp-session-id 需在后续请求中携带。\n\n' +
    '## 列出可用工具\n\n' +
    'powershell\n' +
    '$sessionId = $initResp.headers."mcp-session-id"\n\n' +
    '$body = @{\n' +
    '    jsonrpc = "2.0"\n' +
    '    id = 2\n' +
    '    method = "tools/list"\n' +
    '} | ConvertTo-Json -Depth 10\n\n' +
    '$tools = Invoke-RestMethod -Uri $env:MCP_URL `\n' +
    '    -Method Post `\n' +
    '    -ContentType "application/json" `\n' +
    '    -Headers @{ \n' +
    '        Authorization = "Bearer $env:MCP_TOKEN"\n' +
    '        "mcp-session-id" = $sessionId\n' +
    '    } `\n' +
    '    -Body $body\n\n' +
    '$tools.result.tools | Format-Table name, description\n' +
    '\n\n' +
    '## 调用工具示例\n\n' +
    '### grep_search（全文搜索）\n\n' +
    'powershell\n' +
    '$body = @{\n' +
    '    jsonrpc = "2.0"\n' +
    '    id = 3\n' +
    '    method = "tools/call"\n' +
    '    params = @{\n' +
    '        name = "grep_search"\n' +
    '        arguments = @{\n' +
    '            query = "transformer"\n' +
    '            limit = 10\n' +
    '            context = 2\n' +
    '        }\n' +
    '    }\n' +
    '} | ConvertTo-Json -Depth 10\n\n' +
    '$result = Invoke-RestMethod -Uri $env:MCP_URL `\n' +
    '    -Method Post `\n' +
    '    -ContentType "application/json" `\n' +
    '    -Headers @{ \n' +
    '        Authorization = "Bearer $env:MCP_TOKEN"\n' +
    '        "mcp-session-id" = $sessionId\n' +
    '    } `\n' +
    '    -Body $body\n\n' +
    '$result | ConvertTo-Json -Depth 10\n' +
    '\n\n' +
    '### search_info（搜索文献元数据）\n\n' +
    'powershell\n' +
    '$body = @{\n' +
    '    jsonrpc = "2.0"\n' +
    '    id = 4\n' +
    '    method = "tools/call"\n' +
    '    params = @{\n' +
    '        name = "search_info"\n' +
    '        arguments = @{\n' +
    '            query = "attention"\n' +
    '            limit = 5\n' +
    '            year_from = 2017\n' +
    '        }\n' +
    '    }\n' +
    '} | ConvertTo-Json -Depth 10\n\n' +
    '$result = Invoke-RestMethod -Uri $env:MCP_URL `\n' +
    '    -Method Post `\n' +
    '    -ContentType "application/json" `\n' +
    '    -Headers @{ \n' +
    '        Authorization = "Bearer $env:MCP_TOKEN"\n' +
    '        "mcp-session-id" = $sessionId\n' +
    '    } `\n' +
    '    -Body $body\n\n' +
    '$result | ConvertTo-Json -Depth 10\n' +
    '\n\n' +
    '### read_markdown（读取文献 Markdown）\n\n' +
    'powershell\n' +
    '$body = @{\n' +
    '    jsonrpc = "2.0"\n' +
    '    id = 5\n' +
    '    method = "tools/call"\n' +
    '    params = @{\n' +
    '        name = "read_markdown"\n' +
    '        arguments = @{\n' +
    '            id = 42\n' +
    '            page = 1\n' +
    '            page_size = 5000\n' +
    '        }\n' +
    '    }\n' +
    '} | ConvertTo-Json -Depth 10\n\n' +
    '$result = Invoke-RestMethod -Uri $env:MCP_URL `\n' +
    '    -Method Post `\n' +
    '    -ContentType "application/json" `\n' +
    '    -Headers @{ \n' +
    '        Authorization = "Bearer $env:MCP_TOKEN"\n' +
    '        "mcp-session-id" = $sessionId\n' +
    '    } `\n' +
    '    -Body $body\n\n' +
    '$result | ConvertTo-Json -Depth 10\n' +
    '\n\n' +
    '### get_document_info（获取文献详情）\n\n' +
    'powershell\n' +
    '$body = @{\n' +
    '    jsonrpc = "2.0"\n' +
    '    id = 6\n' +
    '    method = "tools/call"\n' +
    '    params = @{\n' +
    '        name = "get_document_info"\n' +
    '        arguments = @{ id = 42 }\n' +
    '    }\n' +
    '} | ConvertTo-Json -Depth 10\n\n' +
    '$result = Invoke-RestMethod -Uri $env:MCP_URL `\n' +
    '    -Method Post `\n' +
    '    -ContentType "application/json" `\n' +
    '    -Headers @{ \n' +
    '        Authorization = "Bearer $env:MCP_TOKEN"\n' +
    '        "mcp-session-id" = $sessionId\n' +
    '    } `\n' +
    '    -Body $body\n\n' +
    '$result | ConvertTo-Json -Depth 10\n' +
    '\n\n' +
    '### get_stats（获取统计信息）\n\n' +
    'powershell\n' +
    '$body = @{\n' +
    '    jsonrpc = "2.0"\n' +
    '    id = 7\n' +
    '    method = "tools/call"\n' +
    '    params = @{\n' +
    '        name = "get_stats"\n' +
    '        arguments = @{}\n' +
    '    }\n' +
    '} | ConvertTo-Json -Depth 10\n\n' +
    '$result = Invoke-RestMethod -Uri $env:MCP_URL `\n' +
    '    -Method Post `\n' +
    '    -ContentType "application/json" `\n' +
    '    -Headers @{ \n' +
    '        Authorization = "Bearer $env:MCP_TOKEN"\n' +
    '        "mcp-session-id" = $sessionId\n' +
    '    } `\n' +
    '    -Body $body\n\n' +
    '$result | ConvertTo-Json -Depth 10\n' +
    '\n\n' +
    '## 完整会话示例\n\n' +
    'powershell\n' +
    '$ErrorActionPreference = "Stop"\n\n' +
    '$env:MCP_URL = if ($env:MCP_URL) { $env:MCP_URL } else { "http://localhost:5001/assist/mcp/stream" }\n' +
    '$env:MCP_TOKEN = if ($env:MCP_TOKEN) { $env:MCP_TOKEN } else { "' + token + '" }\n\n' +
    '# 1. 初始化\n' +
    '$initBody = @{\n' +
    '    jsonrpc = "2.0"\n' +
    '    id = 1\n' +
    '    method = "initialize"\n' +
    '    params = @{\n' +
    '        protocolVersion = "2024-11-05"\n' +
    '        clientInfo = @{ name = "powershell-skill"; version = "1.0.0" }\n' +
    '        capabilities = @{}\n' +
    '    }\n' +
    '} | ConvertTo-Json -Depth 10\n\n' +
    '$initResp = Invoke-RestMethod -Uri $env:MCP_URL `\n' +
    '    -Method Post `\n' +
    '    -ContentType "application/json" `\n' +
    '    -Headers @{ Authorization = "Bearer $env:MCP_TOKEN" } `\n' +
    '    -Body $initBody\n\n' +
    'Write-Host "Initialized:" -ForegroundColor Green\n' +
    '$initResp | Format-List\n\n' +
    '$sessionId = $initResp.headers."mcp-session-id"\n' +
    'if (-not $sessionId) {\n' +
    '    throw "Failed to get session ID"\n' +
    '}\n\n' +
    '# 2. 列出工具\n' +
    '$toolsBody = @{\n' +
    '    jsonrpc = "2.0"\n' +
    '    id = 2\n' +
    '    method = "tools/list"\n' +
    '} | ConvertTo-Json -Depth 10\n\n' +
    '$toolsResp = Invoke-RestMethod -Uri $env:MCP_URL `\n' +
    '    -Method Post `\n' +
    '    -ContentType "application/json" `\n' +
    '    -Headers @{ \n' +
    '        Authorization = "Bearer $env:MCP_TOKEN"\n' +
    '        "mcp-session-id" = $sessionId\n' +
    '    } `\n' +
    '    -Body $toolsBody\n\n' +
    'Write-Host "`nAvailable tools:" -ForegroundColor Green\n' +
    '$toolsResp.result.tools | Format-Table name, description -AutoSize\n\n' +
    '# 3. 调用 grep_search\n' +
    '$grepBody = @{\n' +
    '    jsonrpc = "2.0"\n' +
    '    id = 3\n' +
    '    method = "tools/call"\n' +
    '    params = @{\n' +
    '        name = "grep_search"\n' +
    '        arguments = @{ query = "transformer"; limit = 5 }\n' +
    '    }\n' +
    '} | ConvertTo-Json -Depth 10\n\n' +
    '$grepResult = Invoke-RestMethod -Uri $env:MCP_URL `\n' +
    '    -Method Post `\n' +
    '    -ContentType "application/json" `\n' +
    '    -Headers @{ \n' +
    '        Authorization = "Bearer $env:MCP_TOKEN"\n' +
    '        "mcp-session-id" = $sessionId\n' +
    '    } `\n' +
    '    -Body $grepBody\n\n' +
    'Write-Host "`ngrep_search result:" -ForegroundColor Green\n' +
    '$grepResult | ConvertTo-Json -Depth 10\n' +
    '\n\n' +
    '## 错误处理\n\n' +
    '工具调用失败时返回统一错误格式：\n' +
    'json\n' +
    '{ "error": { "code": "invalid_argument", "message": "..." } }\n' +
    '\n\n' +
    '常见错误码：\n' +
    '- invalid_argument：参数错误\n' +
    '- not_found：文档不存在\n' +
    '- unauthorized：Token 无效或缺失\n\n' +
    '> PowerShell 中可通过 $result.error 检测错误。\n'
}

const skillContent = computed(() => {
  return activeSkill.value === 'curl' ? getCurlSkill() : getPowerShellSkill()
})

async function load() {
  try {
    const sys = await getSystemConfig()
    mcpToken.value = sys.mcp_token || 'change-me'
  } catch { /* ignore */ }
  try {
    const svc = await getServiceConfig()
    baseUrl.value = svc.base_url || ''
  } catch { /* ignore */ }
}

function copySkill() {
  navigator.clipboard.writeText(skillContent.value)
  showSuccess('已复制到剪贴板')
}

function downloadSkill() {
  const ext = activeSkill.value === 'curl' ? 'sh' : 'ps1'
  const blob = new Blob([skillContent.value], { type: 'text/plain' })
  const url = URL.createObjectURL(blob)
  const a = document.createElement('a')
  a.href = url
  a.download = 'mcp-skill.' + ext
  a.click()
  URL.revokeObjectURL(url)
}

onMounted(load)
</script>

<style scoped>
.hint {
  color: var(--text-secondary);
  margin-bottom: 16px;
  line-height: 1.6;
}
.skill-tabs {
  display: flex;
  gap: 4px;
  margin-bottom: 12px;
  border-bottom: 1px solid var(--border);
  overflow-x: auto;
  -webkit-overflow-scrolling: touch;
  flex-wrap: nowrap;
}
.skill-tabs::-webkit-scrollbar {
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
.skill-actions {
  display: flex;
  gap: 8px;
  margin-bottom: 16px;
}
.skill-content {
  background: #1e1e1e;
  color: #d4d4d4;
  padding: 16px;
  border-radius: 8px;
  overflow-x: auto;
  font-size: 13px;
  line-height: 1.6;
  white-space: pre-wrap;
  word-break: break-all;
  max-height: 70vh;
  overflow-y: auto;
  margin: 0;
}

@media (max-width: 768px) {
  .skill-actions {
    flex-direction: column;
  }
  .skill-actions .btn {
    width: 100%;
  }
  .skill-content {
    font-size: 12px;
    padding: 12px;
    max-height: 60vh;
  }
}

@media (max-width: 480px) {
  .skill-content {
    font-size: 11px;
    padding: 10px;
  }
}
</style>