# AGENTS.md — OKB-Assist

面向 AI 编程助手的工作指南。修改本仓库前先读此文件。



如果在嵌入式设备上运行，例如orangepi，请不要编译后端，执行代码检查即可。
```
cargo check
```

## 项目简介

OKB-Assist 是一个**本地学术文献库管理系统**（论文与专著）。流程：上传/登记 PDF → MinerU 解析为 Markdown → Ollama 抽取元数据 → 向量库（默认 Qdrant）语义建索引 → 通过 Web UI 或 MCP 服务检索。

- **后端**：Rust（Axum + SQLx + Tokio），位于 `backend-rs/`，API 接口与旧 Python 版兼容
- **前端**：TypeScript + Vue 3（Vite 构建），位于 `frontend/`
- **数据**：SQLx + SQLite（`okb_assist.db`）
- **向量库**：Qdrant（默认），适配器层同时支持 Milvus / Chroma
- **外部服务**：MinerU（解析）、Ollama（LLM/嵌入）、Qdrant（向量）、Fastembed（嵌入服务）、OpenWebUI（可选前端）
- **客户端工具**：`client-rs/` — Zotero CSV 导入工具（Rust）

## 包管理与常用命令

### 后端（Rust）

后端代码在 `backend-rs/` 目录下，使用 Cargo：

```bash
cd backend-rs
cargo build                  # 编译（debug）
cargo build --release        # 编译（release）
cargo run                    # 启动（默认 host 0.0.0.0、port 5001）
cargo run -- --host 0.0.0.0 --port 5001 --log-level debug   # 指定参数启动
```

> `okb_assist` 支持 argparse 等价命令行参数：`--host`、`--port`（默认 5001）、`--log-level`（trace/debug/info/warn/error，默认 info）。

### 前端

```bash
cd frontend
pnpm install         # 安装前端依赖
pnpm run dev         # 启动开发服务器（端口 5173，代理 /assist 到后端 5001）
pnpm run build       # 构建到 frontend/dist/
```

> **注意**：`pnpm run type-check` 目前不可用（`vue-tsc` 与当前 TypeScript 版本不兼容）。构建通过即视为类型检查通过。

### 客户端工具

```bash
cd client-rs
cargo build --release
# 用法见 client-rs/README.md
```

## 目录结构

| 路径 | 说明 |
|------|------|
| `backend-rs/src/main.rs` | **入口**：Axum app、路由/MCP/中间件 |
| `backend-rs/src/routers/` | 路由组：`documents.py`、`pipeline.py`、`admin.py`、`config.py`、`openapi.py` |
| `backend-rs/src/services/` | 后端适配器：`qdrant.rs`、`ollama.rs`、`mineru.rs`、`grep_search.rs`、`crossref.rs`、`pdf_meta.rs`、`vector_db.rs`（抽象接口） |
| `backend-rs/src/config_manager.rs`、`backend-rs/src/config.rs` | 配置加载（JSON 文件，带缓存） |
| `backend-rs/src/database.rs`、`backend-rs/src/models.rs` | 数据模型与 SQLite 连接 |
| `backend-rs/src/paths.rs` | 由 `system.json` 模板解析 PDF/Markdown/info/asset 路径 |
| `backend-rs/src/mcp_server.rs` | MCP 服务 |
| `backend-rs/src/settings.rs`、`backend-rs/src/utils.rs` | 设置与工具函数 |
| `backend-rs/config.json` | 服务配置（MinerU/Ollama/向量库），可由 UI 编辑 |
| `backend-rs/system.json` | 系统配置（token、DB URL、上传路径、路径模板），需手动改 |
| `client-rs/` | Zotero CSV 导入客户端（Rust） |
| `frontend/src/components/` | Vue 组件：`AceEditor.vue`（编辑+分屏预览+KaTeX）、`MarkdownViewer.vue`（marked 渲染）、`ConfirmDialog.vue`、`AppHeader.vue`（含汉堡菜单）、`AppNav.vue`（桌面端导航）、`MobileMenu.vue`（移动端侧滑菜单）、`StatusBadge.vue`、`Toast.vue`、`TokenModal.vue`、`McpConfigPanel.vue` |
| `frontend/src/views/` | 页面视图：`MarkdownEditView.vue`、`MarkdownView.vue`、`HomeView.vue`（桌面表格/移动卡片）、`DetailView.vue`（桌面表格/移动堆叠）、`DocManageView.vue`、`UploadView.vue`、`ConfigView.vue`、`AdminView.vue`、`MonitorView.vue`、`PointView.vue`、`DuplicatesView.vue`、`ToolsView.vue`、`McpSetupView.vue` |
| `frontend/src/router/` | Vue Router 配置（15+ 路由，`base: '/assist/'`） |
| `frontend/src/stores/` | Pinia 存储：`pipeline.ts`、`toast.ts`、`token.ts` |
| `frontend/src/composables/` | 组合式函数：`useToast.ts`、`useRequireToken.ts` |
| `frontend/src/types/` | TypeScript 类型定义：`document.ts`、`config.ts`、`pipeline.ts` |
| `frontend/src/utils/` | 工具：`mathRenderer.ts`（KaTeX/MathJax 公式渲染） |
| `frontend/src/api/` | API 客户端：`client.ts`、`documents.ts`、`pipeline.ts`、`admin.ts`、`config.ts` |
| `frontend/` | **Vue 3 + TypeScript SPA 前端**（Vite 构建，输出到 `frontend/dist/`，包管理器 `pnpm`） |
| `frontend/dist/` | 前端构建产物（`index.html`、`assets/`），由后端 serving |
| `data/` | 运行时数据（`okb_assist.db`、`_uploads/` 等），由 `system.json` 模板决定路径 |
| `document/` | 参考文档（`mcp.md` 最完整，列出全部 MCP 工具） |

