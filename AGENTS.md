# CODEBUDDY.md — OKB-Assist

面向 AI 编程助手（CodeBuddy / Claude / Cursor）的工作指南。修改本仓库前先读此文件。

## 项目简介

OKB-Assist 是一个**本地学术文献库管理系统**（论文与专著）。流程：上传/登记 PDF → MinerU 解析为 Markdown → Ollama 抽取元数据 → 向量库（默认 Qdrant）语义建索引 → 通过 Web UI 或 MCP 服务检索。

- 语言：Python >=3.12，前端 TypeScript + Vue 3
- 主框架：FastAPI + Uvicorn（ASGI，纯 JSON API 后端）+ Vue 3 SPA 前端（Vite 构建）
- 数据：SQLAlchemy 2.0 + SQLite（`okb_assist.db`）
- 向量库：Qdrant（默认），适配器层同时支持 Milvus / Chroma
- 外部服务：MinerU（解析）、Ollama（LLM/嵌入）、Qdrant（向量）、Fastembed（嵌入服务）、OpenWebUI（可选前端）

## 包管理与常用命令

包管理器是 `uv`（`backend/uv.lock` 已提交，`backend/uv.toml` 指向 SUSTech 镜像）。**没有 Makefile、没有 lint/test 目标、没有 pre-commit。**

后端代码在 `backend/` 目录下，所有 `uv` 命令需在该目录执行：

```bash
cd backend
uv sync                                  # 安装依赖（或 uv pip install -e .）
uv run okb_assist_main.py                # 启动主程序（uvicorn，默认 host 0.0.0.0、port 5001、reload=False）
uv run python -m uvicorn okb_assist_main:app --host 0.0.0.0 --port 5001   # 生产式启动
uv run scripts/fastembed_server.py       # 启动 Fastembed 嵌入服务
uv run scripts/<name>.py ...             # 运行维护脚本（见 backend/scripts/）
```

> 注意：
> - `okb_assist_main.py` 已支持 argparse，可接收 `--host`、`--port`、`--reload` 参数（默认 `0.0.0.0` / `5001` / `False`）。`--reload` 需显式传入才开启热重载，否则默认关闭。
> - `config.json` 与 `system.json` 默认在**启动时的工作目录**创建/读取。为保持路径稳定，建议始终从 `backend/` 目录启动后端。

## 目录结构

| 路径 | 说明 |
|------|------|
| `backend/okb_assist_main.py` | **入口**：创建 FastAPI app、挂载路由/MCP/中间件 |
| `backend/app/routers/` | 路由组：`documents.py`、`pipeline.py`、`admin.py`、`config.py`、`openapi.py` |
| `backend/app/services/` | 后端适配器：`qdrant.py`、`ollama.py`、`mineru.py`、`milvus.py`、`chroma.py`、`grep_search.py`、`vector_db.py`（抽象接口） |
| `backend/app/mineru_fast_api/` | MinerU 解析服务的**自动生成 OpenAPI 客户端**（勿手改） |
| `backend/app/models.py`、`backend/app/database.py` | 数据模型与 SQLite 连接（`init_db()` 启动时建表） |
| `backend/app/config_manager.py`、`backend/app/config.py` | 配置加载（JSON 文件，带缓存） |
| `backend/app/paths.py` | 由 `system.json` 模板解析 PDF/Markdown/info/asset 路径 |
| `backend/app/mcp_server.py` | MCP 服务（`FastMCP("OKB-Assist")`） |
| `backend/scripts/` | 维护/迁移脚本 + shell 启动器 |
| `frontend/src/components/` | Vue 组件：`AceEditor.vue`（590 行，编辑+分屏预览+KaTeX）、`MarkdownViewer.vue`（marked 渲染）、`ConfirmDialog.vue`（确认弹窗）、`AppHeader.vue`、`AppNav.vue`、`StatusBadge.vue`、`Toast.vue`、`TokenModal.vue`、`McpConfigPanel.vue` |
| `frontend/src/views/` | 页面视图：`MarkdownEditView.vue`、`MarkdownView.vue`、`HomeView.vue`、`DetailView.vue`、`DocManageView.vue`、`UploadView.vue`、`ConfigView.vue`、`AdminView.vue`、`MonitorView.vue`、`PointView.vue`、`DuplicatesView.vue`、`ToolsView.vue`、`McpSetupView.vue` |
| `frontend/src/router/` | Vue Router 配置（15+ 路由，`base: '/assist/'`） |
| `frontend/src/stores/` | Pinia 存储：`pipeline.ts`、`toast.ts`、`token.ts` |
| `frontend/src/composables/` | 组合式函数：`useToast.ts`、`useRequireToken.ts` |
| `frontend/src/types/` | TypeScript 类型定义：`document.ts`、`config.ts`、`pipeline.ts` |
| `frontend/src/utils/` | 工具：`mathRenderer.ts`（KaTeX/MathJax 公式渲染） |
| `frontend/src/api/` | API 客户端：`client.ts`、`documents.ts`、`pipeline.ts`、`admin.ts`、`config.ts` |
| `frontend/` | **Vue 3 + TypeScript SPA 前端**（Vite 构建，输出到 `frontend/dist/`，包管理器 `pnpm`） |
| `frontend/dist/` | 前端构建产物（`index.html`、`assets/`），由后端 serving |
| `backend/config.json` | 服务配置（MinerU/Ollama/向量库），**git 忽略，可由 UI 编辑** |
| `backend/system.json` | 系统配置（token、DB URL、上传路径、路径模板），**git 忽略，需手动改** |
| `data/`、`uploads/` | 运行时数据路径由 `system.json` 模板决定（均 git 忽略） |
| `document/` | 参考文档（`mcp.md` 最完整，列出全部 MCP 工具） |

