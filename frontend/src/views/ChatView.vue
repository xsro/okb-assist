<template>
  <div class="chat-view">
    <div class="chat-header">
      <h2>AI 问答</h2>

      <div class="chat-controls">
        <label class="control-item">
          <span class="control-label">模型</span>
          <select v-model="selectedModel" :disabled="loading || !models.length">
            <option v-for="m in models" :key="m.name" :value="m.name">
              {{ m.name }} · {{ m.protocol }} / {{ m.model }}
            </option>
          </select>
        </label>
        <label class="control-item switch" :title="tools.length ? toolsTitle : ''">
          <input v-model="toolsEnabled" type="checkbox" :disabled="loading" />
          <span>文献库工具{{ tools.length ? `（${tools.length}）` : '' }}</span>
        </label>
        <label class="control-item">
          <span class="control-label">工具协议</span>
          <select v-model="toolMode" :disabled="loading || !toolsEnabled" title="默认跟随模型配置；可临时覆盖：auto 先试原生，不支持时降级为提示词 JSON">
            <option value="">跟随模型配置</option>
            <option value="auto">自动（先原生后降级）</option>
            <option value="native">原生 tools</option>
            <option value="prompt">提示词 JSON</option>
          </select>
        </label>
        <button class="btn btn-sm btn-outline" @click="showSystem = !showSystem">
          {{ showSystem ? '收起系统提示词' : '系统提示词' }}
        </button>
        <button class="btn btn-sm btn-outline" @click="toggleHistory">
          历史会话{{ sessions.length ? `（${sessions.length}）` : '' }}
        </button>
        <button
          class="btn btn-sm"
          :disabled="!items.length || saving"
          @click="save"
        >
          {{ saving ? '保存中…' : sessionId ? '更新会话' : '保存会话' }}
        </button>
        <button class="btn btn-sm btn-outline" :disabled="!items.length" @click="clearChat">
          清空对话
        </button>
      </div>
    </div>

    <div class="session-bar">
      <span class="session-chip" :class="sessionId ? 'saved' : 'unsaved'">
        {{ sessionId ? `已保存：${sessionTitle}` : '未保存（点击「保存会话」写入服务端文件）' }}
      </span>
      <span v-if="dirty && sessionId" class="session-dirty">有未保存的修改</span>
    </div>

    <div v-if="historyOpen" class="history-panel">
      <div class="history-head">
        <span class="history-head-title">历史会话</span>
        <span class="history-hint">仅列出你有权访问的会话（admin 可看到全部）</span>
        <button class="btn btn-sm btn-outline" @click="loadSessions">刷新</button>
        <button class="btn btn-sm btn-outline" @click="historyOpen = false">收起</button>
      </div>
      <div v-if="historyLoading" class="history-empty">加载中…</div>
      <div v-else-if="!sessions.length" class="history-empty">暂无保存的会话</div>
      <ul v-else class="history-list">
        <li
          v-for="s in sessions"
          :key="s.id"
          class="history-item"
          :class="{ current: s.id === sessionId }"
        >
          <button class="history-main" @click="openSession(s.id)" :title="`打开：${s.title}`">
            <span class="history-title">{{ s.title }}</span>
            <span class="history-meta">
              <span class="history-badge" :class="`role-${s.role}`">
                {{ s.mine ? '我' : roleLabel(s.role) }}
              </span>
              <span>{{ s.message_count }} 条</span>
              <span v-if="s.model">{{ s.model }}</span>
              <span>{{ formatTime(s.updated_at) }}</span>
              <span v-if="!s.mine" class="history-other">他人会话</span>
            </span>
          </button>
          <button
            class="btn btn-sm btn-outline-danger"
            @click="removeSession(s)"
            :title="s.mine ? '删除该会话' : 'admin 删除他人会话'"
          >
            删除
          </button>
        </li>
      </ul>
    </div>

    <div v-if="showSystem" class="system-editor">
      <textarea
        v-model="systemPrompt"
        rows="3"
        placeholder="留空则使用服务端默认系统提示词"
      ></textarea>
    </div>

    <div v-if="loadError" class="chat-empty">
      <p>加载模型列表失败：{{ loadError }}</p>
      <button class="btn btn-sm btn-outline" @click="loadModels">重试</button>
    </div>

    <div v-else-if="!models.length" class="chat-empty">
      <p>尚未配置任何 LLM 模型。</p>
      <p class="hint">请在「配置 → LLM 模型池」中添加模型后刷新本页。</p>
    </div>

    <div v-else ref="listEl" class="chat-messages">
      <div v-if="!items.length" class="chat-empty">
        <p>开始提问吧，例如「帮我找几篇关于钠离子电池的文献并总结方法差异」。</p>
        <p class="hint">
          开启「文献库工具」后，模型会通过本系统 MCP 工具检索文献库（只读），并标注来源 ID。
        </p>
      </div>

      <div v-for="(msg, idx) in items" :key="idx" class="msg" :class="`msg-${msg.role}`">
        <div class="msg-role">{{ msg.role === 'user' ? '我' : 'AI' }}</div>
        <div class="msg-body">
          <pre v-if="msg.role === 'user'" class="msg-plain">{{ msg.content }}</pre>
          <template v-else>
            <div v-for="(note, ni) in msg.notes" :key="`n${ni}`" class="msg-note">{{ note }}</div>
            <details v-if="msg.steps.length" class="tool-steps">
              <summary>
                调用工具 {{ msg.steps.length }} 次<template v-if="failedSteps(msg)">（{{ failedSteps(msg) }} 次失败）</template>
                <span v-if="msg.mode" class="tool-mode">· {{ modeLabel(msg.mode) }}</span>
              </summary>
              <div v-for="(step, si) in msg.steps" :key="si" class="tool-step">
                <div class="tool-step-head">
                  <span class="tool-name">{{ step.name }}</span>
                  <span class="tool-args">{{ formatArgs(step.arguments) }}</span>
                  <span v-if="step.is_error" class="tool-error">失败</span>
                </div>
                <pre class="tool-preview">{{ step.preview }}</pre>
              </div>
            </details>
            <MarkdownViewer
              :content="msg.content"
              :lazy-math="false"
              :load-images="false"
            />
          </template>
        </div>
      </div>

      <div v-if="loading" class="msg msg-assistant">
        <div class="msg-role">AI</div>
        <div class="msg-body">
          <span class="typing">{{ toolsEnabled && tools.length ? '检索/生成中…' : '生成中…' }}</span>
          <button class="btn btn-sm btn-outline-danger" @click="abort">停止</button>
        </div>
      </div>
    </div>

    <div class="chat-input">
      <textarea
        v-model="draft"
        rows="3"
        placeholder="输入问题，Enter 发送，Shift+Enter 换行"
        :disabled="loading || !models.length"
        @keydown.enter.exact.prevent="send"
      ></textarea>
      <button
        class="btn chat-send"
        :disabled="loading || !draft.trim() || !models.length"
        @click="send"
      >
        {{ loading ? '生成中…' : '发送' }}
      </button>
    </div>
  </div>