## 配置（JSON 文件，非环境变量）

- `backend-rs/config.json`：服务配置（MinerU/Ollama/向量库），运行时改后需调用 `reload_config()` 或 `/assist/api/config/reload` 端点才生效。
- `backend-rs/system.json`：系统配置，改后**必须重启进程**才生效（进程内缓存）。
- 配置读取层：`backend-rs/src/config_manager.rs`（带锁 + 进程内缓存）；`backend-rs/src/config.rs:Settings` 是单例代理，属性每次从 JSON 实时读取。
- 添加新配置字段时：在 `config_manager.rs` 的默认值中加默认值；若是敏感字段，扩展脱敏函数。
- 文件路径来自 `system.json` 的路径模板（含 `{id}` 占位符），在 `backend-rs/src/paths.rs` 解析，**不要**在 `Document` 模型里加路径列。

### 路径变量替换

system.json 中的路径属性支持以下变量：

| 变量 | 说明 |
|------|------|
| `{id}` | 文档 ID |
| `{system_dir}` | system.json 所在目录的绝对路径 |
| `{system_path}` | system.json 的完整绝对路径 |
| `{env:VAR_NAME}` | 环境变量 `VAR_NAME` 的值 |
| `{cwd}` | 当前工作目录 |

支持变量替换的路径属性：`markdown_path`、`info_path`、`crossref_path`、`markdown_asset_path`、`pdf_path`、`uploads_folder`、`config_path`、`log_path`。

示例：

```json
{
  "markdown_path": "{system_dir}/data/markdowns/{id}.md",
  "pdf_path": "{system_dir}/data/pdfs/{id}/{id}.pdf",
  "uploads_folder": "{system_dir}/data/_uploads"
}
```

> 所有路径属性保持向后兼容：不使用变量时，相对路径相对于当前工作目录解析。

## 架构与请求流

