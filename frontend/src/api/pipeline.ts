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
    `/assist/api/pipeline/parse/${id}/`
  )
}

/** 元数据抽取方式 */
export type ExtractMethod = 'llm' | 'heuristic'

/**
 * 提取元数据。
 *
 * - `llm`（默认）：让大模型读 Markdown 校对字段，覆盖全但慢；
 * - `heuristic`：只从 Markdown 的「引用格式 / 引用本文 / DOI」里抽，几毫秒完成，
 *   没有这些线索的文档会被后端跳过（日志里有说明）。
 */
export function extractDocument(id: number, method: ExtractMethod = 'llm') {
  return apiPost<{ detail: string; status: string; method?: string }>(
    `/assist/api/pipeline/extract/${id}/`,
    undefined,
    { params: { method } }
  )
}

/** 批量提取元数据（`method` 含义同 `extractDocument`） */
export function startBatchExtract(method: ExtractMethod = 'llm') {
  return apiPost<{ detail: string; pending: number; method?: string }>(
    '/assist/api/pipeline/batch/start-extract/',
    undefined,
    { params: { method } }
  )
}

/** 建立索引 */
export function indexDocument(id: number, vectorDbId?: string) {
  return apiPost<{ detail: string; status: string }>(
    `/assist/api/pipeline/index/${id}/`,
    undefined,
    { params: vectorDbId ? { vector_db_id: vectorDbId } : undefined }
  )
}

/** 获取文档在各数据库的索引状态 */
export function getDocumentIndexes(id: number) {
  return apiGet<{ document_id: number; indexes: DocumentIndexInfo[] }>(
    `/assist/api/pipeline/indexes/${id}/`
  )
}

/**
 * 清空解析产物（删除 Markdown + 资源包），文档随之回到「待解析」。
 *
 * 状态不再可由客户端设置：文献状态由文件事实推导，想重新解析只能清空产物。
 * 这是破坏性操作，调用方需二次确认。
 */
export function clearParseOutput(id: number) {
  return apiPost<{
    detail: string
    status: string
    removed_markdown: boolean
    removed_asset: boolean
  }>(`/assist/api/pipeline/reset/${id}/`)
}

/** 获取文档状态 */
export function getDocumentStatus(id: number) {
  return apiGet<DocumentStatusInfo>(
    `/assist/api/pipeline/status/${id}/`
  )
}

/** Crossref 补充元数据 */
export function crossrefDocument(id: number) {
  return apiPost<{ detail: string; status: string }>(
    `/assist/api/pipeline/crossref/${id}/`
  )
}

/** 从 PDF 内嵌元数据提取 */
export function extractPdfMeta(id: number) {
  return apiPost<{ detail: string; status: string }>(
    `/assist/api/pipeline/extract-pdf-meta/${id}/`
  )
}

// ── 批量操作 ────────────────────────────────────────────

/** 批量启动解析（只处理「有 PDF 但缺少 Markdown」的文档） */
export function startBatch() {
  return apiPost<{ detail: string; pending: number }>(
    '/assist/api/pipeline/batch/start/'
  )
}

/** 批量解析「有 PDF 但缺少 Markdown」的文档（没有 PDF 的记录会被跳过） */
export function startBatchParse() {
  return apiPost<{ detail: string; pending: number }>(
    '/assist/api/pipeline/batch/start-parse/'
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