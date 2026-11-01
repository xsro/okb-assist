# OKB-Assist MCP 连接指南

OKB-Assist 提供了 [MCP (Model Context Protocol)](https://modelcontextprotocol.io/) 服务器，允许 AI 助手（如 CodeBuddy、Claude Desktop、Codex、Pi Agent、OpenCode、Cursor 等）直接调用文献管理工具。

> 完整的工具列表与参数说明见 [MCP 工具文档](./mcp-tools.md)。

## 认证配置

MCP 服务使用 Bearer Token 认证，支持两种凭据：

| 凭据 | 说明 |
|---|---|
| **个人访问令牌**（推荐） | 用户在「账户」页签发的令牌，**权限需选 MCP**（`scope: mcp`），并要求账号已开启「MCP 访问」。令牌明文可以是自动生成的 `okb_…`，也可以是自定义字符串。权限跟随该用户角色，泄露只需撤销自己那把 |
| **会话 token** | 浏览器登录拿到的 `sess_…`（既能调 API 也能调 MCP，一般只用于调试） |

> 旧版的全局 `system.json.mcp_token`（等价管理员）**已移除**；需要自动化场景请用管理员账号签发一把 `okb_…` 个人令牌。

- 启用认证后，客户端必须携带 `Authorization: Bearer <token>`（或 `X-Token: <token>`）
- 令牌权限**严格二选一**：`MCP` 令牌只能调 `/assist/mcp`（不能调 `/assist/api/*`），`API` 令牌反之
- **MCP 令牌的明文可随时查看**：在「工具 → MCP 配置」页可直接下拉选中自己的 MCP 令牌并生成配置，
  或到「账户」页的令牌列表点「显示 / 复制」；配置页也因此会在生成的客户端配置里直接写入明文。
  API 令牌不做此处理（明文仍只在创建时显示一次）
- 账号被禁用 / 令牌被撤销后立即 401
- 未开启 MCP 的用户即使有令牌也会被拒绝：`Unauthorized: MCP access not granted`
- 受信任子网免鉴权需**显式**把 `system.json` 的 `trusted_subnet_enabled` 设为 `true`
  （默认关闭，即使 `trusted_subnets` 已配网段也不生效）
- 用户管理与令牌说明见 [user-management.md](./user-management.md)

---

## 连接方式

OKB-Assist **只提供一种** MCP 传输端点：

| 传输 | 端点 URL | 说明 |
|------|----------|------|
| **Streamable HTTP** | `http://<host>:<port>/assist/mcp/stream` | 唯一端点；精确路径、末尾无斜杠；只响应 `POST`（`OPTIONS` 用于预检） |

实现细节（据 `backend-rs/src/mcp_server.rs`）：请求体一次性读完后按 JSON-RPC 方法返回**单个 JSON 响应**，
属于 Streamable HTTP 的最小子集 —— 没有 `GET` 推流通道、没有 `Mcp-Session-Id` 会话管理、没有 `Last-Event-ID` 断线重放。

> **未实现的传输**（按这些方式配置会连不上，均返回 404）：
>
> - **legacy SSE**：`/assist/mcp/sse`（`GET /sse` + `POST /messages`）不存在；
> - **stdio**：不提供本地子进程模式。

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

### 方式二：HTTP 本地连接

适用于 AI 工具与 OKB-Assist 在同一台机器上运行的情况。MCP 服务器通过 HTTP 协议通信：

```
http://localhost:5001/assist/mcp/stream
```

> Rust 后端**只有 HTTP 这一种传输**（`/assist/mcp/stream` 上的 POST JSON-RPC）。
> 不支持 stdio 模式，也没有实现 legacy SSE 传输（`/assist/mcp/sse` 不存在，请求返回 404）。

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
# 最小 JSON-RPC 请求（ping 不需要任何文献数据）
# 无凭证 → 401；带 MCP 权限令牌 → 200，body 为 {"jsonrpc":"2.0","id":1,"result":{}}
curl -s -X POST http://192.168.1.100:5001/assist/mcp/stream \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer <token>" \
  -d '{"jsonrpc":"2.0","id":1,"method":"ping"}'
```

> 端点**只接受 `POST`**：`GET /assist/mcp/stream`、`/assist/mcp/sse`，以及 `/assist/mcp/*` 下的其它路径
> 都会落到 SPA fallback，返回 `404 {"detail":"Not Found"}`。成功时是 `200` 的普通 JSON 响应，不是 `406`，也不是事件流。

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
  "url": "http://127.0.0.1:5001/assist/mcp/stream",
  "headers": {
    "Authorization": "Bearer okb_你的个人令牌"
  }
}
```

---

## 故障排除

### MCP 端点返回401

- 检查请求携带的凭证是否是有效的个人令牌（**权限必须是 MCP**，且账号与令牌都开启 MCP）或会话 token（`sess_…`）
- `scope: api` 的令牌调 MCP 会直接 401：请在「账户」页重新创建一把 **MCP** 权限的令牌
- 确认客户端请求中携带了正确的 `Authorization: Bearer <token>` header
- 检查 `system.json` 的 `trusted_subnets` 白名单（配合 `trusted_subnet_enabled = true`）是否包含当前 IP

### 工具调用返回空结果

- 确认文档已通过完整 pipeline 处理（状态为 `indexed`）
- 向量搜索需要文档先被索引到 Qdrant
- 全文搜索（grep）不需要索引，可直接使用

### 服务未启动

- 确认在 `backend-rs/` 目录下运行：`cargo run`
- 检查 Rust 工具链：`cargo --version`
- 检查端口 5001 是否被占用

### 返回 404（GET 请求 / `/assist/mcp/sse` / 末尾带斜杠）

- 本服务只有一个 MCP 端点 `POST /assist/mcp/stream`：对该路径发 `GET`、访问 `/assist/mcp/sse`、
  或把 URL 写成 `/assist/mcp/stream/` 都会 404
- 客户端传输类型要选 `http` / `streamable-http`，**不要选 `sse`**（SSE 传输未实现）
- 请求体必须是合法 JSON-RPC 2.0（缺 `jsonrpc` 字段返回 400，缺 `id` 也返回 400）

### 客户端提示未收到事件流 / 不支持

- 本实现只在 POST 上返回单个 JSON，**不返回 `text/event-stream`**：
  少数只按 SSE 解析的 Streamable HTTP 客户端会连不上，请换用支持 `application/json` 响应的客户端（如 Claude Code）
