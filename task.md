我运行了`data\run.sh`.并让用户检查了一下调用情况。
请帮我根据用户的建议优化。

---


测试完成，以下是完整的测试报告和优化建议。

---

## okb-assist MCP 工具测试报告 & 优化建议

### 一、当前状态总览

9 个工具全部可用，核心功能正常。参数映射修复（`id` 统一）已生效。

| 工具 | 状态 | 说明 |
|------|:--:|------|
| `get_stats` | ✅ | 返回知识库统计 |
| `list_doc_types` | ✅ | 14 种文档类型 |
| `list_documents` | ✅ | 分页列表，支持 query 过滤 |
| `search_info` | ✅ | 元数据搜索，最多返回 10 条 |
| `grep_search` | ✅ | 全文搜索，最多返回 10 条 |
| `get_document_abstract` | ✅ | 获取摘要 |
| `get_document_info` | ✅ | 获取完整元数据 |
| `read_markdown` | ✅ | 分页读取 Markdown 全文 |
| `get_pdf_url` | ✅ | 获取 PDF 下载链接 |

---

### 二、发现的问题

#### 🔴 问题 1：`list_documents` 的 `page_size` 参数无效

```
传入 {"page_size": 5}  →  仍然返回 20 条/页
```

`page_size` 被忽略，始终固定为 20。这导致知识库 3,592 篇文档需要翻 180 页。

#### 🔴 问题 2：搜索工具返回上限过低且不可配置

- `search_info` 最多返回 **10 条**，无分页
- `grep_search` 最多返回 **10 条**，无分页
- 对于 3,592 篇文档的知识库，10 条结果的覆盖率太低

#### 🟡 问题 3：`grep_search` 返回内容过于臃肿

每条结果的 `content` 字段包含：
- 原始 LaTeX 数学公式（未渲染）
- HTML 实体（如 `&#x2013;`）
- 完整的参考文献列表
- 有时是整段论文内容而非片段

导致单次响应可能非常大，且噪声高。

#### 🟡 问题 4：中文搜索能力不一致

| 工具 | 中文查询 | 结果 |
|------|---------|------|
| `search_info` | "无人机协同控制编队飞行" | 0 条 |
| `grep_search` | "编队控制" | 10 条（正常） |

`search_info` 似乎只检索英文元数据（标题/作者），对中文支持差；`grep_search` 全文检索中文正常。两者行为不一致。

#### 🟡 问题 5：`list_documents` 响应字段过多

每条文档返回 **15 个字段**（abstract, authors, category, detail_url, doc_type, doi, filename, has_markdown, id, journal, keywords, language, markdown_url, pdf_url, status, title, year），其中很多字段对搜索结果无用（如 `detail_url`, `markdown_url`, `pdf_url`, `filename`）。

#### 🟡 问题 6：`search_info` 的 `limit` 参数未明确

文档未说明 `limit` 是否生效，测试中传入 `{"limit": 3}` 似乎未限制结果数（仍返回 10 条中的前几条）。

---

### 三、优化建议

#### 优先级 P0 — 功能修复

| # | 建议 | 说明 |
|---|------|------|
| 1 | **修复 `list_documents` 的 `page_size`** | 使 `page_size` 参数生效，允许调用方控制每页条数（建议 10-100） |
| 2 | **为搜索工具增加分页** | `search_info` 和 `grep_search` 增加 `page` / `offset` 参数，突破 10 条限制 |
| 3 | **统一中英文搜索行为** | `search_info` 应支持中文标题/关键词检索，或在文档中明确标注支持的语言 |

#### 优先级 P1 — 效率提升

| # | 建议 | 说明 |
|---|------|------|
| 4 | **增加字段过滤参数** | `list_documents` 和 `search_info` 支持 `fields` 参数，如 `{"fields": ["id", "title", "year"]}`，只返回必要字段 |
| 5 | **`grep_search` 增加上下文截断** | 每条 content 限制在 200-500 字符，截断而非返回整段文献；保留 `context_before` / `context_after` 参数 |
| 6 | **增加 `max_results` 参数** | 允许调用方显式控制返回条数，而非固定 10 条 |

#### 优先级 P2 — 体验优化

| # | 建议 | 说明 |
|---|------|------|
| 7 | **增加按年份/类型过滤** | `search_info` 增加 `year_from`, `year_to`, `doc_type` 等过滤参数 |
| 8 | **`grep_search` 返回结构化片段** | 将 content 拆分为 `snippet`（精简摘要）+ `full_match`（完整匹配段），默认只返回 snippet |
| 9 | **增加 `search_combined` 统一搜索** | 合并 `search_info` 和 `grep_search`，一次调用同时返回元数据匹配和全文匹配结果，按相关性排序 |
| 10 | **`read_markdown` 增加章节过滤** | 支持 `section` 参数，只返回指定章节（如只返回 Abstract + Introduction），减少传输量 |

#### 优先级 P3 — 错误处理增强

| # | 建议 | 说明 |
|---|------|------|
| 11 | **统一错误响应格式** | 所有工具的 error 返回统一为 `{"error": {"code": "...", "message": "..."}}`，包含错误码 |
| 12 | **增加速率限制提示** | 当调用频率过高时返回 `429` 风格的提示而非静默失败 |

---

### 四、参数一致性检查

修复后所有工具参数已统一为 `id`：

```
search_info       →  query (string)
grep_search       →  query (string)
list_documents    →  page, limit, query (all optional)
get_document_abstract →  id (int)
get_document_info     →  id (int)
read_markdown         →  id (int), page (int)
get_pdf_url           →  id (int)
get_stats             →  (无参数)
list_doc_types        →  (无参数)
```

✅ 无已知参数名冲突。