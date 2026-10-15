// ── 简易问答（Chat）相关类型 ────────────────────────────

/** 对话消息角色 */
export type ChatRole = 'system' | 'user' | 'assistant' | 'tool'

/** 模型发起的一次工具调用（arguments 已由服务端展开为对象） */
export interface ToolCall {
  id?: string
  name: string
  /** 调用参数（OpenAI 协议下服务端会把 JSON 字符串解析为对象） */
  arguments: Record<string, unknown>
}

/**
 * 发给模型的消息。
 * assistant 消息可携带 tool_calls；tool 角色消息回传结果（含 tool_call_id/tool_name）。
 */
export interface ChatMessage {
  role: ChatRole
  content: string
  tool_calls?: ToolCall[]
  tool_call_id?: string | null
  tool_name?: string | null
}

/** 模型池中可用于对话的模型（不含 key 等敏感字段） */
export interface ChatModelInfo {
  name: string
  protocol: string
  model: string
  /** 该模型配置的工具调用方式：auto / native / prompt */
  tool_mode?: string
}

export interface ChatModelsResponse {
  models: ChatModelInfo[]
  count: number
  /** 默认模型名（模型池第一项） */
  default: string
}

/** 可被模型调用的 MCP 工具 */
export interface ChatToolInfo {
  name: string
  description: string
}

export interface ChatToolsResponse {
  count: number
  tools: ChatToolInfo[]
}

/** 一次工具调用的执行记录（供前端展示） */
export interface ToolStep {
  /** 第几轮工具调用（从 1 开始） */
  round: number
  name: string
  arguments: Record<string, unknown>
  is_error: boolean
  /** 结果预览（完整结果在 delta 的 tool 消息里） */
  preview: string
  chars: number
}

/** 工具调用方式 */
export type ChatToolMode = 'auto' | 'native' | 'prompt'

export interface ChatRequest {
  /** 模型池中的模型名，留空取第一项 */
  model?: string
  messages: ChatMessage[]
  /** 覆盖默认系统提示词 */
  system?: string
  temperature?: number
  max_tokens?: number
  /** 是否允许模型调用本系统 MCP 工具（默认 true） */
  tools?: boolean
  /** 工具调用方式覆盖（默认取模型配置，再回退 auto） */
  tool_mode?: ChatToolMode
  max_tool_rounds?: number
}

export interface ChatResponse {
  content: string
  /** 本轮的工具调用记录 */
  steps: ToolStep[]
  /** 本轮新增的消息（含工具调用/结果），追加到本地 transcript 即可续接上下文 */
  delta: ChatMessage[]
  /** 流程说明（如自动降级为提示词协议） */
  notes: string[]
  /** 实际使用的模型池条目名 */
  name: string
  protocol: string
  model: string
  /** 实际使用的工具协议：native / prompt */
  tool_mode: string
  /** 实际执行的工具轮数 */
  tool_rounds: number
}

// ── 会话历史（显式保存，一个会话一个后端文件）──────────────

/** 展示用条目（本地渲染结构，原样存入会话文件） */
export interface ChatDisplayItem {
  role: 'user' | 'assistant'
  content: string
  steps: ToolStep[]
  /** 本轮实际使用的工具协议 */
  mode?: string
  /** 流程说明（如自动降级） */
  notes?: string[]
}

/** 会话元信息（列表用；后端不回传 owner_key，只用 mine 标记） */
export interface ChatSessionMeta {
  id: string
  title: string
  /** 保存时的角色 */
  role: string
  model: string
  tool_mode: string
  message_count: number
  created_at: string
  updated_at: string
  /** 是否属于当前 token（admin 列表里可能有别人的会话） */
  mine: boolean
}

export interface ChatSessionListResponse {
  count: number
  sessions: ChatSessionMeta[]
}

/** 打开会话时返回的完整内容 */
export interface ChatSessionDetail extends ChatSessionMeta {
  items: ChatDisplayItem[]
  transcript: ChatMessage[]
}

export interface ChatSessionSaveRequest {
  title?: string
  model?: string
  tool_mode?: string
  items: ChatDisplayItem[]
  transcript: ChatMessage[]
}

export interface ChatSessionSaveResponse extends ChatSessionMeta {
  /** true = 新建，false = 覆盖保存 */
  created: boolean
}
