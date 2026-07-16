# OKB-Assist MCP 工具配置指南

OKB-Assist 提供了 [MCP (Model Context Protocol)](https://modelcontextprotocol.io/) 服务器，允许 AI 助手（如 Claude Desktop、Cursor 等）直接调用文献管理工具。

## 可用工具

| 工具名称 | 功能 | 参数 |
|---------|------|------|
| `search_documents` | 语义搜索文献内容 | `query: str`, `limit: int = 5` |
| `read_markdown` | 读取文献 Markdown 内容 | `doc_id: int`, `page: int = 1`, `page_size: int = 5000` |
| `get_document_info` | 获取文献详细信息 | `doc_id: int` |
| `list_documents` | 搜索/列出文献 | `query: str`, `status: str`, `page: int`, `page_size: int` |
| `get_pdf_url` | 获取 PDF 链接 | `doc_id: int` |
| `get_document_abstract` | 获取文献摘要 | `doc_id: int` |

## 可用资源

| URI | 内容 |
|-----|------|
| `okb://documents/{doc_id}` | 文档详情 JSON |
| `okb://documents/{doc_id}/markdown` | 文档 Markdown 全文 |

---

## 连接方式

### Codex：Streamable HTTP 远程连接

Codex 使用 MCP Streamable HTTP 连接 OKB-Assist，服务启动后端点为：

```
http://<host>:<port>/assist/mcp/
```

例如本地运行：`http://localhost:5001/assist/mcp/`

在 Codex 的 `config.toml` 中添加：

```toml
[mcp_servers.okb_assist]
url = "http://localhost:5001/assist/mcp/"
startup_timeout_sec = 20
tool_timeout_sec = 120
```

---

### 方式一：SSE 远程连接（推荐）

适用于网络可达的 OKB-Assist 服务。服务启动后，MCP SSE 端点自动挂载在：

```
http://<host>:<port>/assist/mcp/sse
```

例如本地运行：`http://localhost:5001/assist/mcp/sse`

#### Claude Desktop 配置

编辑配置文件 `~/.claude/claude_desktop_config.json`（macOS）或 `%APPDATA%\Claude\claude_desktop_config.json`（Windows）：

```json
{
  "mcpServers": {
    "okb-assist": {
      "url": "http://localhost:5001/assist/mcp/sse"
    }
  }
}
```

#### Cursor 配置

在项目根目录创建 `.cursor/mcp.json`：

```json
{
  "mcpServers": {
    "okb-assist": {
      "url": "http://localhost:5001/assist/mcp/sse"
    }
  }
}
```

#### Claude Code 配置

在 Claude Code 中运行：

```bash
claude mcp add okb-assist --transport sse http://localhost:5001/assist/mcp/sse
```

或手动编辑 `.claude.json`：

```json
{
  "mcpServers": {
    "okb-assist": {
      "type": "sse",
      "url": "http://localhost:5001/assist/mcp/sse"
    }
  }
}
```

---

### 方式二：stdio 本地连接

适用于 AI 工具与 OKB-Assist 在同一台机器上运行的情况。MCP 服务器通过标准输入/输出通信。

#### Claude Desktop 配置

```json
{
  "mcpServers": {
    "okb-assist": {
      "command": "uv",
      "args": ["run", "python", "-m", "app.mcp_server"],
      "cwd": "/path/to/okb-assist"
    }
  }
}
```

#### Claude Code 配置

```bash
claude mcp add okb-assist -- uv run python -m app.mcp_server
```

#### Cursor 配置

```json
{
  "mcpServers": {
    "okb-assist": {
      "command": "uv",
      "args": ["run", "python", "-m", "app.mcp_server"],
      "cwd": "/path/to/okb-assist"
    }
  }
}
```

> **注意**: 将 `cwd` 替换为 OKB-Assist 项目的实际路径。

---

## 验证连接

### 1. 确认服务运行

```bash
# 检查服务是否启动
curl http://localhost:5001/assist/api/admin/services/status
```

### 2. 测试 MCP 端点

```bash
# 测试 SSE 端点是否可访问
curl -N http://localhost:5001/assist/mcp/sse
```

### 3. 使用 MCP Inspector 测试

```bash
npx @modelcontextprotocol/inspector uv run python -m app.mcp_server
```

---

## 使用示例

连接成功后，可以在 AI 助手中直接使用自然语言调用工具：

- **搜索文献**: "帮我搜索关于机器学习优化的文献"
- **阅读文献**: "读取文档 42 的 Markdown 内容"
- **查看信息**: "文档 42 的详细信息是什么？"
- **获取摘要**: "给我看看文档 42 的摘要"
- **列出文献**: "列出所有已索引的文献"

---

## Web 工具面板

除了 MCP 协议，OKB-Assist 还提供了一个 Web 工具面板，可在浏览器中直接使用：

```
http://localhost:5001/assist/tools
```

功能包括：
- **语义搜索**: 输入查询，显示带相似度分数的搜索结果
- **文档浏览**: 分页列表，支持搜索和状态过滤
- **Markdown 阅读**: 选择文档后渲染 Markdown 内容
- **文档信息**: 显示元数据、PDF 链接等

---

## 故障排除

### MCP 端点无法访问

- 确认 OKB-Assist 服务已启动且端口正确
- 检查 `mcp` 包是否安装：`uv pip list | grep mcp`

### 工具调用返回空结果

- 确认文档已通过完整 pipeline 处理（状态为 `indexed`）
- 向量搜索需要文档先被索引到 Qdrant

### stdio 模式启动失败

- 确认在 OKB-Assist 项目根目录下运行
- 检查 Python 环境：`uv run python -c "import mcp; print('OK')"`
