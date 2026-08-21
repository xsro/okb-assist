// ── 流水线相关类型 ────────────────────────────────────────

export interface PipelineQueue {
  max_concurrent_tasks: number
  running_tasks: number
  available_slots: number
}

export interface ActiveTask {
  doc_id: number
  doc_title: string
  task_type: string
  started_at: string
  status_message: string
  status: string
}

export interface BatchInfo {
  batch_id: string
  total: number
  completed: number
  failed: number
  status: 'running' | 'paused' | 'completed' | 'error'
}

export interface DocumentIndexInfo {
  vector_db_id: string
  collection_name: string
  status: string
  error_message: string | null
  updated_at: string | null
}

export interface BatchProgress {
  active: boolean
  stage: string
  vector_db_id: string
  total: number
  processed: number
  current_batch: number
  total_batches: number
  batch_size: number
  pause_seconds: number
  started_at: string | null
  errors: number
}

export interface PipelineStatus {
  max_concurrent_tasks: number
  running_tasks: number
  available_slots: number
  batch_progress?: BatchProgress
}

export interface DocumentStatusInfo {
  id: number
  status: string
  index_status: string
  error_message: string | null
}