## 配置（JSON 文件，非环境变量）

- `config.json`：服务配置，运行时改后需调用 `reload_config()` 或 `/assist/api/config/reload` 端点才生效。
- `system.json`：系统配置，改后**必须重启进程**才生效（进程内缓存）。
- 配置读取层：`backend/app/config_manager.py`（带锁 + 进程内缓存）；`backend/app/config.py:Settings` 是单例代理，属性每次从 JSON 实时读取。
- 添加新配置字段时：在 `backend/app/config_manager.py` 的 `DEFAULT_CONFIG` / `DEFAULT_SYSTEM` 加默认值；若是敏感字段，扩展 `mask_sensitive` / `mask_system_config` 脱敏函数。
- 文件路径来自 `system.json` 的路径模板（含 `{id}` 占位符），在 `backend/app/paths.py` 解析，**不要**在 `Document` 模型里加路径列。
- 环境变量极少：`HF_ENDPOINT=https://hf-mirror.com`、`HF_HUB_DISABLE_XET=1`（Fastembed 中国镜像，硬编码在 `backend/scripts/fastembed_server.py`）；`OKB_ASSIST_TOKEN`/`OKB_ASSIST_URL` 被部分脚本与 OpenWebUI 集成读取。

## 架构与请求流

1. **Web/API 层**：`backend/okb_assist_main.py` + `backend/app/routers/*`。`TokenMiddleware` 仅对 `/assist/api/*` 校验 `X-Token`/query `token`，放行 `/assist/mcp`、`/assist/assets`（前端静态资源）、`/assist/uploads`、`/assist/file`、`/redirect`，以及对 `/assist/api/documents/` 下含 `/image/` 的图片 URL 放行。`token` 为 `change-me`（或未设置）时整体跳过校验；来自 `192.168.1.0/24` 局域网的请求也免校验（见 `okb_assist_main.py` 的 `TokenMiddleware`）。CORS 限定前端来源（开发 `localhost:5173`，生产同源 `localhost:5001`）。
2. **数据模型**：`Document`（主记录 + 状态）、`DocumentVectorIndex`（每个向量库的索引状态，唯一键 `(document_id, vector_db_id)`）。状态机用 `DocStatus` / `IndexStatus` 枚举（`backend/app/models.py`）。
3. **摄取流水线**（`backend/app/routers/pipeline.py`，核心状态机）：
   - parse（MinerU）→ `parsing` → `markdown_done`
   - extract（Ollama）→ `extracting` → `meta_done`
   - index（向量库）→ `indexing` → `indexed`
   - 含批量控制器、暂停/恢复/重置、信号量并发限制（受 `max_concurrent_tasks` 约束）。
   - **所有阶段均在后台执行**（见下方「后台任务」一节）—— 请求先返回，协程在事件循环继续；进度与状态机记录在 `Document` 上，可轮询查询。
