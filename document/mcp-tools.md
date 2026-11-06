# OKB-Assist MCP 工具列表

OKB-Assist MCP 服务器提供以下工具，供 AI 助手（如 CodeBuddy、Claude Desktop、Codex、Pi Agent、OpenCode、Cursor 等）调用。

> **注意**：`grep_search` 底层为纯 Rust 实现（`memchr` + `regex` crate），**不是**系统 `grep` 命令。正则语法为 Rust `regex` crate 风格（PCRE-like），与传统 BRE 有所不同。

## 工具总览

| 工具名称 | 功能 | 核心参数 |
|---------|------|---------|
| `grep_search` | 全文搜索（纯 Rust 实现，轻量快速），支持正则/字面量、**游标分页**、上下文截断、期刊/年份/文档范围过滤 | `query`, `regex`, `algorithm`, `doc_ids`, `context`, `start_after_id`, `journal`, `year_start`, `year_end` |
| `search_info` | 搜索文献元数据（中英文），支持分页、字段过滤、年份/类型过滤、排序 | `query`, `fields`, `year_from`, `year_to`, `doc_type`, `sort_by`, `sort_order` |
| `read_markdown` | 读取文献 Markdown 内容（行切片分页），或按标题提取指定章节 | `id`, `line_start`, `line_count`, `section`, `sections` |
| `get_document_info` | 获取文献详细信息（含元数据、状态、PDF/Markdown 链接） | `id` |
| `list_documents` | 搜索/列出文献，支持标题/作者搜索、**文件事实过滤**（`md_state`/`pdf_state`/`zip_state`/`synced`/`has_*`/`pdf_health`）与类型过滤、字段过滤、排序 | `query`, `doc_type`, `md_state`, `pdf_state`, `zip_state`, `synced`, `has_md`, `has_pdf`, `has_zip`, `pdf_health`, `page`, `page_size`, `fields`, `sort_by`, `sort_order` |
| `get_document_abstract` | 获取文献摘要（中英文） | `id` |
| `get_stats` | 获取知识库统计信息（总数、状态分布、类型分布） | 无 |
| `list_doc_types` | 列出所有已使用的文献类型 | 无 |
| `get_toc` | 获取文献目录（标题+行号），不受分页影响 | `id` |
| `edit_markdown` 🔒 | **编辑**文献 Markdown 源码（按行范围替换/插入/删行，支持 `expected_text` 校验与 `dry_run` 预览） | `id`, `line_start`, `line_count`, `content`, `delete`, `expected_text`, `base_hash`, `dry_run` |
| `replace_markdown` 🔒 | **整篇覆盖**文献 Markdown（要求显式提供非空内容） | `id`, `content`, `base_hash`, `dry_run` |
| `find_replace_markdown` 🔒 | Markdown 字面量查找替换（默认要求唯一命中，`all=true` 替换全部） | `id`, `find`, `replace`, `all`, `base_hash`, `dry_run` |

> 🔒 标记的工具是**写操作**，仅 `admin` 角色的调用方可见且可用：非 admin 的 `tools/list` 里不会出现，直接调用会返回 `forbidden` 错误。

---

## 工具详细说明

### `grep_search` — 全文搜索

对文献 Markdown 源文件执行全文搜索。纯 Rust 内部实现，无需外部进程，无需向量库索引。

**特性：**
- 结果按文档 ID **升序**排列，分页稳定可预测
- 使用 **游标分页**（cursor-based）：通过 `start_after_id` 实现高效深分页
- 每次请求从上次返回的 `last_id` 之后继续扫描，无需重复扫描

**参数：**

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `query` | string | **必填** | 搜索关键词或正则表达式 |
| `regex` | bool | `true` | 是否将 `query` 解释为正则表达式；`false` 时为字面量精确匹配 |
| `algorithm` | string | `"full"` | 搜索算法：`"full"`（全量扫描）或 `"fast"`（元数据预筛候选后扫描） |
| `limit` | int | `10` | 每页返回条数（别名：`max_results`） |
| `max_results` | int | `10` | `limit` 的别名 |
| `start_after_id` | int | `0` | **游标**：仅返回 ID > 此值的文档。首次请求可不传（从头开始）；后续请求传上一次响应的 `last_id` 即可获取下一页 |
| `context` | int | `2` | 匹配行前后各保留的上下文行数 |
| `max_context_chars` | int | `500` | 每条结果 `content` 的最大字符数；`0` 表示不截断 |
| `doc_ids` | string | `""` | 限定搜索的文档 ID，支持逗号分隔和范围（如 `"1,2,5-100"`） |
| `journal` | string | `""` | 按期刊名模糊过滤（忽略大小写） |
| `year_start` | int | `0` | 起始年份（>0 时生效） |
| `year_end` | int | `0` | 结束年份（>0 时生效） |

