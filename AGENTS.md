# AGENTS.md — OKB-Assist

面向 AI 编程助手的工作指南。修改本仓库前先读此文件。



如果在嵌入式设备上运行，例如 orangepi，请不要编译后端，执行代码检查即可。
```
cargo check
```

## 项目简介

OKB-Assist 是一个**本地学术文献库管理系统**（论文与专著）。流程：上传/登记 PDF → MinerU 解析为 Markdown → Ollama 抽取元数据 → 向量库（默认 Qdrant）语义建索引 → 通过 Web UI 或 MCP 服务检索。

- **后端**：Rust（Axum + SQLx + Tokio），位于 `backend-rs/`
- **前端**：TypeScript + Vue 3（Vite 8 构建），位于 `frontend/`
- **数据**：SQLx + SQLite（`okb_assist.db`）
- **向量库**：Qdrant（默认），适配器层同时支持 Milvus / Chroma
- **外部服务**：MinerU（解析）、LLM（Ollama 原生 / OpenAI 兼容协议，模型池 + 信息补全模型）、Embedding 服务（各向量库独立配置，默认 Ollama 原生协议）、Qdrant（向量）、Fastembed（嵌入服务）、OpenWebUI（可选前端）、mutool（PDF 元数据提取，未启用 mupdf feature 时使用）
- **客户端工具**：`client-rs/` — Zotero CSV 导入工具（Rust）
- **用户体系**：注册 / 密码登录 / Token 登录 / MCP 授权，用户数据存 `config.json` 的 `users` 段（详见下方「用户与认证」）
- **网盘**：每个用户一个私有目录 + 管理员创建的公有文件夹（按角色阈值授权），与文献库完全独立（详见下方「用户网盘」）

## 包管理与常用命令

### 后端（Rust）

后端代码在 `backend-rs/` 目录下，使用 Cargo：

```bash
cd backend-rs
cargo build                  # 编译（debug，默认不启用 mupdf，使用外部 mutool）
cargo build --features mupdf # 编译（debug，启用内置 mupdf）
cargo build --release        # 编译（release，默认不启用 mupdf）
cargo build --release --features mupdf # 编译（release，启用内置 mupdf）
cargo run                    # 启动（默认 host 0.0.0.0、port 5001）
cargo run --features mupdf   # 启用内置 mupdf 启动
cargo run -- --host 0.0.0.0 --port 5001 --log-level debug   # 指定参数启动
```

> `okb_assist` 支持 argparse 等价命令行参数：`--host`、`--port`（默认 5001）、`--log-level`（trace/debug/info/warn/error，默认 info）。
>
> **Feature `mupdf`**：默认不启用，使用外部 `mutool` 工具提取 PDF 元数据。启用后使用内置 `mupdf` 库，编译时间更长但无需外部依赖。

### 前端

```bash
cd frontend
pnpm install         # 安装前端依赖
pnpm run dev         # 启动开发服务器（端口 5173，代理 /assist 到后端 5001）
pnpm run build       # 构建到 frontend/dist/
```

> `pnpm run type-check` 因 `vue-tsc` 与当前 TypeScript 版本不兼容而不可用，构建通过即视为类型检查通过。

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
| `backend-rs/src/auth.rs` | 角色判定（`Role`/`assert_role`）、凭证解析（`Principal`/`resolve_principal`）、首次启动引导 |
| `backend-rs/src/users.rs` | 用户存储（`config.json` 的 `users` 段，Argon2id 密码、sha256 令牌、最后管理员不变量） |
| `backend-rs/src/session.rs` | 服务端会话表（`sess_…`，内存态，7 天滑动续期） |
| `backend-rs/src/logger.rs` | 三路日志：文件 + 控制台 + **进程内环形缓冲**（实时日志数据源，见易错点 22） |
| `backend-rs/src/routers/` | 路由组：`documents.rs`、`pipeline.rs`、`chat.rs`、`admin.rs`、`config.rs`、`auth.rs`、`users.rs`、`drive.rs`、`openapi.rs`、`mod.rs` |
| `backend-rs/src/services/drive.rs` | 用户网盘：路径安全、角色阈值、配额统计、目录列举、Content-Type 推断 |
| `backend-rs/src/services/pdf_engine.rs` | PDF 解析引擎运行时选择（`mutool` 命令行 / 内置 `mupdf`，含能力兜底） |
| `backend-rs/src/services/` | 后端适配器：`bibtex.rs`（BibTeX 解析 / LaTeX 还原）、`file_state.rs`（文件事实 → 文献状态推导 + 运行时状态注册表）、`llm.rs`（LLM/Embedding，支持 ollama 原生与 openai 兼容协议）、`qdrant.rs`、`mineru.rs`、`grep_search.rs`、`crossref.rs`、`pdf_meta.rs`、`citation.rs`（BibTeX / GB/T 7714 引用条目）、`vector_db.rs`、`datalab.rs`、`query_parser.rs`、`chat_store.rs`（问答会话文件存取）、`image_cache.rs`（图片资源包按 doc 的 LRU 缓存） |
| `backend-rs/src/config_manager.rs`、`backend-rs/src/config.rs` | 配置加载（JSON 文件，带缓存） |
| `backend-rs/src/database.rs`、`backend-rs/src/models.rs` | 数据模型与 SQLite 连接 |
| `backend-rs/src/paths.rs` | 由 `system.json` 模板解析 PDF/Markdown/info/asset 路径 |
| `backend-rs/src/mcp_server.rs` | MCP 服务 |
| `backend-rs/src/settings.rs`、`backend-rs/src/utils.rs` | 全局 Settings 单例、工具函数（含 `resolve_token`：解析 `!cat <path>` 形式的敏感值） |
| `backend-rs/config.json` | 服务配置（MinerU/LLM/向量库），可由 UI 编辑 |
| `backend-rs/system.json` | 系统配置（token、MCP token、白名单、DB URL、上传路径、路径模板），需手动改 |
| `client-rs/` | Zotero CSV 导入客户端（Rust）。配置默认读 `~/.okb/client.json`（`url`/`base_url`、`token`、`storage_root`），成功拉到的 doi-hash 全量索引会自动写到 `~/.okb/client/docs.json` |
| `frontend/src/components/` | Vue 组件 — 见下方「前端架构 → 组件一览」 |
| `frontend/src/views/` | 页面视图 — 见下方「前端架构 → 路由」 |
| `frontend/src/router/index.ts` | Vue Router 配置（14 路由，`base: '/assist/'`） |
| `frontend/src/stores/` | Pinia 存储：`pipeline.ts`、`toast.ts`、`token.ts`、`error.ts` |
| `frontend/src/composables/` | 组合式函数：`useToast.ts`、`useRequireToken.ts`、`useResponsive.ts`、`usePagination.ts`、`useDebounce.ts` |
| `frontend/src/types/` | TypeScript 类型定义：`document.ts`、`config.ts`、`pipeline.ts` |
| `frontend/src/utils/` | 工具：`mathRenderer.ts`（KaTeX/MathJax 公式渲染）、`searchParser.ts`（搜索语法解析） |
| `frontend/src/api/` | API 客户端：`client.ts`、`documents.ts`、`pipeline.ts`、`chat.ts`、`admin.ts`、`config.ts` |
| `frontend/` | **Vue 3 + TypeScript SPA 前端**（Vite 8 构建，输出到 `frontend/dist/`，包管理器 `pnpm`） |
| `frontend/dist/` | 前端构建产物（`index.html`、`assets/`），由后端 serving |
| `data/` | 运行时数据（`okb_assist.db`、`_uploads/`、`tokens/` 等），由 `system.json` 模板决定路径；实际位置为 `backend-rs/data/`，已被 `backend-rs/.gitignore` 忽略 |
| `document/` | 参考文档（同时是 **docsify 文档站**，由 `system.json` 的 `static_dirs` 挂到 `/document`）：`index.html`（docsify 入口）、`README.md`（首页/索引）、`_sidebar.md`（左侧目录）、`_404.md`、`assets/`（自有样式与登录守卫）；内容页 `mcp.md`（MCP 连接指南）、`mcp-tools.md`（工具列表）、`user-management.md`（用户/登录/令牌）、`drive.md`（网盘）、`build.md`（部署构建）、`search.md`（搜索语法）、`webdav-plan.md`（网盘 WebDAV 同步方案与 TODO）、`db-status-removal-plan.md`（文献状态去库化）、`datalab.to.md`、`openwebui.md`、`qdrant.md`、`udisks2-automount.md`、`mineru4/`、`skills/` |

## 配置（JSON 文件，非环境变量）

- `backend-rs/config.json`：服务配置（MinerU/LLM/向量库），运行时改后需调用 `reload_config()` 或 `/assist/api/config/reload` 端点才生效。
- `backend-rs/system.json`：系统配置，改后**必须重启进程**才生效（进程内缓存）。
- 配置读取层：`backend-rs/src/config_manager.rs`（带锁 + 进程内缓存）；`backend-rs/src/config.rs:Settings` 是单例代理，属性每次从 JSON 实时读取。
- 添加新配置字段时：在 `config_manager.rs` 的默认值中加默认值；若是敏感字段，扩展脱敏函数。
- `config.json` 中 MinerU 配置支持多配置数组，通过 `active_mineru` 按 `name` 选择当前使用的配置。每个配置的 `max_tasks` 控制该 MinerU 服务的并发解析数（对应 `pipeline.rs` 中独立的 `MINERU_SEMAPHORE`），`task_timeout` 控制单任务超时，`token` 为 API 认证凭据。
- 文件路径来自 `system.json` 的路径模板（含 `{id}` 占位符），在 `backend-rs/src/paths.rs` 解析，**不要**在 `Document` 模型里加路径列。

### LLM / 信息补全 / Embedding 配置（`config.json`）

三套模型配置相互独立，互不影响：

- `llm.models`：**LLM 模型池**（数组，每项 `{name, protocol, url, key, model, tool_mode?}`）。前端「问答」tab（工具页 `/assist/tools?tab=chat`）用它在池中选择模型对话（见下方「问答（Chat）」）。`name` 用于其他配置引用；`tool_mode`（`auto`/`native`/`prompt`，缺省 `auto`）控制该模型的工具调用协议。
- `llm_extract`：**信息补全（元数据抽取）专用模型**，字段 `{from, protocol, url, key, model}`。**留空的字段从 `from` 指定的模型池条目继承**（`from` 为空或未命中则取第一项）；`model` 也可留空继承。
  同一段还存放与模型无关的**抽取策略**：`prompt`（自定义提示词模板，空 = 内置默认，支持 `{content}` / `{metadata}` 占位符）、`max_input_chars`（送入模型的正文最大字符数，默认 6000）、`max_tokens`（默认 2048）、`overwrite`（默认 `true`；`false` = 只补空字段）。
- `vector_dbs[].embedding`：**Embedding 配置**，字段 `{protocol, url, key, model}`，每个向量库各自一份，**与 LLM 模型池完全独立**。字段缺失时使用自身默认值（`protocol` = `ollama`、`url`/`key` 为空、`model` = `nomic-embed-text`），不从模型池继承。
- 解析入口（`backend-rs/src/config.rs`）：`Settings::llm_models()`、`Settings::llm_extract_config()`、`Settings::extract_client()`、`Settings::extract_options(existing)`（提示词模板 + 截断长度 + `overwrite`）、`Settings::embedding_config_for(vdb)`；模型类均返回 `ModelEndpoint`（可 `.client()` 得到 `LlmClient`）。
- **协议差异由 `LlmClient`（`backend-rs/src/services/llm.rs`）内部处理**，调用方无需关心：
  - Ollama 原生：`/api/generate`、`/api/embed`、`/api/tags`；生成结果取 `response`，向量取 `embeddings`（**元素是裸数组**）。
  - OpenAI 兼容：`/v1/chat/completions`、`/v1/embeddings`、`/v1/models`；生成结果取 `choices[0].message.content`，向量取 `data[].embedding`（按 `index` 排序）。
  - OpenAI 协议的 `url` 未以 `/v1` 结尾时自动补全（如 `http://127.0.0.1:11434` → `http://127.0.0.1:11434/v1`）。
- **配置归一化**（`ConfigManager::load_service_config()` → `normalize_service_config()`，作用于磁盘原始内容、与默认值合并之前，幂等）只补齐结构性缺省值，不识别任何历史格式：
  - `llm.models` 必须是数组，每项补齐 `name`（`model-N`）与 `protocol`（`ollama`）；文件未提供 `llm` 时不凭空创建空池，否则会盖掉默认模型池（`deep_merge` 对数组是整体替换）。
  - 各向量库 `embedding` 补齐 `protocol`（`ollama`）、`url`/`key`（空串）、`model`（`nomic-embed-text`）。
  - `users` 段必须是对象、`items` 必须是数组。
