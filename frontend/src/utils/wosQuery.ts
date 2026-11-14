/**
 * Web of Science 风格检索式（前端解析侧）。
 *
 * **语法真相在后端**：`backend-rs/src/services/query_parser.rs` 负责把检索式编译成 SQL，
 * 本文件只做三件事，供搜索栏高亮 / 语法错误提示 / 高级搜索面板的实时校验使用：
 *
 * 1. `parseWosQuery()` —— 解析出「生效了哪些条件」+ 语法错误（含出错位置）；列表页只用其中的错误信息，
 *    条件列表（`conditions`）由高级搜索面板显示「共 N 个条件」；
 * 2. `highlightWosQuery()` —— 给搜索框的背景层上色；
 * 3. `WOS_FIELDS` / `WOS_OPERATORS` / `WOS_EXAMPLES` —— 面板里的字段标签与示例。
 *
 * 两份实现必须保持同步：新增字段标签时先改后端 `resolve_tag()`，再补 `WOS_FIELDS`。
 * 前端校验刻意比后端宽松一点（数值/枚举能校验的就校验），真正的裁决仍是后端 400。
 */

// ── 字段标签表 ──────────────────────────────────────────

export type WosFieldKind = 'text' | 'number' | 'enum' | 'state' | 'health' | 'sort' | 'order'

export interface WosField {
  /** 面板上显示 / 插入的标签（WoS 风格大写） */
  tag: string
  /** 中文名 */
  label: string
  kind: WosFieldKind
  /** 简短的取值说明 */
  hint: string
  /** 其它等价写法（含旧语法的小写前缀） */
  aliases?: string[]
}

export const WOS_FIELDS: WosField[] = [
  { tag: 'TS', label: '主题（题名/关键词/摘要）', kind: 'text', hint: '可跟括号组：TS=(a OR b)' },
  { tag: 'TI', label: '标题', kind: 'text', hint: '标题关键词，支持 * 通配符', aliases: ['title'] },
  { tag: 'AU', label: '作者', kind: 'text', hint: '作者名，支持 * 通配符', aliases: ['author', 'authors'] },
  { tag: 'SO', label: '期刊/会议', kind: 'text', hint: '含空格用引号', aliases: ['journal'] },
  { tag: 'AB', label: '摘要', kind: 'text', hint: '摘要关键词', aliases: ['abstract'] },
  { tag: 'AK', label: '作者关键词', kind: 'text', hint: '关键词字段', aliases: ['kw', 'keyword', 'keywords'] },
  { tag: 'DO', label: 'DOI', kind: 'text', hint: '10.1000/xyz123', aliases: ['doi'] },
  { tag: 'DT', label: '文献类型', kind: 'enum', hint: 'journalArticle / thesis / book …', aliases: ['type', 'doc_type'] },
  { tag: 'PY', label: '年份', kind: 'number', hint: '2023 / 2020-2023 / >2020', aliases: ['year'] },
  { tag: 'LA', label: '语言', kind: 'text', hint: 'zh / en', aliases: ['lang', 'language'] },
  { tag: 'SRC', label: '来源', kind: 'text', hint: 'source 字段', aliases: ['source'] },
  { tag: 'FN', label: '文件名', kind: 'text', hint: 'PDF 文件名', aliases: ['filename'] },
  { tag: 'CAT', label: '分类', kind: 'text', hint: 'category 字段', aliases: ['category'] },
  { tag: 'ALL', label: '全部字段', kind: 'text', hint: '不写标签时的默认行为（自由文本也走这里）', aliases: ['ft', 'any'] },
  { tag: 'ID', label: '文献 ID', kind: 'number', hint: '12 / 1-30 / >100', aliases: ['an'] },
  { tag: 'PDF', label: 'PDF 文件', kind: 'state', hint: 'present / absent / unknown', aliases: ['has_pdf'] },
  { tag: 'MD', label: 'Markdown 文件', kind: 'state', hint: 'present / absent / unknown', aliases: ['has_md'] },
  { tag: 'ZIP', label: '资源包', kind: 'state', hint: 'present / absent / unknown', aliases: ['has_zip'] },
  { tag: 'HEALTH', label: 'PDF 健康', kind: 'health', hint: 'ok / warning / error / missing / unchecked', aliases: ['pdf_health'] },
  { tag: 'filesize_pdf', label: 'PDF 大小', kind: 'number', hint: '>1MB / 10KB-2MB', aliases: ['pdf_size'] },
  { tag: 'filesize_md', label: 'MD 大小', kind: 'number', hint: '>10KB', aliases: ['md_size'] },
  { tag: 'filesize_zip', label: 'ZIP 大小', kind: 'number', hint: '>100KB', aliases: ['zip_size'] },
  { tag: 'SORT', label: '排序字段', kind: 'sort', hint: 'PY / TI / AU / id / updated_at', aliases: ['sort_by'] },
  { tag: 'ORDER', label: '排序方向', kind: 'order', hint: 'asc / desc', aliases: ['sort_order'] },
]

