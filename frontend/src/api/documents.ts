import { apiGet, apiPost, apiPut, apiDelete, apiUpload } from './client'
import type {
  Document,
  SearchResult,
  SearchResultList,
  SimilarTitleGroup
} from '@/types/document'

// ── 文档 CRUD ───────────────────────────────────────────

/** 文档列表（分页 + 筛选 + 排序） */
export function listDocuments(params: {
  q?: string
  status_filter?: string
  doc_type_filter?: string
  search_fields?: string
  sort_by?: 'created_at' | 'title' | 'year' | 'id' | 'authors' | 'status' | 'updated_at'
  sort_order?: 'asc' | 'desc'
  page?: number
  page_size?: number
}) {
  return apiGet<SearchResultList>('/assist/api/documents/', params)
}

/** 文档详情 */
export function getDocument(id: number) {
  return apiGet<Document>(`/assist/api/documents/${id}/`)
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

/** 上传 PDF */
export function uploadPdf(file: File) {
  const fd = new FormData()
  fd.append('file', file)
  return apiUpload<Document>('/assist/api/documents/upload/', fd)
}

/** 按路径登记 */
export function registerPath(filePath: string) {
  return apiPost<Document>('/assist/api/documents/register/', {
    file_path: filePath
  })
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
  } = {}
) {
  const { docIds, limit = 10, context = 2, algorithm = 'full', regex = true } = options
  const res = await apiGet<{ results: SearchResult[]; query: string }>(
    '/assist/api/documents/grep-search/',
    {
      q: pattern,
      limit,
      context,
      doc_ids: docIds?.join(','),
      algorithm,
      regex
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

/** 元数据搜索 */
export function searchInfo(query: string, limit = 10) {
  return apiGet<{ results: Document[]; query: string; total: number }>(
    '/assist/api/documents/search-info/',
    { q: query, limit }
  )
}

// ── 文件访问 ────────────────────────────────────────────

/** 获取 PDF URL */
export function getPdfUrl(id: number): string {
  return `/assist/api/documents/${id}/pdf/`
}

/** 获取免 token 的 PDF 别名 URL */
export function getFileAlias(id: number) {
  return apiGet<{ url: string }>(`/assist/api/documents/${id}/file-alias/`)
}

/** 替换文档 PDF */
export function replacePdf(id: number, file: File) {
  const fd = new FormData()
  fd.append('file', file)
  return apiUpload<Document>(`/assist/api/documents/${id}/pdf/`, fd)
}

/** 获取 Markdown 内容 */
export function getMarkdown(id: number, page = 1) {
  return apiGet<{
    content: string
    total_pages: number
    current_page: number
  }>(`/assist/api/documents/${id}/markdown/`, { page })
}

/** 保存 Markdown */
export function saveMarkdown(id: number, content: string) {
  return apiPut<{ message: string }>(
    `/assist/api/documents/${id}/markdown/`,
    { content }
  )
}

/** 获取图片 URL */
export function getImageUrl(id: number, filename: string): string {
  return `/assist/api/documents/${id}/image/${filename}/`
}

// ── 辅助查询 ────────────────────────────────────────────

/** 获取所有文档类型 */
export function getDocTypes() {
  return apiGet<{ doc_types: string[] }>('/assist/api/documents/doc-types/')
}

/** 获取相似标题（去重用） */
export function getSimilarTitles(title: string) {
  return apiGet<SimilarTitleGroup>(
    '/assist/api/documents/similar-titles/',
    { title }
  )
}