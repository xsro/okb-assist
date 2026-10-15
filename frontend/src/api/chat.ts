import { apiDelete, apiGet, apiPost, apiPut } from './client'
import type {
  ChatModelsResponse,
  ChatRequest,
  ChatResponse,
  ChatSessionDetail,
  ChatSessionListResponse,
  ChatSessionSaveRequest,
  ChatSessionSaveResponse,
  ChatToolsResponse
} from '@/types/chat'

// ── 简易问答（Chat）─────────────────────────────────────

/** 可用于对话的模型列表（来自 config.json 的 llm.models 模型池） */
export function listChatModels() {
  return apiGet<ChatModelsResponse>('/assist/api/chat/models')
}

/** 模型可调用的 MCP 工具清单（本系统只读工具） */
export function listChatTools() {
  return apiGet<ChatToolsResponse>('/assist/api/chat/tools')
}

/**
 * 发送对话请求（非流式，一次返回完整回答）。
 * 服务端会在本轮内完成「模型 → MCP 工具 → 模型」的循环。
 * 生成耗时可能远超默认 30s，这里单独放大超时；`signal` 用于中止生成。
 */
export function sendChat(payload: ChatRequest, signal?: AbortSignal) {
  return apiPost<ChatResponse>('/assist/api/chat', payload, { timeout: 300000, signal })
}

// ── 会话历史（历史记录只在前端点击「保存」时写入后端文件）──

/** 会话列表：admin 可看到全部（带 mine 标记），其余角色只看得到自己的 */
export function listChatSessions() {
  return apiGet<ChatSessionListResponse>('/assist/api/chat/sessions')
}

/** 打开某个会话（含展示条目与 transcript） */
export function getChatSession(id: string) {
  return apiGet<ChatSessionDetail>(`/assist/api/chat/sessions/${encodeURIComponent(id)}`)
}

/** 保存为新会话 */
export function createChatSession(payload: ChatSessionSaveRequest) {
  return apiPost<ChatSessionSaveResponse>('/assist/api/chat/sessions', payload)
}

/** 覆盖保存已有会话 */
export function updateChatSession(id: string, payload: ChatSessionSaveRequest) {
  return apiPut<ChatSessionSaveResponse>(
    `/assist/api/chat/sessions/${encodeURIComponent(id)}`,
    payload
  )
}

/** 删除会话（归属人或 admin） */
export function deleteChatSession(id: string) {
  return apiDelete<{ detail: string; id: string }>(
    `/assist/api/chat/sessions/${encodeURIComponent(id)}`
  )
}
