# OKB-Assist MCP 工具配置指南

OKB-Assist 提供了 [MCP (Model Context Protocol)](https://modelcontextprotocol.io/) 服务器，允许 AI 助手（如 Claude Desktop、Cursor 等）直接调用文献管理工具。

## 可用工具

| 工具名称 | 功能 | 参数 |
|---------|------|------|
| `grep_search` | 全文搜索（基于 grep，轻量快速） | `query: str`, `limit: int = 10`, `context: int = 2`, `doc_ids: str` |
| `search_info` | 搜索文献元数据（标题、作者、期刊等） | `query: str`, `limit: int = 10` |
| `search_documents` | 语义搜索文献内容 | `query: str`, `limit: int = 5` |
| `read_markdown` | 读取文献 Markdown 内容（分页） | `doc_id: int`, `page: int = 1`, `page_size: int = 5000` |
| `get_document_info` | 获取文献详细信息 | `doc_id: int` |
| `list_documents` | 搜索/列出文献 | `query: str`, `status: str`, `doc_type: str`, `page: int`, `page_size: int` |
| `get_pdf_url` | 获取 PDF 链接 | `doc_id: int` |
| `get_document_abstract` | 获取文献摘要 | `doc_id: int` |
| `get_stats` | 获取知识库统计信息 | 无 |
| `list_doc_types` | 列出所有已使用的文献类型 | 无 |

## 可用资源

| URI | 内容 |
|-----|------|
| `okb://documents/{doc_id}` | 文档详情 JSON |
| `okb://documents/{doc_id}/markdown` | 文档 Markdown 全文 |

---

## 认证配置

MCP 服务使用 Bearer Token 认证。Token 在 `system.json` 的 `mcp_token` 字段中配置：

```json
{
  "mcp_token": "your-secret-token"
}
```

- Token 为 `change-me` 时**禁用认证**（开发模式）
- Token 为其他值时，客户端必须在请求中携带 `Authorization: Bearer <token>`
- `system.json` 中的 `token` 字段用于 API 认证，与 `mcp_token` 独立

---

## 连接方式

OKB-Assist 同时提供两种 MCP 传输端点，二者**会话相互独立**（连接其中一个不会影响另一个，客户端不应假设两者会话共享）：

| 传输 | 端点 URL | 说明 |
|------|----------|------|
| **Streamable HTTP（推荐）** | `http://<host>:<port>/assist/mcp/stream` | 当前标准传输；精确路径，末尾无斜杠 |
| **SSE（legacy 兼容）** | `http://<host>:<port>/assist/mcp/sse` | 旧版传输，仅供需要 SSE 的客户端使用 |

> 两种传输都使用同一个 `mcp_token` 进行 Bearer 认证，工具与资源完全一致。

### 方式一：Streamable HTTP 远程连接（推荐）

适用于网络可达的 OKB-Assist 服务，端点为：

```
http://<host>:<port>/assist/mcp/stream
```

例如本地运行：`http://192.168.1.100:5001/assist/mcp/stream`

#### Claude Desktop 配置

编辑配置文件 `~/.claude/claude_desktop_config.json`：

```json
{
  "mcpServers": {
    "okb-assist": {
      "type": "http",
      "url": "http://192.168.1.100:5001/assist/mcp/stream",
      "headers": {
        "Authorization": "Bearer we-network-control"
      }
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
      "url": "http://192.168.1.100:5001/assist/mcp/stream",
      "headers": {
        "Authorization": "Bearer we-network-control"
      }
    }
  }
}
```

#### Claude Code 配置

```bash
claude mcp add okb-assist --transport http http://192.168.1.100:5001/assist/mcp/stream --header "Authorization: Bearer we-network-control"
```

#### Codex 配置（Streamable HTTP）

Streamable HTTP 端点地址为 `/assist/mcp/stream`（注意末尾无斜杠，是精确路径）：

```toml
[mcp_servers.okb_assist]
url = "http://192.168.1.100:5001/assist/mcp/stream"
startup_timeout_sec = 20
tool_timeout_sec = 120
http_headers = {
    Authorization = "Bearer we-network-control"
}
```

---

### 方式二：SSE 远程连接（legacy 兼容）

适用于需要 SSE 传输的客户端。服务启动后，MCP SSE 端点自动挂载在：

```
http://<host>:<port>/assist/mcp/sse
```

例如本地运行：`http://192.168.1.100:5001/assist/mcp/sse`

#### Claude Desktop 配置

编辑配置文件 `~/.claude/claude_desktop_config.json`（macOS）或 `%APPDATA%\Claude\claude_desktop_config.json`（Windows）：

```json
{
  "mcpServers": {
    "okb-assist": {
      "url": "http://192.168.1.100:5001/assist/mcp/sse",
      "headers": {
        "Authorization": "Bearer we-network-control"
      }
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
      "url": "http://192.168.1.100:5001/assist/mcp/sse",
      "headers": {
        "Authorization": "Bearer we-network-control"
      }
    }
  }
}
```

#### Claude Code 配置

编辑 `~/.claude/settings.json`：

```json
{
  "mcpServers": {
    "okb-assist": {
      "type": "sse",
      "url": "http://192.168.1.100:5001/assist/mcp/sse",
      "headers": {
        "Authorization": "Bearer we-network-control"
      }
    }
  }
}
```

或使用命令行：

```bash
claude mcp add okb-assist --transport sse http://192.168.1.100:5001/assist/mcp/sse --header "Authorization: Bearer we-network-control"
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
curl http://192.168.1.100:5001/assist/api/admin/services/status
```

### 2. 测试 MCP 端点

```bash
# Streamable HTTP 端点（精确路径，带 token 应返回 406，无 token 应返回 401）
curl -s -o /dev/null -w "%{http_code}\n" http://192.168.1.100:5001/assist/mcp/stream \
  -H "Authorization: Bearer we-network-control"

# SSE 端点（无 token 应返回401；带 token 返回 SSE 事件流）
curl -s -N http://192.168.1.100:5001/assist/mcp/sse \
  -H "Authorization: Bearer we-network-control"
```

> 注意：Streamable HTTP 端点为精确路径 `/assist/mcp/stream`，末尾不要加斜杠；SSE 端点为 `/assist/mcp/sse`。

### 3. 使用 MCP Inspector 测试

```bash
npx @modelcontextprotocol/inspector uv run python -m app.mcp_server
```

---

## 使用示例

连接成功后，可以在 AI 助手中直接使用自然语言调用工具：

- **全文搜索**: "帮我用 grep 搜索包含 transformer 的文献"
- **指定范围搜索**: "在文档 42 和 99 中搜索 transformer"
- **按作者搜索**: "搜索作者是 Hinton 的文献"
- **按标题搜索**: "有没有标题包含 Attention 的论文"
- **按期刊搜索**: "列出发表在 Nature 上的文献"
- **语义搜索**: "帮我搜索关于机器学习优化的文献"
- **阅读文献**: "读取文档 42 的 Markdown 内容"
- **查看信息**: "文档 42 的详细信息是什么？"
- **获取摘要**: "给我看看文档 42 的摘要"
- **列出文献**: "列出所有已索引的 journalArticle 类型文献"
- **统计信息**: "知识库有多少篇文献？各类型分别多少？"
- **文献类型**: "知识库中有哪些文献类型？"

---

## Web 工具面板

除了 MCP 协议，OKB-Assist 还提供了一个 Web 工具面板，可在浏览器中直接使用：

```
http://192.168.1.100:5001/assist/tools
```

功能包括：
- **全文搜索**: 基于 grep 的关键词/正则搜索，支持自定义返回数量
- **语义搜索**: 输入查询，显示带相似度分数的搜索结果
- **文档浏览**: 分页列表，支持搜索和状态过滤
- **Markdown 阅读**: 选择文档后渲染 Markdown 内容
- **文档信息**: 显示元数据、PDF 链接等

---

## 故障排除

### MCP 端点返回401

- 检查 `system.json` 中的 `mcp_token` 配置
- 确认客户端请求中携带了正确的 `Authorization: Bearer <token>` header
- Token 为 `change-me` 时认证会被禁用

### 工具调用返回空结果

- 确认文档已通过完整 pipeline 处理（状态为 `indexed`）
- 向量搜索需要文档先被索引到 Qdrant
- 全文搜索（grep）不需要索引，可直接使用

### stdio 模式启动失败

- 确认在 OKB-Assist 项目根目录下运行
- 检查 Python 环境：`uv run python -c "import mcp; print('OK')"`

### SSE 消息端点404

- 确认使用最新版本的 MCP 配置（`mount_path=""` 已修复路径重复问题）
- 重启服务后重试
