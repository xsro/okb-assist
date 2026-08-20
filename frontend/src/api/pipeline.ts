import { apiGet, apiPost } from './client'
import type {
  PipelineStatus,
  ActiveTask,
  BatchInfo,
  DocumentStatusInfo
} from '@/types/pipeline'

// ── 流水线状态 ──────────────────────────────────────────

/** 获取流水线状态 */
export function getPipelineStatus() {
  return apiGet<PipelineStatus>('/assist/api/pipeline/queue/status')
}

/** 获取活跃任务 */
export function getActiveTasks() {
  return apiGet<{ tasks: ActiveTask[]; count: number }>(
    '/assist/api/pipeline/tasks/active'
  )
}

// ── 单文档操作 ──────────────────────────────────────────

/** 启动单文档处理 */
export function processDocument(id: number, stages?: string[]) {
  return apiPost<{ message: string }>(
    `/assist/api/pipeline/documents/${id}/process`,
    { stages }
  )
}

/** 解析文档 */
export function parseDocument(id: number) {
  return apiPost<{ message: string }>(
    `/assist/api/pipeline/documents/${id}/parse`
  )
}

/** 提取元数据 */
export function extractDocument(id: number) {
  return apiPost<{ message: string }>(
    `/assist/api/pipeline/documents/${id}/extract`
  )
}

/** 建立索引 */
export function indexDocument(id: number) {
  return apiPost<{ message: string }>(
    `/assist/api/pipeline/documents/${id}/index`
  )
}

/** 重置文档状态 */
export function resetDocument(id: number) {
  return apiPost<{ message: string }>(
    `/assist/api/pipeline/documents/${id}/reset`
  )
}

/** 获取文档状态 */
export function getDocumentStatus(id: number) {
  return apiGet<DocumentStatusInfo>(
    `/assist/api/pipeline/documents/${id}/status`
  )
}

// ── 批量操作 ────────────────────────────────────────────

/** 批量启动所有待处理文档 */
export function startBatch() {
  return apiPost<{ detail: string; pending: number }>(
    '/assist/api/pipeline/batch/start'
  )
}

/** 批量暂停 */
export function pauseBatch(batchId: string) {
  return apiPost<{ message: string }>(
    `/assist/api/pipeline/batch/${batchId}/pause`
  )
}

/** 批量恢复 */
export function resumeBatch(batchId: string) {
  return apiPost<{ message: string }>(
    `/assist/api/pipeline/batch/${batchId}/resume`
  )
}

/** 批量重置错误 */
export function resetBatchErrors(batchId: string) {
  return apiPost<{ message: string }>(
    `/assist/api/pipeline/batch/${batchId}/reset-errors`
  )
}

/** 批量重置超时 */
export function resetBatchTimeouts(batchId: string) {
  return apiPost<{ message: string }>(
    `/assist/api/pipeline/batch/${batchId}/reset-timeout-errors`
  )
}

/** 推进就绪批次 */
export function promoteReadyBatches() {
  return apiPost<{ message: string }>(
    '/assist/api/pipeline/batch/promote-ready'
  )
}

/** 获取批次状态 */
export function getBatchStatus(batchId: string) {
  return apiGet<BatchInfo>(`/assist/api/pipeline/batch/${batchId}/status`)
}