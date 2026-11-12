import client, { apiGet, apiPost, apiPut, apiDelete, apiUpload } from './client'
import type {
  Document,
  PublicDocument,
  SearchResult,
  SearchResultList,
  SimilarTitleGroup,
  SimilarDoiGroup,
  TocItem,
  MarkdownResponse,
  MarkdownLinesSaveResult,
  ChunkResponse,
  PdfInfoResponse,
  CrossrefRecordResponse,
  CitationFormat,
  CitationResponse,
  BibtexImportResponse
} from '@/types/document'

// ── 文档 CRUD ───────────────────────────────────────────

/** 文档列表（分页 + 筛选 + 排序） */
export function listDocuments(params: {
  q?: string
  doc_type_filter?: string
  search_fields?: string
  sort_by?: 'created_at' | 'title' | 'year' | 'id' | 'authors' | 'updated_at' | 'pdf_size' | 'md_size' | 'zip_size'
  sort_order?: 'asc' | 'desc'
  page?: number
  page_size?: number
  year?: number
  year_min?: number
  year_max?: number
  id?: number
  id_min?: number
  id_max?: number
  journal?: string
  authors?: string
  // ── 文件事实筛选（取代原 status_filter）──
  /** present / absent / unknown（逗号多选） */
  md_state?: string
  pdf_state?: string
  zip_state?: string
  /** 1 = 已同步过文件事实，0 = 未同步 */
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
}) {
  return apiGet<SearchResultList>('/assist/api/documents/', params)
}

/** 文档详情 */
export function getDocument(id: number) {
  return apiGet<Document>(`/assist/api/documents/${id}/`)
}

/**
 * 访客（未登录）读取单篇文献的公开信息。
 *
 * 仅当后端 `system.json` 的 `public_detail_enabled = true` 时可用，否则返回 401
 * （由 axios 拦截器转成跳登录页）。
 */
export function getPublicDocument(id: number) {
  return apiGet<PublicDocument>(`/assist/api/documents/public/${id}`)
}

/** 更新文档元数据 */
export function updateDocument(id: number, data: Partial<Document>) {
  return apiPut<Document>(`/assist/api/documents/${id}/`, data)
}

/** 删除文档 */
export function deleteDocument(id: number) {
  return apiDelete<{ message: string }>(`/assist/api/documents/${id}/`)
}

// ── 上传与登记 ──────────────────────────────────────────

/**
 * 计算文件 SHA-256 哈希（浏览器 Web Crypto API）
 */
export async function computeFileHash(file: File): Promise<string> {
  const buffer = await file.arrayBuffer()
  const hashBuffer = await crypto.subtle.digest('SHA-256', buffer)
  const hashArray = Array.from(new Uint8Array(hashBuffer))
  return hashArray.map((b) => b.toString(16).padStart(2, '0')).join('')
}

/**
 * 查询某个 hash 是否已存在于数据库中。
 * 200 → 存在并返回文档；404 → 不存在。
 */
export async function checkHashExists(hash: string) {
  try {
    const doc = await apiGet<Document>(`/assist/api/documents/by-hash/${hash}/`)
    return { exists: true as const, doc }
  } catch (e: any) {
    if (e?.response?.status === 404) {
      return { exists: false as const }
    }
    throw e
  }
}

/** 上传阶段：hashing（本地算校验值）→ uploading（传输字节）→ processing（服务端查重/抽元数据/落盘） */
export type UploadPhase = 'hashing' | 'uploading' | 'processing'

/** 上传接口的返回：新建时就是一个文档对象，去重命中时额外带 status 等字段 */
export interface UploadResult extends Partial<Document> {
  id: number
  /** attached = 补进已有条目（DOI 相同且该条目缺 PDF）；replaced = 覆盖了已有条目的 PDF；skipped = 未改动 */
  status?: 'attached' | 'replaced' | 'skipped'
  attached?: boolean
  replaced?: boolean
  skipped?: boolean
  /** 命中的重复类型 */
  reason?: 'hash' | 'doi'
  /** 命中的已有文档 ID（与 id 相同） */
  existing_id?: number
  doc?: Document
}

