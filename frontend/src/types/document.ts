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
  /** 派生状态：由文件事实（size 列）+ 运行时推导，非数据库字段 */
  status: DocStatus
  status_message: string | null
  /** 文件事实是否已从磁盘同步过（false = 状态未知） */
  synced: boolean
  fs_synced_at?: string | null
  has_pdf: boolean
  has_md: boolean
  has_zip: boolean
  index_status: IndexStatus
  indexed_dbs?: string[]
  filename: string
  file_hash: string | null
  file_size: number | null
  /** 文件大小：来自「从磁盘同步」的列，可能滞后（详情页会实时校正） */
  pdf_size: number | null
  md_size: number | null
  zip_size: number | null
  /** PDF 健康检查结果（`ok` / `warning` / `error` / `missing`，未检查时为 null/undefined） */
  pdf_health?: string | null
  /** 健康检查原因 */
  pdf_health_message?: string | null
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
  doc_type_filter?: string
  sort_by?: 'created_at' | 'title' | 'year' | 'id' | 'authors' | 'updated_at' | 'pdf_size' | 'md_size' | 'zip_size'
  sort_order?: 'asc' | 'desc'
  page?: number
  page_size?: number
  /** 文件事实三态：present / absent / unknown（逗号多选） */
  md_state?: string
  pdf_state?: string
  zip_state?: string
  /** 1 = 已同步文件事实，0 = 未同步 */
  synced?: 0 | 1
  has_md?: 0 | 1
  has_pdf?: 0 | 1
  has_zip?: 0 | 1
  pdf_size_min?: number
  pdf_size_max?: number
  md_size_min?: number
  md_size_max?: number
  zip_size_min?: number
  zip_size_max?: number
  /** ok / warning / error / missing / unchecked（逗号多选） */
  pdf_health?: string
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

// ── BibTeX 批量新建条目 ──────────────────────────────────

/** 单条 BibTeX 的处理结果 */
export interface BibtexImportItem {
  /** created 新建 / create 可新建（预览）/ exists 已存在 / duplicate 文件内重复 / invalid 无效 / error 失败 */
  action: 'created' | 'create' | 'exists' | 'duplicate' | 'invalid' | 'error'
  /** BibTeX 引用键 */
  key: string
  title: string
  year: number | null
  doi: string
  /** 命中已有文档时的文档 ID */
  existing_id?: number
  /** 新建成功时的文档 ID */
  id?: number
  /** 命中依据：doi / title / in-file-doi / in-file-title / no-title */
  reason?: string
  /** 可读说明 */
  detail?: string
}

/** BibTeX 批量导入响应（后端 /documents/import-bibtex） */
export interface BibtexImportResponse {
  dry_run: boolean
  /** 文件里解析到的条目数 */
  total: number
  /** 实际处理的条目数（受 entry_limit 截断） */
  processed: number
  truncated: boolean
  entry_limit: number
  /** 库中缺失的条目数（预览时即“可新建”） */
  missing: number
  created_count: number
  existing_count: number
  duplicate_count: number
  invalid_count: number
  failed_count: number
  created: { id: number; key: string; title: string; doi: string; year: number | null }[]
  items: BibtexImportItem[]
  items_truncated: boolean
  warnings: string[]
  skipped_entries: number
  detail: string
}
