import { apiGet, apiPost } from './client'
import type { ServiceStatus } from '@/types/config'

// ── 系统统计 ────────────────────────────────────────────

export interface SystemStats {
  total_documents: number
  indexed_count: number
  error_count: number
  total_size: number
  pdf_size: number
  md_size: number
  zip_size: number
  status_counts: Record<string, number>
}

/** 获取统计信息 */
export function getStats() {
  return apiGet<SystemStats>('/assist/api/admin/stats')
}

/** 获取服务状态 */
export function getServiceStatus() {
  return apiGet<ServiceStatus>('/assist/api/admin/services/status')
}

// ── 维护操作 ────────────────────────────────────────────

/** 维护任务选项：`onlyMissing` = 只处理对应字段缺失（NULL/空）的文档 */
export interface MaintenanceOptions {
  onlyMissing?: boolean
}

function maintenancePath(path: string, opts: MaintenanceOptions = {}) {
  return opts.onlyMissing ? `${path}?only_missing=1` : path
}

/** 重算哈希（后台任务，进度见「活跃任务」）；`onlyMissing` = 只补没有 file_hash 的文档 */
export function recalculateHashes(opts: MaintenanceOptions = {}) {
  return apiPost<{ detail: string; started: boolean }>(
    maintenancePath('/assist/api/admin/recalculate-hashes', opts)
  )
}

/** PDF 健康检查（后台任务，进度见「活跃任务」）；`onlyMissing` = 只检查从未检查过的文档 */
export function checkPdfHealth(opts: MaintenanceOptions = {}) {
  return apiPost<{ detail: string; started: boolean }>(
    maintenancePath('/assist/api/admin/check-pdf-health', opts)
  )
}

/** 单篇 PDF 健康检查（同步返回，结果同时写回数据库） */
export interface PdfHealthResult {
  id: number
  /** ok / warning / error / missing */
  status: string
  message: string
  page_count: number | null
  /** 实际使用的解析引擎：mutool / mupdf */
  engine: string
  pdf_path: string
}

export function checkDocumentPdfHealth(id: number) {
  return apiPost<PdfHealthResult>(
    `/assist/api/admin/documents/${id}/check-pdf-health`
  )
}

/** 文献去重（按文件哈希合并重复文档） */
export function deduplicateDocuments() {
  return apiPost<{ message: string; merged_groups: number; deleted_docs: number }>(
    '/assist/api/admin/dedup'
  )
}

/** 同步文件信息到数据库（PDF/MD/ZIP 文件大小）；`onlyMissing` = 只补有空缺的文档 */
export function syncFileSizes(opts: MaintenanceOptions = {}) {
  return apiPost<{ detail: string }>(
    maintenancePath('/assist/api/admin/sync-file-sizes', opts)
  )
}

/** 获取 MinerU 任务列表 */
export function getMinerUTasks() {
  return apiGet<{ tasks: unknown[] }>('/assist/api/admin/mineru/tasks')
}

/** 获取 Qdrant 集合信息 */
export function getQdrantCollections() {
  return apiGet<{ collections: unknown[] }>(
    '/assist/api/admin/qdrant/collections'
  )
}

// ── 日志查看 ────────────────────────────────────────────

export interface LogEntry {
  message: string
  level: string
  timestamp: string
  target: string
  file?: string
  line?: number
  [key: string]: unknown
}

export interface LogResponse {
  entries: LogEntry[]
  total: number
  file: string
  displayed: number
  error?: string
  /** 本次从文件尾实际回扫的字节数 */
  scanned_bytes?: number
  /** true = 回扫字节预算用尽，文件开头还有更早的日志未加载 */
  scan_truncated?: boolean
}

/** 获取日志 */
export function getLogs(params: {
  lines?: number
  level?: string
  q?: string
}) {
  return apiGet<LogResponse>('/assist/api/admin/logs', params)
}