export interface UploadOptions {
  /**
   * 命中重复（同 DOI）时的处置：
   * - `error`（默认）：409 返回已有条目信息，由界面询问用户是否覆盖；
   * - `replace`：直接用本次文件覆盖已有条目的 PDF。
   */
  onDuplicate?: 'error' | 'skip' | 'replace'
  /** 显式 DOI（服务端可自行从 PDF 抽取，一般不用传） */
  doi?: string
}

export interface UploadProgress {
  phase: UploadPhase
  /** 已上传字节（hashing 阶段为 0） */
  loaded: number
  /** 文件总字节 */
  total: number
  /** 0–100；hashing 阶段为 0，processing 阶段为 100 */
  percent: number
}

/**
 * 上传 PDF（上传前先本地算 hash 查重，force 可跳过）
 *
 * 服务端还会按 DOI 去重：DOI 相同且已有条目**缺 PDF** 时直接把文件补进那个条目
 * （200 `status: attached`，不新建记录）；已有条目**已有 PDF** 时返回 409
 * （`error: duplicate, kind: doi, has_pdf: true`），界面询问后再带
 * `onDuplicate: 'replace'` 重传即可覆盖。
 *
 * `onProgress` 报告的是**真实**的上传字节进度（axios `onUploadProgress`），
 * 其中 `phase === 'processing'` 表示字节已全部发出、正等服务端处理。
 */
export async function uploadPdf(
  file: File,
  force = false,
  onProgress?: (progress: UploadProgress) => void,
  options: UploadOptions = {}
) {
  // 非强制模式：先算 hash、先查重，命中则直接拒绝，不上传
  if (!force) {
    onProgress?.({ phase: 'hashing', loaded: 0, total: file.size, percent: 0 })
    let hash: string
    try {
      hash = await computeFileHash(file)
    } catch (e) {
      // 读不出文件（被移动/删除、浏览器内存不足）：打上标记让上层能给可读提示
      throw {
        code: 'HASH_FAILED',
        message: e instanceof Error ? e.message : String(e),
      }
    }
    const check = await checkHashExists(hash)
    if (check.exists) {
      throw {
        response: {
          status: 409,
          data: {
            error: 'duplicate',
            message: `文件已存在 (hash: ${hash.slice(0, 16)}...)`,
            existing_id: check.doc.id,
          },
        },
      }
    }
  }

  const fd = new FormData()
  // 字段顺序敏感：`on_duplicate` / `doi` 必须在 `file` 之前（后端边读边落盘）
  if (options.onDuplicate) fd.append('on_duplicate', options.onDuplicate)
  if (options.doi) fd.append('doi', options.doi)
  fd.append('file', file)
  fd.append('force', String(force))

  onProgress?.({ phase: 'uploading', loaded: 0, total: file.size, percent: 0 })
  return apiUpload<UploadResult>('/assist/api/documents/upload/', fd, {
    // 大文件上传可能远超 axios 默认的 30s，这里不设超时（由浏览器/后端决定）
    timeout: 0,
    onUploadProgress: (event) => {
      const total = event.total && event.total > 0 ? event.total : file.size
      const loaded = event.loaded ?? 0
      // 字节发完（100%）后还在等服务端响应，这一段没有进度可报
      const sent = total > 0 && loaded >= total
      onProgress?.({
        phase: sent ? 'processing' : 'uploading',
        loaded,
        total,
        percent: total > 0 ? Math.min(100, Math.round((loaded / total) * 100)) : 0,
      })
    },
  })
}

/** 按路径登记 */
export function registerPath(filePath: string) {
  return apiPost<Document>('/assist/api/documents/register/', {
    file_path: filePath
  })
}

