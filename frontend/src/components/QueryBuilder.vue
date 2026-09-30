<template>
  <div v-if="visible" class="qb-overlay" @click.self="close">
    <div class="qb-modal">
      <div class="qb-header">
        <h3>🔍 高级搜索构建器</h3>
        <button class="qb-close" @click="close" title="关闭">×</button>
      </div>

      <div class="qb-body">
        <!-- 自由文本搜索 -->
        <div class="qb-section">
          <label class="qb-label">自由文本</label>
          <input
            v-model="form.keyword"
            type="text"
            class="qb-input"
            placeholder="关键词（标题/作者/文件名）"
          />
        </div>

        <!-- 字段过滤 -->
        <div class="qb-grid">
          <div class="qb-section">
            <label class="qb-label">标题</label>
            <input v-model="form.title" type="text" class="qb-input" placeholder="标题关键词" />
          </div>
          <div class="qb-section">
            <label class="qb-label">作者</label>
            <input v-model="form.author" type="text" class="qb-input" placeholder="作者名" />
          </div>
          <div class="qb-section">
            <label class="qb-label">期刊</label>
            <input v-model="form.journal" type="text" class="qb-input" placeholder="期刊名" />
          </div>
          <div class="qb-section">
            <label class="qb-label">DOI</label>
            <input v-model="form.doi" type="text" class="qb-input" placeholder="10.1000/xyz123" />
          </div>
        </div>

        <!-- 年份范围 -->
        <div class="qb-section">
          <label class="qb-label">年份</label>
          <div class="qb-range">
            <input
              v-model="form.yearFrom"
              type="number"
              class="qb-input qb-input-sm"
              placeholder="起始"
              min="1900"
              :max="new Date().getFullYear()"
            />
            <span class="qb-range-sep">—</span>
            <input
              v-model="form.yearTo"
              type="number"
              class="qb-input qb-input-sm"
              placeholder="结束"
              min="1900"
              :max="new Date().getFullYear()"
            />
          </div>
        </div>

        <!-- 下拉选择 -->
        <div class="qb-grid">
          <div class="qb-section">
            <label class="qb-label">文献类型</label>
            <select v-model="form.docType" class="qb-select">
              <option value="">全部</option>
              <option value="article">article（期刊论文）</option>
              <option value="book">book（专著）</option>
              <option value="bookSection">bookSection（章节）</option>
              <option value="thesis">thesis（学位论文）</option>
              <option value="conferencePaper">conferencePaper（会议论文）</option>
              <option value="report">report（报告）</option>
              <option value="journalArticle">journalArticle（期刊文章）</option>
              <option value="preprint">preprint（预印本）</option>
              <option value="patent">patent（专利）</option>
              <option value="dataset">dataset（数据集）</option>
            </select>
          </div>
          <div class="qb-section">
            <label class="qb-label">处理状态</label>
            <select v-model="form.status" class="qb-select">
              <option value="">全部</option>
              <option value="uploaded">uploaded（已上传）</option>
              <option value="parsing">parsing（解析中）</option>
              <option value="markdown_done">done（解析完成）</option>
              <option value="error">error（出错）</option>
            </select>
          </div>
        </div>

        <!-- 排序 -->
        <div class="qb-grid">
          <div class="qb-section">
            <label class="qb-label">排序字段</label>
            <select v-model="form.sortBy" class="qb-select">
              <option value="">默认（更新时间）</option>
              <option value="year">year（年份）</option>
              <option value="title">title（标题）</option>
              <option value="authors">authors（作者）</option>
              <option value="id">id（ID）</option>
              <option value="created_at">created_at（创建时间）</option>
              <option value="updated_at">updated_at（更新时间）</option>
            </select>
          </div>
          <div class="qb-section">
            <label class="qb-label">排序方向</label>
            <select v-model="form.sortOrder" class="qb-select">
              <option value="desc">desc（降序）</option>
              <option value="asc">asc（升序）</option>
            </select>
          </div>
        </div>

        <!-- 生成的查询预览 -->
        <div class="qb-preview">
          <label class="qb-label">生成的查询</label>
          <div class="qb-preview-box">{{ generatedQuery || '（空）' }}</div>
        </div>
      </div>

      <div class="qb-footer">
        <button class="btn btn-primary" @click="apply" :disabled="!generatedQuery">
          ✅ 应用搜索
        </button>
        <button class="btn btn-outline" @click="close">取消</button>
      </div>
    </div>
  </div>
</template>

<script setup lang="ts">
import { ref, reactive, computed } from 'vue'

const visible = ref(false)

interface QueryForm {
  keyword: string
  title: string
  author: string
  journal: string
  doi: string
  yearFrom: string
  yearTo: string
  docType: string
  status: string
  sortBy: string
  sortOrder: string
}