- 前端对应组件：`LlmConfigSection.vue`（模型池增删/排序/单项连接测试）、`LlmExtractConfigSection.vue`（`from` 选择 + 继承占位符 + 生效值预览 + 提示词模板/截断长度/`overwrite` 开关）、`VectorDbConfigSection.vue`（每个向量库的嵌入协议/URL/Key/模型）。连接测试走 `/assist/api/config/test`：`type=llm` 用请求体参数（表单实时测试），`type=llm_extract` 由服务端按已保存配置（含继承）解析。
- **敏感字段写 `!cat <path>` 引用，不要把密钥写进 JSON**：`mineru[].token` 以及 `llm.models[].key` 都支持以 `!` 开头的值 —— `resolve_token()`（`backend-rs/src/utils.rs`）会读取 `!cat <path>` 指向的文件（路径相对进程 `cwd`，即 `backend-rs/`）并去首尾空白，结果进程内缓存。**LLM 密钥的解析发生在 `ModelEndpoint::client()` 与连接测试的 `test_llm()`**，因此 `config.json` 与 `/assist/api/config` 里只会出现 `!cat ./data/tokens/xxx` 这样的引用，不回显真实密钥；密钥文件放在 `backend-rs/data/tokens/`（已被 git 忽略）。
- 服务状态端点键为 `llm`（不再是 `ollama`），如 `/assist/api/admin/services/status` 返回 `{mineru, llm, fastembed, vector_dbs, qdrant}`；其中 `llm` 探测的是**信息补全所用的端点**（含 `protocol`/`url`/`model`/`models`）。
- `PUT /assist/api/config` 会校验 `llm.models` 每项都有非空 `url` 与 `model`，否则返回 400。`llm_extract` 不做校验（空值即继承）。

### PDF 解析引擎（`config.json` 的 `pdf` 段）

PDF 相关能力（元数据/Info 字典/正文抽取、PDF 信息报告、**PDF 健康检查**）有两套实现，**运行时**按配置选：

```json
{ "pdf": { "engine": "mutool" } }
```

| `engine` | 实现 | 何时可选 |
|---|---|---|
| `mutool`（默认） | 外部命令行 `mutool`（`system.json` 的 `mutool_path`） | 永远可选 |
| `mupdf` | 内置 `mupdf` 库 | 仅当二进制用 `--features mupdf` 编译 |

- 解析入口：`services/pdf_engine.rs` —— `configured()`（只反映配置值）、`active()`（配置值 + 编译特性兜底，选 `mupdf` 但没编入 feature 时自动回退 `mutool`）、`MUPDF_AVAILABLE`（`cfg!(feature = "mupdf")`）。
- **两套实现始终编译**（mutool 路径只依赖 `std::process::Command`），`pdf_meta` / `pdf_health` 的公共入口按 `active()` 分发；`mupdf` 那份仍在 `#[cfg(feature = "mupdf")]` 下。
- 保存时校验：`engine` 只接受 `mutool` / `mupdf`，否则 400；选了 `mupdf` 但当前二进制没编入 feature 也返回 400（回退只用于「配置值来自换过的二进制」这种场景，不在写入口悄悄改写用户选择）。
- `GET /assist/api/config` 额外下发只读的 `pdf_capabilities: {mutool: true, mupdf: bool, active: "mutool"|"mupdf"}`，前端据此禁用「内置 mupdf」选项并标出实际生效的引擎。
- 前端：`components/config/PdfEngineSection.vue`（单选卡片 + 生效中/不可用标记；选 mupdf 时提示「检不出可修复的 xref 损坏」）。

### 用户与认证（`config.json` 的 `users` 段）

用户数据存在 `config.json` 的 `users` 段，由 `backend-rs/src/users.rs` 独占读写（其他代码不要直接改这段）：

```json
{
  "users": {
    "version": 1,
    "allow_registration": true,
    "require_approval": true,
    "registration_role": "view-only",
    "registration_mcp": true,
    "session_ttl_hours": 168,
    "items": [
      {
        "id": "u_3c420f47d351178f",
        "username": "admin",
        "display_name": "管理员",
        "role": "admin",
        "status": "active",
        "mcp": true,
        "password_hash": "$argon2id$v=19$m=19456,t=2,p=1$…",
        "must_set_password": false,
        "api_tokens": [
          { "id": "at_0ba285287ecd", "label": "zotero", "token_sha256": "…", "scope": "api",
            "created_at": "…", "last_used_at": null },
          { "id": "at_0ba285287ece", "label": "mcp", "token_sha256": "…", "scope": "mcp",
            "token": "okb_…（明文，仅 MCP 令牌有）",
            "created_at": "…", "last_used_at": null }
        ],
        "created_at": "…", "updated_at": "…",
        "failed_logins": 0, "locked_until": null
      }
    ]
  }
}
```

- **密码**：Argon2id PHC 字符串（`Argon2::default()` = m=19456,t=2,p=1），`password_hash` 永不出现在任何 API 响应/日志中（统一走 `User::to_public_json()`）。
- **长期凭证**：鉴权始终只认 `sha256`（`token_sha256`）。明文留存策略按作用域区分：
  - **`scope = mcp`**：**额外存明文字段 `token`**（`ApiToken::token`），并随 `to_public_json()` 回显（响应字段 `api_tokens[].token`）。
    这样账户页与「工具 → MCP 配置」页可以随时回显令牌，直接填进 MCP 客户端配置；
    `ApiToken::plaintext()` 保证只有 MCP 作用域才会吐出明文（API 令牌即使字段有值也不回显）。
    改版前签发的 MCP 令牌没有该字段（哈希不可逆），前端按「明文不可恢复，请重新签发」处理。
  - **`scope = api`**：只存哈希，明文**只在签发响应里出现一次**。
  - 两种来源：
  - **自动生成**：`okb_<32B base64url>`（`users::generate_api_token()`）；
  - **用户自定义**：请求体带 `token` 字段，按 `users::validate_token_plaintext()` 校验 —— 长度
    `MIN_TOKEN_LEN..=MAX_TOKEN_LEN`（8–256 字符）、仅可见 ASCII（`is_ascii_graphic()`，禁止空格/中文/控制字符），
    并**全局唯一**（同一明文不能登记到两个账号，`UserError::TokenTaken` → 409）；同一用户重复登记幂等。
    自定义时建议只用 `[A-Za-z0-9_-]`：静态目录的 `okb_static` cookie 只接受 URL-safe 字符，
    含 `.`/`!` 等字符的令牌走 `?token=` 能认证，但页内相对资源拿不到 cookie。
- **令牌作用域（`scope`，严格二选一）**：`api`（可调 `/assist/api/*`、**不能**调 MCP）/
  `mcp`（可调 `/assist/mcp`、**不能**调 `/assist/api/*`）。`TokenScope` 缺字段默认 `api`（最小权限）。
  `resolve_principal` 把它算成 `Principal.api` / `Principal.mcp`（后者还需 `用户.mcp = true`）；
  `token_middleware` 对 `Principal.api == false` 的令牌返回 401 `code: mcp_only_token`，
  `static_files::authorize` 同样要求 `p.api`。**会话 token（密码登录）不受限**：`api = true`，`mcp = 用户.mcp`。
- **状态**：`pending`（待审核）/ `active` / `disabled`（禁用后会话与令牌立即失效）。
- **自助注册默认权限**：角色取 `registration_role`（不允许 admin），MCP 权限取 `registration_mcp`（**默认 `true`**，即注册即具备 MCP 权限；管理员可在用户管理页关闭或逐个用户调整）。
- **不变量**：至少保留一个 `active` 的 `admin`；最后一个管理员不能降级/禁用/删除（`UserError::LastAdmin` → 409）。
- **登录防护**：连续失败 5 次后按 `30s * 2^n` 指数退避写 `locked_until`（上限 1h）；登录/注册各有内存滑动窗口限流；用户名不存在时跑一次等时 Argon2 校验（`users::verify_dummy`）防枚举。

#### 会话（`session.rs`）

- 密码登录签发 `sess_<32B base64url>`，存**进程内存**（`SessionStore`，`Mutex<HashMap>`），重启即失效。
- TTL 取自 `users.session_ttl_hours`（默认 168h）并**滑动续期**：每次成功校验把 `expires_at` 推到 `now + ttl`，空闲超期才失效。
- `validate()` 顺带惰性清理过期会话；用户被禁用/删除时会话立即作废（`resolve_principal` 里顺手 `revoke`）。

#### 鉴权链路（`main.rs::token_middleware`）

仅保护 `/assist/api/*`，顺序：

1. `/image/` 图片 URL 放行（注入 `ViewOnly`）；
2. **未初始化门禁**：`auth::is_initialized()` 为 false（没有任何 active admin）时，除 `/auth/status|bootstrap` 外一律 401（`code: not_initialized`）；
3. 公开端点放行：`/assist/api/auth/{status,login,register,bootstrap}`；
4. 受信任子网：**仅当** `system.json` 的 `trusted_subnet_enabled = true` 且 IP 命中 `trusted_subnets` 时，注入 `Principal::local(trusted_subnet_role, true)`；
5. `auth::resolve_principal()`：`sess_` 前缀 → 会话表；否则按 sha256 匹配用户 `api_tokens`；
6. 未命中 → 401。

命中后注入 **三样** Extension：`auth::Principal`（新代码用）、`auth::Role`（兼容既有 40+ 处 `Extension<Role>`）、`auth::OwnerKey`（会话归属，`u:<user_id>`，免鉴权为 `local`）。

#### 启动引导（`main()`）

首个管理员**只**由前端初始化页调 `POST /assist/api/auth/bootstrap` 创建；`system.json` 里没有任何全局凭据字段。启动时没有 active admin 只打一条 `warn`。

#### 接口一览

| 方法 | 路径 | 权限 | 说明 |
|---|---|---|---|
| GET | `/assist/api/auth/status` | 公开 | 初始化状态、用户数/待审核数、注册策略、会话 TTL |
| POST | `/assist/api/auth/bootstrap` | 无 admin 时 | 创建首个管理员并登录 |
| POST | `/assist/api/auth/register` | 公开（受开关控制） | 自助注册 → 默认 `pending` |
| POST | `/assist/api/auth/login` | 公开 | 用户名+密码 → 会话 token |
| POST | `/assist/api/auth/logout` | 已登录 | 撤销当前会话 |
| GET | `/assist/api/auth/me` | 已登录 | 当前主体（含令牌元数据，无哈希）＋ `permissions` / `must_set_password`；**前端用它做 Token 登录与启动恢复**，成功时顺带下发 `okb_static` cookie |
| POST | `/assist/api/auth/password` | 已登录 | 改密（撤销该用户全部会话；有密码时需 `old_password`） |
| GET/POST | `/assist/api/auth/tokens` | 已登录 | 查看/签发**自己的**令牌（`{label, scope: api\|mcp, token?}`，`token` 留空自动生成） |
| DELETE | `/assist/api/auth/tokens/:id` | 已登录 | 撤销自己的令牌 |
| GET | `/assist/api/admin/users` | admin | 用户列表（`?status=`），返回 `count/pending/users` |
| POST | `/assist/api/admin/users` | admin | 新建用户 |
| PUT/DELETE | `/assist/api/admin/users/:id` | admin | 改角色/状态/mcp/备注名；删除（级联撤销会话） |
| POST | `/assist/api/admin/users/:id/approve` | admin | 审核通过 |
| POST | `/assist/api/admin/users/:id/password` | admin | 重置密码（`revoke_tokens` 可选） |
| GET/POST | `/assist/api/admin/users/:id/tokens` | admin | 查看/签发该用户令牌（请求体同上） |
| DELETE | `/assist/api/admin/users/:id/tokens/:token_id` | admin | 撤销 |
| GET/PUT | `/assist/api/admin/users/settings` | admin | 注册策略 / 会话 TTL |

- `GET /assist/api/config` 不再返回 `users` 段，只给 `users_summary: {count, pending, allow_registration}`；`PUT /assist/api/config` 也无法改写 `users`（`ConfigManager` 的 `PROTECTED_CONFIG_SECTIONS`）。
- CLI 兜底：`okb_assist --reset-password <用户名> --new-password '<密码>'`（省略密码则从 stdin 读）。

#### MCP 鉴权（`mcp_server.rs`）

依次接受：用户会话（`mcp = 用户.mcp`）/ **MCP 权限的个人令牌**（`Principal.mcp = 用户.mcp && scope == mcp`）→ 受信任子网（需 `trusted_subnet_enabled`）。**不再有全局 `mcp_token` 入口**；`scope = api` 的令牌调 MCP 会被拒（`Unauthorized`），未开 MCP 的用户返回 `Unauthorized: MCP access not granted`。

