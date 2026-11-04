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
}

/** PDF 解析引擎：外部 mutool 命令行 或 内置 mupdf 库（需 --features mupdf） */
export type PdfEngineName = 'mutool' | 'mupdf'

/** PDF 解析引擎配置（`config.json` 的 `pdf` 段） */
export interface PdfEngineConfig {
  engine: PdfEngineName
}

/** 当前二进制的引擎能力（`GET /assist/api/config` 额外下发，不可写入） */
export interface PdfCapabilities {
  /** mutool 命令行总是可选（系统未安装时健康检查会报 warning） */
  mutool: boolean
  /** 内置 mupdf 是否编入（`--features mupdf`） */
  mupdf: boolean
  /** 实际生效的引擎（配置为 mupdf 但未编入 feature 时回退为 mutool） */
  active: PdfEngineName
}

export interface ServiceConfig {
  base_url: string
  active_mineru?: string
  mineru: MinerUConfig[]
  /** PDF 解析引擎（mutool 命令行 / 内置 mupdf） */
  pdf?: PdfEngineConfig
  /** 引擎能力（只读，服务端按编译特性下发） */
  pdf_capabilities?: PdfCapabilities
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