**返回格式：**

```json
{
  "query": "transformer",
  "limit": 10,
  "returned": 5,
  "last_id": 52,
  "results": [
    {
      "id": 42,
      "content": "...matching lines with context...",
      "truncated": false,
      "original_length": 320,
      "title": "Attention Is All You Need",
      "authors": "Vaswani et al.",
      "year": 2017,
      "journal": "NeurIPS"
    }
  ]
}
```

> **游标分页说明**：
> - `last_id` 为本次返回结果中最大的文档 ID；若为 `null`，表示已扫描完全部文档，没有更多结果
> - 获取下一页：将下次请求的 `start_after_id` 设为上次响应的 `last_id` 即可
> - 无需 `offset`、`page`、`has_more` 等传统分页字段
> - 深分页场景下无需从头扫描，性能恒定（O(limit)）
>
> **排序保证**：结果始终按文档 ID **升序**排列，无论是否指定 `doc_ids`。分页结果稳定可预测，不受文件系统差异或文档增删影响。

---

### `grep_search` 的 grep 语法详解

`grep_search` 的 `query` 参数支持两种匹配模式，由 `regex` 参数控制：

#### 一、正则模式（`regex=true`，默认）

使用 Rust `regex` crate，正则表达式语法为 **PCRE-like** 风格（与 Perl/Python `re` 模块类似），**不是**传统 BRE（基本正则表达式）。

**核心差异（与 BRE 的区别）：**

| 字符 | BRE 中 | Rust regex 中 |
|------|--------|---------------|
| `+` | 字面量，需 `\+` 才表示"1次及以上" | **元字符**，直接表示"1次及以上" |
| `?` | 字面量，需 `\?` 才表示"0或1次" | **元字符**，直接表示"0或1次" |
| `\|` | 表示"或" | **`|`** 直接表示"或" |
| `(` `)` | 字面量，需 `\(` `\)` 才表示分组 | **元字符**，直接表示分组 |
| `{` `}` | 字面量，需 `\{` `\}` 才表示重复 | **元字符**，直接表示重复 |
| `\d` | 不支持 | **支持**，表示数字 `[0-9]` |
| `\w` | 不支持 | **支持**，表示单词字符 `[a-zA-Z0-9_]` |
| `\s` | 不支持 | **支持**，表示空白字符 |
| `.` | 匹配任意字符 | 匹配任意字符（换行符除外） |

**常用正则语法表：**