### 用户网盘（`config.json` 的 `drive` 段 + `system.json` 的 `drive_root`）

每个用户有独立私有目录，另有管理员创建的**公有文件夹**（按**角色阈值**授权）。网盘与文献库
（`documents` / 解析流水线 / 向量库）**完全独立**：上传到网盘的文件不会入库、不会解析。

#### 存储布局

```
{drive_root}/users/{user_id}/...        # 私有：只有本人可读写
{drive_root}/_public/{folder_id}/...    # 公有：按 read_role / write_role 阈值判定
```

路径全部来自 `system.json` 的 `drive_root`（解析规则同 `static_dirs[].dir`：相对路径按 `cwd`、
绝对路径原样使用、支持 `{system_dir}` 与 `{env:VAR}` 变量，改后需重启），目录懒创建。

#### 策略（`config.json.drive`）

```json
{
  "drive": {
    "enabled": true,
    "max_upload_mb": 200,
    "default_quota_mb": 0,
    "quota_overrides": { "u_3c420f47d351178f": 5120 },
    "public_folders": [
      { "id": "pf_1a2b3c4d", "name": "团队资料",
        "read_role": "view-only", "write_role": "admin",
        "created_at": "2026-01-01T00:00:00Z" }
    ]
  }
}
```

- **角色阈值**：等级 `admin(3) > view-upload(2) > view-only(1)`，`角色等级 >= 阈值` 即可读/写。
  例：`read_role=view-only` + `write_role=admin` = 全站可读、仅管理员可写。
- **配额**：`default_quota_mb`（0 = 不限），`quota_overrides` 按 `user_id` 覆盖（值 `0` = 该用户不限）。
  统计方式为遍历该用户私有目录累加文件大小；**配额只约束私有目录**。
- **`drive` 是受保护段**：`ConfigManager::PROTECTED_CONFIG_SECTIONS = ["users", "drive"]`，
  `PUT /assist/api/config` 无法改写它；所有写操作走 `ConfigManager::update_section("drive", ..)`
  （加写锁 → 读盘 → 只改这段 → 原子写）。

#### 接口

| 方法 | 路径 | 权限 | 说明 |
|---|---|---|---|
| GET | `/assist/api/drive` | 已登录 | 概览：开关、配额、当前用户可访问的公有文件夹（含 `can_write`） |
| GET | `/assist/api/drive/list?scope&path` | 已登录 | 列目录（`scope=private` 或 `public:<id>`），返回面包屑/占用/上限 |
| POST | `/assist/api/drive/upload` | 已登录 | multipart：`scope`、`path` + 一个或多个 `file`（字段顺序敏感） |
| POST | `/assist/api/drive/mkdir` | 有写权限 | 新建文件夹 |
| POST | `/assist/api/drive/rename` | 有写权限 | 重命名 |
| POST | `/assist/api/drive/move` | 有写权限 | 移动到目录（同名自动加 `(n)` 后缀） |
| DELETE | `/assist/api/drive/file?scope&path&recursive` | 有写权限 | 删除；非空目录需 `recursive=1` |
| GET | `/assist/api/drive/download?scope&path&token` | 已登录 | 附件下载（支持 Range） |
| GET | `/assist/api/drive/raw?scope&path&token` | 已登录 | 内联预览（图片/PDF；非可预览类型强制下载防 XSS） |
| GET | `/assist/api/drive/text?scope&path` | 已登录 | 文本预览（≤512KB，超长 `truncated=true`） |
| GET/POST | `/assist/api/admin/drive/folders[/:id]` | admin | 公有文件夹列表/创建（`PUT` 改名称与阈值，`DELETE ?force=1`） |
| GET/PUT | `/assist/api/admin/drive/quotas[/:user_id]` | admin | 配额总览/设置（`{quota_mb, clear}`） |
| GET/PUT | `/assist/api/admin/drive/settings` | admin | `enabled` / `max_upload_mb` / `default_quota_mb` |

- `download`/`raw` 支持 `?token=`（`<img>`/`<iframe>` 无法带 `X-Token` 头），由中间件的查询参数解析兜底。
- 上传上限：`routers::drive::body_limit_bytes()` 计算 `DefaultBodyLimit`（10 个文件 + 1MB 余量，上限 512MB），
  单个文件另按 `max_upload_mb` 边读边校验；先写 `.upload-*.part` 再 rename，超限/失败即删除临时文件。
- 若要让新的 `max_upload_mb` 完全生效需重启（body limit 在启动时计算）。

#### 路径安全（`services/drive.rs`）

