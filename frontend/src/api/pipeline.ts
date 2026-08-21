import { apiGet, apiPost } from './client'
import type {
  PipelineStatus,
  ActiveTask,
  BatchInfo,
  DocumentStatusInfo,
  DocumentIndexInfo,
  BatchProgress
} from '@/types/pipeline'

// ── 流水线状态 ──────────────────────────────────────────

/** 获取流水线状态 */
export function getPipelineStatus() {
  return apiGet<PipelineStatus>('/assist/api/pipeline/queue/status/')
}

/** 获取活跃任务 */
export function getActiveTasks() {
  return apiGet<{ tasks: ActiveTask[]; count: number }>(
    '/assist/api/pipeline/tasks/active/'
  )
}

// ── 单文档操作 ──────────────────────────────────────────

/** 解析文档 */
export function parseDocument(id: number) {
  return apiPost<{ detail: string; status: string }>(
    `/assist/api/pipeline/${id}/parse/`
  )
}

/** 提取元数据 */
export function extractDocument(id: number) {
  return apiPost<{ detail: string; status: string }>(
    `/assist/api/pipeline/${id}/extract/`
  )
}

/** 建立索引 */
export function indexDocument(id: number, vectorDbId?: string) {
  return apiPost<{ detail: string; status: string }>(
    `/assist/api/pipeline/${id}/index/`,
    undefined,
    { params: vectorDbId ? { vector_db_id: vectorDbId } : undefined }
  )
}

/** 获取文档在各数据库的索引状态 */
export function getDocumentIndexes(id: number) {
  return apiGet<{ document_id: number; indexes: DocumentIndexInfo[] }>(
    `/assist/api/pipeline/${id}/indexes/`
  )
}

/** 重置文档状态 */
export function resetDocument(id: number) {
  return apiPost<{ detail: string }>(
    `/assist/api/pipeline/${id}/reset/`
  )
}

/** 获取文档状态 */
export function getDocumentStatus(id: number) {
  return apiGet<DocumentStatusInfo>(
    `/assist/api/pipeline/${id}/status/`
  )
}

/** Crossref 补充元数据 */
export function crossrefDocument(id: number) {
  return apiPost<{ detail: string; status: string }>(
    `/assist/api/pipeline/${id}/crossref/`
  )
}

/** 从 PDF 内嵌元数据提取 */
export function extractPdfMeta(id: number) {
  return apiPost<{ detail: string; status: string }>(
    `/assist/api/pipeline/${id}/extract-pdf-meta/`
  )
}

// ── 批量操作 ────────────────────────────────────────────

/** 批量启动所有待处理文档 */
export function startBatch() {
  return apiPost<{ detail: string; pending: number }>(
    '/assist/api/pipeline/batch/start/'
  )
}

/** 批量暂停 */
export function pauseBatch() {
  return apiPost<{ detail: string }>('/assist/api/pipeline/batch/pause/')
}

/** 批量恢复 */
export function resumeBatch() {
  return apiPost<{ detail: string }>('/assist/api/pipeline/batch/resume/')
}

/** 批量重置错误 */
export function resetBatchErrors(targetStatus?: string) {
  return apiPost<{ detail: string }>(
    '/assist/api/pipeline/batch/reset-errors/',
    undefined,
    { params: targetStatus ? { target_status: targetStatus } : undefined }
  )
}

/** 批量重置超时 */
export function resetBatchTimeouts(targetStatus?: string) {
  return apiPost<{ detail: string }>(
    '/assist/api/pipeline/batch/reset-timeout-errors/',
    undefined,
    { params: targetStatus ? { target_status: targetStatus } : undefined }
  )
}

/** 推进就绪批次 */
export function promoteReadyBatches() {
  return apiPost<{ detail: string }>(
    '/assist/api/pipeline/batch/promote-ready/'
  )
}

/** 获取批次状态 */
export function getBatchStatus() {
  return apiGet<BatchProgress>('/assist/api/pipeline/batch/status/')
}