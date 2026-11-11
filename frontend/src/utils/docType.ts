/**
 * 文档类型（`documents.doc_type`）的唯一候选词表。
 *
 * 与后端 `backend-rs/src/services/doc_type.rs` 的 `STANDARD` 一一对应：**只认 Zotero 标准类型名**。
 * 历史上这里散落着好几套写法（`article` / `journal` / `conference` / `book_section` / `dataset` …），
 * 选出来的筛选条件命中不了另一种写法，库里也就堆积了同义词碎片——所以新增/修改类型的地方
 * 一律从这里取候选，不要再手写 `<option>`。
 *
 * 后端 `GET /assist/api/documents/doc-types` 也会下发同一份 `standard`（含标签），
 * 编辑页会在挂载时用它覆盖静态表，避免前后端词表长期漂移。
 */

export interface DocTypeOption {
  /** 写入数据库的标准值 */
  value: string
  /** 中文标签 */
  label: string
}

/** 候选类型（顺序即下拉顺序；与后端 `services::doc_type::STANDARD` 保持一致） */
export const DOC_TYPE_OPTIONS: DocTypeOption[] = [
  { value: 'journalArticle', label: '期刊论文' },
  { value: 'conferencePaper', label: '会议论文' },
  { value: 'thesis', label: '学位论文' },
  { value: 'book', label: '专著' },
  { value: 'bookSection', label: '专著章节' },
  { value: 'preprint', label: '预印本' },
  { value: 'report', label: '报告' },
  { value: 'patent', label: '专利' },
  { value: 'review', label: '综述' },
  { value: 'webpage', label: '网页' },
  { value: 'presentation', label: '演示' },
  { value: 'manuscript', label: '手稿' },
  { value: 'document', label: '其他/文档' }
]

/**
 * 归一化到标准类型（后端 `normalize()` 的前端镜像）。
 *
 * 只用于**展示**与「当前值不在候选里」时的兜底判断，真正的写入归一化在后端完成。
 */
export function normalizeDocType(raw?: string | null): string {
  const text = (raw || '').trim()
  if (!text) return ''
  const key = text.toLowerCase().replace(/[_\-]+/g, ' ').replace(/\s+/g, ' ').trim()
  const alias: Record<string, string> = {
    journalarticle: 'journalArticle',
    'journal article': 'journalArticle',
    journal: 'journalArticle',
    article: 'journalArticle',
    periodical: 'journalArticle',
    'magazine article': 'journalArticle',
    'newspaper article': 'journalArticle',
    conferencepaper: 'conferencePaper',
    'conference paper': 'conferencePaper',
    conference: 'conferencePaper',
    'conference proceedings': 'conferencePaper',
    proceedings: 'conferencePaper',
    'proceedings article': 'conferencePaper',
    inproceedings: 'conferencePaper',
    thesis: 'thesis',
    dissertation: 'thesis',
    degree: 'thesis',
    mastersthesis: 'thesis',
    'master thesis': 'thesis',
    phdthesis: 'thesis',
    'phd thesis': 'thesis',
    book: 'book',
    monograph: 'book',
    'edited book': 'book',
    'reference book': 'book',
    booksection: 'bookSection',
    'book section': 'bookSection',
    bookchapter: 'bookSection',
    'book chapter': 'bookSection',
    chapter: 'bookSection',
    incollection: 'bookSection',
    inbook: 'bookSection',
    'book part': 'bookSection',
    'book set': 'bookSection',
    preprint: 'preprint',
    'posted content': 'preprint',
    postedcontent: 'preprint',
    report: 'report',
    techreport: 'report',
    'technical report': 'report',
    patent: 'patent',
    review: 'review',
    'peer review': 'review',
    peerreview: 'review',
    webpage: 'webpage',
    website: 'webpage',
    'web page': 'webpage',
    blogpost: 'webpage',
    'blog post': 'webpage',
    presentation: 'presentation',
    'conference presentation': 'presentation',
    manuscript: 'manuscript',
    document: 'document',
    other: 'document',
    misc: 'document',
    generic: 'document',
    unknown: 'document',
    dataset: 'document',
    'reference entry': 'document'
  }
  if (alias[key]) return alias[key]
  const direct = DOC_TYPE_OPTIONS.find((o) => o.value.toLowerCase() === key)
  return direct ? direct.value : 'document'
}

/** 中文标签（未知值原样返回，历史脏值也能显示出来） */
export function docTypeLabel(raw?: string | null): string {
  const value = (raw || '').trim()
  if (!value) return ''
  const hit = DOC_TYPE_OPTIONS.find((o) => o.value === value)
  return hit ? hit.label : value
}

/** 「标签（值）」形式，用于需要同时暴露原始值的场合 */
export function docTypeLabelWithValue(raw?: string | null): string {
  const value = (raw || '').trim()
  if (!value) return ''
  const label = docTypeLabel(value)
  return label === value ? value : `${label}（${value}）`
}
