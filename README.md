# 个人文献库

本仓库实现一个简易的文献库，可以通过 MCP 访问。

## 后端（Rust）

后端代码位于 `backend-rs/` 目录，使用 Cargo 启动：

```bash
cd backend-rs
cargo build --release
cargo run -- --host 0.0.0.0 --port 5001
```

### 配置

系统配置文件为 `backend-rs/system.json`，修改后需要重启服务生效。路径属性支持变量替换：

| 变量 | 说明 |
|------|------|
| `{id}` | 文档 ID |
| `{system_dir}` | system.json 所在目录的绝对路径 |
| `{system_path}` | system.json 的完整绝对路径 |
| `{env:VAR_NAME}` | 环境变量 `VAR_NAME` 的值 |
| `{cwd}` | 当前工作目录 |

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