/**
 * BibTeX 批量新建条目。
 *
 * `dryRun = true` 只做比对预览（哪些能新建、哪些已存在），不写库；
 * `false` 才真正为「库里缺失」的条目建记录（没有 PDF，`pdf_health = missing`）。
 */
export function importBibtex(content: string, dryRun = true) {
  return apiPost<BibtexImportResponse>(
    '/assist/api/documents/import-bibtex/',
    { content, dry_run: dryRun },
    // 大文件（几千条）解析 + 比对可能比默认 30s 久
    { timeout: 120000 }
  )
}

// ── 搜索 ────────────────────────────────────────────────

/** 语义搜索 */
export async function semanticSearch(query: string, limit = 10, vectorDbId?: string) {
  const res = await apiGet<{ results: SearchResult[]; query: string }>(
    '/assist/api/documents/search/',
    { q: query, limit, vector_db_id: vectorDbId }
  )
  res.results = res.results.map((r) => ({
    id: r.document_id,
    title: r.title,
    score: r.score,
    snippet: r.chunk_text || null,
    ...r
  }))
  return res
}

/** 全文搜索（grep） */
export async function grepSearch(
  pattern: string,
  options: {
    docIds?: number[]
    limit?: number
    context?: number
    algorithm?: 'full' | 'fast'
    regex?: boolean
    journal?: string
    yearStart?: number
    yearEnd?: number
    startAfterId?: number
  } = {}
) {
  const {
    docIds,
    limit = 10,
    context = 2,
    algorithm = 'full',
    regex = true,
    journal,
    yearStart,
    yearEnd,
    startAfterId
  } = options
  const res = await apiGet<{ results: SearchResult[]; query: string; last_id: number | null }>(
    '/assist/api/documents/grep-search/',
    {
      q: pattern,
      limit,
      context,
      doc_ids: docIds?.join(','),
      algorithm,
      regex,
      journal,
      year_start: yearStart,
      year_end: yearEnd,
      start_after_id: startAfterId
    }
  )
  res.results = res.results.map((r) => ({
    id: r.document_id,
    title: r.title,
    score: r.score ?? 0,
    snippet: r.content || null,
    ...r
  }))
  return res
}

/** 元数据搜索（高级搜索）
 *
 * `query` 是 **WoS 风格检索式**（`TAG=值` + `AND/OR/NOT` + 括号 + 通配符），也兼容旧写法
 * `field:value`；解析与 SQL 编译全在后端 `services/query_parser.rs`，语法错误返回 400
 * `{code:"query_error", detail, position}`。
 * 返回格式与 listDocuments 兼容，可直接用于 usePagination。
 */
export function searchInfo(
  query: string,
  options: {
    page?: number
    page_size?: number
    pdf_health?: string
    /** present / absent / unknown（逗号多选） */
    pdf_state?: string
    md_state?: string
    zip_state?: string
  } = {}
) {
  const { page = 1, page_size = 20, pdf_health, pdf_state, md_state, zip_state } = options
  return apiGet<SearchResultList>(
    '/assist/api/documents/search-info/',
    {
      q: query,
      page,
      page_size,
      ...(pdf_health ? { pdf_health } : {}),
      ...(pdf_state ? { pdf_state } : {}),
      ...(md_state ? { md_state } : {}),
      ...(zip_state ? { zip_state } : {})
    }
  )
}

// ── 文件访问 ────────────────────────────────────────────

/** 获取 PDF URL（需 token，用于程序内 fetch） */
export function getPdfUrl(id: number): string {
  return `/assist/api/documents/${id}/pdf/`
}

/** 获取免 token 的 PDF 别名 URL */
export function getFileAlias(id: number) {
  return apiGet<{ url: string; expires_at: string }>(`/assist/api/documents/${id}/file-alias/`)
}

