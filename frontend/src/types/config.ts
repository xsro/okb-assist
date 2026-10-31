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

/** LLM 模型池中的一项（「问答」tab 的可选模型，也是抽取/embedding 的回退来源） */
export interface LlmModelEntry extends ModelEndpoint {
  name: string
  /** 工具调用方式：""/"auto"（默认，先原生后降级）、"native"、"prompt" */
  tool_mode?: string
}

/** 信息补全（元数据抽取）使用的模型；字段留空表示从模型池继承 */
export interface LlmExtractConfig extends ModelEndpoint {
  /** 继承来源：模型池中的 name，空 = 第一项 */
  from: string
  /** 自定义提示词模板（支持 {content} / {metadata}；空 = 内置默认模板） */
  prompt?: string
  /** 送入模型的 Markdown 最大字符数（默认 6000） */
  max_input_chars?: number
  /** 生成的最大 token 数（默认 2048） */
  max_tokens?: number
  /** 是否允许用抽取结果覆盖/清空已有元数据（默认 true） */
  overwrite?: boolean
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
  /** LLM 模型池（「问答」tab 的可选模型） */
  llm: { models: LlmModelEntry[] }
  /** 信息补全（元数据抽取）使用的模型 */
  llm_extract: LlmExtractConfig
  vector_dbs: VectorDbConfig[]
}

/** `system.json` 内容（管理员可见）。凭据已全部迁到 `config.json` 的 `users` 段 */
export interface SystemConfig {
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