</template>

<script setup lang="ts">
import { computed, nextTick, onMounted, ref, watch } from 'vue'
import MarkdownViewer from '@/components/MarkdownViewer.vue'
import {
  createChatSession,
  deleteChatSession,
  getChatSession,
  listChatModels,
  listChatSessions,
  listChatTools,
  sendChat,
  updateChatSession
} from '@/api/chat'
import type {
  ChatDisplayItem,
  ChatMessage,
  ChatModelInfo,
  ChatSessionMeta,
  ChatToolInfo,
  ChatToolMode
} from '@/types/chat'
import { useToast } from '@/composables/useToast'
import { useTokenStore } from '@/stores/token'

const { showError, showInfo, showSuccess } = useToast()
const tokenStore = useTokenStore()

const MODELS_KEY = 'okb_chat_model'
const HISTORY_KEY = 'okb_chat_messages'
const TRANSCRIPT_KEY = 'okb_chat_history'
const TOOLS_KEY = 'okb_chat_tools'
const TOOL_MODE_KEY = 'okb_chat_tool_mode'
const SESSION_KEY = 'okb_chat_session'
/** 本地保留的最大展示条数 */
const MAX_ITEMS = 50
/** 发给后端的最大 transcript 消息数（工具调用会额外占用条数） */
const MAX_TRANSCRIPT = 60

/** 展示用条目（文本 + 本轮工具调用记录） */
type DisplayItem = ChatDisplayItem

