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

// ── 实时日志（短轮询环形缓冲，不读日志文件） ──────────────

export interface LogEntry {
  /** 服务端环形缓冲里的序号（稳定标识，用于渲染 key 与展开状态） */
  seq?: number
  message: string
  level: string
  timestamp: string
  target: string
  file?: string
  line?: number
  [key: string]: unknown
}

/** 可选的日志等级（服务端按「该等级及以上」过滤） */
export type LogLevel = 'trace' | 'debug' | 'info' | 'warn' | 'error'

export const LOG_LEVELS: { value: LogLevel; label: string }[] = [
  { value: 'error', label: 'ERROR 及以上' },
  { value: 'warn', label: 'WARN 及以上' },
  { value: 'info', label: 'INFO 及以上' },
  { value: 'debug', label: 'DEBUG 及以上' },
  { value: 'trace', label: '全部（含 TRACE）' }
]

export interface LogResponse {
  /** 本次返回的日志（时间正序，结构与日志文件里的一行一致） */
  entries: LogEntry[]
  /** entries 的条数 */
  count: number
  /** 满足过滤条件的总条数（大于 count 说明被 limit 截断） */
  matched: number
  /** 下次请求用的游标（服务端「最新序号 + 1」） */
  next_seq: number
  latest_seq: number
  /** 缓冲里最旧记录的序号（缓冲区为空时为 null） */
  oldest_seq: number | null
  /** since 之后被挤出缓冲区的条数（>0 表示中间有日志被丢弃） */
  lost: number
  /** true = 命中数超过 limit，应立刻再拉一次 */
  truncated: boolean
  /** 服务端实际使用的等级阈值 */
  level: LogLevel
  /** 环形缓冲容量 */
  capacity: number
  /** 日志文件路径（仅供提示"历史日志在这里"，服务端不读它） */
  file: string
  server_time?: string
}

/** 拉取日志增量：`since` 为上次的 `next_seq`（不传 = 从缓冲区最早开始） */
export function getLogs(params: {
  since?: number
  level?: LogLevel
  limit?: number
  /** 是否包含「本接口自身的请求日志」（默认服务端隐藏） */
  includeSelf?: boolean
}) {
  return apiGet<LogResponse>('/assist/api/admin/logs', {
    since: params.since,
    level: params.level,
    limit: params.limit,
    include_self: params.includeSelf ? 1 : undefined
  })
}