1. **Web/API 层**：`backend-rs/src/main.rs` + `backend-rs/src/routers/*`。`TokenMiddleware` 仅对 `/assist/api/*` 校验 `X-Token`/query `token`，放行 `/assist/mcp`、`/assist/assets`（前端静态资源）、`/assist/uploads`、`/assist/file`、`/redirect`，以及对 `/assist/api/documents/` 下含 `/image/` 的图片 URL 放行。`token` 为 `change-me`（或未设置）时整体跳过校验；来自 `192.168.1.0/24` 局域网的请求也免校验。CORS 限定前端来源（开发 `localhost:5173`，生产同源 `localhost:5001`）。
2. **数据模型**：`Document`（主记录 + 状态）、`DocumentVectorIndex`（每个向量库的索引状态，唯一键 `(document_id, vector_db_id)`）。状态机用 `DocStatus` / `IndexStatus` 枚举（`backend-rs/src/models.rs`）。
3. **摄取流水线**（`backend-rs/src/routers/pipeline.rs`，核心状态机）：
   - parse（MinerU）→ `parsing` → `markdown_done`
   - extract（Ollama）→ `extracting` → `meta_done`
   - index（向量库）→ `indexing` → `indexed`
   - 含批量控制器、暂停/恢复/重置、信号量并发限制（受 `max_concurrent_tasks` 约束）。
   - **所有阶段均在后台执行**（见下方「后台任务」一节）—— 请求先返回，协程在事件循环继续；进度与状态机记录在 `Document` 上，可轮询查询。
4. **文档管理**（`documents.rs`）：CRUD、上传、按路径登记、语义搜索 `/search`、全文搜索 `/grep-search`、`/assist/markdown` 读写、PDF/图片服务、去重。
5. **配置/管理**（`config.rs`、`admin.rs`）：查看/更新服务配置、重连测试、统计、迁移、索引重置。
6. **向量库抽象**（`vector_db.rs` 工厂 `get_vector_db(db_id)` → Qdrant/Milvus/Chroma 适配器）。
7. **MCP 服务**（`mcp_server.rs`）：Streamable HTTP 端点 `/assist/mcp/stream`，旧版 SSE 挂载在 `/assist/mcp`；Bearer token 用 `system.json` 的 `mcp_token` 校验。完整工具列表见 `document/mcp.md`。

## 后台任务（异步执行）

后端大量耗时操作**不在请求内同步完成**，而是在后台运行，请求通常立即返回、由前端轮询状态。

- **机制**：摄取阶段（parse / extract / crossref / extract-pdf-meta / index / process）及批量端点通过 Axum `BackgroundTasks` 提交 Tokio 协程。协程在 HTTP 响应发出后由同一个 Tokio 运行时调度，状态机流转记录在数据库中。
- **并发限制**：受 `tokio::sync::Semaphore(max_concurrent_tasks)` 约束（`system.json` 默认 `3`）。
- **批量进度 / 暂停状态存于进程内存**：模块级全局变量，**进程重启即丢失**。重启后需手动重新触发。
- **真·fire-and-forget**：删除文档时，SQLite 记录与本地文件先同步删除，而 Qdrant 中对应向量点的删除通过 `tokio::spawn` 异步执行，**请求不等待、失败也不可见**。
- **前台（非后台）操作**：连接测试、服务状态、语义/全文搜索、上传均在前端 `await` 内同步完成。MCP 端点是唯一的流式长连接通道，但工具函数内部检索仍是同步 await。
- **无定时任务**：没有 `APScheduler`/`schedule`/守护线程；后台仅为 Tokio 协程。

## 路由前缀

- 前端 SPA：`/assist/*` → fallback 到 `frontend/dist/index.html`（由 Vue Router 处理客户端路由）
- API：`/assist/api/documents`、`/assist/api/pipeline`、`/assist/api/admin`、`/assist/api/config`、`/assist/openapi`
- 文件别名（免 token）：`/assist/file/{filename}`
- MCP：`/assist/mcp/stream`（Streamable HTTP）、`/assist/mcp`（SSE）
- 上传文件：`/assist/uploads/`（由后端直接 serving）

## 前端开发

```bash
cd frontend
pnpm install         # 安装前端依赖
pnpm run dev         # 启动开发服务器（端口 5173，代理 /assist 到后端 5001）
pnpm run build       # 构建到 frontend/dist/
```

> **注意**：`pnpm run type-check` 当前不可用（`vue-tsc` 与 TypeScript 版本不兼容）。构建通过即视为类型检查通过。