/** 面板里可一键插入的字段（排序指令单独放，避免误插） */
export const WOS_FIELD_PALETTE = WOS_FIELDS.filter(
  (f) => f.kind !== 'sort' && f.kind !== 'order'
)

export const WOS_OPERATORS = ['AND', 'OR', 'NOT'] as const

export const WOS_EXAMPLES: { label: string; query: string }[] = [
  { label: '主题 + 年份', query: 'TS=(transformer OR "deep learning") AND PY=2020-2023' },
  { label: '作者前缀 + 题名', query: 'AU=Vaswani* AND TI=attention' },
  { label: '期刊精确短语', query: 'SO="IEEE TRANSACTIONS ON AUTOMATIC CONTROL" AND PY>2020' },
  { label: '排除综述', query: 'TS=transformer NOT DT=review' },
  { label: '只看有 PDF 且健康', query: 'PDF=present AND HEALTH=ok' },
  { label: '按年份排序', query: 'DT=thesis AND PY>=2015 SORT=PY ORDER=desc' },
]

export const WOS_STATE_VALUES = ['present', 'absent', 'unknown']
export const WOS_HEALTH_VALUES = ['ok', 'warning', 'error', 'missing', 'unchecked']

/** 标签 / 旧前缀 → 字段定义（大小写不敏感） */
export function findWosField(name: string): WosField | undefined {
  const key = name.trim().toLowerCase()
  return WOS_FIELDS.find(
    (f) => f.tag.toLowerCase() === key || (f.aliases || []).some((a) => a.toLowerCase() === key)
  )
}

/** 排序字段归一化（与后端 `normalize_sort` 一致） */
export function normalizeWosSort(raw: string): string {
  const v = raw.trim().toLowerCase()
  const map: Record<string, string> = {
    py: 'year',
    y: 'year',
    ti: 'title',
    au: 'authors',
    so: 'journal',
    dt: 'doc_type',
    an: 'id',
    filesize_pdf: 'pdf_size',
    filesize_md: 'md_size',
    filesize_zip: 'zip_size'
  }
  return map[v] || v
}

// ── 解析结果 ────────────────────────────────────────────

export interface WosCondition {
  /** 字段标签（大写，如 `TS`） */
  tag: string
  /** 中文名 */
  label: string
  /** 用户输入的值（原样） */
  value: string
  /** 是否被 NOT 取反 */
  negated: boolean
}

export interface WosError {
  message: string
  /** 出错的字符下标（0-based） */
  position?: number
}

export interface WosParseResult {
  ok: boolean
  error?: WosError
  conditions: WosCondition[]
  sortBy?: string
  sortOrder?: 'asc' | 'desc'
}

// ── 分词 ────────────────────────────────────────────────

type Token =
  | { t: 'lparen'; pos: number }
  | { t: 'rparen'; pos: number }
  | { t: 'and' | 'or' | 'not'; pos: number }
  | { t: 'field'; name: string; sep: string; pos: number }
  | { t: 'word'; value: string; pos: number }
  | { t: 'phrase'; value: string; pos: number }

const BOOL_WORDS: Record<string, 'and' | 'or' | 'not'> = {
  and: 'and',
  '&&': 'and',
  near: 'and',
  same: 'and',
  or: 'or',
  '||': 'or',
  not: 'not',
  '!': 'not'
}

/** 识别开头的字段标签：`标识符` + (`=` / `:` / `>` / `>=` / `<` / `<=`) */
function splitTag(raw: string): { name: string; sep: string; rest: string } | null {
  if (!/^[A-Za-z_]/.test(raw)) return null
  const m = /^([A-Za-z_][A-Za-z0-9_]*)(>=|<=|=|:|>|<)([\s\S]*)$/.exec(raw)
  if (!m) return null
  return { name: m[1], sep: m[2], rest: m[3] }
}

