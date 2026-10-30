// ── 文档相关类型 ────────────────────────────────────────

export type DocStatus =
  | 'uploaded'
  | 'parsing'
  | 'markdown_done'
  | 'error'

export type IndexStatus = 'not_indexed' | 'indexing' | 'indexed' | 'error'

export interface Document {
  id: number
  title: string
  authors: string | null
  journal: string | null
  year: number | null
  doc_type: string | null
  doi: string | null
  abstract: string | null
  keywords: string | null
  status: DocStatus
  status_message: string | null
  index_status: IndexStatus
  indexed_dbs?: string[]
  filename: string
  file_hash: string | null
  file_size: number | null
  pdf_size: number | null
  md_size: number | null
  zip_size: number | null
  created_at: string
  updated_at: string
  has_markdown: boolean
}

export interface SearchResult {
  id: number
  title: string
  score: number
  snippet: string | null
  /** grep 搜索时返回首个匹配行号 */
  first_line?: number
}

export interface SearchResultList {
  items: Document[]
  total: number
  page: number
  page_size: number
  total_pages: number
}

export interface SearchParams {
  q?: string
  status_filter?: string
  doc_type_filter?: string
  sort_by?: 'created_at' | 'title' | 'year' | 'id' | 'authors' | 'status' | 'updated_at'
  sort_order?: 'asc' | 'desc'
  page?: number
  page_size?: number
}

export interface TocItem {
  level: number
  title: string
  line: number
}

/** Markdown 内容响应（行切片 API） */
export interface MarkdownResponse {
  content: string
  total_length: number
  total_lines: number
  line_start: number
  line_count: number
  lines_returned: number
  /** 文件版本（sha256），用于保存时的乐观并发校验 */
  hash: string
}

/** 按行范围保存 Markdown 的响应 */
export interface MarkdownLinesSaveResult {
  detail: string
  line_start: number
  line_count: number
  lines_written: number
  total_lines: number
  md_size: number | null
  /** 写入后的文件 hash */
  hash: string
}

/** Markdown 区块 */
export interface ChunkInfo {
  id: number
  start_line: number
  end_line: number
  content: string
  heading: string | null
}

/** Chunks API 响应 */
export interface ChunkResponse {
  chunks: ChunkInfo[]
  total_lines: number
  doc_id: number
}

export interface SimilarTitleGroup {
  groups: {
    normalized_title: string
    count: number
    documents: Document[]
  }[]
  total_groups: number
  total_documents: number
}
// ── PDF 元信息 / Crossref / 引用条目 ──────────────────────

/** PDF 元信息报告（后端 /pdf-info） */
export interface PdfInfoReport {
  file_size: number | null
  pdf_version: string | null
  page_count: number | null
  encrypted: boolean | null
  /** PDF Info 字典原始键值 */
  info: Record<string, string>
  /** 启发式抽取结果（标题/作者/DOI/卷期页等） */
  heuristics: Record<string, unknown>
  /** 仅在解析失败时出现 */
  error?: string
}

export interface PdfInfoResponse {
  id: number
  filename: string
  pdf: PdfInfoReport
}

/** Crossref 记录响应（后端 /crossref） */
export interface CrossrefRecordResponse {
  id: number
  exists: boolean
  path: string
  /** 原始 JSON（Crossref API 应答） */
  raw: Record<string, unknown> | null
  /** 解析后的摘要（字段与元数据补全一致） */
  parsed: Record<string, unknown> | null
  /** 记录文件修改时间（≈ 抓取时间） */
  modified_at: string | null
}

export type CitationFormat = 'bibtex' | 'gbt7714'

/** 引用条目响应（后端 /citation） */
export interface CitationResponse {
  id: number
  format: CitationFormat
  /** 数据来源：crossref 记录，或退化为文档元数据 */
  source: 'crossref' | 'document'
  key: string
  content: string
}