## 前端架构

### 构建与部署
- **包管理器**：`pnpm`（锁文件 `pnpm-lock.yaml`）
- **构建工具**：Vite 8，输出到 `frontend/dist/`
- **base 路径**：`vite.config.ts` 设 `base: '/assist/'`，因此所有 JS/CSS 资源 URL 以 `/assist/assets/` 开头。后端 `serve_spa` 路由通过 `/assist/{full_path:path}` 匹配资源路径并返回对应文件。
- **Vite proxy**：开发模式下代理 `/assist` → `http://localhost:5001`，前后端分离开发

### 响应式设计
- **断点**：`<= 480px`（小屏手机）、`<= 768px`（平板/大屏手机）、`769px-1024px`（平板横屏）
- **导航**：桌面端水平导航（`AppNav`），移动端汉堡菜单（`MobileMenu` 侧滑面板）
- **文档列表**：桌面端表格，移动端卡片式布局
- **信息表**：桌面端表格，移动端纵向堆叠
- **表单**：多列布局在移动端自动堆叠为单列
- **触摸目标**：最小 44px，按钮支持 `:active` 缩放反馈
- **横向滚动**：表格、代码块、编辑器工具栏在小屏自动横向滚动

### 路由（`src/router/index.ts`）
所有前端路由前缀为 `/assist/`，由 Vue Router 的 `createWebHistory()` 处理：
- `/assist` — 文献列表
- `/assist/detail/:id` — 文档详情
- `/assist/markdown/:id` — 只读 Markdown 查看
- `/assist/markdown/:id/edit` — Markdown 编辑（AceEditor）
- `/assist/upload` — 上传 PDF
- `/assist/config` — 服务配置
- `/assist/admin` — 管理面板
- `/assist/duplicates` — 去重
- `/assist/mcp-setup` — MCP 配置引导
- `/assist/monitor` — 任务监控
- `/assist/point`、`/assist/tools` — 工具页面

### AceEditor 编辑器组件（`src/components/AceEditor.vue`）
基于 `ace-builds@1.44.0` 封装的 Markdown 编辑器，内置完整编辑工具栏：
- **Markdown 快捷插入**：加粗/斜体/删除线/H1-H3/列表/引用/链接/图片/行内代码/代码块/表格/分割线
- **分屏预览**：编辑+预览并排，预览仅渲染编辑器**视口可见行**
- **全屏模式**：固定定位铺满窗口
- **搜索替换**：Ctrl+F / Ctrl+H（`ext-searchbox`）
- **字体缩放**：Ctrl+滚轮 / A+ A− 按钮（范围 8-30px）
- **撤销/重做**：Ctrl+Z / Ctrl+Shift+Z
- **光标/选中信息**：行号列号 + 选中字符数 + 总字数
- **数学公式**：预览通过 `MarkdownViewer` 组件渲染 KaTeX（`katex@0.18.4`）
- **模块加载**：核心模块静态 import；`ext-searchbox` 和 `ext-language_tools` 动态 import
- **移动端**：工具栏横向滚动，状态栏三段式换行

### MarkdownViewer 渲染组件（`src/components/MarkdownViewer.vue`）
- 使用 `marked` 解析 Markdown（GFM + 换行）
- 通过 `renderMath()` 预处理 KaTeX 公式（`$$...$$` 块级、`$...$` 行内）
- `DOMPurify.sanitize()` 净化 HTML，同时保留数学标签
- 支持 `mathMode` 切换（`katex` / `mathjax` / `none`）
- 支持 `loadImages` 开关控制图片加载

### 状态管理
- `useToast()`（`src/composables/useToast.ts`）：`showSuccess()` / `showError()` / `showInfo()` / `showToast()`
- `useRequireToken()`（`src/composables/useRequireToken.ts`）：Token 校验引导
- Pinia stores：`pipeline.ts`（任务状态）、`toast.ts`（消息队列）、`token.ts`（认证令牌）

### API 层
`src/api/` 通过 `axios` 请求后端：
- `client.ts`：axios 实例（baseURL `/assist/api`，自动附加 X-Token）
- `documents.ts`：CRUD + 搜索 + 上传
- `pipeline.ts`：流水线操作
- `admin.ts`、`config.ts`：管理/配置

