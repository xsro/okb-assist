# CODEBUDDY.md — OKB-Assist

面向 AI 编程助手（CodeBuddy / Claude / Cursor）的工作指南。修改本仓库前先读此文件。

## 项目简介

OKB-Assist 是一个**本地学术文献库管理系统**（论文与专著）。流程：上传/登记 PDF → MinerU 解析为 Markdown → Ollama 抽取元数据 → 向量库（默认 Qdrant）语义建索引 → 通过 Web UI 或 MCP 服务检索。

- 语言：Python >=3.12
- 主框架：FastAPI + Uvicorn（ASGI），Jinja2 服务端渲染页面
- 数据：SQLAlchemy 2.0 + SQLite（`okb_assist.db`）
- 向量库：Qdrant（默认），适配器层同时支持 Milvus / Chroma
- 外部服务：MinerU（解析）、Ollama（LLM/嵌入）、Qdrant（向量）、Fastembed（嵌入服务）、OpenWebUI（可选前端）

## 包管理与常用命令

包管理器是 `uv`（`uv.lock` 已提交，`uv.toml` 指向 SUSTech 镜像）。**没有 Makefile、没有 lint/test 目标、没有 pre-commit。**

```bash
uv sync                                  # 安装依赖（或 uv pip install -e .）
uv run okb_assist_main.py                # 启动主程序（uvicorn，host 0.0.0.0，port 5001，reload=True）
uv run uvicorn okb_assist_main:app --host 0.0.0.0 --port 5001   # 生产式启动
uv run scripts/fastembed_server.py       # 启动 Fastembed 嵌入服务
uv run scripts/<name>.py ...             # 运行维护脚本（见 scripts/）
```

> 注意：`README.md` 里 `uv run okb_assist_main.py --host 0.0.0.0` 的 `--host` 参数脚本并不解析（无 argparse），会被静默忽略，host 本就是 `0.0.0.0`。

## 目录结构

| 路径 | 说明 |
|------|------|
| `okb_assist_main.py` | **入口**：创建 FastAPI app、挂载路由/MCP/中间件 |
| `app/routers/` | 路由组：`documents.py`(955 行)、`pipeline.py`(1369 行)、`admin.py`、`config.py`、`openapi.py` |
| `app/services/` | 后端适配器：`qdrant.py`、`ollama.py`、`mineru.py`、`milvus.py`、`chroma.py`、`grep_search.py`、`vector_db.py`（抽象接口） |
| `app/mineru_fast_api/` | MinerU 解析服务的**自动生成 OpenAPI 客户端**（勿手改） |
| `app/templates/` | Jinja2 页面模板 |
| `app/models.py`、`app/database.py` | 数据模型与 SQLite 连接（`init_db()` 启动时建表） |
| `app/config_manager.py`、`app/config.py` | 配置加载（JSON 文件，带缓存） |
| `app/paths.py` | 由 `system.json` 模板解析 PDF/Markdown/info/asset 路径 |
| `app/mcp_server.py` | MCP 服务（`FastMCP("OKB-Assist")`，10 个工具） |
| `scripts/` | 维护/迁移脚本 + shell 启动器 |
| `static/` | 前端资源 `app.js`、`style.css` |
| `config.json` | 服务配置（MinerU/Ollama/向量库），**git 忽略，可由 UI 编辑** |
| `system.json` | 系统配置（token、DB URL、上传路径、路径模板），**git 忽略，需手动改** |
| `data/`、`uploads/` | 运行时数据（均 git 忽略） |
| `document/` | 参考文档（`mcp.md` 最完整，列出全部 MCP 工具） |

## 配置（JSON 文件，非环境变量）

- `config.json`：服务配置，运行时改后需调用 `reload_config()` 或 `/assist/api/config/reload` 端点才生效。
- `system.json`：系统配置，改后**必须重启进程**才生效（进程内缓存）。
- 配置读取层：`app/config_manager.py`（带锁 + 进程内缓存）；`app/config.py:Settings` 是单例代理，属性每次从 JSON 实时读取。
- 添加新配置字段时：在 `DEFAULT_CONFIG` / `DEFAULT_SYSTEM` 加默认值；若是敏感字段，扩展 `mask_sensitive` / `mask_system_config` 脱敏函数（`config_manager.py`）。
- 文件路径来自 `system.json` 的路径模板（含 `{id}` 占位符），在 `app/paths.py` 解析，**不要**在 `Document` 模型里加路径列。
- 环境变量极少：`HF_ENDPOINT=https://hf-mirror.com`、`HF_HUB_DISABLE_XET=1`（Fastembed 中国镜像，硬编码在 `okb_assist_main.py` 与 `scripts/fastembed_server.py`）；`OKB_ASSIST_TOKEN`/`OKB_ASSIST_URL` 被部分脚本与 OpenWebUI 集成读取。

## 架构与请求流

