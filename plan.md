# 修复 okb-assist MCP 服务器握手 500 的问题

## 现象

CodeBuddy 配置的 MCP 服务器 `okb-assist`（`https://xsro20.xyz/assist/mcp/stream`）
无法连接。直接探测发现：

- `GET` 该端点返回 **405**（符合预期，Streamable HTTP 只接受 POST）。
- `POST` `initialize` JSON-RPC 请求返回 **HTTP 500**，响应体为：

  ```
  Missing request extension: Extension of type
  `axum::extract::connect_info::ConnectInfo<core::net::socket_addr::SocketAddr>`
  was not found. Perhaps you forgot to add it? See `axum::Extension`.
  ```

- 无论是否带 `Authorization: Bearer ...` 头，都会返回同样的 500，说明**不是鉴权问题**。
- 服务器前面是 `nginx/1.18.0 (Ubuntu)` 反向代理。

## 根因

后端 `backend-rs` 是用 **Axum 0.7** 写的 MCP 服务。问题出在
`src/mcp_server.rs` 的 `mcp_stream_handler` 处理函数签名上：

```rust
pub async fn mcp_stream_handler(
    Extension(db): Extension<Arc<Database>>,
    Extension(settings): Extension<Arc<Settings>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,   // ← 问题所在
    headers: HeaderMap,
    body: String,
) -> Response {
```

`ConnectInfo<SocketAddr>` 是 Axum 的“硬提取器”（`FromRequestParts`）。它要能成功提取，
前提是服务器启动时用
`app.into_make_service_with_connect_info::<SocketAddr>()` 把 TCP 对端地址注入到每个请求里。

但 `src/main.rs` 的启动代码是：

```rust
axum::serve::serve(listener, app).await?;   // ← 没有 into_make_service_with_connect_info
```

因为没有调用 `into_make_service_with_connect_info`，请求里根本不存在
`ConnectInfo<SocketAddr>` 这个扩展。于是 Axum 永远无法构造该提取器，
对 **每一个** 到达 `mcp_stream_handler` 的请求都返回 500，MCP 的 `initialize`
握手因此永远无法完成，客户端也就报 “Failed to connect”。

> 注意：项目里其实已经有一个安全的 `extract_client_ip` 辅助函数
> （`src/main.rs`），它用 `req.extensions().get::<ConnectInfo<SocketAddr>>()`
> **可选**地读取该扩展，并回退到 `X-Forwarded-For` 头。但 `mcp_stream_handler`
> 没有复用这个安全写法，而是直接把 `ConnectInfo` 当作必填参数，从而触发了崩溃。

## 修复方案

1. **移除 `mcp_stream_handler` 中对 `ConnectInfo` 的硬依赖。**
   改为接收整个 `Request`，通过已有的安全辅助函数 `extract_client_ip`
   （优先 `X-Forwarded-For`，回退 TCP 对端地址，最后 `"unknown"`）来取客户端 IP。
   这样无论服务端是否配置了 `into_make_service_with_connect_info` 都不会再 500。

2. **把 `extract_client_ip` 的优先级调整为“先 `X-Forwarded-For`、后 `ConnectInfo`”**。
   因为服务部署在 nginx 反代之后，`ConnectInfo` 拿到的是 nginx 的地址，
   真实客户端 IP 在 `X-Forwarded-For` 里。先读 XFF 才是反代环境下的正确做法。

3. **在 `main.rs` 的启动处补上 `into_make_service_with_connect_info::<SocketAddr>()`**。
   虽然第 1 步已让 handler 不再依赖它，但加上它可以让直连（无反代）环境下
   日志/鉴权拿到真实的 TCP 对端地址，并与 `extract_client_ip` 的回退逻辑保持一致。

## 改动文件

- `src/main.rs`
  - `extract_client_ip`：改为 `pub(crate)`，并交换 XFF / ConnectInfo 的优先级。
  - `main()` 启动处：`.into_make_service_with_connect_info::<SocketAddr>()`。
- `src/mcp_server.rs`
  - `mcp_stream_handler`：去掉 `ConnectInfo(addr)` 参数，改为接收 `Request`，
    用 `crate::extract_client_ip(&req)` 取 IP，再从请求中拆出 `headers` 与 `body`。
  - 清理不再使用的 `ConnectInfo` / `SocketAddr` 导入。

## 验证

- `cargo build` 通过。
- 重新部署后，对 `POST /assist/mcp/stream` 发送 `initialize` 应返回 200 且带有
  `result.serverInfo`（不再是 500）。
- CodeBuddy 中 `okb-assist` 服务器应能从 “Failed to connect” 变为正常连接。
