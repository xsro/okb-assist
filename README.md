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

> 修改 `system.json` 后需**重启进程**才能生效（进程内缓存）。
> 修改 `config.json` 后需调用 `/assist/api/config/reload` 端点或重启才能生效。

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