// ── 文档相关类型 ────────────────────────────────────────

export type DocStatus =
  | 'uploaded'
  | 'parsing'
  | 'markdown_done'
  | 'extracting'
  | 'meta_done'
  | 'indexing'
  | 'indexed'
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
  index_status: IndexStatus
  file_hash: string | null
  file_size: number | null
  created_at: string
  updated_at: string
  has_markdown: boolean
}

export interface SearchResult {
  id: number
  title: string
  score: number
  snippet: string | null
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

export interface SimilarTitleGroup {
  groups: {
    normalized_title: string
    count: number
    documents: Document[]
  }[]
  total_groups: number
  total_documents: number
}