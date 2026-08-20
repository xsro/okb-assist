// ── 配置相关类型 ────────────────────────────────────────

export interface VectorDbConfig {
  id: string
  name?: string
  type: 'qdrant' | 'milvus' | 'chroma'
  enabled: boolean
  url: string
  collection: string
  api_key?: string
  embedding: {
    source: string
    model: string
  }
}

export interface ServiceConfig {
  mineru: {
    url: string
    key: string
    task_timeout: number
  }
  ollama: {
    url: string
    key: string
    model: string
  }
  vector_dbs: VectorDbConfig[]
}

export interface SystemConfig {
  token: string
  mcp_token: string
  database_url: string
  upload_dir: string
}

export interface ServiceStatusItem {
  status: string
  url?: string
  error?: string
  [key: string]: any
}

export interface ServiceStatus {
  mineru: ServiceStatusItem
  ollama: ServiceStatusItem
  fastembed: ServiceStatusItem
  vector_dbs: ServiceStatusItem[]
  qdrant: ServiceStatusItem
}

export interface ConnectionTestResult {
  status: string
  detail: string
  [key: string]: any
}
