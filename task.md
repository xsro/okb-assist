我运行了`data\run.sh`.
但是发现mcp工具有问题，帮我排查

---

## MCP 工具参数映射故障报告

### 故障现象

以下 4 个文档操作工具在传入正确文档 ID 时，均返回 `{"error": "文档 0 不存在"}`：

| 工具 | 传入参数 | 返回 |
|------|----------|------|
| `get_document_abstract` | `{"id": 398}` | `{"error": "文档 0 不存在"}` |
| `get_document_info` | `{"id": 398}` | `{"error": "文档 0 不存在"}` |
| `read_markdown` | `{"id": 398, "page": 1}` | `{"error": "文档 0 不存在"}` |
| `get_pdf_url` | `{"id": 398}` | `{"error": "文档 0 不存在"}` |

### 根因：参数名不一致

这些工具期望的参数名是 **`doc_id`**，而非 `id`。当传入 `id` 时，工具将其忽略，`doc_id` 默认为 `0`，因此报错"文档 0 不存在"。

**正确调用方式**（可正常工作）：

```
get_document_abstract  →  {"doc_id": 398}  ✅ 正常返回
get_document_info       →  {"doc_id": 398}  ✅ 正常返回
read_markdown           →  {"doc_id": 398, "page": 1}  ✅ 正常返回
get_pdf_url             →  {"doc_id": 398}  ✅ 正常返回
```

### 涉及的工具及参数对照

| 工具名 | 返回结果中的 ID 字段名 | 期望的输入参数名 |
|--------|----------------------|-----------------|
| `search_info` | `id` | —（仅搜索） |
| `list_documents` | `id` | —（仅列表） |
| `grep_search` | `document_id` ⚠️ | —（仅搜索） |
| `get_document_abstract` | — | **`doc_id`** |
| `get_document_info` | — | **`doc_id`** |
| `read_markdown` | — | **`doc_id`** |
| `get_pdf_url` | — | **`doc_id`** |

### 额外的不一致：grep_search 的字段名

`grep_search` 返回的结果中，文档 ID 的字段名是 `document_id`，而 `search_info` 和 `list_documents` 使用的是 `id`。这构成了第三种命名变体。

### 修复建议

1. **短期**：将 `get_document_abstract`、`get_document_info`、`read_markdown`、`get_pdf_url` 四个工具的参数名从 `doc_id` 改为 `id`（或同时兼容两者），与 `search_info` / `list_documents` 保持一致。

2. **中期**：将 `grep_search` 返回结果中的 `document_id` 字段名统一为 `id`。

3. **长期**：在所有工具的 schema 定义中显式标注参数名和返回字段名，并添加自动化测试确保调用方使用正确的参数名，避免此类静默失败（参数被忽略而非报错）。