function lex(input: string): { tokens: Token[]; error?: WosError } {
  const tokens: Token[] = []
  let i = 0

  const pushWord = (raw: string, pos: number): WosError | undefined => {
    const lower = raw.toLowerCase()
    if (BOOL_WORDS[lower]) {
      tokens.push({ t: BOOL_WORDS[lower], pos } as Token)
      return
    }
    if (lower.startsWith('near/') || lower.startsWith('same/')) {
      tokens.push({ t: 'and', pos })
      return
    }
    const tag = splitTag(raw)
    if (tag) {
      tokens.push({ t: 'field', name: tag.name, sep: tag.sep, pos })
      if (tag.rest) {
        const value =
          tag.sep === '>' || tag.sep === '>=' || tag.sep === '<' || tag.sep === '<='
            ? tag.sep + tag.rest
            : tag.rest
        tokens.push({ t: 'word', value, pos })
      }
      return
    }
    tokens.push({ t: 'word', value: raw, pos })
    return
  }

  while (i < input.length) {
    const ch = input[i]
    if (/\s/.test(ch)) {
      i++
      continue
    }
    const pos = i
    if (ch === '(') {
      tokens.push({ t: 'lparen', pos })
      i++
      continue
    }
    if (ch === ')') {
      tokens.push({ t: 'rparen', pos })
      i++
      continue
    }
    if (ch === '-' && i + 1 < input.length && !/\s/.test(input[i + 1])) {
      // 旧写法否定前缀：-year:2020 / -transformer
      tokens.push({ t: 'not', pos })
      i++
      continue
    }
    if (ch === '"' || ch === "'") {
      const quote = ch
      i++
      let buf = ''
      let closed = false
      while (i < input.length) {
        if (input[i] === '\\' && i + 1 < input.length && (input[i + 1] === quote || input[i + 1] === '\\')) {
          buf += input[i + 1]
          i += 2
          continue
        }
        if (input[i] === quote) {
          closed = true
          i++
          break
        }
        buf += input[i]
        i++
      }
      if (!closed) return { tokens, error: { message: '引号未闭合（缺少结尾的引号）', position: pos } }
      tokens.push({ t: 'phrase', value: buf, pos })
      continue
    }
    let buf = ''
    while (i < input.length && !/[\s()"']/.test(input[i])) {
      buf += input[i]
      i++
    }
    const err = pushWord(buf, pos)
    if (err) return { tokens, error: err }
  }

  return { tokens }
}

// ── 语法分析（递归下降，与后端同一套规则） ─────────────

class Parser {
  private toks: Token[]
  private source: string
  private i = 0
  private ctx: WosField | null = null
  conditions: WosCondition[] = []
  sortBy?: string
  sortOrder?: 'asc' | 'desc'

  constructor(toks: Token[], source: string) {
    this.toks = toks
    this.source = source
  }

  private peek(): Token | undefined {
    return this.toks[this.i]
  }

  private next(): Token | undefined {
    const t = this.toks[this.i]
    if (t) this.i++
    return t
  }

  parse(): WosError | undefined {
    const err = this.parseOr()
    if (err) return err
    const extra = this.peek()
    if (extra) return { message: `多余的内容：${tokenText(extra)}`, position: extra.pos }
    return undefined
  }

  private parseOr(): WosError | undefined {
    let err = this.parseAnd()
    if (err) return err
    while (this.peek()?.t === 'or') {
      const sp = this.next() as Token
      if (!this.canStartOperand()) return { message: '`OR` 后面缺少条件', position: sp.pos }
      err = this.parseAnd()
      if (err) return err
    }
    return undefined
  }

  private parseAnd(): WosError | undefined {
    while (this.peek()) {
      const t = this.peek() as Token
      if (t.t === 'rparen' || t.t === 'or') return undefined
      if (t.t === 'and') {
        this.next()
        if (!this.canStartOperand()) return { message: '`AND` 后面缺少条件', position: t.pos }
        const err = this.parseUnary(false)
        if (err) return err
        continue
      }
      const err = this.parseUnary(false)
      if (err) return err
    }
    return undefined
  }

  private canStartOperand(): boolean {
    const t = this.peek()
    if (!t) return false
    return t.t !== 'rparen' && t.t !== 'and' && t.t !== 'or'
  }

  private parseUnary(negated: boolean): WosError | undefined {
    const t = this.peek()
    if (t && t.t === 'not') {
      this.next()
      if (!this.canStartOperand()) return { message: '`NOT` 后面缺少条件', position: t.pos }
      return this.parseUnary(!negated)
    }
    return this.parsePrimary(negated)
  }

  private parsePrimary(negated: boolean): WosError | undefined {
    const tok = this.next()
    if (!tok) return { message: '查询意外结束（这里应该还有一个条件）', position: this.source.length }
    switch (tok.t) {
      case 'lparen': {
        const saved = this.ctx
        const err = this.parseOr()
        if (err) {
          this.ctx = saved
          return err
        }
        if (this.peek()?.t !== 'rparen') {
          this.ctx = saved
          return { message: '括号未闭合（缺少 `)`）', position: tok.pos }
        }
        this.next()
        this.ctx = saved
        return undefined
      }
      case 'rparen':
        return { message: '多余的 `)`', position: tok.pos }
      case 'and':
      case 'or':
        return { message: `运算符 \`${tokenText(tok)}\` 前面缺少条件`, position: tok.pos }
      case 'not':
        return { message: '`NOT` 后面缺少条件', position: tok.pos }
      case 'field':
        return this.parseField(tok, negated)
      case 'phrase':
      case 'word':
        return this.pushLeaf(tok.value, negated, tok.pos)
    }
  }

  private parseField(
    tok: Extract<Token, { t: 'field' }>,
    negated: boolean
  ): WosError | undefined {
    const field = findWosField(tok.name)
    if (!field) {
      if (tok.sep === '=') {
        return {
          message: `未知字段标签 \`${tok.name.toUpperCase()}\`（可用：TS / TI / AU / SO / PY / AB / AK / DO / DT / ALL …）`,
          position: tok.pos
        }
      }
      // 旧写法里的未知前缀：整体当自由文本（如 https://…）
      const value = this.next()
      if (!value || (value.t !== 'word' && value.t !== 'phrase')) return undefined
      return this.pushLeaf(`${tok.name}:${value.value}`, negated, tok.pos)
    }

    if (field.kind === 'sort' || field.kind === 'order') {
      const raw = this.takeValue()
      if ('error' in raw) return raw.error
      if (field.kind === 'order') {
        const v = raw.value.trim().toLowerCase()
        if (v !== 'asc' && v !== 'desc') {
          return { message: `排序方向只能是 asc 或 desc，实际是 \`${raw.value}\``, position: tok.pos }
        }
        this.sortOrder = v
      } else {
        this.sortBy = normalizeWosSort(raw.value)
      }
      return undefined
    }

    // `TS=(a OR b)`：把字段压进组内的裸值
    if (this.peek()?.t === 'lparen' && field.kind === 'text') {
      const saved = this.ctx
      this.ctx = field
      const err = this.parsePrimary(negated)
      this.ctx = saved
      return err
    }

    const raw = this.takeValue()
    if ('error' in raw) return raw.error
    return this.validateAndPush(field, raw.value, negated, tok.pos)
  }

  private takeValue(): { value: string } | { error: WosError } {
    const t = this.next()
    if (!t || (t.t !== 'word' && t.t !== 'phrase')) {
      return {
        error: {
          message: '字段标签后面需要一个值（含空格请用引号）',
          position: t ? t.pos : this.source.length
        }
      }
    }
    return { value: t.value }
  }

  private pushLeaf(value: string, negated: boolean, pos: number): WosError | undefined {
    const field = this.ctx
    if (!field) {
      this.conditions.push({ tag: 'ALL', label: '全文', value, negated })
      return undefined
    }
    if (field.kind === 'text') {
      this.conditions.push({ tag: field.tag, label: field.label, value, negated })
      return undefined
    }
    return this.validateAndPush(field, value, negated, pos)
  }

  private validateAndPush(
    field: WosField,
    value: string,
    negated: boolean,
    pos: number
  ): WosError | undefined {
    const v = value.trim()
    if (!v) return { message: `\`${field.tag}=\` 后面缺少值`, position: pos }

    if (field.kind === 'number') {
      if (!parseNumeric(v)) {
        return {
          message: `\`${field.tag}\` 需要数值或范围（如 2023 / 2020-2023 / >2020），实际是 \`${value}\``,
          position: pos
        }
      }
    } else if (field.kind === 'state') {
      for (const item of v.split(',').map((s) => s.trim()).filter(Boolean)) {
        if (!WOS_STATE_VALUES.includes(item.toLowerCase())) {
          return {
            message: `\`${field.tag}\` 的取值 \`${item}\` 非法（仅支持 present / absent / unknown）`,
            position: pos
          }
        }
      }
    } else if (field.kind === 'health') {
      for (const item of v.split(',').map((s) => s.trim()).filter(Boolean)) {
        if (!WOS_HEALTH_VALUES.includes(item.toLowerCase())) {
          return {
            message: `\`${field.tag}\` 的取值 \`${item}\` 非法（仅支持 ok / warning / error / missing / unchecked）`,
            position: pos
          }
        }
      }
    }

    this.conditions.push({ tag: field.tag, label: field.label, value: v, negated })
    return undefined
  }
}

function tokenText(t: Token): string {
  switch (t.t) {
    case 'field':
      return t.name + t.sep
    case 'word':
      return t.value
    case 'phrase':
      return `"${t.value}"`
    default:
      return t.t === 'lparen' ? '(' : t.t === 'rparen' ? ')' : t.t.toUpperCase()
  }
}

/** 数值 / 范围 / 带单位大小是否可解析 */
function parseNumeric(raw: string): boolean {
  const v = raw.trim()
  const m = /^(>=|<=|>|<|=)?\s*([\d.]+\s*(?:B|KB|MB|GB)?)(?:\s*-\s*([\d.]+\s*(?:B|KB|MB|GB)?))?$/i.exec(v)
  if (!m) return false
  const num = (s: string) => Number.parseFloat(s)
  if (!Number.isFinite(num(m[2]))) return false
  if (m[3] !== undefined && !Number.isFinite(num(m[3]))) return false
  return true
}

// ── 对外 API ────────────────────────────────────────────

/** 解析检索式：给出生效条件 + 语法错误（与服务端同一套规则） */
export function parseWosQuery(input: string): WosParseResult {
  const trimmed = (input || '').trim()
  if (!trimmed) return { ok: true, conditions: [] }

  const { tokens, error } = lex(trimmed)
  if (error) return { ok: false, error, conditions: [] }

  const parser = new Parser(tokens, trimmed)
  const err = parser.parse()
  if (err) {
    return {
      ok: false,
      error: { message: err.message, position: err.position },
      conditions: parser.conditions,
      sortBy: parser.sortBy,
      sortOrder: parser.sortOrder
    }
  }
  return {
    ok: true,
    conditions: parser.conditions,
    sortBy: parser.sortBy,
    sortOrder: parser.sortOrder
  }
}

function escapeHtml(s: string): string {
  return s
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;')
    .replace(/"/g, '&quot;')
}

// 注意**不要**在 `=` 后面加 `\b`：`TS=(…` 与 `SO="…"` 里 `=` 后面是非单词字符，
// 加了 `\b` 就匹配不上（括号组与引号短语的标签会漏高亮）。
const FIELD_TAG_RE = new RegExp(`\\b(${WOS_FIELDS.map((f) => f.tag).join('|')})=`, 'g')

/** 搜索框背景层高亮（返回 HTML，输入已经过转义） */
export function highlightWosQuery(input: string): string {
  if (!input) return ''
  let html = escapeHtml(input)
  // 字段标签（`TS=` / `TI=` …；冒号写法留给自由文本，避免误标 https://）
  html = html.replace(FIELD_TAG_RE, '<span class="hl-field">$1=</span>')
  // 引号短语（内容可能含 &amp; 等转义实体，不能用 [^&]*）
  html = html.replace(/(&quot;(?:(?!&quot;)[\s\S])*?&quot;|'[^']*?')/g, '<span class="hl-quote">$1</span>')
  // 布尔运算符
  html = html.replace(/\b(AND|OR|NOT)\b/g, '<span class="hl-op">$1</span>')
  // 括号
  html = html.replace(/([()])/g, '<span class="hl-paren">$1</span>')
  // 通配符
  html = html.replace(/([*?$])/g, '<span class="hl-wildcard">$1</span>')
  return html
}