| 语法 | 含义 | 示例 |
|------|------|------|
| `abc` | 字面匹配（忽略大小写） | `transformer` |
| `.` | 匹配任意单个字符（换行除外） | `a.c` → `abc`, `a1c` |
| `*` | 前一元素出现 0 次或多次 | `a*` → `""`, `a`, `aa` |
| `+` | 前一元素出现 1 次或多次 | `a+` → `a`, `aa`（不匹配空串） |
| `?` | 前一元素出现 0 或 1 次 | `a?` → `""`, `a` |
| `[abc]` | 字符类，匹配 a/b/c 之一 | `[aeiou]` 匹配元音 |
| `[a-z]` | 字符范围 | `[0-9]` 匹配数字 |
| `[^abc]` | 取反字符类 | `[^0-9]` 匹配非数字 |
| `^` | 行首锚定 | `^Introduction` |
| `$` | 行尾锚定` | `方法$` |
| `\|` 或 `|` | 逻辑或 | `cat\|dog` 或 `cat|dog` |
| `(...)` | 分组 | `(ab)+` → `ab`, `abab` |
| `{m,n}` | 重复 m 到 n 次 | `a{2,4}` → `aa`, `aaa`, `aaaa` |
| `{m,}` | 重复至少 m 次 | `a{2,}` → `aa`, `aaa`, ... |
| `{m}` | 精确 m 次 | `a{3}` → `aaa` |
| `\d` | 数字字符 | `\d{4}-\d{2}-\d{2}` 日期格式 |
| `\w` | 单词字符 | `\w+` 匹配标识符 |
| `\s` | 空白字符 | `\s+` 匹配一个或多个空白 |
| `(?i)` | 内联大小写忽略标记 | 已由全局 `(?i)` 自动添加 |

**注意：**

- 匹配 `.`、`*`、`+`、`?`、`|`、`(`、`)`、`[`、`]`、`{`、`}`、`\`、`^`、`$` 等元字符本身时需用 `\` 转义（如 `\.`），或改用 `regex=false` 字面模式。
- Rust `regex` crate **不支持** lookahead/lookbehind（`(?=...)`、`(?<=...)`等），也不支持 backreference（`\1`）。
- 所有匹配**始终忽略大小写**（内部使用 `(?i)` 标志），无需手动指定。

#### 二、字面模式（`regex=false`）

`query` 中的所有字符按原义处理，不做正则解释。适合搜索包含正则元字符的精确文本。

**示例：**

| 意图 | query | regex | 说明 |
|------|-------|-------|------|
| 搜包含 `(A+B)` 的行 | `(A+B)` | `false` | 字面模式，括号无需转义 |
| 搜包含 `a*b` 的行 | `a*b` | `false` | 字面模式，`*` 无需转义 |

#### 搜索算法：`full` vs `fast`

| 算法 | 说明 | 适用场景 |
|------|------|---------|
| `full`（默认） | 扫描所有文档的 Markdown 文件 | 精确搜索，确保不遗漏 |
| `fast` | 先通过元数据（标题/摘要/关键词/作者）预筛候选文档，再扫描候选文件的 Markdown | 大规模库中快速定位可能相关的文档 |

> `fast` 模式仅在**未指定 `doc_ids`** 时生效；指定 `doc_ids` 后自动回退到 `full` 模式。

#### `doc_ids` 范围语法

`doc_ids` 支持逗号分隔和连字符范围：

```
1,2,5-100,4
```

- 单个 ID：`42`
- 范围：`10-20`（包含两端，自动按大小排序）
- 混合：`1,3,5-10,50`

---

### `search_info` — 元数据搜索

搜索文献的元数据字段（标题、作者、期刊、关键词、摘要、DOI 等），支持中英文。基于 SQL `LIKE` 模糊匹配。

**参数：**

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `query` | string | **必填** | 搜索关键词 |
| `limit` | int | `10` | 每页条数（别名：`max_results`） |
| `max_results` | int | `10` | `limit` 的别名 |
| `page` | int | `1` | 页码（1-based） |
| `offset` | int | `0` | 偏移量（0-based），提供时覆盖 `page` |
| `year_from` | int | `0` | 起始年份过滤 |
| `year_to` | int | `0` | 结束年份过滤 |
| `doc_type` | string | `""` | 文献类型过滤（逗号分隔多值） |
| `fields` | string[] | — | 字段白名单，如 `["id","title","year"]`；省略返回全部字段 |
| `sort_by` | string | `"updated_at"` | 排序字段 |
| `sort_order` | string | `"desc"` | 排序方向：`"asc"` 或 `"desc"` |

**可排序字段：** `title`, `authors`, `year`, `id`, `journal`, `doc_type`, `created_at`, `updated_at`, `pdf_size`, `md_size`, `zip_size`

**返回格式：**

```json
{
  "query": "transformer",
  "total": 15,
  "page": 1,
  "limit": 10,
  "total_pages": 2,
  "results": [
    {
      "id": 42,
      "title": "Attention Is All You Need",
      "authors": "Vaswani et al.",
      "year": 2017,
      "journal": "NeurIPS",
      ...
    }
  ]
}
```

---

### `read_markdown` — 读取 Markdown

读取文献的 Markdown 源文件，支持两种模式：

1. **分页模式**（默认）：按行切片返回，`line_start`（0-based）+ `line_count` 控制范围
2. **章节提取模式**：指定 `section` 或 `sections` 参数，按标题提取章节内容

**参数：**

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `id` | int | **必填** | 文档 ID |
| `line_start` | int | `0` | 起始行号（0-based） |
| `line_count` | int | `5000` | 返回行数 |
| `section` | string | — | 按标题提取单个章节（不区分大小写，支持子串匹配） |
| `sections` | string[] | — | 按标题提取多个章节 |

返回体中的 `hash` 为文件 sha256，供写类工具的 `base_hash` 使用。

**章节匹配规则：**

- 不区分大小写
- 支持子串匹配：`"Intro"` 可匹配 `"Introduction"`
- 自动包含匹配章节的所有子章节（级别更深的后续标题）
- 未匹配时返回 `error` 及 `available_sections`（顶层标题列表供参考）
- 章节提取优先于分页模式

**返回格式（分页模式）：**

```json
{
  "id": 42,
  "title": "Attention Is All You Need",
  "line_start": 0,
  "line_count": 5000,
  "lines_returned": 320,
  "total_lines": 850,
  "hash": "31ab…",
  "content": "# Title\n\nFirst paragraph..."
}
```

**返回格式（章节提取模式）：**

```json
{
  "id": 42,
  "title": "Attention Is All You Need",
  "section": "Introduction",
  "matched_sections": ["Introduction"],
  "hash": "31ab…",
  "content": "# Introduction\n\nIn this paper..."
}
```

> `hash` 是文件真实字节的 sha256，可直接作为写工具的 `base_hash` 传入（两种模式都会返回）。

---

### `get_document_info` — 文档详细信息

获取文献的完整信息，包括元数据、处理状态、PDF/Markdown 链接。

**参数：** `id`（int，必填）

**返回字段：**

| 字段 | 说明 |
|------|------|
| `id` | 文档 ID |
| `filename` | 原始文件名 |
| `title` | 标题 |
| `authors` | 作者 |
| `year` | 年份 |
| `doi` | DOI |
| `journal` | 期刊 |
| `keywords` | 关键词 |
| `abstract` | 摘要 |
| `category` | 分类 |
| `doc_type` | 文献类型 |
| `language` | 语言 |
| `status` | 派生状态（`uploaded` / `parsing` / `markdown_done` / `error`） |
| `status_message` | 派生状态的说明文案 |
| `synced` | 文件事实是否已从磁盘同步过（`false` = 状态未知） |
| `has_markdown` / `has_pdf` / `has_asset` | 三个文件是否「已同步且存在」 |
| `pdf_health` / `pdf_size` / `md_size` | PDF 健康与文件大小（来自同步列） |
| `pdf_need_auth_url` | 需要认证的 PDF 下载链接 |
| `temp_downloadable_pdf_url` | 无需认证的临时 PDF 下载链接（有效至 `temp_downloadable_pdf_expiration`） |
| `temp_downloadable_pdf_expiration` | 临时链接过期时间 |
| `markdown_url` | Markdown 阅读链接 |
| `detail_url` | 详情页链接 |

---

### `list_documents` — 列出文献

搜索或列出文献，支持标题/作者模糊搜索、**文件事实过滤**（取代原 `status` 参数）与类型过滤、字段白名单、排序。

**参数：**

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `query` | string | `""` | 搜索关键词（匹配标题、作者、文件名） |
| `doc_type` | string | `""` | 文献类型过滤（逗号分隔多值） |
| `md_state` / `pdf_state` / `zip_state` | string | `""` | 文件三态：`present` / `absent` / `unknown`（逗号分隔多值） |
| `synced` | string | `""` | `1` = 文件事实已同步，`0` = 未同步（状态未知） |
| `has_md` / `has_pdf` / `has_zip` | string | `""` | `1` 有 / `0` 无（不含未同步语义，需精确用三态） |
| `pdf_health` | string | `""` | `ok` / `warning` / `error` / `missing` / `unchecked`（逗号分隔多值） |
| `page` | int | `1` | 页码 |
| `page_size` | int | `20` | 每页条数（1-100，别名：`limit`） |
| `limit` | int | `20` | `page_size` 的别名 |
| `fields` | string[] | — | 字段白名单 |
| `sort_by` | string | `"created_at"` | 排序字段 |
| `sort_order` | string | `"desc"` | 排序方向 |

**可排序字段：** `title`, `authors`, `year`, `id`, `journal`, `doc_type`, `created_at`, `updated_at`, `pdf_size`, `md_size`, `zip_size`

**返回格式：**

```json
{
  "total": 100,
  "page": 1,
  "page_size": 20,
  "total_pages": 5,
  "items": [
    {
      "id": 1,
      "title": "...",
      ...
    }
  ]
}
```

---

### `get_document_abstract` — 文献摘要

获取文献摘要信息，中英文摘要同时返回（如果存在）。

**参数：** `id`（int，必填）

**返回格式：**

```json
{
  "id": 42,
  "title": "Attention Is All You Need",
  "abstract": "中文摘要...",
  "abstract_en": "English abstract...",
  "language": "zh"
}
```

---

### `get_stats` — 知识库统计

获取知识库统计信息，无需参数。

**返回格式：**

```json
{
  "total_documents": 150,
  "status_counts": {
    "indexed": 120,
    "markdown_done": 10,
    "parsing": 5,
    "uploaded": 10,
    "error": 5
  },
  "type_counts": {
    "journalArticle": 80,
    "conferencePaper": 40,
    "book": 20,
    "thesis": 10
  },
  "indexed_count": 120
}
```

---

### `list_doc_types` — 文献类型列表

列出知识库中所有已使用的文献类型（Zotero 标准类型），无需参数。

**返回格式：**

```json
{
  "doc_types": ["journalArticle", "conferencePaper", "book", "thesis"]
}
```

---

### `get_toc` — 文档目录

从 Markdown 源文件提取目录（标题 + 级别 + 行号），不受分页影响。

**参数：** `id`（int，必填）

**返回格式：**

```json
{
  "id": 42,
  "title": "Attention Is All You Need",
  "total_headings": 15,
  "toc": [
    {"level": 1, "title": "Introduction", "line": 0},
    {"level": 2, "title": "Background", "line": 5},
    {"level": 1, "title": "Related Work", "line": 12},
    ...
  ]
}
```

---

### `edit_markdown` — 编辑 Markdown（按行范围）

> 🔒 **仅 admin**。语义与 Web 端「源码编辑」的按行替换接口（`PUT /assist/api/documents/:id/markdown/lines`）完全一致。

直接读写 Markdown **源文件**：写入的是文件真实内容，**不做图片路径重写/还原**（与 `read_markdown` 返回的内容严格互逆，读出来原样写回去字节不变）。

**参数：**

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `id` | int | **必填** | 文档 ID |
| `line_start` | int | **必填** | 起始行（0-based，含端点），超出总行数时截断到文件末尾 |
| `line_count` | int | `0` | 被替换的现有行数；`0` = 纯插入 |
| `content` | string | `""` | 替换文本；新行数 = `content.split('\n').count()`（空串等于 **1 个空行**，不是删行） |
| `delete` | bool | `false` | `true` = 直接删除 `[line_start, line_start + line_count)` 这些行（忽略 `content`） |
| `expected_text` | string | — | 目标行范围当前应当等于的**精确文本**；不一致时不写入，返回实际内容与 `suggested_line_start` |
| `base_hash` | string | — | 乐观并发：之前 `read_markdown` / 编辑响应返回的 `hash`；不一致返回 `conflict` |
| `dry_run` | bool | `false` | 只预览不写盘，返回 `old_content` / `new_content` |

**推荐流程：**

1. `read_markdown` 读取目标行，记下 `content` 与 `hash`（或先 `dry_run` 预览）；
2. 编辑时同时传 `expected_text`（要替换的那几行原文，防止行号错位）与 `base_hash`（防止并发覆盖）；
3. 用响应里的 `hash` 作为下一次编辑的 `base_hash`。

**返回格式：**

```json
{
  "id": 42,
  "title": "Attention Is All You Need",
  "detail": "Markdown 已更新",
  "action": "replace",
  "changed": true,
  "line_start": 12,
  "line_count": 2,
  "lines_written": 3,
  "total_lines": 851,
  "md_size": 48211,
  "hash": "9f2c…",
  "previous_hash": "31ab…"
}
```

`action` 取值 `replace` / `insert` / `delete`。内容与原文件一致时返回 `changed: false` 且不写盘。

**`expected_text` 不匹配时的返回（未写入）：**

```json
{
  "error": {"code": "mismatch", "message": "expected_text 与目标行范围的实际内容不一致，未写入。…"},
  "line_start": 12,
  "line_count": 2,
  "actual_content": "12 行起的真实内容…",
  "suggested_line_start": 87
}
```

---

### `replace_markdown` — 整篇覆盖 Markdown

> 🔒 **仅 admin**。会**替换整个文件**，只在「改写大部分内容」时使用；局部修改请用 `edit_markdown` / `find_replace_markdown`。

**参数：**

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `id` | int | **必填** | 文档 ID |
| `content` | string | **必填** | 新的完整 Markdown 内容；**不允许为空**（清空请用 `edit_markdown` + `delete`） |
| `base_hash` | string | — | 乐观并发校验，不一致返回 `conflict` |
| `dry_run` | bool | `false` | 只预览不写盘 |

**返回格式：**

```json
{
  "id": 42,
  "title": "Attention Is All You Need",
  "detail": "Markdown 已整篇覆盖",
  "action": "replace_all",
  "changed": true,
  "total_lines": 1200,
  "md_size": 51200,
  "hash": "b7e1…",
  "previous_hash": "31ab…"
}
```

---

### `find_replace_markdown` — 字面量查找替换

> 🔒 **仅 admin**。区分大小写的**字面量**替换（不是正则）。

比行号更稳的改法：把 `read_markdown` 里看到的一小段原文当作 `find` 直接替换。

**参数：**

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `id` | int | **必填** | 文档 ID |
| `find` | string | **必填** | 要查找的字面量文本（非空） |
| `replace` | string | `""` | 替换文本；空串 = 删除命中的文本 |
| `all` | bool | `false` | `false` 时要求全文**恰好命中 1 处**；`true` 时替换全部（上限 200 处，超出返回 `too_many`） |
| `base_hash` | string | — | 乐观并发校验，不一致返回 `conflict` |
| `dry_run` | bool | `false` | 只预览不写盘 |

**返回格式：**

```json
{
  "id": 42,
  "title": "Attention Is All You Need",
  "detail": "Markdown 已更新",
  "action": "find_replace",
  "changed": true,
  "occurrences": 1,
  "replaced": 1,
  "matches": [{"line": 87, "text": "该方法在 3 个数据集上取得最好效果。"}],
  "total_lines": 850,
  "md_size": 48190,
  "hash": "c1d4…",
  "previous_hash": "31ab…"
}
```

**多命中时的返回（未写入）：**

```json
{
  "error": {"code": "ambiguous", "message": "find 命中 3 处，未写入。请给出更长的唯一文本，或传 all=true 替换全部（上限 200 处）"},
  "occurrences": 3,
  "matches": [{"line": 12, "text": "…"}, {"line": 87, "text": "…"}, {"line": 201, "text": "…"}],
  "hash": "31ab…"
}
```

> 编辑只改 Markdown 文件本身（并同步 `md_size`）：不会自动重新解析 PDF，也不会重建向量索引。

---

## 通用参数说明

### 分页

`search_info`、`list_documents` 使用传统 offset 分页：

- `page`（1-based 页码）与 `offset`（0-based 偏移量）同时提供时 **`offset` 优先**
- `limit` / `max_results` / `page_size` 为每页条数别名
- 返回 `total` + `total_pages`

`grep_search` 使用**游标分页**（cursor-based）：

- 通过 `start_after_id` 参数指定游标，仅返回 ID > 此值的文档
- 响应返回 `last_id`（本次结果中最大的文档 ID），传给下次请求的 `start_after_id` 即可获取下一页
- `last_id` 为 `null` 时表示已扫完全部，没有更多结果
- 结果始终按 ID 升序排列，分页稳定可预测

### 字段过滤

`search_info`、`list_documents` 支持 `fields` 参数（字符串数组），只返回指定字段；省略时返回全部字段。

### 统一错误格式

工具出错时返回：

```json
{"error": {"code": "...", "message": "..."}}
```

常见错误码：`invalid_argument`、`not_found`、`conflict`（`base_hash` 不匹配）、`mismatch`（`expected_text` 不一致）、`ambiguous`（`find` 多命中）、`too_many`、`forbidden`（写类工具需要 admin）、`write_failed`。