4. **文档管理**（`documents.py`）：CRUD、上传、按路径登记、语义搜索 `/search`、全文搜索 `/grep-search`、`/assist/markdown` 读写、PDF/图片服务、去重。
5. **配置/管理**（`config.py`、`admin.py`）：查看/更新服务配置、重连测试、统计、迁移、索引重置。
6. **向量库抽象**（`vector_db.py` 工厂 `get_vector_db(db_id)` → Qdrant/Milvus/Chroma 适配器）。
7. **MCP 服务**（`backend/app/mcp_server.py`）：Streamable HTTP 端点 `/assist/mcp/stream`，旧版 SSE 挂载在 `/assist/mcp`；Bearer token 用 `system.json` 的 `mcp_token` 校验。完整工具列表见 `document/mcp.md`。

## 后台任务（异步执行）

后端大量耗时操作**不在请求内同步完成**，而是在后台运行，请求通常立即返回、由前端轮询状态。理解这一点对排查"为什么改了没生效 / 任务卡住"至关重要。

- **机制**：摄取阶段（parse / extract / crossref / extract-pdf-meta / index / process）及批量端点（`/batch/start`、`/batch/resume`、`/batch/start-parse`、`/batch/start-extract`、`/batch/start-index`、`/batch/start-full`）通过 FastAPI `BackgroundTasks` 提交协程。协程在 HTTP 响应发出后由同一个事件循环调度（协作式异步，**不是独立线程**），状态机（`DocStatus`）流转记录在数据库中。
- **并发限制**：受 `asyncio.Semaphore(max_concurrent_tasks)` 约束（`system.json` 默认 `3`）。同时运行的任务数不超过该值，超出排队。
- **批量进度 / 暂停状态存于进程内存全局变量**：`_batch_progress`、`_batch_paused`、`_running_tasks` 等是模块级变量，**进程重启即丢失**。暂停（`/batch/pause`）只在处理完当前任务后生效；重启后由 `lifespan` 中的 `reset_stuck_tasks()` 把卡在 `parsing`/`extracting`/`indexing` 的文档重置回上一状态（见 `okb_assist_main.py`）。因此**批量任务进度不持久化**，重启后需手动重新触发。
- **真·fire-and-forget**：删除文档（`documents.py` 的 `delete_document`）时，SQLite 记录与本地文件先同步删除，而 Qdrant 中对应向量点的删除通过 `asyncio.ensure_future`（`app/services/qdrant.py:delete_document_points`）脱离请求生命周期异步执行，**请求不等待、失败也不可见**。
- **前台（非后台）操作**：连接测试 `/assist/api/config/test`、服务状态 `/assist/api/admin/services/status`、语义/全文搜索、上传均在前端 `await` 内同步完成。MCP 端点是唯一的流式长连接通道（`/assist/mcp/stream` 与 `/assist/mcp` SSE），但工具函数内部检索仍是同步 await。
- **无定时任务**：全仓库没有 `APScheduler`/`schedule`/守护线程；后台仅为事件循环协程，不要假设存在周期性任务。

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
pnpm run type-check  # TypeScript 类型检查
```

开发时前端独立端口运行，通过 Vite proxy 访问后端 API。生产环境构建后由 FastAPI 直接 serving 静态文件。

## 前端架构

### 构建与部署
- **包管理器**：`pnpm`（锁文件 `pnpm-lock.yaml`）
- **构建工具**：Vite 8，输出到 `frontend/dist/`
- **base 路径**：`vite.config.ts` 设 `base: '/assist/'`，因此所有 JS/CSS 资源 URL 以 `/assist/assets/` 开头。后端 `okb_assist_main.py` 的 `serve_spa` 路由通过 `/assist/{full_path:path}` 匹配资源路径并返回对应文件。
- **Vite proxy**：开发模式下代理 `/assist` → `http://localhost:5001`，前后端分离开发

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
- **分屏预览**：编辑+预览并排，预览仅渲染编辑器**视口可见行**（通过 `getFirstVisibleRow()` + `screenToDocumentRow()` 实现）
- **全屏模式**：固定定位铺满窗口
- **搜索替换**：Ctrl+F / Ctrl+H（`ext-searchbox`）
- **字体缩放**：Ctrl+滚轮 / A+ A− 按钮（范围 8-30px）
- **撤销/重做**：Ctrl+Z / Ctrl+Shift+Z
- **光标/选中信息**：行号列号 + 选中字符数 + 总字数
- **数学公式**：预览通过 `MarkdownViewer` 组件渲染 KaTeX（`katex@0.18.4`）
- **模块加载**：核心模块（theme/mode）静态 import 注入 AMD 系统；`ext-searchbox` 和 `ext-language_tools` 动态 import（处理自引用依赖）。生产环境 `basePath` 使用 CDN 兜住 extension 的合法动态请求。

