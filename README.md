# 个人文献库

本仓库实现一个简易的文献库，可以通过 MCP 访问。

## 外部依赖

本项目依赖以下外部命令行工具：

| 工具 | 用途 | 配置方式 |
|------|------|----------|
| `grep` | 全文搜索（`/api/documents/grep-search/`） | `system.json` 的 `grep_path`（默认 `grep`） |
| `pdfcpu` | PDF 元数据提取（上传/注册/元数据补全时） | `system.json` 的 `pdfcpu_path`（默认 `pdfcpu`） |

> 安装方式：
> - **grep**：Linux/macOS 系统通常自带；Windows 需安装 Git Bash 或 WSL
> - **pdfcpu**：`brew install pdfcpu`（macOS）或从 [https://github.com/pdfcpu/pdfcpu](https://github.com/pdfcpu/pdfcpu) 下载二进制

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

### 工作目录（`cwd`）

程序启动时会 `chdir` 到 `cwd` 指定的目录，后续所有相对路径都相对于该目录解析。`cwd` 的值支持 `{system_dir}` 和 `{system_path}` 变量替换：

```json
{
  "cwd": "{system_dir}"
}
```

### 路径属性

除了 `cwd` 和 `config_path` 支持 `{system_dir}` 替换外，其他路径属性均为相对于 `cwd` 的相对路径或者系统绝对路径，仅支持 `{id}` 变量（文档 ID）：

| 属性 | 说明 | 示例 |
|------|------|------|
| `database_url` | SQLite 连接串 | `"sqlite:///data/okb_assist.db"` |
| `markdown_path` | Markdown 文件路径 | `"data/markdowns/{id}.md"` |
| `pdf_path` | PDF 文件路径 | `"data/pdfs/{id}/{id}.pdf"` |
| `info_path` | 元信息 JSON 路径 | `"data/markdowns/{id}.json"` |
| `uploads_folder` | 上传目录 | `"data/_uploads"` |

详见 [AGENTS.md](AGENTS.md) 的配置章节。

## 启动向量化数据库和索引服务

```bash
cd data
qdrant
```

```bash
cd backend-rs
# Fastembed 嵌入服务（如需）
# 旧版 Python 脚本已移除，如需启动请查看 scripts/ 目录或使用 Docker
```

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

## 其他选择

### 使用 Docker 启动向量数据库

```
sudo docker pull docker.1ms.run/qdrant/qdrant:latest
sudo docker run -p 6333:6333 -v $(pwd)/qdrant_data:/qdrant/storage docker.1ms.run/qdrant/qdrant:latest
```

可以访问 `http://localhost:6333/dashboard` 管理向量数据库。