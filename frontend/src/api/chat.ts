import { apiGet, apiPost } from './client'
import type {
  ChatModelsResponse,
  ChatRequest,
  ChatResponse,
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