const models = ref<ChatModelInfo[]>([])
const tools = ref<ChatToolInfo[]>([])
const selectedModel = ref('')
const toolsEnabled = ref(localStorage.getItem(TOOLS_KEY) !== '0')
const toolMode = ref<ChatToolMode | ''>((localStorage.getItem(TOOL_MODE_KEY) as ChatToolMode) || '')
const items = ref<DisplayItem[]>([])
/** 发给后端的 transcript：包含 assistant.tool_calls 与 tool 结果，用于跨轮续接上下文 */
const transcript = ref<ChatMessage[]>([])
const draft = ref('')
const systemPrompt = ref('')
const loading = ref(false)
const showSystem = ref(false)
const loadError = ref('')
const listEl = ref<HTMLElement | null>(null)

// 会话历史（显式保存）
const sessionId = ref('')
const sessionTitle = ref('')
const dirty = ref(false)
const saving = ref(false)
const historyOpen = ref(false)
const historyLoading = ref(false)
const sessions = ref<ChatSessionMeta[]>([])

let controller: AbortController | null = null

const toolsTitle = computed(() =>
  tools.value.map(t => `${t.name}：${t.description}`).join('\n')
)

// ── 模型 / 工具列表 ─────────────────────────────────────

async function loadModels() {
  loadError.value = ''
  try {
    const res = await listChatModels()
    models.value = res.models
    const saved = localStorage.getItem(MODELS_KEY)
    if (saved && models.value.some(m => m.name === saved)) {
      selectedModel.value = saved
    } else {
      selectedModel.value = res.default || models.value[0]?.name || ''
    }
  } catch (e) {
    const err = e as { response?: { data?: { detail?: string }; status?: number }; message?: string }
    models.value = []
    loadError.value = err.response?.data?.detail || err.message || '请求失败'
  }
}

async function loadTools() {
  try {
    const res = await listChatTools()
    tools.value = res.tools
  } catch {
    // 工具清单仅用于展示（数量/悬浮说明），失败不影响对话
    tools.value = []
  }
}

watch(selectedModel, v => {
  if (v) localStorage.setItem(MODELS_KEY, v)
})

watch(toolsEnabled, v => {
  localStorage.setItem(TOOLS_KEY, v ? '1' : '0')
})

watch(toolMode, v => {
  localStorage.setItem(TOOL_MODE_KEY, v)
})

// 进入页面时可能尚未配置 token（首次加载必然 401），设置 token 后重新拉取
watch(
  () => tokenStore.token,
  (t, prev) => {
    if (t && t !== prev) {
      void loadModels()
      void loadTools()
    }
  }
)

// ── 本地持久化 ──────────────────────────────────────────

function restoreHistory() {
  try {
    const raw = localStorage.getItem(HISTORY_KEY)
    if (raw) {
      const parsed = JSON.parse(raw) as DisplayItem[]
      if (Array.isArray(parsed)) {
        items.value = parsed.filter(
          m => (m.role === 'user' || m.role === 'assistant') && typeof m.content === 'string'
        )
      }
    }
    const rawTranscript = localStorage.getItem(TRANSCRIPT_KEY)
    if (rawTranscript) {
      const parsed = JSON.parse(rawTranscript) as ChatMessage[]
      if (Array.isArray(parsed)) transcript.value = parsed
    }
  } catch {
    localStorage.removeItem(HISTORY_KEY)
    localStorage.removeItem(TRANSCRIPT_KEY)
  }
}

function persistHistory() {
  try {
    localStorage.setItem(HISTORY_KEY, JSON.stringify(items.value.slice(-MAX_ITEMS)))
    localStorage.setItem(TRANSCRIPT_KEY, JSON.stringify(trimTranscript(transcript.value)))
  } catch {
    // 超出配额时忽略（仅影响本地持久化）
  }
}

/**
 * 裁剪 transcript：条数上限 + 去掉开头的孤儿 tool 消息
 * （tool 结果消息必须紧跟其 assistant.tool_calls，否则部分 API 会报错）
 */
function trimTranscript(msgs: ChatMessage[]): ChatMessage[] {
  let cut = msgs.slice(-MAX_TRANSCRIPT)
  while (cut.length && cut[0].role === 'tool') cut = cut.slice(1)
  return cut
}

function clearChat() {
  items.value = []
  transcript.value = []
  sessionId.value = ''
  sessionTitle.value = ''
  dirty.value = false
  persistSession()
  localStorage.removeItem(HISTORY_KEY)
  localStorage.removeItem(TRANSCRIPT_KEY)
}

// ── 会话历史（显式保存到后端文件）──────────────────────