/**
 * 通过**需要 Token** 的 API 端点拉取 PDF 二进制（走 `X-Token` 头）。
 *
 * 用于「网页内预览」：拿到 Blob 后由前端 `URL.createObjectURL` 临时挂给 `<iframe>`，
 * 全程不经过免 token 的 `/assist/file/` 别名端口，凭证不落到地址栏/历史记录里。
 *
 * `onProgress` 拿到实时下载量；服务端未回 `Content-Length` 时 `total` 为 0、`percent` 为 0。
 * `signal` 用于取消（面板关闭 / 切换文档时中止下载，避免在后台把整个 PDF 拉完）。
 */
export interface PdfDownloadProgress {
  loaded: number
  total: number
  /** 0–100；`total` 未知时为 0 */
  percent: number
}

export interface FetchPdfBlobOptions {
  /** 实时下载进度 */
  onProgress?: (info: PdfDownloadProgress) => void
  /** 取消下载；取消后 Promise 以 axios 的 `CanceledError`（`code: 'ERR_CANCELED'`）reject */
  signal?: AbortSignal
}

export async function fetchPdfBlob(id: number, options: FetchPdfBlobOptions = {}): Promise<Blob> {
  const { onProgress, signal } = options
  const res = await client.get<Blob>(`/assist/api/documents/${id}/pdf/`, {
    responseType: 'blob',
    // 大文件下载可能远超 axios 默认的 30s，这里不设超时（与上传 / 网盘下载一致）
    timeout: 0,
    signal,
    onDownloadProgress: (ev: { loaded: number; total?: number }) => {
      if (!onProgress) return
      const loaded = ev.loaded || 0
      const total = ev.total && ev.total > 0 ? ev.total : 0
      onProgress({
        loaded,
        total,
        percent: total > 0 ? Math.min(100, Math.round((loaded / total) * 100)) : 0
      })
    }
  })
  return res.data
}

/** 重算文档哈希 */
export function rehashDocument(id: number) {
  return apiPost<{ id: number; file_hash: string; old_file_hash: string | null; updated_at: string }>(
    `/assist/api/documents/${id}/rehash/`,
    {}
  )
}

/**
 * 替换文档 PDF。
 * onUploadProgress 可选，接收 0–100 的实时上传百分比。
 */
export function replacePdf(
  id: number,
  file: File,
  onUploadProgress?: (percent: number) => void
) {
  const fd = new FormData()
  fd.append('file', file)
  return apiUpload<Document>(
    `/assist/api/documents/${id}/pdf/`,
    fd,
    onUploadProgress ? { onUploadProgress } : undefined
  )
}

/**
 * 替换文档 Markdown。
 * onUploadProgress 可选，接收 0–100 的实时上传百分比。
 */
export function replaceMarkdown(
  id: number,
  file: File,
  onUploadProgress?: (percent: number) => void
) {
  const fd = new FormData()
  fd.append('file', file)
  return apiUpload<Document>(
    `/assist/api/documents/${id}/replace-markdown/`,
    fd,
    onUploadProgress ? { onUploadProgress } : undefined
  )
}

/**
 * 替换文档图片资源包 (ZIP)。
 * onUploadProgress 可选，接收 0–100 的实时上传百分比。
 */
export function replaceAsset(
  id: number,
  file: File,
  onUploadProgress?: (percent: number) => void
) {
  const fd = new FormData()
  fd.append('file', file)
  return apiUpload<Document>(
    `/assist/api/documents/${id}/replace-asset/`,
    fd,
    onUploadProgress ? { onUploadProgress } : undefined
  )
}

/** 获取 Markdown 内容（行切片） */
export function getMarkdown(
  id: number,
  options: { line_start?: number; line_count?: number; full?: boolean; raw?: boolean } = {}
) {
  const { line_start, line_count, full = false, raw = false } = options
  const params: Record<string, string | number> = { full: full ? 1 : 0 }
  if (line_start !== undefined) params.line_start = line_start
  if (line_count !== undefined) params.line_count = line_count
  if (raw) params.raw = 1
  return apiGet<MarkdownResponse>(`/assist/api/documents/${id}/markdown/`, params)
}