const form = reactive<QueryForm>({
  keyword: '',
  title: '',
  author: '',
  journal: '',
  doi: '',
  yearFrom: '',
  yearTo: '',
  docType: '',
  status: '',
  sortBy: '',
  sortOrder: 'desc',
})

let resolveFn: ((query: string | null) => void) | null = null

/** 生成查询字符串 */
const generatedQuery = computed(() => {
  const parts: string[] = []

  // 自由文本
  if (form.keyword.trim()) {
    // 含空格时用引号包裹
    const kw = form.keyword.trim()
    parts.push(kw.includes(' ') ? `"${kw}"` : kw)
  }

  // 字段过滤
  if (form.title.trim()) parts.push(`title:${form.title.trim()}`)
  if (form.author.trim()) {
    const author = form.author.trim()
    parts.push(author.includes(' ') ? `author:"${author}"` : `author:${author}`)
  }
  if (form.journal.trim()) {
    const journal = form.journal.trim()
    parts.push(journal.includes(' ') ? `journal:"${journal}"` : `journal:${journal}`)
  }
  if (form.doi.trim()) parts.push(`doi:${form.doi.trim()}`)

  // 年份
  const yf = form.yearFrom.trim()
  const yt = form.yearTo.trim()
  if (yf && yt) {
    if (yf === yt) {
      parts.push(`year:${yf}`)
    } else {
      parts.push(`year:${yf}-${yt}`)
    }
  } else if (yf) {
    parts.push(`year:>=${yf}`)
  } else if (yt) {
    parts.push(`year:<=${yt}`)
  }

  // 文献类型
  if (form.docType) {
    parts.push(`type:${form.docType}`)
  }

  // 状态（使用 status: 语法）
  if (form.status) {
    parts.push(`status:${form.status}`)
  }

  // 排序
  if (form.sortBy) {
    parts.push(`sort:${form.sortBy}`)
    if (form.sortOrder) {
      parts.push(`order:${form.sortOrder}`)
    }
  }

  return parts.join(' ')
})

/** 打开构建器，可传入初始查询。返回生成的 query string 或 null（取消时）。 */
function show(initialQuery = ''): Promise<string | null> {
  // 解析初始查询填入表单
  resetForm()
  if (initialQuery.trim()) {
    parseInitialQuery(initialQuery.trim())
  }
  visible.value = true
  return new Promise((resolve) => {
    resolveFn = resolve
  })
}

function close() {
  visible.value = false
  resolveFn?.(null)
}

function apply() {
  visible.value = false
  resolveFn?.(generatedQuery.value)
}

function resetForm() {
  form.keyword = ''
  form.title = ''
  form.author = ''
  form.journal = ''
  form.doi = ''
  form.yearFrom = ''
  form.yearTo = ''
  form.docType = ''
  form.status = ''
  form.sortBy = ''
  form.sortOrder = 'desc'
}