function restoreSession() {
  try {
    const raw = localStorage.getItem(SESSION_KEY)
    if (!raw) return
    const parsed = JSON.parse(raw) as { id?: string; title?: string }
    sessionId.value = parsed.id || ''
    sessionTitle.value = parsed.title || ''
  } catch {
    localStorage.removeItem(SESSION_KEY)
  }
}

function persistSession() {
  if (sessionId.value) {
    localStorage.setItem(
      SESSION_KEY,
      JSON.stringify({ id: sessionId.value, title: sessionTitle.value })
    )
  } else {
    localStorage.removeItem(SESSION_KEY)
  }
}

function roleLabel(role: string): string {
  const labels: Record<string, string> = {
    admin: '管理员',
    'view-only': '只读',
    'view-upload': '查看+上传'
  }
  return labels[role] || role || '未知'
}

function formatTime(iso: string): string {
  if (!iso) return ''
  const d = new Date(iso.replace(' ', 'T') + (iso.includes('Z') ? '' : 'Z'))
  if (Number.isNaN(d.getTime())) return iso
  return d.toLocaleString()
}

async function loadSessions() {
  historyLoading.value = true
  try {
    const res = await listChatSessions()
    sessions.value = res.sessions
  } catch (e) {
    const err = e as { response?: { data?: { detail?: string } }; message?: string }
    showError(err.response?.data?.detail || err.message || '加载会话列表失败')
    sessions.value = []
  } finally {
    historyLoading.value = false
  }
}

async function toggleHistory() {
  historyOpen.value = !historyOpen.value
  if (historyOpen.value) await loadSessions()
}

/** 保存当前会话（首次为新建，之后为覆盖保存） */
async function save() {
  if (!items.value.length || saving.value) return
  saving.value = true
  try {
    const payload = {
      title: sessionTitle.value || undefined,
      model: selectedModel.value || undefined,
      tool_mode: toolMode.value || undefined,
      items: items.value,
      transcript: transcript.value
    }
    const res = sessionId.value
      ? await updateChatSession(sessionId.value, payload)
      : await createChatSession(payload)
    sessionId.value = res.id
    sessionTitle.value = res.title
    dirty.value = false
    persistSession()
    showSuccess(res.created ? `会话已保存（${res.message_count} 条）` : '会话已更新')
    if (historyOpen.value) await loadSessions()
  } catch (e) {
    const err = e as { response?: { data?: { detail?: string } }; message?: string }
    showError(err.response?.data?.detail || err.message || '保存会话失败')
  } finally {
    saving.value = false
  }
}

/** 打开历史会话（覆盖当前对话） */
async function openSession(id: string) {
  if (
    dirty.value &&
    items.value.length &&
    !confirm('当前对话有未保存的内容，确定打开历史会话并覆盖吗？')
  ) {
    return
  }
  try {
    const detail = await getChatSession(id)
    items.value = Array.isArray(detail.items) ? detail.items : []
    transcript.value = Array.isArray(detail.transcript) ? detail.transcript : []
    sessionId.value = detail.id
    sessionTitle.value = detail.title
    dirty.value = false
    persistSession()
    persistHistory()
    historyOpen.value = false
    await scrollToBottom()
    showInfo(`已打开会话：${detail.title}`)
  } catch (e) {
    const err = e as { response?: { data?: { detail?: string }; status?: number }; message?: string }
    showError(
      err.response?.status === 403
        ? '无权访问该会话'
        : err.response?.data?.detail || err.message || '打开会话失败'
    )
  }
}

async function removeSession(s: ChatSessionMeta) {
  if (!confirm(`确定删除会话「${s.title}」？此操作不可恢复。`)) return
  try {
    await deleteChatSession(s.id)
    if (sessionId.value === s.id) {
      // 只解除绑定，保留当前界面上的对话内容（标记为未保存）
      sessionId.value = ''
      sessionTitle.value = ''
      persistSession()
    }
    await loadSessions()
    showInfo('会话已删除')
  } catch (e) {
    const err = e as { response?: { data?: { detail?: string } }; message?: string }
    showError(err.response?.data?.detail || err.message || '删除会话失败')
  }
}

// ── 展示辅助 ────────────────────────────────────────────

function failedSteps(msg: DisplayItem): number {
  return msg.steps.filter(s => s.is_error).length
}

function modeLabel(mode: string): string {
  return mode === 'prompt' ? '提示词 JSON 协议' : '原生 tools'
}

