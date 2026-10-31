# okb-assist

OKB is my personal service to organize papers, books as a MCP for AI agents like Codex, Claude.
I name this by "Open-webui Knowledge Base" because I used to use this to extend my open webui's knowledge base.
Now it only a place for manage papers, parse to markdowns and serve as MCP.
I use pi agent to code this and test this.

## 外部依赖

本项目依赖 `mutool` 处理 PDF 元数据提取

- release中的版本将mupdf的功能内置了
- 如果编译的时候未启用 `mupdf` feature ，则需要系统 PATH 中需包含 `mutool` 
  - **mutool**：来自 MuPDF 工具集，安装方式见 <https://mupdf.com/downloads/>

## 后端（Rust）

后端代码位于 `backend-rs/` 目录，使用 Cargo 启动：

```bash
cd backend-rs
cargo build --release
cargo run -- --host 0.0.0.0 --port 5001
```

## 配置

> 修改 `system.json` 后需**重启进程**才能生效（进程内缓存）。
> 修改 `config.json` 后需调用 `/assist/api/config/reload` 端点或重启才能生效。

系统配置文件为 `backend-rs/system.json`，修改后需要重启服务生效。

### 系统配置

```json
{
  "trusted_subnets": [],
  "trusted_subnet_enabled": false,
  "trusted_subnet_role": "admin",
  "max_concurrent_tasks": 3,
  "drive_root": "data/drive"
}
```

> 凭据不再放在 `system.json`：管理员账号、密码与个人令牌（`okb_…`）全部存在 `config.json` 的 `users` 段，
> 首个管理员由前端初始化页创建（旧版的 `token` / `mcp_token` 两个全局凭据字段已移除）。

| 字段 | 说明 |
|------|------|
| `trusted_subnets` | 受信任子网白名单（CIDR 数组） |
| `trusted_subnet_enabled` | **默认 `false`**：必须显式开启后，`trusted_subnets` 命中的请求才免鉴权 |
| `trusted_subnet_role` | 命中受信任子网时授予的角色（默认 `admin`） |
| `max_concurrent_tasks` | 全局 extract/index 最大并发数 |
| `drive_root` | 用户网盘根目录（私有 `users/{user_id}/`、公有 `_public/{folder_id}/`），默认 `data/drive`；
  可写绝对路径（如 `/mnt/disk/okb-drive`）把网盘放到独立磁盘，支持 `{system_dir}` / `{env:VAR}` |

### 用户与登录

用户数据存放在 `config.json` 的 `users` 段（密码只存 Argon2id 哈希），支持：

- **首次初始化**：访问 `/assist/login`，页面提示创建第一个管理员（无需任何预置 token）；
- **密码登录**：登录后签发 7 天滑动续期的会话 token（`sess_…`，存 localStorage，请求头 `X-Token`）；
- **Token 登录**：可直接粘贴个人访问令牌（`okb_…`）或会话 token；
- **注册**：默认开放自助注册，新用户为 `pending`，需管理员在「用户管理」页审核；
- **个人令牌**：每个用户在「账户」页签发/撤销，用于 Zotero、MCP 客户端等；权限**二选一**（`API` / `MCP`），且可自定义令牌明文（≥ 8 位、可见 ASCII）；
- **MCP 访问**：用户级开关，`用户.mcp = true` 且令牌权限为 `MCP` 时才可调用（API 令牌不能调 MCP，反之亦然）。

忘记管理员密码时的兜底：

```bash
cd backend-rs
./target/release/okb_assist --reset-password admin --new-password '新密码'
```

> ⚠️ 从旧版本升级：`config.json` 里旧的 `permissions` 条目会被一次性迁移成 `legacy-xxx` 占位用户
> （`pending`、无凭据），**旧 token 全部失效**，需管理员为其重置密码或直接删除。
> 详见 [document/user-management.md](./document/user-management.md)。

### 用户网盘

- 每个用户有独立私有网盘（`{drive_root}/users/{user_id}/`），支持多级目录、上传下载、常见格式在线预览；
- 管理员可创建**公有文件夹**（`{drive_root}/_public/{folder_id}/`），权限按**角色阈值**配置：
  `read_role` / `write_role`，等级 `admin > view-upload > view-only`，用户角色 ≥ 阈值即可读/写；
- 配额默认不限，可按用户单独覆盖（0 = 不限）；单文件大小上限默认 200MB；
- 与文献库/解析流水线**完全独立**；
- 策略存在 `config.json` 的 `drive` 段（配置页不可改写，走 `/assist/api/admin/drive/*`）。

前端入口：导航「网盘」（`/assist/drive`）；管理员另有「网盘管理」（`/assist/drive/admin`）。
详细说明见 [document/drive.md](./document/drive.md)。

### 工作目录（`cwd`）

程序启动时会 `chdir` 到 `cwd` 指定的目录，后续所有相对路径都相对于该目录解析。`cwd` 的值支持 `{system_dir}` 变量替换：

```json
{
  "cwd": "{system_dir}"
}
```

### 路径属性

除了 `cwd` 支持 `{system_dir}` 替换外，其他路径属性均为相对于 `cwd` 的相对路径或者系统绝对路径，仅支持 `{id}` 变量（文档 ID）：

| 属性 | 说明 | 示例 |
|------|------|------|
| `database_url` | SQLite 连接串 | `"sqlite:///data/okb_assist.db"` |
| `markdown_path` | Markdown 文件路径 | `"data/markdowns/{id}.md"` |
| `pdf_path` | PDF 文件路径 | `"data/pdfs/{id}/{id}.pdf"` |
| `info_path` | 元信息 JSON 路径 | `"data/markdowns/{id}.json"` |
| `crossref_path` | Crossref 数据路径 | `"data/markdowns/{id}_crossref.json"` |
| `markdown_asset_path` | Markdown 资产包路径 | `"data/pdfs/{id}/{id}.zip"` |
| `uploads_folder` | 上传目录 | `"data/_uploads"` |
| `mutool_path` | mutool 可执行文件路径 | `"mutool"` |
| `ui_path` | 前端 UI 构建产物路径 | `"frontend/dist"` |


## 启动 Web UI

```bash
cd frontend
pnpm run build
# 构建产物由后端直接 serving，访问 http://localhost:5001/assist/
```

开发模式：

```bash
cd frontend
pnpm run dev
# 访问 http://localhost:5173/assist/
```

## TODO

程序虽然包含部分向量数据库相关的语义搜索功能，但是由于我觉得可能用处不大，所以没有测试。