/** 获取文档目录（标题 + 行号） */
export function getTOC(id: number) {
  return apiGet<{ toc: TocItem[] }>(`/assist/api/documents/${id}/toc/`)
}

/** 获取 Markdown 区块（按 heading 边界分组） */
export function getChunks(id: number) {
  return apiGet<ChunkResponse>(`/assist/api/documents/${id}/chunks/`)
}

/** 保存 Markdown（baseHash 为读取时的文件版本，用于 409 并发检测） */
export function saveMarkdown(id: number, content: string, baseHash?: string) {
  return apiPut<{ detail: string; hash: string }>(
    `/assist/api/documents/${id}/markdown/`,
    { content, base_hash: baseHash }
  )
}

/**
 * 按行范围保存 Markdown 源码（就地编辑）。
 * line_start 为 0-based 含端点，line_count 为被替换的行数（0 = 纯插入）。
 * 写入的是文件真实内容（不做图片路径还原），与 getMarkdown(..., {raw:true}) 配对使用。
 * base_hash 为加载时的文件 hash，不匹配时后端返回 409。
 */
export function saveMarkdownLines(
  id: number,
  payload: {
    line_start: number
    line_count: number
    content: string
    /** true 时显式删除该行范围（忽略 content） */
    delete?: boolean
    base_hash?: string
  }
) {
  return apiPut<MarkdownLinesSaveResult>(
    `/assist/api/documents/${id}/markdown/lines/`,
    payload
  )
}

/** 获取图片 URL */
export function getImageUrl(id: number, filename: string): string {
  return `/assist/api/documents/${id}/image/${filename}/`
}

// ── 辅助查询 ────────────────────────────────────────────

/** 获取所有文档类型
 *
 * `doc_types` = 库里实际用到的值（可能还有历史脏值）；
 * `standard`  = 唯一认可的标准词表（值 + 中文标签），编辑/筛选都用它。
 */
export function getDocTypes() {
  return apiGet<{
    doc_types: string[]
    standard?: Array<{ value: string; label: string }>
  }>('/assist/api/documents/doc-types/')
}

/** 获取相似标题分组（去重用，后端按归一化标题 + 编辑距离聚类） */
export function getSimilarTitles() {
  return apiGet<SimilarTitleGroup>('/assist/api/documents/similar-titles/')
}

/** 按归一化 DOI 查找重复文献组（用于手动去重） */
export function getSimilarDois() {
  return apiGet<SimilarDoiGroup>('/assist/api/documents/similar-dois/')
}

/** 将 sourceIds 合并进 targetId（去重合并，源文档将被删除） */
export function mergeDocuments(targetId: number, sourceIds: number[]) {
  return apiPost<{ status: string; target_id: number; merged_ids: number[]; detail: string }>(
    '/assist/api/documents/merge/',
    { target_id: targetId, source_ids: sourceIds }
  )
}
// ── PDF 元信息 / Crossref / 引用条目 ─────────────────────

/** PDF 元信息（Info 字典 + 页数 + 启发式抽取结果） */
export function getPdfInfo(id: number) {
  return apiGet<PdfInfoResponse>(`/assist/api/documents/${id}/pdf-info/`)
}

/** 已保存的 Crossref 原始记录 */
export function getCrossrefRecord(id: number) {
  return apiGet<CrossrefRecordResponse>(`/assist/api/documents/${id}/crossref/`)
}

/** 生成引用条目（BibTeX / GB/T 7714） */
export function getCitation(
  id: number,
  format: CitationFormat = 'bibtex',
  source: 'auto' | 'crossref' | 'document' = 'auto'
) {
  return apiGet<CitationResponse>(`/assist/api/documents/${id}/citation/`, { format, source })
}