### MarkdownViewer 渲染组件（`src/components/MarkdownViewer.vue`）
- 使用 `marked` 解析 Markdown（GFM + 换行）
- 通过 `renderMath()`（`src/utils/mathRenderer.ts`）预处理 KaTeX 公式（`$$...$$` 块级、`$...$` 行内）
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
- 函数签名与 Pydantic 模型普遍使用类型注解。
- 使用 PEP 604 联合类型（`str | None`），需 Python 3.10+（项目锁定 3.12，勿降级语法）。
- 状态用 `enum.Enum`（`DocStatus`、`IndexStatus`）。
- 生成的 MinerU 客户端（`backend/app/mineru_fast_api/`）勿手改，应重新生成。
- 新增路径需求改 `system.json` 模板，而非数据库。

## ⚠️ 易错点（编辑前必读）


2. **配置改动不会自动生效**：`config.json` 改后需 reload；`system.json` 改后需重启进程（进程内缓存）。
3. **MCP 路由注册顺序有依赖**（`backend/okb_assist_main.py`）：必须在 SSE 挂载 `app.mount("/assist/mcp", ...)` **之前**，用 `app.add_route("/assist/mcp/stream", ...)` 精确注册 Streamable HTTP 端点，且在模块加载时完成。否则 `/assist/mcp/stream` 会被 SSE 挂载吞掉或 404。**不要“整理”这个顺序。**
4. **`mcp` 依赖锁定 `<2`**（`pyproject.toml`）。曾因升级到 2.x 导致 MCP 端点失效。不要擅自升到 2.x，除非重新验证 MCP 端点。
5. **没有测试、没有 lint**：编辑后无法跑测试验证。应在 `backend/` 目录手动 `uv run okb_assist_main.py` 确认能启动，并用 curl 校验端点。`backend/app/routers/pipeline.py` 与 `backend/app/routers/documents.py` 是大文件，改动要小心。
6. **硬编码的局域网 IP**（`192.168.1.x`）出现在 `config.json`、`system.json`、脚本中，是部署相关配置，视为环境配置而非代码。`okb_assist_main.py` 的 `TokenMiddleware` 另把 `192.168.1.0/24` 作为 **LAN 免 Token 白名单**硬编码，改动需谨慎。
8. **SQLite 单写者**：`database.py` 设 `check_same_thread=False`，并发写入可行但仍是瓶颈，勿引入大量并发写。
9. **SPA Fallback 路由**（`backend/okb_assist_main.py`）：`@app.get("/assist/{full_path:path}")` 必须放在所有路由之后，它会拦截所有 `/assist/*` 请求并返回 `frontend/dist/index.html`。命中 `api/`、`mcp/`、`uploads/`、`file/` 前缀时会被重定向到带斜杠地址或返回 404，避免吞掉 API 和 MCP 端点。
10. **前端构建产物在 `frontend/dist/`**：`pnpm run build` 输出到 `frontend/dist/`，构建后 `frontend/dist/index.html` 是 SPA 入口。`vite.config.ts` 的 `base: '/assist/'` 使所有资源 URL 以 `/assist/assets/` 开头，与后端 `serve_spa` 路由匹配。
11. **CORS 已限定**：开发环境只允许 `localhost:5173`，生产环境只允许同源 `localhost:5001`。如需其他前端域名，修改 `backend/okb_assist_main.py` 中的 `allow_origins` 列表。

## 入口与关键文件速查

- 入口/配置：`backend/okb_assist_main.py`、`backend/app/config_manager.py`、`backend/app/config.py`、`backend/config.json`、`backend/system.json`
- 架构核心：`backend/app/routers/pipeline.py`、`backend/app/routers/documents.py`、`backend/app/models.py`、`backend/app/services/vector_db.py`、`backend/app/mcp_server.py`
- 前端：`frontend/`（Vue 3 + TypeScript + Vite）
- MCP 工具参考（务必链接）：`document/mcp.md`
- 启动说明：`README.md`
