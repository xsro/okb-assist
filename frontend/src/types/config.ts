// ── 配置相关类型 ────────────────────────────────────────

export interface VectorDbConfig {
  id: string
  name?: string
  type: 'qdrant' | 'milvus' | 'chroma'
  enabled: boolean
  url: string
  collection: string
  api_key?: string
  embedding: ModelEndpoint
}

/** LLM / Embedding 协议：ollama 原生协议 或 openai 兼容协议 */
export type LlmProtocol = 'ollama' | 'openai'

/**
 * 单个模型端点（协议 + 地址 + 凭据 + 模型名）。
 * 同时用于 LLM 模型池、抽取专用模型、各向量库的 embedding 段。
 */
export interface ModelEndpoint {
  protocol: LlmProtocol
  url: string
  key: string
  model: string
}

/** LLM 模型池中的一项（预留给后续问答系统） */
export interface LlmModelEntry extends ModelEndpoint {
  name: string
}

/** 信息补全（元数据抽取）使用的模型；字段留空表示从模型池继承 */
export interface LlmExtractConfig extends ModelEndpoint {
  /** 继承来源：模型池中的 name，空 = 第一项 */
  from: string
}

export interface MinerUConfig {
  type: string
  url: string
  token: string
  name: string
  task_timeout: number
  tier?: string
  max_tasks?: number
  /** @deprecated 改用 tier */
  model_version?: string
}

export interface ServiceConfig {
  base_url: string
  active_mineru?: string
  mineru: MinerUConfig[]
  /** LLM 模型池（预留给后续问答系统） */
  llm: { models: LlmModelEntry[] }
  /** 信息补全（元数据抽取）使用的模型 */
  llm_extract: LlmExtractConfig
  vector_dbs: VectorDbConfig[]
}

export interface SystemConfig {
  token: string
  mcp_token: string
  database_url: string
  cwd: string
}

export interface ServiceStatusItem {
  status: string
  url?: string
  error?: string
  [key: string]: any
}

export interface ServiceStatus {
  mineru: ServiceStatusItem | ServiceStatusItem[]
  llm: ServiceStatusItem
  fastembed: ServiceStatusItem
  vector_dbs: ServiceStatusItem[]
  qdrant: ServiceStatusItem
}

export interface ConnectionTestResult {
  status: string
  detail: string
  [key: string]: any
}