- `validate_name()`：拒绝 `/`、`\`、`..`、`.`、控制字符、>200 字节；
- `sanitize_rel_path()`：逐段过滤空段/`.`，拒绝 `..`/绝对路径/控制字符/超长（1024 字节）；
- `resolve_path()`：拼接后对**最近的已存在祖先**做 `canonicalize()`，必须仍在作用域根内（防符号链接逃逸）；
- `validate_folder_id()`：`[A-Za-z0-9_-]{1,32}`（同时用于 `user_id` 与 `folder_id`）；
- 列目录时跳过符号链接；删除时禁止删除作用域根目录。

#### 前端

- `views/DriveView.vue`（`/assist/drive`，全部角色）：左侧作用域（我的网盘 + 公有文件夹，标注可写/只读），
  右侧文件表（进入目录 / 预览 / 下载 / 重命名 / 移动 / 删除）、拖拽上传 + 进度、配额条；
  预览支持图片、PDF、文本/Markdown（文本走 `/drive/text`）；只读作用域不显示写操作按钮。
- `views/DriveAdminView.vue`（`/assist/drive/admin`，admin）：网盘开关/单文件上限/默认配额、
  公有文件夹增删改（名称 + 读写阈值）、逐用户配额覆盖。
- `api/drive.ts`、`types/drive.ts`；导航「网盘」（`/assist/drive/admin` 仅 admin 可见）。

### 元数据抽取（信息补全）的校对语义

实现在 `backend-rs/src/routers/pipeline.rs` 的 `do_extract_impl` + `backend-rs/src/services/llm.rs` 的 `extract_metadata` / `build_extract_prompt`。

- **提示词可配置**：`config.json` 的 `llm_extract.prompt` 为空时用内置 `DEFAULT_EXTRACT_PROMPT`；否则整体替换。占位符：`{content}`（按 `max_input_chars` 截断的 Markdown 正文）、`{metadata}`（数据库里已有元数据的 JSON）。自定义模板漏写 `{content}` 时自动在末尾补正文。
- **输入包含原元数据**：`MetaRecord::from_doc` 把数据库中的 16 个字段拍成 JSON 一起送入模型，由模型逐字段校对。
- **输出是纠错结果而非只填空**：`MetaRecord::apply` 按 `overwrite` 合并 ——
  - `overwrite = true`（默认）：模型返回的字段以模型为准；返回 `""` / `[]` / `null` 表示“请清空”（对应“原值明显不对且正文里也没有”的情况）；模型未返回的字段保持原值。
  - `overwrite = false`：只补空字段。
  - `type` 走 `normalize_doc_type()` 的 Zotero 标准映射；`year` 只接受 1000–2200。
- **安全阀**：模型输出全空（`metadata_is_blank`）直接放弃写入；合并后标题与作者同时为空也放弃写入。因此可以对同一文档反复抽取以纠正旧值。
- **日志**：`info!` 依次输出“原元数据 → 模型输出 → 字段变化清单（`字段: 旧 → 新`）→ 更新后元数据”，失败/放弃路径用 `warn!`。

### 问答（Chat，工具页「问答」tab：`/assist/tools?tab=chat`）

基于 `llm.models` 模型池的**对话 + MCP 工具调用**（非 RAG：不自动向量检索，改由模型自己选工具查库）。

- `backend-rs/src/routers/chat.rs`：
  - `GET /assist/api/chat/models` —— 返回模型池列表 `{models:[{name,protocol,model,tool_mode}], count, default}`（**不返回 key**）。
  - `GET /assist/api/chat/tools` —— 可被模型调用的 MCP 工具清单 `{count, tools:[{name,description}]}`。
  - `POST /assist/api/chat` —— 请求 `{model?, messages, system?, temperature?, max_tokens?, tools?, tool_mode?, max_tool_rounds?}`，返回 `{content, steps, delta, notes, name, protocol, model, tool_mode, tool_rounds}`。
- 模型选择：`Settings::llm_model_config(name)`（`backend-rs/src/config.rs`）按 `name` 取池中一项，未命中/留空回退第一项。
- 对话生成：`LlmClient::chat_with_tools()`（`backend-rs/src/services/llm.rs`），Ollama 走 `/api/chat`，OpenAI 兼容走 `/chat/completions`；消息 role 允许 `system`/`user`/`assistant`/`tool`，无 `system` 时自动补默认系统提示词（开工具时再追加工具规则）；`max_tokens` 上限 8192、`temperature` 限幅 0-2。
- **MCP 工具接入**：直接复用 `McpServer`（`backend-rs/src/mcp_server.rs`）的工具集（`list_tools()` / `call_tool()`，与 `/assist/mcp` 同一套实现；9 个只读工具 + 3 个 Markdown 写工具），服务端在**本请求内**完成「模型 → 工具 → 模型」循环（默认最长 5 轮、可用 `max_tool_rounds` 调整至 8），单条工具结果截断 12k 字符，重复的相同调用复用结果。**写工具按调用方角色过滤**：`McpServer::new()` 需传 `Role`，非 admin 的 `list_tools()` 不返回 `edit_markdown` / `replace_markdown` / `find_replace_markdown`，`call_tool()` 也直接拒绝（`forbidden`）。
- **两种工具协议（`tool_mode`）**：
  - `native`：原生 function calling（请求带 `tools`，响应取 `tool_calls`）；
  - `prompt`：提示词协议（system 里给出工具 JSON Schema，模型输出 `{"tool_calls":[{"name","arguments"}]}`，解析后以 `user` 消息回传结果）——适用于不支持 function calling 的网关/思考型模型；
  - `auto`（默认）：先试 native，服务商报错含 tool/function 或模型首轮不动手反而声称自己“无法访问文献库”时，自动降级为 prompt（响应里的 `notes` 会说明）。
  - 取值来源：请求参数 `tool_mode` > 模型条目的 `llm.models[].tool_mode` > `auto`。**注意：没有充分系统提示词时，部分模型（如 dots3-note-prev）不会主动发起工具调用。**
- **认证与权限**：`GET/POST /assist/api/*` 仍是全局 Token/子网鉴权；chat 本身允许全部已认证角色（含 `view-only`），工具集对非 admin 只有只读查询，admin 额外可见 3 个 Markdown 写工具（`edit_markdown` / `replace_markdown` / `find_replace_markdown`），与 MCP 端点同一套角色过滤。
- **非流式**：一次请求返回完整回答（前端 axios 超时单独放大到 300s，支持「停止」中止）。前端把 `delta`（含 `assistant.tool_calls` 与 `tool` 结果）追加到本地 transcript 并回传，以保留跨轮工具上下文（裁剪时不能把 `tool` 消息与其前置 `assistant.tool_calls` 拆开）。
- 前端：`ChatView.vue` + `api/chat.ts` + `types/chat.ts`；已并入「工具」页的问答 tab（`ChatView` 支持 `embedded` 属性：隐藏自带的页面标题、压低高度），导航栏不再单独放「问答」入口；展示工具调用卡片（工具名/参数/结果预览/失败标记）、本轮协议与降级提示；本地状态存 `localStorage`（`okb_chat_messages` 展示条目、`okb_chat_history` transcript、`okb_chat_model`、`okb_chat_tools`、`okb_chat_tool_mode`、`okb_chat_session` 当前会话绑定）。

### 问答会话历史（显式保存，一个会话一个文件）

**只有用户在前端点击「保存会话」才会落盘**；对话过程中后端不写任何东西（`localStorage` 只是前端草稿）。

- **存储**：`services/chat_store.rs`，一个会话一个 JSON 文件，路径模板 `system.json` 的 `chat_session_path`（默认 `data/chat_sessions/{id}.json`，`{id}` 为**字符串**，走 `ConfigManager::substitute_path_variables_str`）。会话 ID 形如 `20261015T081515912-523019`（UTC 时间到毫秒 + 随机后缀，字典序即时间序），写入为**原子写**（`*.tmp` + rename）。
- **文件内容**：`version`/`id`/`title`/`owner_key`/`role`/`model`/`tool_mode`/`message_count`/`created_at`/`updated_at`/`items`（展示条目）/`transcript`（完整对话，含 `assistant.tool_calls` 与 `tool` 结果，重新打开后可继续对话）。
- **按角色/人归属（复用现有 permissions 机制）**：`owner_key` 由认证主体派生（`Principal::owner_key()`：用户 → `u:<user_id>`；受信任子网这类免鉴权主体 → `local`），**只在服务端比对、绝不在 API 响应里回传**；列表只返回 `mine` 与保存时的 `role`。规则：**admin 可看/可删全部，其余只能看自己的**（同一角色不同 token 相互不可见）。归属与 `created_at` 在更新时保持文件原值，客户端无法改写归属。
- **接口**（均在 `/assist/api/chat/*`，角色限制与 chat 一致）：
  - `GET /assist/api/chat/sessions` —— 列表（`{count, sessions:[{id,title,role,model,tool_mode,message_count,created_at,updated_at,mine}]}`，默认最多 200 条，按 `updated_at` 倒序）；
  - `GET /assist/api/chat/sessions/:id` —— 读取（额外含 `items`/`transcript`）；
  - `POST /assist/api/chat/sessions` —— 新建（显式保存）；
  - `PUT /assist/api/chat/sessions/:id` —— 覆盖保存（归属人或 admin）；
  - `DELETE /assist/api/chat/sessions/:id` —— 删除（归属人或 admin）。
- **校验/限制**：ID 白名单 `[A-Za-z0-9_-]` 且 ≤64 字符（防路径穿越，非法 ID 一律 404）；`items` ≤ 200 条、`transcript` ≤ 300 条、各自序列化后 ≤ 2MB，否则 400。标题缺省时取首条用户消息首行（截断 40 字）。
- **前端**：工具栏「保存会话 / 更新会话」+ 状态行（`已保存：<标题>` / `未保存`，有修改时提示）+「历史会话」面板（标题、归属徽标（我 / 角色名 / 他人会话标记）、条数、模型、时间、打开/删除）；打开会覆盖当前对话（有未保存内容时先确认），清空对话会解除会话绑定。

### 工作目录与路径解析

程序启动时会 `chdir` 到 `cwd` 指定的目录，后续所有相对路径都相对于该目录解析。

- `cwd` 的值支持 `{system_dir}` 变量替换（在 `ConfigManager::new()` 中解析）
- `config_path` 不支持变量替换，为相对于 system.json 所在目录的路径
- 其他路径属性均为相对于 `cwd` 的相对路径，仅支持 `{id}` 变量（文档 ID）
- `env:VAR_NAME` 语法在所有路径属性中均支持

### system.json 字段一览

| 字段 | 说明 | 默认值 |
|------|------|--------|
| `cwd` | 工作目录，支持 `{system_dir}` 替换 | `{system_dir}` |
| `trusted_subnets` | 受信任子网白名单（CIDR 数组） | `[]` |
| `trusted_subnet_enabled` | 是否启用受信任子网免鉴权（**默认关闭**，需显式打开） | `false` |
| `trusted_subnet_role` | 命中受信任子网时授予的角色 | `admin` |
| `max_concurrent_tasks` | 全局 extract/index 最大并发数 | `3` |
| `database_url` | SQLite 连接串 | `sqlite:///data/okb_assist.db` |
| `markdown_path` | Markdown 文件路径模板 | `data/markdowns/{id}.md` |
| `info_path` | 元信息 JSON 路径模板 | `data/markdowns/{id}.json` |
| `chat_session_path` | 问答会话文件路径模板（`{id}` 为会话 ID，字符串） | `data/chat_sessions/{id}.json` |
| `runtime_state_path` | 运行时状态 sidecar 路径模板（`{id}` = 文档 ID；空串 = 只用进程内存） | `data/pipeline/{id}.json` |
| `drive_root` | 用户网盘根目录（私有 `users/{user_id}/`、公有 `_public/{folder_id}/`）；相对路径按 `cwd`，也可写绝对路径（独立磁盘/挂载点），支持 `{system_dir}` / `{env:VAR}` | `data/drive` |
| `crossref_path` | Crossref 数据路径模板 | `data/markdowns/{id}_crossref.json` |
| `markdown_asset_path` | Markdown 资产包路径模板 | `data/pdfs/{id}/{id}.zip` |
| `pdf_path` | PDF 文件路径模板 | `data/pdfs/{id}/{id}.pdf` |
| `uploads_folder` | 上传目录 | `data/_uploads` |
| `static_dirs` | 静态目录挂载数组（把本地文件夹挂到 URL 前缀下），见下方小节 | `[]` |
| `config_path` | config.json 路径（相对于 system.json 所在目录） | `config.json` |
| `log_path` | 日志路径 | `okb-log.jsonl` |
| `mutool_path` | mutool 可执行文件路径 | `mutool` |
| `ui_path` | 前端 UI 构建产物路径 | `frontend/dist` |

> **路径模板**：`cwd` 支持 `{system_dir}` 替换；其他路径属性相对于 `cwd`，支持 `{id}` 和 `{env:VAR_NAME}` 变量。
> **trusted_subnets**：空数组 `[]` 表示不信任任何子网。即使配置了网段，也必须把 `trusted_subnet_enabled` 设为 `true` 才生效（默认关闭，避免「在局域网内即管理员」被默认踩到）。

### 静态目录挂载（`system.json` 的 `static_dirs`）

把本地文件夹挂到任意 URL 前缀下做只读文件服务（下载/预览 PDF、放前端 demo、把导出目录交给外部工具抓取）。实现在 `backend-rs/src/static_files.rs`，**路由在启动时注册，改配置需重启进程**。

```json
"static_dirs": [
  {
    "path": "/static",
    "dir": "data/static",
    "try_index": true,
    "index": "index.html",
    "list_dir": false,
    "auth": true,
    "cache_control": true
  }
]
```

| 字段 | 默认 | 说明 |
|---|---|---|
| `path` | — | URL 前缀，必填、以 `/` 开头；`/static/a.txt` → `<dir>/a.txt`。与内置路由（`/assist/api`、`/assist/mcp`、`/assist/uploads`、`/assist/file`、`/assist/assets`、`/assist/openapi`）冲突的挂载会被跳过并打 `warn` |
| `dir` | — | 本地目录，必填；相对路径按 `cwd` 解析，支持 `{system_dir}` 与 `{env:VAR}` |
| `try_index` | `true` | 命中目录时补 `index` 指定的首页（语义同 nginx `index`，**没有** SPA 全量回退） |
| `index` | `index.html` | 首页文件名 |
| `list_dir` | `false` | 目录无首页时返回简单文件列表（默认关闭） |
| `auth` | `true` | 是否需要凭证：会话 cookie（`okb_static`）/ 会话 token / 用户 API token（`X-Token` 头、`Authorization: Bearer`、`?token=`），受信任子网需 `trusted_subnet_enabled = true`；`false` 为完全公开 |
| `cache_control` | `true` | 是否附带 `Cache-Control`（静态资源长缓存，`index.html` 不缓存；关闭时统一 `no-cache`） |

- **只读**：只注册 `get()`（axum 的 GET 路由同时响应 `HEAD` 并自动去掉响应体），其他方法 405。
- **安全**：请求子路径做 percent-decoding 后拒绝 `..` / 反斜杠 / NUL，最终路径必须 `canonicalize()` 且仍在挂载根目录内（指向目录外的软链同样被拒）。
- **目录补斜杠**：命中目录但 URL 未以 `/` 结尾时返回 301 补 `/`，保证首页里的相对链接正确。
- **自动登录跳转**：`auth: true` 且未通过鉴权时 ——
  - 浏览器页面导航（`Accept` 含 `text/html`）→ **302 到 `/assist/login?redirect=<原路径%3Fquery>`**，登录/已登录自动回跳（`LoginView.vue::redirectAfterLogin` 对非 SPA 路径用 `window.location.replace` 整页跳回）；
  - 其他请求（curl / fetch / 下载工具）→ 401 JSON，不会被重定向搞坏。
- **凭证 cookie（`okb_static`）**：浏览器直接导航带不了 `X-Token`，所以登录成功（`POST /auth/login`）、`GET /auth/me`（刷新页面 / Token 登录）、以及静态目录上用 `?token=` 认证成功时都会下发
  `Set-Cookie: okb_static=<token>; Path=/; HttpOnly; SameSite=Lax; Max-Age=<session_ttl_hours>`；登出（`POST /auth/logout`）下发 `Max-Age=0` 清除并顺手`revoke` cookie 里的会话。
  **只有 `static_files` 读这个 cookie，`token_middleware` / API 一律不认**（避免扩大 API 鉴权面）；下发时顺带解决页面内相对资源（图片/子页面）的鉴权问题。
- 重复 `path`、`path` 为 `/`、缺 `path`/`dir` 的条目一律忽略并打 `warn`；**前缀不允许互相嵌套**（`/a` 与 `/a/b` 在 axum 的路由表里会冲突，只保留更长的 `/a/b`），匹配时最长前缀优先。
- **仓库自带的文档站**：`system.json` 默认把 `/document` 挂到仓库 `document/` 目录（`try_index: true`、`index: index.html`、`cache_control: false`，改完 Markdown 刷新即可见）。该目录是 docsify 站点：
  - `README.md` 首页（卡片式索引）、`_sidebar.md` 左侧目录、`_404.md` 未找到页、子目录各自的 `_sidebar.md`（docsify 会就近取用，新增子目录最好补一个，否则会多一次 404 探测）；
  - docsify 运行时 / 主题 / 插件走 **jsDelivr CDN**（版本已锁定），`assets/css/custom.css`（多巴胺绿主调）与 `assets/js/auth-guard.js` 是站内自有文件；
  - 鉴权靠 `static_dirs` 自己的判定（`okb_static` cookie）：未登录时 `auth-guard.js` 用一次 `README.md` 探针拿到 401 就整页跳 `/assist/login?redirect=<当前地址>`，登录后自动回跳；否则 docsify 会把 401 渲染成「404」；
  - **新增文档后要在 `_sidebar.md` 与首页 `README.md` 里各补一条链接**（搜索索引也按侧栏链接生成）。

## 架构与请求流

1. **Web/API 层**：`backend-rs/src/main.rs` + `backend-rs/src/routers/*`。`TokenMiddleware` 仅对 `/assist/api/*` 校验凭证（`X-Token` 头 → `Authorization: Bearer` → query `token`），放行 `/assist/mcp`、`/assist/assets`（前端静态资源）、`/assist/uploads`、`/assist/file`、`/redirect`，以及 `/assist/api/documents/` 下含 `/image/` 的图片 URL。凭证由 `auth::resolve_principal` 解析成 `Principal`（会话 `sess_…` → 用户 API token `okb_…`）；**系统未初始化（没有任何 active admin）时只允许 `/auth/status|bootstrap`**；受信任子网仅在 `trusted_subnet_enabled = true` 时生效。CORS 限定前端来源（开发 `localhost:5173`，生产同源 `localhost:5001`）。
2. **数据模型**：`Document`（主记录 + **文件事实**）、`DocumentVectorIndex`（每个向量库的索引状态，唯一键 `(document_id, vector_db_id)`）。**`documents` 不再有 `status` / `status_message` / `progress` 列**：文献状态在读取时由 `services/file_state.rs::derive_state()` 从「文件事实列（`pdf_size`/`md_size`/`zip_size` + `fs_synced_at`）+ 进程内运行时状态」推导，枚举仍沿用 `DocStatus`（`backend-rs/src/models.rs`），索引状态仍用 `IndexStatus`。
3. **摄取流水线**（`backend-rs/src/routers/pipeline.rs`，核心状态机）：
   - parse（MinerU）→ `parsing`（**仅运行时**）→ `markdown_done`（= `md_size` 落库）
   - extract（信息补全模型，`llm_extract`）→ `extracting` → `meta_done`
   - index（向量库）→ `indexing` → `indexed`
   - 含批量控制器、暂停/恢复/重置、两级信号量并发限制：MinerU 解析使用独立 `Semaphore(max_tasks)`（按配置），extract/index 使用全局 `Semaphore(max_concurrent_tasks)`（`system.json`）。
   - **所有阶段均在后台执行**（见下方「后台任务」一节）—— 请求先返回，协程在事件循环继续；进度与状态机记录在 `Document` 上，可轮询查询。
4. **文档管理**（`documents.rs`）：CRUD、上传、按路径登记、语义搜索 `/search`、全文搜索 `/grep-search`、`/assist/markdown` 读写、PDF/图片服务、去重。
   - **上传/替换一律流式**：`upload`、`replace-pdf`、`replace-markdown`、`replace-asset` 都用 `stream_field_to_temp_file()` 把 multipart 分片边写盘边算 sha256（临时文件放 `pdf_path` 模板的上两级目录，即 `data/pdfs/.upload-*.part`，同盘 rename 到最终位置；失败/去重命中时由 `TempUpload` 守卫自动删除）。**不要再改成 `field.bytes()` + `Vec` 然后 clone**，那会让峰值内存变成文件体积的 2–3 倍。`register`（按路径登记）只流式算哈希 + `tokio::fs::copy`，不读入内存。
   - **PDF 元数据只传路径**（`pdf_meta::extract_pdf_metadata_from_path_async` / `extract_pdf_report_from_path_async`）：mutool 直接读原文件（不再写临时副本），mupdf feature 用 `Document::open`。
   - **`/image/:filename` 走按 doc 的 zip LRU 缓存**（`services/image_cache.rs`，64 MB 预算、单包 16 MB 上限、mtime+size 失效）：同一篇文档的 N 张图只读一次 `images.zip`。换包（`replace-asset`）与删文档时必须调 `image_cache::invalidate(doc_id)`。
   - `GET /assist/api/documents/:id/markdown/?line_start&line_count&full&raw` —— 按行切片读取；`raw=1` 返回文件原始内容（不做图片路径重写），供 Markdown 编辑器保证无损读写；响应含 `hash`（文件真实字节的 sha256）。
   - `PUT /assist/api/documents/:id/markdown/lines` —— 按行范围替换源码（admin）。语义：`line_start` 0-based 含端点，`line_count` 为被替换行数（0 = 纯插入），新行数 = `content.split('\n').count()`（与「切片 join('\n')」严格互逆），写入为原子写且不做图片路径还原（与 `raw=1` 配对）。删行必须显式传 `delete: true`（空字符串固定等于 1 个空行，不能表达删行）。
   - **乐观并发控制**：`PUT .../markdown/` 与 `.../markdown/lines/` 都接受可选 `base_hash`（取自 GET 的 `hash`），不一致时返回 **409** `{code:"conflict", hash, detail}`；不传则跳过校验。两个写接口的响应都返回写入后的 `hash`。
   - `GET /assist/api/documents/:id/pdf-info` —— PDF 元信息报告：`{pdf: {file_size, pdf_version, page_count, encrypted, info, heuristics}}`。`info` 为 PDF Info 字典原始键值（mupdf feature 下只有标准字段；否则由 `mutool show trailer.Info` 解析，含自定义键如 `/doi`），`heuristics` 复用 `pdf_meta` 的启发式抽取结果。页数取 `trailer/Root/Pages/Count`，回退 `mutool pages` 计数；加密判断取 trailer 的 `/Encrypt`。
   - `GET /assist/api/documents/:id/crossref` —— 读取 `crossref_path` 模板对应的已保存 Crossref 记录，返回 `{exists, path, raw, parsed, modified_at}`（`raw` 为 Crossref API 完整应答，`parsed` 与元数据补全同源）。
   - `GET /assist/api/documents/:id/citation?format=bibtex|gbt7714&source=auto|crossref|document` —— 生成引用条目，返回 `{format, source, key, content}`。默认 `source=auto`：有 Crossref 记录就用它，否则退化为数据库元数据（`source=document`，卷期页会缺失）；`source=crossref` 且无记录时返回 404。引用键为「第一作者姓 + 年份 + 标题首个实词」。
   - **DOI 比对统一归一化**：`doi_key()`（`normalize_doi` 去 `https://doi.org/` 等前缀与尾部标点 + 转小写）用于 `GET /documents/by-doi/:doi`（先精确匹配，未命中再 `LIKE` 粗筛 + `doi_key` 校验，兼容历史数据里带前缀/大小写不一致的值）与 `POST /documents/diff-dois`；写入口也归一化 —— `PUT /documents/:id`（`update_document`）与 `pipeline` 的 `MetaRecord::apply`。新增 DOI 写/比对照旧走这几个入口，别再直接字符串相等比较。
   - **全量比对索引**（`GET /documents/doi-hash-index`）：一次返回库内全部文献的 `{count, with_doi, with_hash, generated_at, documents:[{id, doi, doi_key, file_hash, pdf_health}]}`（`doi_key` 已归一化小写，空 DOI/空哈希为 `null`），供 `client-rs` 批量比对；客户端拉一次就完成整库比对，不再按 DOI 逐条请求 `by-doi`。只返回比对必需字段（不带 title/abstract），新增字段时注意保持轻量。
   - **上传去重**（`POST /documents/upload`，`upload_document`）：先用 `find_doc_by_hash`（只查 `file_hash` 索引，**不抽 PDF 元数据**）比哈希；未命中且请求里**显式**给了 `doi` 表单字段时再用 `find_doc_by_doi` 比 DOI（PDF 里抽到的 DOI 不参与去重，保持既有行为）。命中重复的行为由 `on_duplicate` 控制：`error`（默认，409 `{error:"duplicate", kind:"hash"|"doi", existing_id, existing:{file_hash,pdf_health,…}}`）/ `skip`（200 `{status:"skipped", existing_id, id, doc}`）/ `replace`（同哈希则等价 skip，否则用本次文件覆盖已有文档的 PDF，200 `{status:"replaced", existing_id, doc}`，复用 `persist_pdf_for_doc`，与 `POST /{id}/pdf` 同一套副作用）。`force=true` 跳过全部去重检查。新文档的 `doi` 直接取显式字段（`fill_meta_fields` 只补空）。去重依赖 `database.rs::init` 建的 `idx_documents_file_hash` / `idx_documents_doi`（`CREATE INDEX IF NOT EXISTS`，幂等），新增按列等值查找的功能时同步补索引。
   - **BibTeX 批量新建条目**（`POST /assist/api/documents/import-bibtex`，`import_bibtex`）：请求体 `{content, dry_run}`，`content` 为 `.bib` 文件原始文本（≤8 MB，单次最多处理 2000 条）。服务端用 `services/bibtex.rs` 手写解析器（`@article{key,…}` / `@article(key,…)`、花括号嵌套、`"…"` 引号值、`#` 拼接、`@string` 宏、`@comment`/`@preamble` 跳过、LaTeX 重音与转义还原），把条目映射到 `documents` 的 16 个元数据字段（`filename` 用 `<引用键>.bib` 占位，`source` 取 `publisher` 否则 `bibtex`，类型走 Zotero 名）。**只新建库里缺失的条目**：先按文件内 DOI/标题去重，再与全库比对（`doi_key` 归一化 DOI 优先，其次 `normalize_title` + 年份），命中则跳过并回传 `existing_id`；`dry_run=true` 只返回预览统计（不做写库）。新建记录**直接写入 `fs_synced_at`**（三个 size 保持 `NULL` = 三个文件全部「已同步且不存在」），`pdf_health = 'missing'`，因此派生状态为 `uploaded` + 「PDF 缺失」。响应 `{dry_run,total,processed,truncated,missing,created_count,existing_count,duplicate_count,invalid_count,failed_count,created,items,items_truncated,warnings,detail}`，`items[].action ∈ created|create|exists|duplicate|invalid|error`。
   - **手动去重的检测端点**（都只读，`admin` 才能访问，供 `/assist/duplicates` 页使用）：
     - `GET /assist/api/documents/similar-titles` —— 归一化标题 + 编辑距离（`SIMILARITY_THRESHOLD`）贪心聚类，返回 `{groups:[{normalized_title,count,documents}],total_groups,total_documents}`；
     - `GET /assist/api/documents/similar-dois` —— 按 `doi_key()`（`normalize_doi` 去 `https://doi.org/` 等前缀与尾部标点 + 转小写）分组，只返回组内 ≥2 篇的组，返回 `{groups:[{doi_key,doi,count,documents}],total_groups,total_documents}`（`doi` 是组内第一个非空原值，仅用于展示；无 DOI 的文档不参与）。
     - 两个端点的 `documents` 元素都是完整 `DocumentOut`；合并统一走 `POST /assist/api/documents/merge`（`{target_id, source_ids}`，会合并元数据、删源文档与其文件/向量）。
5. **配置/管理**（`config.rs`、`admin.rs`）：查看/更新服务配置、重连测试、统计、按文件哈希去重、索引重置。
6. **问答**（`chat.rs`）：`GET /assist/api/chat/models`、`GET /assist/api/chat/tools` 列出模型池与可调用的 MCP 工具，`POST /assist/api/chat` 基于 `llm.models` 做多轮对话，并可在本请求内循环调用 MCP 工具（非流式）。见下方「问答（Chat）」。
7. **向量库抽象**（`vector_db.rs` 工厂 `get_vector_db(db_id)` → Qdrant/Milvus/Chroma 适配器）。
8. **MCP 服务**（`mcp_server.rs`）：**只有一个** `POST /assist/mcp/stream` 端点（Streamable HTTP 的最小可用子集：读完请求体 → 返回单个 JSON 响应；**没有 SSE 传输**、没有 `Mcp-Session-Id` 会话、没有事件流）；Bearer 认证接受用户会话/个人令牌（`Principal.mcp`）、受信任子网（需 `trusted_subnet_enabled`，角色取 `trusted_subnet_role`，与 API 中间件一致，**不再硬编码 admin**）。工具集 = 9 个只读工具 + 3 个 Markdown 写工具（`edit_markdown` / `replace_markdown` / `find_replace_markdown`，**仅 admin 可见/可用**，`McpServer::new()` 必须传调用方 `Role`；写路径复用 `documents.rs` 的 `write_atomic` + `sync_doc_file_sizes`，不重写图片路径）。完整工具列表见 `document/mcp-tools.md`。

## 后台任务（异步执行）

后端大量耗时操作**不在请求内同步完成**，而是在后台运行，请求通常立即返回、由前端轮询状态。

- **机制**：摄取阶段（parse / extract / crossref / extract-pdf-meta / index / process）及批量端点通过 `tokio::spawn` 提交 Tokio 协程。协程在 HTTP 响应发出后由同一个 Tokio 运行时调度，状态机流转记录在数据库中。
- **并发限制**：两级信号量独立控制。**全局** `Semaphore(max_concurrent_tasks)`（`system.json` 默认 `3`）限制 extract（Ollama 元数据抽取）和 index（向量索引）的并发。**MinerU 专用** `Semaphore(max_tasks)`（每个配置独立，默认 `3`）限制 PDF 解析并发。两者互不抢占，避免 MinerU 慢解析阻塞其他文档的抽取/索引。
- **批量进度 / 暂停状态存于进程内存**：模块级全局变量，**进程重启即丢失**。重启后需手动重新触发。
- **维护类任务也走后台**：`POST /assist/api/admin/recalculate-hashes`（重算 `file_hash`）、`POST /assist/api/admin/check-pdf-health`（批量 PDF 健康检查）、`POST /assist/api/admin/sync-file-sizes`（同步文件大小）都立即返回、后台执行。三者都支持 `?only_missing=1`（也接受 `true` / `yes` / `on`）：只处理「缺失」的文档 —— `file_hash` 为空 / `pdf_health` 为空 / **`fs_synced_at` 为空（从未同步过文件事实）**，即「补缺」而非全量重做（注意：`size` 列为 `NULL` 现在表示「文件确实不存在」，**不再**是「缺失」），返回体里的 `only_missing` 回显实际模式；管理页「维护操作」里「补充缺失 …」与「… 全部」按钮成对使用同一端点。重算哈希与健康检查都是**逐个串行**（不并发、不占流水线信号量），每条用 `pipeline::track_maintenance_task()` 登记到「活跃任务」表（`task_type = "hash"` / `"pdf-health"`，进度为「第 i/N 条」，文案只存进程内、不写 `documents.status_message`）；进程内各有 `HASH_RECALC_RUNNING` / `PDF_HEALTH_RUNNING` 标记防止重复提交，进程重启即中断。

### PDF 健康检查（`services/pdf_health.rs`）

后台串行检查每篇文档的 PDF 能否使用，结果写 `documents.pdf_health`（`ok` / `warning` / `error` / `missing`，`NULL` = 未检查）与 `documents.pdf_health_message`（原因）。

- 两个入口：批量 `POST /assist/api/admin/check-pdf-health`（后台串行，见上）；**单篇** `POST /assist/api/admin/documents/:id/check-pdf-health`（**同步**返回 `{id,status,message,page_count,engine,pdf_path}` 并回写这两列，供文档管理页「PDF 健康」按钮使用，仅 admin）。
- 检查顺序：文件存在 → 非空文件 → 文件头 1 KB 含 `%PDF-` → 结构（页数、是否加密，按 `config.json` 的 `pdf.engine` 用 `mutool show` / `mutool pages` 或内置 mupdf）→ 文件尾 2 KB 含 `%%EOF` → **解析器 stderr 诊断**。
- 判级：缺文件/空文件/非 PDF/读不出页数 = `error`/`missing`；缺 `%%EOF`、加密、`mutool` 不可用（跳过结构检查）= `warning`；**结构损坏但解析器自动修复成功（stderr 有诊断）= `warning`**。
- **必须看 mutool 的 stderr，不能只看页数**：`startxref` 偏移错误 / xref 表偏移全错这类文件，mutool 会打印 `format error: cannot recognize xref format`、`warning: trying to repair broken xref`、`warning: repairing PDF document` 到 stderr，然后**自动修复**并正常输出页数，**退出码仍是 0**。旧实现只看页数，会把这类「已损坏」的文件误判成 `ok`（实测 `data/pdfs/1016/1016.pdf`、`3613.pdf` 就是这种：startxref 指向 xref 表中段）。
- 诊断处理：`push_diagnostics()` 去重收集 stderr 行（探活/`trailer/Encrypt`/`Pages/Count`/`pages` 四次调用都收），`summarize_diagnostics()` 归为「交叉引用表(xref)损坏 / 数据被截断 / 对象语法错误 / 对象引用无效」并附原始首行；只要诊断非空就判 `warning`，消息写明损坏类型与「严格阅读器可能打不开」。**正常文件 mutool 的 stderr 为空**，因此误报率低（实测本库 13 个文件只有上述 2 个触发）。
- 限制：内置 `mupdf` 引擎（`pdf.engine = "mupdf"`）没有回传修复警告的接口，只能拿到页数，检不出「xref 损坏但能自动修复」；查看这类问题请把引擎切回 `mutool`。
- **刻意不复用** `pdf_meta::extract_pdf_report_from_path`：那会整份抽取正文，批量检查太贵；这里只读文件头尾各几 KB + 读一次页数 + 收一次 stderr（单文件几十毫秒）。
- 替换 PDF（`POST /assist/api/documents/:id/pdf`）会把这两列重置为 `NULL`（旧结果对新文件无效），**且不影响解析阶段**：状态由文件事实推导，已解析的文档换扫描件后仍是 `markdown_done`。`reset_status` 字段已**删除**（收到则忽略并 `warn`）；要重新解析必须调 `POST /assist/api/pipeline/reset/:id`（清空 Markdown + 资源包，破坏性）。上传去重 `on_duplicate=replace` 同样不再重置状态。
- **真·fire-and-forget**：删除文档时，SQLite 记录与本地文件先同步删除，而 Qdrant 中对应向量点的删除通过 `tokio::spawn` 异步执行，**请求不等待、失败也不可见**。
- **前台（非后台）操作**：连接测试、服务状态、语义/全文搜索、上传、问答（`/assist/api/chat`）均在前端 `await` 内同步完成。MCP 端点也是「收完整请求体 → 返回一个 JSON」，**不是流式长连接**，工具函数内部检索同样是同步 await。
- **无定时任务**：没有 `APScheduler`/`schedule`/守护线程；后台仅为 Tokio 协程。

### 文献状态：由文件事实推导（无状态列）

`documents` **没有** `status` / `status_message` / `progress` 列（已删除，旧值备份在 `documents_status_legacy`）。
状态在读取时由 `services/file_state.rs::derive_state()` 推导，输入只有两样：

1. **文件事实**（数据库列，写时同步）：`pdf_size` / `md_size` / `zip_size` + `fs_synced_at`；
2. **运行时状态**（进程内）：`pipeline::active_tasks_map()` + `file_state::LAST_ERROR`（可镜像到 sidecar 文件）。

三态语义（每个文件独立）：`fs_synced_at IS NULL` = **未同步/未知**；`fs_synced_at` 非空且 `size` 为 `NULL`/`0` = **确定不存在**；`size > 0` = **存在**。

推导顺序（`derive_state`）：

| # | 条件 | status | message |
|---|---|---|---|
| 1 | 有活动任务 | `parsing` | 任务实时文案（进度取页数比或百分比） |
| 2 | 有运行时错误 | `error` | 错误原文 |
| 3 | `md_size > 0` | `markdown_done` | `PDF 解析完成` |
| 4 | `mineru_task_id` 非空 | `parsing` | `正在恢复上次解析任务…` |
| 5 | `fs_synced_at` 为空 | `uploaded` | `文件状态未同步`（`synced=false`） |
| 6 | `pdf_size > 0` | `uploaded` | `待解析` |
| 7 | 其他 | `uploaded` | `PDF 缺失，请先上传 PDF` |

- **错误不落库**：重启后 `error` 消失，文档回到规则 5/6/7（即「可以重新解析」）。若 `system.json` 配了 `runtime_state_path`（默认 `data/pipeline/{id}.json`），启动时 `file_state::load_all()` 会把上次失败原因读回内存。
- **启动时后台全量同步一次文件事实**（`main.rs`，`tokio::spawn`），修复带外文件改动 / 崩溃遗留 / 迁移后偏差。
- 单篇 `GET /assist/api/documents/:id` 会**实时 stat 并回写**这三列（详情页永远准；列表用同步列，零 stat）。
- 原「人工改状态」端点 `POST /assist/api/admin/documents/status` 与 `StatusFixBody` **已删除**；替代品是 `GET /assist/api/admin/file-state/reconcile?fix=0`（只读报告「同步列 vs 磁盘」差异，`fix=1` 重新同步）。
- **阶段门禁与 worker 的分工（别搞混）**：
  - 门禁（`parse` / `extract` / `index` / `full` 的 handler，以及批量选择 SQL）只读**文件事实**（`has_md()` / `has_pdf()` / `COALESCE(md_size,0)`）+ `runtime_task(id)` 判断是否已有任务在跑，**不读派生状态、也不 stat 文件**，所以可能滞后（带外改动需要 sync 才反映），但贵在零 IO。
  - worker（`do_parse_impl` / `do_extract_impl` / `do_index_impl` / `run_extract_pdf_meta`）在真正干活前会 **stat 真实文件**并优雅退出（缺 PDF/MD 直接 return 或写 `document_vector_index.error_message`），所以门禁滞后最多「白跑一次」，不会损坏数据。
  - 由此得出一条约定：**门禁判断用文件事实，不要用 `doc_state(...).status`** —— 否则一条残留的运行时错误会把 `extract`/`index` 误挡在门外（有 Markdown 也报「没有 Markdown」）。
- 「替换 PDF」不再影响解析阶段；重新解析唯一通道是 `POST /assist/api/pipeline/reset/:id`（删除 Markdown + 资源包 + `image_cache::invalidate` + 重新同步，破坏性、UI 需二次确认）。
- 迁移：`database.rs::init()` 若发现 `status` 列存在，先建 `documents_status_legacy` 备份再 `DROP COLUMN`；`fs_synced_at` 不回填（NULL = 未同步），由启动同步补齐。

## 路由前缀

- 前端 SPA：`/assist/*` → fallback 到 `frontend/dist/index.html`（由 Vue Router 处理客户端路由）
- 静态目录挂载：`system.json` 的 `static_dirs[].path` 决定前缀（默认无挂载）
- API：`/assist/api/documents`、`/assist/api/pipeline`、`/assist/api/chat`、`/assist/api/admin`、`/assist/api/config`、`/assist/api/auth/me`、`/assist/openapi`
- 文件别名（免 token）：`/assist/file/{filename}`
- MCP：`/assist/mcp/stream`（Streamable HTTP，仅 POST/OPTIONS；SSE 与 stdio 均未实现，`/assist/mcp/*` 其它路径 404）
- 上传文件：`/assist/uploads/`（由后端直接 serving）

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

### 组件一览（`src/components/`）

| 组件 | 说明 |
|------|------|
| `AppHeader.vue` | 顶部导航栏（含汉堡菜单） |
| `AppNav.vue` | 桌面端水平导航 |
| `MobileMenu.vue` | 移动端侧滑菜单 |
| `StatusBadge.vue` | 文件事实徽章：**主文字直接显示实际存在的文件类型**（`pdf\|md\|zip` / `pdf\|md` / `无文件` / `未知`），PDF 健康有问题时内嵌标注（`pdf(bad)` 损坏 / `pdf(warn)` 警告，如 `pdf(bad)\|md`），另附「解析中 42%」「未同步」「⚠」等小标；也支持直接传 `status` 给流水线任务状态用 |
| `Toast.vue` | 消息提示 |
| `ConfirmDialog.vue` | 确认对话框 |
| `TokenModal.vue` | 账号面板（当前用户/角色、凭证来源、「退出登录」、跳转账户页）。**不提供页内切换账号** —— 切换走「退出登录 → 登录页选择账号」 |
| `views/UsersView.vue` | 用户管理（表格/待审核/令牌/注册策略），既可作为独立页面也可 `embedded` 嵌入配置页 tab |
| `ErrorBoundary.vue` | 错误边界（catch 渲染异常）；默认整块降级，`compact` 模式为页内局部降级（小巧卡片、不渲染全局浮动徽章与错误面板），用于包裹「高级搜索构建器」这类小功能，避免一个字段抛错就把整页卸载 |
| `SkeletonLoading.vue` | 骨架屏 |
| `MarkdownViewer.vue` | Markdown 渲染（marked + KaTeX + DOMPurify） |
| `CollapsiblePanel.vue` | 通用折叠面板（默认收起，首次展开才触发 `open` 事件，供懒加载使用） |
| `detail/DocumentPdfInfoPanel.vue` | 详情页「PDF 元信息」折叠面板（页数/Info 字典/启发式抽取） |
| `detail/DocumentCrossrefPanel.vue` | 详情页「Crossref 记录」折叠面板（摘要 + 原始 JSON，可触发抓取） |
| `detail/DocumentCitationPanel.vue` | 详情页「引用条目」折叠面板（BibTeX / GB/T 7714，可复制/下载） |
| `QueryBuilder.vue` | 高级搜索构建器 + 列管理（双选项卡面板） |
| `AceEditor.vue` | Markdown 编辑器（ace-builds 1.44.0），编辑+自适应分屏预览+KaTeX（桌面左右 / 移动上下，可拖拽比例）；`showToolbar` / `showPreviewControls` / `readOnly` 可裁剪为纯源码编辑器 |
| `McpConfigPanel.vue` | MCP 配置面板 |
| `McpSkillPanel.vue` | MCP Skill 面板 |
| `config/` | 配置子组件：`BaseUrlSection.vue`、`PdfEngineSection.vue`、`MineruConfigSection.vue`、`LlmConfigSection.vue`、`LlmExtractConfigSection.vue`、`VectorDbConfigSection.vue` |
| `BibtexImportPanel.vue` | 上传页的「从 BibTeX 批量新建条目」面板（选 `.bib` → 预览可新建/已存在 → 只为缺失条目建记录，逐条结果可跳转已有/新建文档）；`api/documents.ts::importBibtex(content, dryRun)` |

### 路由与视图（`src/router/index.ts` + `src/views/`）

所有前端路由前缀为 `/assist/`，由 Vue Router 的 `createWebHistory()` 处理：

| 路由 | 视图 | 说明 | 权限 |
|------|------|------|------|
| `/assist/login` | `LoginView.vue` | 登录 / 注册 / Token 登录 / 首次初始化管理员（`meta.public`）；本机登录过的账号以卡片列表列出（点一下填好用户名，仍需输密码），另有「登录新账号」入口 | 公开 |
| `/assist/account` | `AccountView.vue` | 账户：资料、修改密码、个人访问令牌（Zotero/MCP） | 全部 |
| `/assist/config` | `ConfigView.vue` | 配置页（内含两个 tab：**服务配置** / **用户管理**——用户管理由 `UsersView.vue` 以 `embedded` 模式渲染） | admin |
| `/assist/drive` | `DriveView.vue` | 网盘：我的网盘 + 公有文件夹（列目录/上传/下载/预览/重命名/移动/删除） | 全部 |
| `/assist/drive/admin` | `DriveAdminView.vue` | 网盘管理：开关/单文件上限/默认配额、公有文件夹与角色阈值、逐用户配额 | admin |
| `/assist` | `HomeView.vue` | 文献列表（桌面表格/移动卡片 + 列管理 + 高级搜索） | 全部 |
| `/assist/detail/:id` | `DetailView.vue` | 文档详情（元信息 + PDF 元信息 / Crossref 记录 / 引用条目三个懒加载折叠区） | 全部 |
| `/assist/markdown/:id` | `MarkdownView.vue` | Markdown 阅读（浮动设置按钮收纳全部工具；行号默认开、移动端默认关；设置面板的「源码编辑」跳到编辑器并从当前阅读行开始载入） | 全部 |
| `/assist/markdown/:id/edit` | `MarkdownEditView.vue` | Markdown 编辑（AceEditor）；支持 `?line=a-b` 只载入指定行范围，阅读页「源码编辑」即跳到这里 | admin |
| `/assist/upload` | `UploadView.vue` | 上传 PDF | admin / view-upload |
| `/assist/doc/:id` | `DocManageView.vue` | 文档管理（含「PDF 健康」单篇检查，仅 admin 可见） | 全部 |
| `/assist/admin` | `AdminView.vue` | 监控面板（统计 + PDF 健康概览 + 维护操作 + 流水线监控） | admin |
| `/assist/duplicates` | `DuplicatesView.vue` | 去重（两种检测模式：标题相似 / DOI 重复，确认后合并到指定文档） | admin |
| `/assist/point` | `PointView.vue` | 向量库管理 | admin |
| `/assist/tools` | `ToolsView.vue` | 工具面板（全文搜索、语义搜索、问答、公式编辑、MCP 配置；支持 `?tab=` 直达，切 tab 同步地址栏） | 全部 |
| `/assist/mcp-setup` | —（重定向） | 旧路径，重定向到 `/assist/tools?tab=mcp` | 全部 |
| `/assist/chat` | —（重定向） | 旧路径，重定向到 `/assist/tools?tab=chat`（问答已并入工具页） | 全部 |
| `/assist/logs` | `LogViewer.vue` | 日志查看 | admin |

### AceEditor 编辑器组件（`src/components/AceEditor.vue`）
基于 `ace-builds@1.44.0` 封装的 Markdown 编辑器，内置完整编辑工具栏：
- **Markdown 快捷插入**：加粗/斜体/删除线/H1-H3/列表/引用/链接/图片/行内代码/代码块/表格/分割线
- **分屏预览（自适应）**：`showPreviewControls` 为真时默认开启分屏 —— **桌面端（>768px）左编辑器 / 右预览**，**移动端（≤768px）上编辑器 / 下预览**；中间分隔条可拖拽（`--split-ratio` CSS 变量，20%–80%）或双击复位 50:50
- **视口跟随的实现要点**：Ace 没有 `scroll` 事件，`session` 的 `changeScrollTop` 只覆盖程序化滚动，鼠标滚轮/拖动滚动条必须监听 renderer 里原生可滚动的 `.ace_scrollbar-v` 的 `scroll`；**`editor.getFirstVisibleRow()` / `getLastVisibleRow()` 返回的已经是文档行号**（`$computeLayerConfig` 内部已做过 screen→document 转换），**不能再调 `screenToDocumentRow()` 二次转换**（否则自动换行时会算错、预览只覆盖部分可见行）；容器尺寸变化靠 `ResizeObserver` 触发 `editor.resize()` + 重新计算可见行
- **预览行范围由用户设定（不再跟随滚动）**：头部两个输入框即渲染范围（`第 [起] – [止] 行`，文件行号 1-based、已加 `firstLineNumber` 偏移，回车/失焦生效）；编辑器滚动**不再**改变预览，预览面板可独立上下滚动；同一范围内编辑时保留预览滚动位置，范围变化时回到顶部。
- **「同步可见」按钮**：把渲染范围设为编辑器当前可视范围（`getFirstVisibleRow()/getLastVisibleRow()`，不足 6 行时自动补足）。首次挂载自动同步一次（`initialRangeSynced`），之后完全由用户控制。
- **范围约束**：单次最多渲染 `MAX_PREVIEW_LINES = 1000` 行；边界落在 `$$` 块级公式中间时会自动向外吸附到完整公式块（`snapRangeToMathBlocks`），避免预览里出现半截 `$$`。全屏预览（`⛶`）仍渲染本段全文。
- **全屏模式**：固定定位铺满窗口
- **搜索替换**：Ctrl+F / Ctrl+H（`ext-searchbox`）
- **字体缩放**：Ctrl+滚轮 / A+ A− 按钮（范围 8-30px）
- **撤销/重做**：Ctrl+Z / Ctrl+Shift+Z
- **光标/选中信息**：行号列号 + 选中字符数 + 总字数
- **数学公式**：预览通过 `MarkdownViewer` 组件渲染 KaTeX（`katex@0.18.4`）
- **模块加载**：核心模块静态 import；`ext-searchbox` 和 `ext-language_tools` 动态 import
- **移动端**：工具栏横向滚动，状态栏自动折行（桌面左右分栏 ⇄ 移动上下堆叠由 `useResponsive()` 的 `isMobile` 驱动）

### MarkdownViewer 渲染组件（`src/components/MarkdownViewer.vue`）
- 按 chunk 增量渲染：`chunk.id` 不变则不重渲染（`renderedChunkIds`）
- 公式：`extractMathPlaceholders()` 先把 `$..$` / `$$..$$` 换成占位符，再按**视口惰性**用 KaTeX 渲染；MathJax 模式按 chunk 增量 `typesetPromise`
- **公式渲染出错时只显示错误信息**：KaTeX 用 `throwOnError: true, strict: 'ignore'`，解析失败替换为 `.math-error`（`⚠ 公式渲染失败：<KaTeX 错误信息>`），**不显示 LaTeX 源码**（原文只放在 `title` 里供悬停查看）；KaTeX 整体加载失败时统一转为 `.math-error`（`markAllMathErrors`）。`mathMode: 'none'` 属于用户选择，仍显示原始 LaTeX。
- 支持 `mathMode` / `loadImages` / `showLineNumbers` / `lazyMath` 等开关
- **行号映射（`data-line`）**：行号为 1-based 且与原始 md 文件严格对应。实现方式是按 `marked` 的 top-level token 顺序累加 `token.raw` 的换行数；**多行块级公式被占位符折叠，必须用 `MathMatch.lineSpan` 把被吃掉的换行数补回**，否则每个跨 N 行的 `$$` 片段都会让后续行号漂移 N-1 行。逐块渲染用 `marked.parser([token])`，保证每个块能带上自己的起始行号
- `data-line` 的消费方：阅读页行号栏、`?target_line=` 深链、搜索/TOC 跳转、「源码编辑」定位起点行（`currentReadingLine`）

### 状态管理
- `useToast()`（`src/composables/useToast.ts`）：`showSuccess()` / `showError()` / `showInfo()` / `showToast()`
- `useRequireToken()`（`src/composables/useRequireToken.ts`）：需登录操作的前置校验；未登录时不再弹 Token 面板，而是跳 `/assist/login?redirect=<当前地址>`
- `useResponsive()`（`src/composables/useResponsive.ts`）：响应式断点检测（`isMobile` / `isDesktop` / `isTablet` / `isSmallMobile`）
- `usePagination()`（`src/composables/usePagination.ts`）：分页加载封装（`items` / `loading` / `page` / `total` / `load`）
- `useDebounce()`（`src/composables/useDebounce.ts`）：防抖工具
- Pinia stores：`pipeline.ts`（任务状态）、`toast.ts`（消息队列）、`token.ts`（**认证状态**：会话 token + 当前用户 + 角色/权限位，导出 `useTokenStore`，别名 `useAuthStore`）、`error.ts`（错误记录）

### API 层
`src/api/` 通过 `axios` 请求后端：
- `client.ts`：axios 实例（baseURL `/assist/api`，自动附加 X-Token）
- `documents.ts`：CRUD + 搜索 + 上传 + 文件访问 URL 生成
- `pipeline.ts`：流水线操作
- `admin.ts`、`config.ts`：管理/配置

### 类型定义
- `src/types/document.ts` — `Document`（含 `synced` / `fs_synced_at` / `has_pdf` / `has_md` / `has_zip`，`status` 是派生值）、`DocStatus`、`SearchResult`、`SearchResultList`、`TocItem`、`MarkdownResponse`、`ChunkResponse`
- `src/types/chat.ts` — `ChatMessage`、`ChatModelInfo`、`ChatModelsResponse`、`ChatRequest`、`ChatResponse`
- `src/types/config.ts` — `ServiceConfig`、`SystemConfig`
- `src/types/pipeline.ts` — `PipelineState`、`BatchProgress`

## MinerU 解析与图片处理

OKB-Assist 通过 MinerU API 将 PDF 解析为 Markdown，支持三种接口类型：
- **local**（自部署 V1 API）：通过 `mineru-kit api-server` 部署，`document/mineru4/mineru-api-server-report.md` 包含完整 API 参考。
- **official**（官方云 V4 精准解析 API）：通过 Token 认证，`document/mineru4/mineru-cloud-api.md`。
- **official-lightweight**（官方云 V1 Agent 轻量 API）：无需 Token，IP 限频。

`backend-rs/src/services/mineru.rs` 实现了完整的 API 客户端。

### 请求的输出格式

`submit_job()` 请求 `["markdown", "zip"]` 两种输出格式：
- **`markdown`**：独立 Markdown，图片以 `data:image/...;base64,...` 内嵌
- **`zip`**：自包含 ZIP 包，含独立图片文件 + 引用本地路径的 Markdown

### 图片存储逻辑（`get_task_result()`）

1. **优先从 ZIP 中提取 Markdown**（图片引用为本地路径如 `images/xxx.png`）
2. ZIP 中的图片同时提取为独立文件
3. 回退方案：若 ZIP 中无 Markdown，使用独立 Markdown 输出（保留 data URI）
4. 图片同时打包为 `images.zip` 存入 asset 路径

### `finish_parse_result()` 的职责

完成解析后：
1. 复制 `.md` 到 `data/markdowns/{id}.md`
2. 复制目录中的独立图片文件到同一目录
3. 复制 `images.zip` 到 `data/pdfs/{id}/{id}.zip`（asset 路径）

> **注意**：ZIP 中的 Markdown 引用的是相对路径图片（如 `images/xxx.png`），因此独立图片文件必须与 Markdown 放在同一目录层级，否则前端无法渲染图片。

## 编码约定

- **注释与 docstring 用中文**，与现有代码保持一致。
- 后端：Rust 2021 edition，使用 `tokio::main` 异步运行时，`axum` 路由，`sqlx` 数据库。
- 前端：Vue 3 + TypeScript，`<script setup lang="ts">`，`<style scoped>`。
- 状态用 `enum`（`DocStatus`、`IndexStatus`）。
- 新增路径需求改 `system.json` 模板，而非数据库。
- 前端响应式断点：`480px` / `768px` / `1024px`，移动优先。

## ⚠️ 易错点（编辑前必读）

1. **鉴权现状**：只认 `users` 段（会话 token / 用户个人令牌）。`system.json` 里**没有任何全局凭据字段**；`trusted_subnets` 默认不生效，需 `trusted_subnet_enabled = true`；系统未初始化时只有 `/auth/status|bootstrap` 可用。`TokenMiddleware` 仅对 `/assist/api/*` 生效。个人令牌的 `scope` **严格互斥**（api 只能调 API、mcp 只能调 MCP），新增受凭证保护的端点时记得同步判断 `Principal.api` / `Principal.mcp`，不要只看「令牌有效」。
2. **配置改动不会自动生效**：`config.json` 改后需 reload；`system.json` 改后需重启进程（进程内缓存）。
3. **MCP 只有一条路由，没有 SSE**（`backend-rs/src/main.rs`）：`.route("/assist/mcp/stream", post(..).options(..))`，处理器在 `mcp_server.rs::mcp_stream_handler`。全仓库没有任何 `axum::response::Sse` / `text/event-stream` / `Mcp-Session-Id` 代码；`/assist/mcp/sse`、`GET /assist/mcp/stream` 等未注册路径会走 `spa_fallback` 的 `mcp/` 前缀判定返回 404 JSON（不会回退到 index.html）。若要新增 SSE 传输，需自行实现 `GET` 流 + `POST` 消息通道，并在文档里同步说明。
4. **没有测试、没有 lint**：编辑后需手动 `cargo build` 确认编译通过，并用 curl 校验端点。
5. **局域网白名单配置**：`TokenMiddleware` 和 MCP 鉴权的 LAN 免 Token 白名单由 `system.json` 的 `trusted_subnets`（CIDR 数组）+ `trusted_subnet_enabled`（默认 `false`）+ `trusted_subnet_role`（默认 `admin`）共同控制；网段配了但开关没开时**不生效**。
6. **SQLite 单写者**：并发写入可行但仍是瓶颈，勿引入大量并发写。
7. **SPA Fallback 路由**（`backend-rs/src/main.rs`）：`@app.get("/assist/{full_path:path}")` 必须放在所有路由之后。命中 `api/`、`mcp/`、`uploads/`、`file/` 前缀时避免吞掉 API 和 MCP 端点。
8. **前端构建产物在 `frontend/dist/`**：`pnpm run build` 输出到 `frontend/dist/`。`vite.config.ts` 的 `base: '/assist/'` 使所有资源 URL 以 `/assist/assets/` 开头。
9. **CORS 已限定**：开发环境只允许 `localhost:5173`，生产环境只允许同源 `localhost:5001`。
10. **移动端导航**：`MobileMenu` 通过 Vue 的 `provide/inject` 机制与 `AppHeader` 通信。`App.vue` 提供 `toggleMobileMenu` / `closeMobileMenu`，`AppHeader` 注入并控制汉堡菜单状态。
11. **Rust 异步**：后台任务用 `tokio::spawn`，不要在 `async` 块中使用 `std::thread::sleep` 或阻塞 I/O。
12. **SQLx 编译时检查**：`sqlx` 默认在 `debug` 模式下会校验 SQL 查询，如果迁移了数据库结构需运行 `cargo sqlx prepare`（需安装 `sqlx-cli`）。
    **给 `documents` 加列的完整清单**（漏一处就会在运行时 `FromRow` 缺列报错/静默取空）：
   - `database.rs::init()` 的 `CREATE TABLE` + 同一个函数里的**轻量迁移数组**（`CREATE TABLE IF NOT EXISTS` 不会改已有库，新列必须在迁移数组里逐个 `ALTER TABLE`）；`documents` 现有列：16 个元数据字段 + `mineru_task_id` / `fs_synced_at` / `qdrant_collection` / `vector_db_id` / `created_at` / `updated_at` / `pdf_size` / `md_size` / `zip_size` / `pdf_health` / `pdf_health_message`（**没有** `status` / `status_message` / `progress`）；
   - `models.rs::Document` 字段；
   - 三份 `DOC_COLUMNS`：`routers/documents.rs`、`routers/pipeline.rs`、`mcp_server.rs`（列顺序需与 `Document` 字段顺序一致）；
   - `documents.rs::DocumentOut` + `doc_to_out()`（**不返回 `status` / `status_message`**：只给数据库记录 + `has_*` / `synced` / `fs_synced_at` / `pdf_health*` / `qdrant_collection` / `vector_db_id`，以及非数据库的 `runtime_task`、`last_error`；展示状态由前端 `utils/docStatus.ts::deriveDocStatus()` 推导）；
   - `documents.rs` 里两处 `Document { .. }` 字面量（上传 / 按路径登记）。
13. **MinerU Markdown 图片处理**（`backend-rs/src/services/mineru.rs`）：`get_task_result()` 必须优先从 ZIP 输出提取 Markdown（含本地图片引用），而非从独立 Markdown 输出（含 data URI）。`finish_parse_result()` 在 `pipeline.rs` 中还需**复制独立图片文件**到目标目录，否则 Markdown 中的相对路径引用会失效。
14. **MinerU API 的 output_formats 顺序无关**：`submit_job()` 必须同时请求 `"markdown"` 和 `"zip"` 两个格式。`zip` 格式包含解析结果的完整输出（Markdown + 图片），而 `markdown` 格式是自包含的 data URI 版本。两套输出分别下载后合并处理。
15. **`users` / `drive` 段不要手改、不要经 `/assist/api/config` 写入**：`ConfigManager::save_config()` 会跳过 `PROTECTED_CONFIG_SECTIONS = ["users", "drive"]` 并从磁盘取回真值；用户写操作必须走 `users::UserStore`（内部 `update_users_section` 原子写 + 写锁）。密码哈希/令牌哈希永不回显。
    **注意 MCP 令牌明文会落盘**（`users.items[].api_tokens[].token`，仅 `scope = mcp`）：`config.json` 的读权限等同于对应账号的 MCP 权限，新增备份/导出/日志功能时不要顺手把 `users` 段带出去。
16. **删改用户要级联撤销凭据**：禁用/删除用户、重置密码都要 `sessions.revoke_user(&id)`（`routers/users.rs` 已处理）；`set_password` **不会**吊销用户已签发的令牌（需管理员显式 `revoke_tokens`）。
17. **网盘路径必须过 `resolve_path()`**：任何来自客户端的相对路径都要经 `sanitize_rel_path` + canonicalize 校验，禁止直接 `base.join(user_input)`（防 `../` 与符号链接逃逸）；新增文件操作端点时同样处理。
18. **`drive` 段是受保护配置**：修改公有文件夹/配额/设置必须走 `/assist/api/admin/drive/*`（内部 `update_section`），不要扩展 `PUT /assist/api/config` 去写它，否则会与 `save_config` 的“从磁盘取回真值”逻辑打架。
19. **全屏（`meta.bare`）路由的高度链**：`main-content-bare` → `.error-boundary-wrap` → 页面容器 → `AceEditor` 每一层都必须是纵向 flex（`flex:1; min-height:0`）。`App.vue` 用 `.main-content-bare :deep(.error-boundary-wrap)` 补了这一层；漏掉就会让内部 `height:100%/flex:1` 的编辑器塌陷成内容高度（表现为编辑区只有 200px 高、下方一大片空白）。
18. **静态目录挂载（`static_dirs`）的路由注册顺序**（`main.rs::create_app`）：这些路由必须在 `.layer(middleware)` **之前**注册 —— 处理器靠 Extension 里的 `Settings`/`UserStore`/`SessionStore` 做鉴权，而 axum 的 `Router::layer` 只对已存在的路由生效（这也是 `spa_fallback` 改用全局 `settings` 单例的原因）。另外 axum 的 `/*rest` 通配符**不匹配空剩余路径**，`/prefix`、`/prefix/`、`/prefix/*rest` 三条都要注册，否则访问不了 `/prefix/`。
19. **静态目录的鉴权与内置路由不同**：它不走 `token_middleware`（后者只覆盖 `/assist/api/*`），而是在处理器内自己判（`static_files::authorize`：`okb_static` cookie / 会话・用户令牌 / 受信任子网），所以新增挂载时要保留这条判定，不能直接读文件。另外：`axum::body::Body` **不是 `Sync`**，`serve_mount` 里不能把 `&Request` 跨 `await` 传给 helper，否则整个 future 不再是 `Send`，axum 会报 `Handler` 未实现（现已改成传 `&str` / `&StaticMount`）。
20. **前端启动重定向必须先 `await router.isReady()`**（`App.vue`）：硬导航（直接输入 URL / 外链）时 `route` 还是初始 `START_LOCATION`，`route.fullPath` 是 `/`，不等待会把 `?redirect=` 覆盖成 `/`，导致“登录后回跳”失效。
21. **大文件唯一路径是流式**：上传/解析链路里 **禁止** `tokio::fs::read(整份 PDF)`、`field.bytes()`、`resp.bytes().to_vec()` 之后只为了“顺手算个哈希/交给下一个函数”。哈希用 `utils::calculate_file_hash_async`（流式）或 `stream_field_to_temp_file` 里顺带算；上传请求体用 `reqwest::Body::wrap_stream(ReaderStream::new(file))` **并显式给 `Content-Length`**（否则退化成 chunked，OSS 预签名 URL 会校验失败）；multipart 用 `Part::stream_with_length`。
22. **日志查看只读进程内环形缓冲，不读日志文件**：`logger.rs` 在文件层 / stdout 层之外多挂了一个 `fmt::layer().json()`（`RingBufferWriter`）——每条事件写完（写入器的 `Drop`）把**整行 JSON** 追加进 `LogStore` 的 `VecDeque`（`RING_CAPACITY = 2000`，进程重启即清空）。因为与 JSONL 文件层同源，缓冲里的行与 `okb-log.jsonl` 里的行逐字节一致。
    - `GET /assist/api/admin/logs?since=&level=&limit=&include_self=` 是**纯内存**增量接口（无 `spawn_blocking`、无磁盘 IO）：`since` 传上次响应的 `next_seq`，返回 `seq > since` 且不低于 `level` 的条目；`truncated = true` 表示命中数超过 `limit`（前端应立刻再拉一次追平），`lost` 是 `since` 之后被挤出缓冲区的条数。
    - **默认过滤掉本接口自身的请求日志**（`SELF_LOG_PATH = "/assist/api/admin/logs"`，匹配 `fields.path` / `fields.uri` / `spans[].uri`），否则前端每秒轮询会把自己的请求日志刷进列表；要看它们就传 `include_self=1`。过滤谓词在 `LogStore::snapshot` 内生效，`matched` / `truncated` / 游标都按过滤后的结果算，不会卡住游标。
    - 每条返回的条目都会补一个 `seq` 字段：前端拿它做 `v-for` 的 key 和「展开哪一条」的标识，**不要改回用数组下标**（追加新日志会让展开项错位，历史 bug 就是展开后自动折叠）。
    - 环层的序列化**不受 `--log-level` 限制**（`EnvFilter` 用 `.with_filter()` 只贴在文件层与 stdout 层上），否则前端把等级切到 debug/trace 会永远是空的。`--log-level` 依旧只决定落盘/终端能看到什么。
    - 前端 `LogViewer.vue` 每 1 秒拉一次增量、`MAX_ENTRIES = 2000` 封顶（页面隐藏时暂停、回前台补一次），关键词过滤是纯客户端 `computed`（不触发请求、不重连），改等级 = 重置游标重新载入。
    - **不要再改回读日志文件**（旧的 `ReverseLines` / 字节游标 `since` / 半行处理 / `scanned_bytes` 已随 `get_logs` 一并删除）；历史日志的去处是 `system.json` 的 `log_path` 文件，页面只回显路径。

23. **PDF 引擎是运行时选择，不是编译期开关**：`pdf_meta` / `pdf_health` 的公共入口一律先 `services::pdf_engine::active()` 再分发，mutool 那套实现**必须始终编译**（不要再加 `#[cfg(not(feature = "mupdf"))]`）。新增 PDF 能力时要两套实现都补，否则切引擎后会静默失能；另外 `mupdf` 引擎拿不到 stderr 诊断，依赖诊断的功能（如「损坏但可修复」判定）要在 UI 上提示用户切回 `mutool`。
24. **BibTeX 导入的条目没有 PDF**：`import_bibtex` 建的记录写入 `fs_synced_at`（三个 size = NULL，即「已同步且确定不存在」）+ `pdf_health = 'missing'`，派生状态自然是 `uploaded` / 「PDF 缺失」；**不要**在 `do_parse_impl` 里把这种记录打成 `error`（worker 里已加「PDF 不存在就跳过」的真实文件守卫，批量解析只能空转一次）。后续上传 PDF 走文档管理页的「替换 PDF」（`POST /documents/:id/pdf`），上传页重新上传会另建一条记录（上传去重只在显式传 `doi` 表单字段时比 DOI），需要时用去重页合并。

25. **`year` 列是 SQLite 动态类型，取出时必须 `CAST(NULLIF(TRIM(year), '') AS INTEGER)`**：老库里存在写成 `''`（TEXT 类型）的年份，sqlx 把 TEXT 解码成 `Option<i64>` 会直接报
    `mismatched types; Rust type Option<i64> (as SQL type INTEGER) is not compatible with SQL type TEXT`，**整条 `fetch_all` 一起失败**。
    历史教训：`import_bibtex` 用 `sqlx::query_as(...).fetch_all().await.unwrap_or_default()` 吞掉这个错误，索引变成空表 → 一个重复都检不出来（131 条全部报「可新建」，直接导入会造出 131 条重复记录）。
    现有约定：`DOC_COLUMNS`（documents.rs / pipeline.rs / mcp_server.rs 三份）+ `import_bibtex` 的索引查询都带这个 CAST；`database.rs::init()` 启动时还会把历史 TEXT 年份一次性修成 INTEGER/NULL。
    **新增任何读取 `year` 的查询都必须带 CAST，且不要再对查询结果 `unwrap_or_default()`**（读不到就该报错，不能静默当成空库）。

## 入口与关键文件速查

- 后端入口：`backend-rs/src/main.rs`
- 配置：`backend-rs/config.json`、`backend-rs/system.json`、`backend-rs/src/config_manager.rs`、`backend-rs/src/config.rs`
- PDF 引擎选择：`backend-rs/src/services/pdf_engine.rs`（`mutool` / `mupdf`，`config.json` 的 `pdf.engine`）
- 架构核心：`backend-rs/src/routers/pipeline.rs`、`backend-rs/src/routers/documents.rs`、`backend-rs/src/models.rs`、`backend-rs/src/services/vector_db.rs`、`backend-rs/src/mcp_server.rs`
- 前端：`frontend/`（Vue 3 + TypeScript + Vite 8）
- MCP 工具参考：`document/mcp.md`
- 文档站（docsify）：`document/index.html`、`document/README.md`、`document/_sidebar.md`、`document/assets/css/custom.css`
- 搜索语法参考：`document/search.md`
- 启动说明：`README.md`