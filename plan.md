# MCP 服务改进计划

基于对 `backend-rs/src/mcp_server.rs` 和 `document/mcp.md` 的审计，按优先级分组。

---

## 🔴 P0 — 必须修复

### 1. 文档与实现不一致：SSE 端点不存在 ✅ 已修复

**问题**：`document/mcp.md` 多处提到 SSE 端点 `/assist/mcp/sse`，但代码中未实现。

**处理**：从 `document/mcp.md` 中清除所有 SSE 引用（传输表格、SSE 配置章节、curl 测试命令、故障排除项）。不再实现 SSE。

### 2. 缺少关键路径日志 ✅ 已实现

`mcp_server.rs` 全链路日志已添加：initialize（info）、tools/call 入口（info）+ 完成（debug）、认证失败（warn，含客户端 IP）、JSON-RPC 解析错误（warn）、未知 method（warn）。

---

## 🟡 P1 — 应修复

### 3. Resources 声明与实现不匹配 ✅ 已修复

**问题**：`document/mcp.md` 列出两个资源 URI，但服务端未实现。

**处理**：从 `document/mcp.md` 中移除"可用资源"表格。纯工具接口已覆盖所有功能，Resources 无法替代搜索、过滤、分页等工具特性，不计划实现。

### 4. 缺少前置参数校验 ✅ 已实现

`call_tool` 入口添加了 `validate_required_string` 和 `validate_required_i64` 方法，通过宏 `check_required_str!` / `check_required_id!` 在 match 分支中 fail-fast 校验必需参数。grep_search/search_info校验 query，read_markdown/get_document_info/get_document_abstract/get_toc校验 id。

### 5. 缺少 offset/limit 上限保护 ✅ 已实现

`parse_pagination` 添加 `MAX_OFFSET = 10000` 上限，超出时 clamp 并记录 warn 日志。

### 6. 缺少 capabilities 声明 ✅ 已实现

`initialize` 响应现在声明 `tools`、`resources`、`logging` 三项能力。

### 7. JSON-RPC 版本校验缺失 ✅ 已实现

`mcp_stream_handler` 入口现在校验 `jsonrpc: "2.0"` 和 id 字段存在性（非通知请求必须带 id），非法请求返回 400 + 明确错误信息。

### 8. `resources/read` 错误响应不规范 ✅ 已实现

`resources/read` 现在返回 `{"contents": []}` 而非 `-32601 Method not found`。

---

## 🟢 P2 — 值得改进

### 9. 日志级别协商（logging/setLevel） ✅ 已实现

`notifications/` 分支处理 `logging/setLevel`，通过 `AtomicU8` 全局变量记录当前级别（默认 info）。

### 10. OPTIONS 预检请求处理 ✅ 已实现

`main.rs` 中添加 `.options(mcp_server::mcp_options_handler)`，`mcp_options_handler` 返回 200 + Allow header。

### 11. 反向代理/负载均衡友好 ✅ 已实现

`mcp_stream_handler` 现在通过 `ConnectInfo<SocketAddr>` 获取 TCP 对端 IP 进行 LAN 白名单检查，不再依赖可伪造的 `x-forwarded-for` 头。

### 12. 参数别名文档化到工具描述 ✅ 已实现

各工具的 `inputSchema` 中，参数的 `description` 字段现在更明确地标注了别名关系（alias、legacy），并在工具顶层 description 中提及分页参数别名。

---

## 🔵 P3 — 未来考虑

### 13. Progress 通知支持

MCP 规范允许服务端在工具执行期间发送 `notifications/progress` 通知。如果将来添加耗时工具（如触发 PDF 解析），应支持此协议。

### 14. 异步工具结果模式

对于可能长时间运行的工具，设计"提交 → 返回 task_id → 轮询结果"的异步模式。

### 15. 添加 `sections` 工具分类元信息

利用 MCP 工具定义中的组织形式字段，按功能域对工具分组（搜索类、读取类、统计类）。

---

## 执行顺序建议

| 步骤 | 内容 | 状态 |
|------|------|------|
| 1 | 添加日志（P0 #2） | ✅ |
| 2 | JSON-RPC 校验 + capabilities（P1 #6 #7） | ✅ |
| 3 | resources/read 改为空结果（P1 #8） | ✅ |
| 4 | 前置参数校验 + offset 上限（P1 #4 #5） | ✅ |
| 5 | 日志级别协商（P2 #9） | ✅ |
| 6 | OPTIONS 处理 + IP 获取（P2 #10 #11） | ✅ |
| 7 | 文档描述优化（P2 #12） | ✅ |