/**
 * GitHub 风格高级搜索查询解析器
 *
 * 支持的语法：
 *   keyword                 自由文本搜索
 *   field:value             字段精确匹配
 *   field:>value            数值大于
 *   field:>=value           数值大于等于
 *   field:<value            数值小于
 *   field:<=value           数值小于等于
 *   field:min-max           数值范围
 *   "exact phrase"          精确短语
 *   -field:value            排除条件
 *   expr1 OR expr2          逻辑或
 *
 * 示例：
 *   transformer title:attention year:>2018 status:done type:article
 *   author:smith journal:Nature
 *   "deep learning" year:2020-2023 OR year:2018
 */

// ── 支持的字段前缀列表 ──────────────────────────────────

export interface FieldDef {
  prefix: string       // 查询前缀，如 "author"
  field: string        // 映射到后端的 search_fields / filter 参数名
  type: 'text' | 'number' | 'status' | 'doc_type'
  label: string        // 显示用中文名
}

export const SEARCH_FIELD_DEFS: FieldDef[] = [
  { prefix: 'title', field: 'title', type: 'text', label: '标题' },
  { prefix: 'author', field: 'authors', type: 'text', label: '作者' },
  { prefix: 'authors', field: 'authors', type: 'text', label: '作者' },
  { prefix: 'keyword', field: 'keywords', type: 'text', label: '关键词' },
  { prefix: 'keywords', field: 'keywords', type: 'text', label: '关键词' },
  { prefix: 'abstract', field: 'abstract', type: 'text', label: '摘要' },
  { prefix: 'journal', field: 'journal', type: 'text', label: '期刊' },
  { prefix: 'doi', field: 'doi', type: 'text', label: 'DOI' },
  { prefix: 'source', field: 'source', type: 'text', label: '来源' },
  { prefix: 'filename', field: 'filename', type: 'text', label: '文件名' },
  { prefix: 'category', field: 'category', type: 'text', label: '分类' },
  { prefix: 'type', field: 'doc_type', type: 'doc_type', label: '文献类型' },
  { prefix: 'doc_type', field: 'doc_type', type: 'doc_type', label: '文献类型' },
  { prefix: 'lang', field: 'language', type: 'text', label: '语言' },
  { prefix: 'language', field: 'language', type: 'text', label: '语言' },
  { prefix: 'year', field: 'year', type: 'number', label: '年份' },
  { prefix: 'status', field: 'status', type: 'status', label: '状态' },
  // 排序控制（不参与字段过滤，由 parsedQueryToParams 特殊处理）
  { prefix: 'sort', field: '__sort__', type: 'sort', label: '排序字段' },
  { prefix: 'order', field: '__order__', type: 'sort', label: '排序方向' },
]

// 状态值别名 → 后端 status_filter 值
const STATUS_ALIASES: Record<string, string> = {
  uploaded: 'uploaded',
  parsing: 'parsing',
  done: 'markdown_done',
  markdown_done: 'markdown_done',
  error: 'error',
}

// ── 解析结果类型 ────────────────────────────────────────

export interface ParsedQuery {
  /** 自由文本 token（非 field:value 部分） */
  textTokens: string[]
  /** 字段精确值过滤 */
  fieldFilters: Record<string, string[]>
  /** 数值范围条件（year 等） */
  rangeFilters: Record<string, { min?: number; max?: number }>
  /** 否定条件（-field:value） */
  negations: { field: string; value: string; type: 'text' | 'number' | 'status' | 'doc_type' }[]
  /** OR 分组：每组是一个 ParsedQuery */
  orGroups: ParsedQuery[]
  /** 原始查询字符串 */
  raw: string
  /** 排序字段（sort:xxx） */
  sortBy?: string
  /** 排序方向（order:asc / order:desc） */
  sortOrder?: 'asc' | 'desc'
}

// ── 分词器 ──────────────────────────────────────────────

