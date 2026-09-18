import { apiGet, apiPost } from './client'
import type { ServiceStatus } from '@/types/config'

// ── 系统统计 ────────────────────────────────────────────

export interface SystemStats {
  total_documents: number
  indexed_count: number
  error_count: number
  total_size: number
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

/** 重新计算哈希 */
export function recalculateHashes() {
  return apiPost<{ message: string }>(
    '/assist/api/admin/recalculate-hashes'
  )
}

/** 文献去重（按文件哈希合并重复文档） */
export function deduplicateDocuments() {
  return apiPost<{ message: string; merged_groups: number; deleted_docs: number }>(
    '/assist/api/admin/dedup'
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