### 类型定义
- `src/types/document.ts` — `Document`、`DocStatus`、`SearchResult`
- `src/types/config.ts` — `ServiceConfig`、`SystemConfig`
- `src/types/pipeline.ts` — `PipelineState`、`BatchProgress`

## 编码约定

- **注释与 docstring 用中文**，与现有代码保持一致。
- 后端：Rust 2021 edition，使用 `tokio::main` 异步运行时，`axum` 路由，`sqlx` 数据库。
- 前端：Vue 3 + TypeScript，`<script setup lang="ts">`，`<style scoped>`。
- 状态用 `enum`（`DocStatus`、`IndexStatus`）。
- 新增路径需求改 `system.json` 模板，而非数据库。
- 前端响应式断点：`480px` / `768px` / `1024px`，移动优先。

## ⚠️ 易错点（编辑前必读）

1. **Token 校验**：`token` 为 `change-me`（或未设置）时整体跳过校验；来自 `192.168.1.0/24` 局域网的请求也免校验。`TokenMiddleware` 仅对 `/assist/api/*` 生效。
2. **配置改动不会自动生效**：`config.json` 改后需 reload；`system.json` 改后需重启进程（进程内缓存）。
3. **MCP 路由注册顺序有依赖**（`backend-rs/src/main.rs`）：必须在 SSE 挂载之前精确注册 Streamable HTTP 端点。不要"整理"这个顺序。
4. **没有测试、没有 lint**：编辑后需手动 `cargo build` 确认编译通过，并用 curl 校验端点。
5. **硬编码的局域网 IP**（`192.168.1.x`）出现在 `config.json`、`system.json`、代码中，是部署相关配置，视为环境配置而非代码。`TokenMiddleware` 另把 `192.168.1.0/24` 作为 **LAN 免 Token 白名单**硬编码，改动需谨慎。
6. **SQLite 单写者**：并发写入可行但仍是瓶颈，勿引入大量并发写。
7. **SPA Fallback 路由**（`backend-rs/src/main.rs`）：`@app.get("/assist/{full_path:path}")` 必须放在所有路由之后。命中 `api/`、`mcp/`、`uploads/`、`file/` 前缀时避免吞掉 API 和 MCP 端点。
8. **前端构建产物在 `frontend/dist/`**：`pnpm run build` 输出到 `frontend/dist/`。`vite.config.ts` 的 `base: '/assist/'` 使所有资源 URL 以 `/assist/assets/` 开头。
9. **CORS 已限定**：开发环境只允许 `localhost:5173`，生产环境只允许同源 `localhost:5001`。
10. **移动端导航**：`MobileMenu` 通过 Vue 的 `provide/inject` 机制与 `AppHeader` 通信。`App.vue` 提供 `toggleMobileMenu` / `closeMobileMenu`，`AppHeader` 注入并控制汉堡菜单状态。
11. **Rust 异步**：后台任务用 `tokio::spawn` 或 Axum `BackgroundTasks`，不要在 `async` 块中使用 `std::thread::sleep` 或阻塞 I/O。
12. **SQLx 编译时检查**：`sqlx` 默认在 `debug` 模式下会校验 SQL 查询，如果迁移了数据库结构需运行 `cargo sqlx prepare`（需安装 `sqlx-cli`）。

## 入口与关键文件速查

- 后端入口：`backend-rs/src/main.rs`
- 配置：`backend-rs/config.json`、`backend-rs/system.json`、`backend-rs/src/config_manager.rs`、`backend-rs/src/config.rs`
- 架构核心：`backend-rs/src/routers/pipeline.rs`、`backend-rs/src/routers/documents.rs`、`backend-rs/src/models.rs`、`backend-rs/src/services/vector_db.rs`、`backend-rs/src/mcp_server.rs`
- 前端：`frontend/`（Vue 3 + TypeScript + Vite）
- MCP 工具参考：`document/mcp.md`
- 启动说明：`README.md`