function* tokenize(input: string): Generator<string> {
  const re = /(?:[^\s"]+|"[^"]*")+/g
  let m: RegExpExecArray | null
  while ((m = re.exec(input)) !== null) {
    yield m[0]
  }
}

// ── 主解析函数 ──────────────────────────────────────────

export function parseSearchQuery(input: string): ParsedQuery {
  const trimmed = input.trim()
  const result: ParsedQuery = {
    textTokens: [],
    fieldFilters: {},
    rangeFilters: {},
    negations: [],
    orGroups: [],
    raw: trimmed,
  }

  if (!trimmed) return result

  // 先按 OR 分割（去掉首尾空格，忽略空）
  const orParts = splitOR(trimmed)

  if (orParts.length > 1) {
    // 有多组 OR，分别解析
    result.orGroups = orParts.map((p) => parseSingleQuery(p.trim()))
    return result
  }

  return parseSingleQuery(trimmed)
}

/** 按 OR 分割，但忽略引号内的 OR */
function splitOR(input: string): string[] {
  const parts: string[] = []
  let current = ''
  let inQuote = false
  let i = 0
  while (i < input.length) {
    const ch = input[i]
    if (ch === '"') inQuote = !inQuote
    if (!inQuote && input.substring(i, i + 3) === ' OR ') {
      parts.push(current)
      current = ''
      i += 3
      continue
    }
    // 中文全角 OR 也支持
    if (!inQuote && input.substring(i, i + 3) === ' ＯＲ ') {
      parts.push(current)
      current = ''
      i += 3
      continue
    }
    current += ch
    i++
  }
  if (current) parts.push(current)
  return parts
}

function parseSingleQuery(input: string): ParsedQuery {
  const q: ParsedQuery = {
    textTokens: [],
    fieldFilters: {},
    rangeFilters: {},
    negations: [],
    orGroups: [],
    raw: input,
  }

  for (const token of tokenize(input)) {
    // 精准短语（带引号）
    if (token.startsWith('"') && token.endsWith('"')) {
      q.textTokens.push(token.slice(1, -1))
      continue
    }

    // 否定前缀: -field:value
    if (token.startsWith('-') && token.includes(':')) {
      const rest = token.slice(1)
      const colonIdx = rest.indexOf(':')
      const prefix = rest.slice(0, colonIdx)
      const value = rest.slice(colonIdx + 1)
      const def = findFieldDef(prefix)
      if (def) {
        q.negations.push({ field: def.field, value, type: def.type })
      } else {
        q.textTokens.push(token) // 无法识别，当普通文本
      }
      continue
    }

    // field:value pattern
    if (token.includes(':')) {
      const colonIdx = token.indexOf(':')
      const prefix = token.slice(0, colonIdx)
      const value = token.slice(colonIdx + 1)
      const def = findFieldDef(prefix)
      if (def) {
        applyFieldFilter(q, def, value)
        continue
      }
    }

    // 普通文本 token
    q.textTokens.push(token)
  }

  return q
}

function findFieldDef(prefix: string): FieldDef | undefined {
  return SEARCH_FIELD_DEFS.find(
    (d) => d.prefix === prefix.toLowerCase()
  )
}

function applyFieldFilter(q: ParsedQuery, def: FieldDef, rawValue: string) {
  const value = rawValue.replace(/^["']|["']$/g, '') // 去掉两端引号

  if (def.type === 'number') {
    // 解析数值范围语法: >2020, >=2020, <2020, <=2020, 2020-2023
    const rangeMatch = value.match(
      /^(>=?|<=?)\s*(\d+)$|^(\d+)\s*-\s*(\d+)$|^(\d+)$/
    )
    if (rangeMatch) {
      if (rangeMatch[1] && rangeMatch[2]) {
        // >2020, >=2020, <2020, <=2020
        const op = rangeMatch[1]
        const num = parseInt(rangeMatch[2], 10)
        if (!q.rangeFilters[def.field]) q.rangeFilters[def.field] = {}
        if (op === '>') q.rangeFilters[def.field].min = num + 1
        else if (op === '>=') q.rangeFilters[def.field].min = num
        else if (op === '<') q.rangeFilters[def.field].max = num - 1
        else if (op === '<=') q.rangeFilters[def.field].max = num
      } else if (rangeMatch[3] && rangeMatch[4]) {
        // 2020-2023
        const min = parseInt(rangeMatch[3], 10)
        const max = parseInt(rangeMatch[4], 10)
        q.rangeFilters[def.field] = { min, max }
      } else if (rangeMatch[5]) {
        // 精确值 2020
        const num = parseInt(rangeMatch[5], 10)
        q.rangeFilters[def.field] = { min: num, max: num }
      }
    } else {
      // 无法解析为数值，当文本处理
      addFieldFilter(q, def.field, value)
    }
  } else if (def.type === 'status') {
    // 状态值别名转换
    const mapped = STATUS_ALIASES[value.toLowerCase()] || value
    addFieldFilter(q, def.field, mapped)
  } else if (def.type === 'sort') {
    // sort: 和 order: 直接设置到 ParsedQuery 上
    if (def.field === '__sort__') {
      q.sortBy = value.toLowerCase()
    } else if (def.field === '__order__') {
      const v = value.toLowerCase()
      if (v === 'asc' || v === 'desc') q.sortOrder = v
    }
  } else {
    // 文本字段
    addFieldFilter(q, def.field, value)
  }
}

function addFieldFilter(q: ParsedQuery, field: string, value: string) {
  if (!q.fieldFilters[field]) q.fieldFilters[field] = []
  q.fieldFilters[field].push(value)
}

// ── 解析结果 → API 参数转换 ─────────────────────────────

export interface SearchApiParams {
  q?: string
  search_fields?: string
  status_filter?: string
  doc_type_filter?: string
  year?: number
  year_min?: number
  year_max?: number
  journal?: string
  authors?: string
  sort_by?: string
  sort_order?: 'asc' | 'desc'
}

/**
 * 将 ParsedQuery 转为后端 API 可接受的参数
 */
export function parsedQueryToParams(pq: ParsedQuery): SearchApiParams {
  const params: SearchApiParams = {}

  // 自由文本查询
  if (pq.textTokens.length > 0) {
    params.q = pq.textTokens.join(' ')
  }

  // 字段过滤
  for (const [field, values] of Object.entries(pq.fieldFilters)) {
    if (field === 'status') {
      params.status_filter = values.join(',')
    } else if (field === 'doc_type') {
      params.doc_type_filter = values.join(',')
    } else if (field === 'journal') {
      params.journal = values[0]
    } else if (field === 'authors') {
      params.authors = values[0]
    } else {
      // 按 search_fields 限定字段搜索
      // 如果有自由文本 q，且有限定字段，则将 search_fields 设为该字段
      if (params.q && !params.search_fields) {
        params.search_fields = field
      }
    }
  }

  // 如果只有 fieldFilters 没有 textTokens，将第一个字段值作为 q
  if (!params.q && Object.keys(pq.fieldFilters).length > 0) {
    for (const [field, values] of Object.entries(pq.fieldFilters)) {
      if (field !== 'status' && field !== 'doc_type') {
        params.q = values[0]
        params.search_fields = field
        break
      }
    }
  }

  // 范围过滤（year）
  for (const [field, range] of Object.entries(pq.rangeFilters)) {
    if (field === 'year') {
      if (range.min !== undefined && range.max !== undefined && range.min === range.max) {
        params.year = range.min
      } else {
        if (range.min !== undefined) params.year_min = range.min
        if (range.max !== undefined) params.year_max = range.max
      }
    }
  }

  // 排序控制
  if (pq.sortBy) params.sort_by = pq.sortBy
  if (pq.sortOrder) params.sort_order = pq.sortOrder

  return params
}

/**
 * 将完整的 ParsedQuery（含 OR 分组）转为一组 API 参数
 * 如果有 OR 分组，目前简单处理：仅返回第一组（把 OR 信息放入 q 中通知后端）
 * 更完善的 OR 支持需要后端 SQL 级支持，暂不实现
 */
export function buildSearchParams(
  input: string
): SearchApiParams & { _orQuery?: string } {
  const pq = parseSearchQuery(input)
  const params = parsedQueryToParams(pq)

  // 如果有 OR 分组，将原始查询作为自由文本搜索（后端不支持 OR 时退化为简单文本搜索）
  if (pq.orGroups.length > 0) {
    params._orQuery = input
    // 仍返回第一组参数，但标记了 orQuery
  }

  return params
}

// ── 语法提示 ────────────────────────────────────────────

export const SYNTAX_HELP_ITEMS = [
  { syntax: 'keyword', description: '自由文本搜索（标题/作者/文件名）' },
  { syntax: 'title:关键词', description: '按标题搜索' },
  { syntax: 'author:姓名', description: '按作者搜索' },
  { syntax: 'journal:刊名', description: '按期刊搜索' },
  { syntax: 'year:2023', description: '按年份精确匹配' },
  { syntax: 'year:>2020', description: '年份大于 2020' },
  { syntax: 'year:2020-2023', description: '年份范围' },
  { syntax: 'type:article', description: '按文献类型筛选' },
  { syntax: 'status:done', description: '按状态筛选（uploaded/parsing/done/error）' },
  { syntax: 'sort:year', description: '排序字段（title/year/authors/id/created_at 等）' },
  { syntax: 'order:asc', description: '排序方向（asc 升序 / desc 降序）' },
  { syntax: '-year:2020', description: '排除条件' },
  { syntax: 'a OR b', description: '逻辑或（两个条件满足其一）' },
  { syntax: '"exact phrase"', description: '精确短语匹配' },
]