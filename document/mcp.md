# OKB-Assist MCP 连接指南

OKB-Assist 提供了 [MCP (Model Context Protocol)](https://modelcontextprotocol.io/) 服务器，允许 AI 助手（如 CodeBuddy、Claude Desktop、Codex、Pi Agent、OpenCode、Cursor 等）直接调用文献管理工具。

> 完整的工具列表与参数说明见 [MCP 工具文档](./mcp-tools.md)。

## 认证配置

MCP 服务使用 Bearer Token 认证，支持三种凭据：

| 凭据 | 说明 |
|---|---|
| **个人访问令牌**（推荐） | 用户在「账户」页签发的 `okb_…`，要求账号已开启「MCP 访问」且令牌勾选 MCP。权限跟随该用户角色，泄露只需撤销自己那把 |
| **全局 MCP token** | `system.json` 的 `mcp_token`，等价于本地管理员主体，用于共享/自动化场景 |
| **会话 token** | 浏览器登录拿到的 `sess_…`（一般只用于调试） |

```json
{
  "mcp_token": "your-secret-token",
  "trusted_subnet_enabled": false
}
```

- 启用认证后，客户端必须携带 `Authorization: Bearer <token>`（或 `X-Token: <token>`）
- 账号被禁用 / 令牌被撤销后立即 401
- 未开启 MCP 的用户即使有令牌也会被拒绝：`Unauthorized: MCP access not granted`
- 受信任子网免鉴权需**显式**把 `system.json` 的 `trusted_subnet_enabled` 设为 `true`
  （默认关闭，即使 `trusted_subnets` 已配网段也不生效）
- 用户管理与令牌说明见 [user-management.md](./user-management.md)

---

## 连接方式

OKB-Assist 同时提供两种 MCP 传输端点，二者**会话相互独立**（连接其中一个不会影响另一个，客户端不应假设两者会话共享）：

| 传输 | 端点 URL | 说明 |
|------|----------|------|
| **Streamable HTTP（推荐）** | `http://<host>:<port>/assist/mcp/stream` | 当前标准传输；精确路径，末尾无斜杠 |
| **SSE（legacy 兼容）** | `http://<host>:<port>/assist/mcp/sse` | 旧版传输，仅供需要 SSE 的客户端使用 |

> 两种传输的认证规则一致（个人令牌 / 全局 `mcp_token` / 会话令牌），工具与资源完全一致。

### 方式一：Streamable HTTP 远程连接（推荐）

适用于网络可达的 OKB-Assist 服务，端点为：

```
http://<host>:<port>/assist/mcp/stream
```

例如本地运行：`http://192.168.1.100:5001/assist/mcp/stream`

#### Tencent Codebuddy 配置

在`~/.codebuddy/.mcp.json`中配置：

```json
{
    "mcpServers": {
        "okb-assist": {
            "type":"http",
            "url": "http://192.168.1.100:5001/assist/mcp/stream",
            "headers": {
                "Authorization": "Bearer <token>"
            },
            "description": "控制理论文献知识库"
        }
    }
}
```

#### Claude Desktop 配置

编辑配置文件 `~/.claude/claude_desktop_config.json`：

```json
{
  "mcpServers": {
    "okb-assist": {
      "type": "http",
      "url": "http://192.168.1.100:5001/assist/mcp/stream",
      "headers": {
        "Authorization": "Bearer <token>"
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
        "Authorization": "Bearer <token>"
      }
    }
  }
}
```

#### Claude Code 配置

```bash
claude mcp add okb-assist --transport http http://192.168.1.100:5001/assist/mcp/stream --header "Authorization: Bearer <token>"
```

#### Codex 配置（Streamable HTTP）

Streamable HTTP 端点地址为 `/assist/mcp/stream`（注意末尾无斜杠，是精确路径）：

```toml
[mcp_servers.okb_assist]
url = "http://192.168.1.100:5001/assist/mcp/stream"
startup_timeout_sec = 20
tool_timeout_sec = 120
http_headers = {
    Authorization = "Bearer <token>"
}
```

#### Pi Agent 配置

在 `~/.config/mcp/mcp.json`（或项目级 `.mcp.json`）中配置：

```json
{
  "mcpServers": {
    "okb-assist": {
      "url": "http://192.168.1.100:5001/assist/mcp/stream",
      "headers": {
        "Authorization": "Bearer <token>"
      }
    }
  }
}
```

#### OpenCode 配置

在 `~/.config/opencode/opencode.json`（或项目级 `opencode.json`）中配置：

```json
{
  "mcp": {
    "okb-assist": {
      "type": "remote",
      "url": "http://192.168.1.100:5001/assist/mcp/stream",
      "headers": {
        "Authorization": "Bearer <token>"
      }
    }
  }
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
        "Authorization": "Bearer <token>"
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
        "Authorization": "Bearer <token>"
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
        "Authorization": "Bearer <token>"
      }
    }
  }
}
```

或使用命令行：

```bash
claude mcp add okb-assist --transport sse http://192.168.1.100:5001/assist/mcp/sse --header "Authorization: Bearer <token>"
```

---

### 方式三：HTTP 本地连接

适用于 AI 工具与 OKB-Assist 在同一台机器上运行的情况。MCP 服务器通过 HTTP 协议通信。

> Rust 后端仅支持 HTTP 方式（Streamable HTTP / SSE），不再支持 stdio 模式。

#### Claude Desktop 配置

```json
{
  "mcpServers": {
    "okb-assist": {
      "type": "http",
      "url": "http://localhost:5001/assist/mcp/stream"
    }
  }
}
```

#### Claude Code 配置

Claude Code 支持 HTTP 方式连接 MCP 服务：

```bash
claude mcp add okb-assist --url http://localhost:5001/assist/mcp/stream
```

> 需要先启动后端：`cd backend-rs && cargo run`

#### Cursor 配置

```json
{
  "mcpServers": {
    "okb-assist": {
      "type": "http",
      "url": "http://localhost:5001/assist/mcp/stream"
    }
  }
}
```

> **注意**: 将 URL 中的 `localhost:5001` 替换为实际的后端服务地址。

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
  -H "Authorization: Bearer <token>"

> 注意：Streamable HTTP 端点为精确路径 `/assist/mcp/stream`，末尾不要加斜杠。

### 3. 使用 MCP Inspector 测试

```bash
cd /path/to/okb-assist/backend-rs
cargo run
# 记下服务地址（默认 http://localhost:5001）
```

运行 `npx @modelcontextprotocol/inspector` 命令，
然后在浏览器中打开 MCP Inspector，将servers选项卡的端点 URL 设置为：

```
http://localhost:5001/assist/mcp/stream
```

设置完成后配置文件` ~/.mcp-inspector/mcp.json`大致变成：

```json
"local": {
  "type": "streamable-http",
  "url": "http://127.0.0.1:5001/assist/mcp/stream"
  "headers": {
    "Authorization": "Bearer we-network-control"
  }
}
```

---

## 故障排除

### MCP 端点返回401

- 检查 `system.json` 中的 `mcp_token` 配置
- 确认客户端请求中携带了正确的 `Authorization: Bearer <token>` header
- Token 为 `change-me` 时认证会被禁用
- 检查 `trusted_subnets` 白名单是否包含当前 IP

### 工具调用返回空结果

- 确认文档已通过完整 pipeline 处理（状态为 `indexed`）
- 向量搜索需要文档先被索引到 Qdrant
- 全文搜索（grep）不需要索引，可直接使用

### 服务未启动

- 确认在 `backend-rs/` 目录下运行：`cargo run`
- 检查 Rust 工具链：`cargo --version`
- 检查端口 5001 是否被占用

### Streamable HTTP 返回 406

- 确认请求的 `Content-Type` 为 `application/json`
- Streamable HTTP 端点为精确路径 `/assist/mcp/stream`，末尾不要加斜杠
- 某些客户端可能需要在请求中包含 `Accept: application/json, text/event-stream`