1. **Web/API 层**：`okb_assist_main.py` + `app/routers/*`。`TokenMiddleware` 仅对 `/assist/api/*` 校验 `X-Token`/query `token`，放行 `/assist/mcp`、`/assist/static`、`/assist/uploads`、`/assist/file`、`/redirect`、图片 URL。CORS 全开。
2. **数据模型**：`Document`（主记录 + 状态）、`DocumentVectorIndex`（每个向量库的索引状态，唯一键 `(document_id, vector_db_id)`）。状态机用 `DocStatus` / `IndexStatus` 枚举（`app/models.py`）。
3. **摄取流水线**（`app/routers/pipeline.py`，核心状态机）：
   - parse（MinerU）→ `parsing` → `markdown_done`
   - extract（Ollama）→ `extracting` → `meta_done`
   - index（向量库）→ `indexing` → `indexed`
   - 含批量控制器、暂停/恢复/重置、信号量并发限制（受 `max_concurrent_tasks` 约束）。
4. **文档管理**（`documents.py`）：CRUD、上传、按路径登记、语义搜索 `/search`、全文搜索 `/grep-search`、`/assist/markdown` 读写、PDF/图片服务、去重。
5. **配置/管理**（`config.py`、`admin.py`）：查看/更新服务配置、重连测试、统计、迁移、索引重置。
6. **向量库抽象**（`vector_db.py` 工厂 `get_vector_db(db_id)` → Qdrant/Milvus/Chroma 适配器）。
7. **MCP 服务**（`mcp_server.py`）：Streamable HTTP 端点 `/assist/mcp/stream`，旧版 SSE 挂载在 `/assist/mcp`；Bearer token 用 `system.json` 的 `mcp_token` 校验。完整工具列表见 `document/mcp.md`。

## 路由前缀

- 页面：`/assist/`、`/assist/detail/{id}`、`/assist/markdown/{id}`、`/assist/upload`、`/assist/admin`、`/assist/config`、`/assist/monitor`、`/assist/duplicates` 等
- API：`/assist/api/documents`、`/assist/api/pipeline`、`/assist/api/admin`、`/assist/api/config`、`/assist/openapi`
- 文件别名（免 token）：`/assist/file/{filename}`
- MCP：`/assist/mcp/stream`（Streamable HTTP）、`/assist/mcp`（SSE）

## 编码约定

- **注释与 docstring 用中文**，与现有代码保持一致。
- 函数签名与 Pydantic 模型普遍使用类型注解。
- 使用 PEP 604 联合类型（`str | None`），需 Python 3.10+（项目锁定 3.12，勿降级语法）。
- 状态用 `enum.Enum`（`DocStatus`、`IndexStatus`）。
- 生成的 MinerU 客户端（`app/mineru_fast_api/`）勿手改，应重新生成。
- 新增路径需求改 `system.json` 模板，而非数据库。

## ⚠️ 易错点（编辑前必读）

1. **`.vscode/launch.json` 的 `program` 写的是 `main.py`，但实际入口是 `okb_assist_main.py`，不存在 `main.py`**。调试该配置会失败。
2. **配置改动不会自动生效**：`config.json` 改后需 reload；`system.json` 改后需重启进程（进程内缓存）。
3. **MCP 路由注册顺序有依赖**（`okb_assist_main.py`）：必须在 SSE 挂载 `app.mount("/assist/mcp", ...)` **之前**，用 `app.add_route("/assist/mcp/stream", ...)` 精确注册 Streamable HTTP 端点，且在模块加载时完成。否则 `/assist/mcp/stream` 会被 SSE 挂载吞掉或 404。**不要“整理”这个顺序。**
4. **`mcp` 依赖锁定 `<2`**（`pyproject.toml`）。曾因升级到 2.x 导致 MCP 端点失效。不要擅自升到 2.x，除非重新验证 MCP 端点。
5. **没有测试、没有 lint**：编辑后无法跑测试验证。应手动 `uv run okb_assist_main.py` 确认能启动，并用 curl 校验端点。`pipeline.py`(1369 行) 与 `documents.py`(955 行) 是大文件，改动要小心。
6. **硬编码的局域网 IP**（`192.168.1.x`）出现在 `config.json`、`system.json`、脚本中，是部署相关配置，视为环境配置而非代码。
7. **`uploads/` 按文档数字 ID 建子目录**（git 忽略），`_next_available_id` 与别名逻辑依赖此布局，勿改。
8. **SQLite 单写者**：`database.py` 设 `check_same_thread=False`，并发写入可行但仍是瓶颈，勿引入大量并发写。

## 入口与关键文件速查

- 入口/配置：`okb_assist_main.py`、`app/config_manager.py`、`app/config.py`、`config.json`、`system.json`
- 架构核心：`app/routers/pipeline.py`、`app/routers/documents.py`、`app/models.py`、`app/services/vector_db.py`、`app/mcp_server.py`
- MCP 工具参考（务必链接）：`document/mcp.md`
- 启动说明：`README.md`