/** 从现有查询字符串反填表单（简化实现，仅支持基本字段） */
function parseInitialQuery(q: string) {
  // 提取引号中的内容
  const extractQuoted = (s: string): string | null => {
    const m = s.match(/^"([^"]*)"$/)
    return m ? m[1] : null
  }

  const tokens = q.match(/(?:[^\s"]+|"[^"]*")+/g) || []

  for (const token of tokens) {
    if (token.startsWith('"') && token.endsWith('"')) {
      form.keyword = token.slice(1, -1)
      continue
    }

    if (token.includes(':')) {
      const colonIdx = token.indexOf(':')
      const prefix = token.slice(0, colonIdx)
      let value = token.slice(colonIdx + 1)

      // 去掉值中的引号
      if (value.startsWith('"') && value.endsWith('"')) {
        value = value.slice(1, -1)
      }

      switch (prefix) {
        case 'title':
          form.title = value
          break
        case 'author':
        case 'authors':
          form.author = value
          break
        case 'journal':
          form.journal = value
          break
        case 'doi':
          form.doi = value
          break
        case 'type':
        case 'doc_type':
          form.docType = value
          break
        case 'status':
          form.status = value
          break
        case 'sort':
          form.sortBy = value
          break
        case 'order':
          form.sortOrder = value
          break
        case 'year': {
          // year:2023, year:2020-2023, year:>2020, year:>=2020, etc.
          const rangeMatch = value.match(/^(>=?|<=?)?\s*(\d+)(?:\s*-\s*(\d+))?$/)
          if (rangeMatch) {
            if (rangeMatch[3]) {
              // 2020-2023
              form.yearFrom = rangeMatch[2]
              form.yearTo = rangeMatch[3]
            } else if (rangeMatch[1] === '>') {
              form.yearFrom = String(parseInt(rangeMatch[2], 10) + 1)
            } else if (rangeMatch[1] === '>=') {
              form.yearFrom = rangeMatch[2]
            } else if (rangeMatch[1] === '<') {
              form.yearTo = String(parseInt(rangeMatch[2], 10) - 1)
            } else if (rangeMatch[1] === '<=') {
              form.yearTo = rangeMatch[2]
            } else {
              // 精确年份
              form.yearFrom = rangeMatch[2]
              form.yearTo = rangeMatch[2]
            }
          }
          break
        }
      }
    } else {
      // 自由文本
      form.keyword = token
    }
  }
}

defineExpose({ show })
</script>

<style scoped>
/* ── 遮罩层 ──────────────────────────────────────────── */
.qb-overlay {
  position: fixed;
  inset: 0;
  z-index: 1000;
  background: rgba(0, 0, 0, 0.45);
  display: flex;
  align-items: center;
  justify-content: center;
  padding: 20px;
}

/* ── 模态框 ──────────────────────────────────────────── */
.qb-modal {
  background: var(--bg-white, #fff);
  border-radius: 12px;
  width: 100%;
  max-width: 600px;
  max-height: calc(100vh - 40px);
  display: flex;
  flex-direction: column;
  box-shadow: 0 16px 48px rgba(0, 0, 0, 0.2);
  overflow: hidden;
}

.qb-header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 16px 20px 12px;
  border-bottom: 1px solid var(--border, #e0e0e0);
}

.qb-header h3 {
  margin: 0;
  font-size: 16px;
  font-weight: 600;
}

.qb-close {
  background: none;
  border: none;
  font-size: 22px;
  color: var(--text-secondary, #666);
  cursor: pointer;
  padding: 0 4px;
  line-height: 1;
  border-radius: 4px;
}
.qb-close:hover {
  background: var(--bg, #f5f5f5);
  color: var(--text, #333);
}

/* ── 表单 ────────────────────────────────────────────── */
.qb-body {
  padding: 16px 20px;
  overflow-y: auto;
  flex: 1;
  display: flex;
  flex-direction: column;
  gap: 14px;
}

.qb-section {
  display: flex;
  flex-direction: column;
  gap: 4px;
}

.qb-label {
  font-size: 12px;
  font-weight: 600;
  color: var(--text-secondary, #666);
  text-transform: uppercase;
  letter-spacing: 0.3px;
}

.qb-input {
  padding: 8px 12px;
  border: 1px solid var(--border, #e0e0e0);
  border-radius: 6px;
  font-size: 14px;
  background: var(--bg-white, #fff);
  color: var(--text, #333);
  outline: none;
  transition: border-color 0.15s;
  font-family: inherit;
}
.qb-input:focus {
  border-color: var(--primary, #1976d2);
  box-shadow: 0 0 0 2px rgba(25, 118, 210, 0.1);
}
.qb-input-sm {
  width: 120px;
}

.qb-select {
  padding: 8px 12px;
  border: 1px solid var(--border, #e0e0e0);
  border-radius: 6px;
  font-size: 14px;
  background: var(--bg-white, #fff);
  color: var(--text, #333);
  outline: none;
  cursor: pointer;
  font-family: inherit;
}
.qb-select:focus {
  border-color: var(--primary, #1976d2);
  box-shadow: 0 0 0 2px rgba(25, 118, 210, 0.1);
}

/* ── 网格布局 ────────────────────────────────────────── */
.qb-grid {
  display: grid;
  grid-template-columns: 1fr 1fr;
  gap: 12px;
}

/* ── 年份范围 ────────────────────────────────────────── */
.qb-range {
  display: flex;
  align-items: center;
  gap: 8px;
}
.qb-range-sep {
  color: var(--text-secondary, #666);
  font-size: 14px;
}

/* ── 预览 ────────────────────────────────────────────── */
.qb-preview {
  margin-top: 4px;
}

.qb-preview-box {
  padding: 10px 12px;
  background: var(--bg, #f5f5f5);
  border: 1px solid var(--border, #e0e0e0);
  border-radius: 6px;
  font-size: 13px;
  font-family: 'SF Mono', 'Fira Code', 'Cascadia Code', monospace;
  color: var(--primary, #1976d2);
  word-break: break-all;
  min-height: 36px;
  line-height: 1.5;
}

/* ── 按钮栏 ──────────────────────────────────────────── */
.qb-footer {
  display: flex;
  align-items: center;
  justify-content: flex-end;
  gap: 8px;
  padding: 12px 20px 16px;
  border-top: 1px solid var(--border, #e0e0e0);
}

/* ── 响应式 ──────────────────────────────────────────── */
@media (max-width: 480px) {
  .qb-grid {
    grid-template-columns: 1fr;
  }
  .qb-modal {
    max-height: calc(100vh - 20px);
  }
  .qb-overlay {
    padding: 10px;
  }
}
</style>