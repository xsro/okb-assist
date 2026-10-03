# okb-assist

OKB (Oh my Knowledge Base) is my personal service to organize papers, books as a MCP for AI agents like Codex, Claude.
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
  "token": "change-me",
  "mcp_token": "change-me",
  "trusted_subnets": ["192.168.1.0/24", "127.0.0.1/32"],
  "max_concurrent_tasks": 3
}
```

| 字段 | 说明 |
|------|------|
| `token` | Admin Token，为空或 `change-me` 时跳过鉴权 |
| `mcp_token` | MCP Bearer Token |
| `trusted_subnets` | 受信任子网白名单（CIDR 数组），匹配的请求免 Token 鉴权。空数组 `[]` 表示不信任任何子网 |
| `max_concurrent_tasks` | 全局 extract/index 最大并发数 |

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