function formatArgs(args: Record<string, unknown>): string {
  const entries = Object.entries(args || {}).filter(([, v]) => v !== '' && v !== null)
  if (!entries.length) return ''
  return entries
    .slice(0, 4)
    .map(([k, v]) => `${k}=${typeof v === 'string' ? v : JSON.stringify(v)}`)
    .join(' ')
}

// ── 发送 ────────────────────────────────────────────────

async function scrollToBottom() {
  await nextTick()
  if (listEl.value) listEl.value.scrollTop = listEl.value.scrollHeight
}

async function send() {
  const content = draft.value.trim()
  if (!content || loading.value) return

  items.value.push({ role: 'user', content, steps: [] })
  transcript.value.push({ role: 'user', content })
  draft.value = ''
  dirty.value = true
  loading.value = true
  await scrollToBottom()

  controller = new AbortController()
  try {
    const res = await sendChat(
      {
        model: selectedModel.value || undefined,
        messages: trimTranscript(transcript.value),
        system: systemPrompt.value.trim() || undefined,
        tools: toolsEnabled.value,
        tool_mode: toolsEnabled.value && toolMode.value ? toolMode.value : undefined
      },
      controller.signal
    )
    transcript.value.push(...(res.delta || []))
    transcript.value = trimTranscript(transcript.value)
    items.value.push({
      role: 'assistant',
      content: res.content,
      steps: res.steps || [],
      mode: res.tool_mode,
      notes: res.notes || []
    })
    if (items.value.length > MAX_ITEMS) items.value = items.value.slice(-MAX_ITEMS)
    dirty.value = true
    persistHistory()
  } catch (e) {
    const err = e as { code?: string; response?: { data?: { detail?: string } }; message?: string }
    if (err.code === 'ERR_CANCELED') {
      showInfo('已停止生成')
    } else {
      showError(err.response?.data?.detail || err.message || '请求失败')
    }
  } finally {
    controller = null
    loading.value = false
    await scrollToBottom()
  }
}

function abort() {
  controller?.abort()
}

onMounted(async () => {
  restoreHistory()
  restoreSession()
  await loadModels()
  await loadTools()
  await scrollToBottom()
})
</script>

<style scoped>
.chat-view {
  display: flex;
  flex-direction: column;
  /* 视口高度 - 头部 56px - 主区上下内边距 - 页脚 */
  height: calc(100vh - 200px);
  min-height: 420px;
}

.chat-header {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
  margin-bottom: 12px;
}

.chat-header h2 {
  margin: 0;
  font-size: 20px;
}

.chat-controls {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: 8px;
}

.control-item {
  display: flex;
  align-items: center;
  gap: 6px;
  font-size: 13px;
  color: var(--text-secondary);
}

.control-item select {
  max-width: 280px;
  min-height: 32px;
  padding: 4px 8px;
  border: 1px solid var(--border);
  border-radius: 4px;
  background: var(--bg-white);
  color: var(--text);
}

.control-item.switch {
  cursor: pointer;
  user-select: none;
}

.control-item.switch input {
  width: 16px;
  height: 16px;
  cursor: pointer;
}

.system-editor {
  margin-bottom: 12px;
}

/* ── 会话保存状态 + 历史面板 ── */
.session-bar {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: 8px;
  margin-bottom: 10px;
  font-size: 12px;
}

.session-chip {
  padding: 2px 8px;
  border-radius: 10px;
  background: var(--bg-muted);
  color: var(--text-secondary);
}

.session-chip.saved {
  background: var(--success-bg);
  color: var(--success-text);
}

.session-dirty {
  color: var(--warning-text);
}

.history-panel {
  margin-bottom: 12px;
  border: 1px solid var(--border);
  border-radius: 6px;
  background: var(--bg-white);
  padding: 8px 10px;
}

.history-head {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: 8px;
  margin-bottom: 8px;
}

.history-head-title {
  font-weight: 600;
  font-size: 13px;
}

.history-hint {
  flex: 1;
  color: var(--text-muted);
  font-size: 12px;
}

.history-empty {
  padding: 12px;
  text-align: center;
  color: var(--text-muted);
  font-size: 13px;
}

.history-list {
  list-style: none;
  margin: 0;
  padding: 0;
  max-height: 240px;
  overflow-y: auto;
}

.history-item {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 6px 4px;
  border-top: 1px solid var(--border);
}

.history-item.current {
  background: var(--primary-light);
}

.history-main {
  flex: 1;
  min-width: 0;
  display: flex;
  flex-direction: column;
  gap: 2px;
  border: none;
  background: none;
  padding: 4px;
  text-align: left;
  cursor: pointer;
  color: inherit;
  font: inherit;
}

.history-title {
  font-size: 13px;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.history-meta {
  display: flex;
  flex-wrap: wrap;
  gap: 8px;
  font-size: 11px;
  color: var(--text-muted);
}

.history-badge {
  padding: 0 6px;
  border-radius: 8px;
  background: var(--bg-muted);
  color: var(--text-secondary);
}

.history-badge.role-admin {
  background: var(--danger-bg);
  color: var(--danger-text);
}

.history-badge.role-view-upload {
  background: var(--warning-bg);
  color: var(--warning-text);
}

.history-other {
  color: var(--warning-text);
}

.system-editor textarea {
  width: 100%;
  padding: 8px;
  border: 1px solid var(--border);
  border-radius: 4px;
  font-size: 13px;
  font-family: inherit;
  resize: vertical;
}

.chat-messages {
  flex: 1;
  overflow-y: auto;
  padding: 12px;
  border: 1px solid var(--border);
  border-radius: 6px;
  background: var(--bg-white);
}

.chat-empty {
  padding: 24px 12px;
  text-align: center;
  color: var(--text-muted);
}

.chat-empty .hint {
  font-size: 13px;
}

.msg {
  display: flex;
  gap: 10px;
  margin-bottom: 16px;
}

.msg-role {
  flex: 0 0 32px;
  width: 32px;
  height: 32px;
  border-radius: 50%;
  display: flex;
  align-items: center;
  justify-content: center;
  font-size: 12px;
  font-weight: 600;
  color: #fff;
  background: var(--gray);
}

.msg-user .msg-role {
  background: var(--primary);
}

.msg-body {
  flex: 1;
  min-width: 0;
  padding: 8px 12px;
  border-radius: 6px;
  background: var(--bg);
  overflow-x: auto;
}

.msg-user .msg-body {
  background: var(--primary-light);
}

.msg-plain {
  margin: 0;
  white-space: pre-wrap;
  word-break: break-word;
  font-family: inherit;
  font-size: 14px;
  color: var(--text);
}

/* ── 工具调用展示 ── */
.tool-steps {
  margin: 0 0 10px;
  border: 1px dashed var(--border);
  border-radius: 4px;
  background: var(--bg-white);
  font-size: 12px;
}

.tool-steps > summary {
  padding: 6px 8px;
  cursor: pointer;
  color: var(--text-secondary);
}

.tool-step {
  padding: 6px 8px;
  border-top: 1px solid var(--border);
}

.tool-step-head {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: 6px;
}

.tool-name {
  font-weight: 600;
  color: var(--primary);
}

.tool-args {
  color: var(--text-muted);
  word-break: break-all;
}

.tool-error {
  color: var(--danger-text);
  background: var(--danger-bg);
  border-radius: 3px;
  padding: 0 4px;
}

.tool-mode {
  color: var(--text-muted);
}

.msg-note {
  margin: 0 0 8px;
  padding: 4px 8px;
  border-left: 3px solid var(--warning);
  background: var(--warning-bg);
  color: var(--warning-text);
  font-size: 12px;
  border-radius: 3px;
}

.tool-preview {
  margin: 4px 0 0;
  max-height: 160px;
  overflow: auto;
  white-space: pre-wrap;
  word-break: break-word;
  color: var(--text-secondary);
  font-size: 12px;
}

.typing {
  color: var(--text-secondary);
  font-size: 14px;
  margin-right: 8px;
}

.chat-input {
  display: flex;
  gap: 8px;
  margin-top: 12px;
}

.chat-input textarea {
  flex: 1;
  padding: 8px;
  border: 1px solid var(--border);
  border-radius: 4px;
  font-size: 14px;
  font-family: inherit;
  resize: vertical;
}

.chat-send {
  align-self: stretch;
  min-width: 80px;
}

@media (max-width: 768px) {
  .chat-view {
    height: calc(100vh - 180px);
  }

  .chat-header {
    flex-direction: column;
    align-items: flex-start;
  }

  .control-item {
    width: 100%;
  }

  .control-item select {
    flex: 1;
    max-width: none;
  }

  .msg-role {
    flex-basis: 26px;
    width: 26px;
    height: 26px;
    font-size: 11px;
  }

  .chat-input {
    flex-direction: column;
  }

  .chat-send {
    min-height: var(--touch-min);
  }
}
</style>
