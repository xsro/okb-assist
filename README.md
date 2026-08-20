# 个人文献库

本仓库实现一个简易的文献库，可以通过mcp访问。

后端代码位于 `backend/` 目录，启动前请先进入该目录：

```bash
cd backend
uv run okb_assist_main.py --host 0.0.0.0
# 或生产式启动：uv run python -m uvicorn okb_assist_main:app --host 0.0.0.0 --port 5001
```

### 启动向量化数据库和索引服务

```bash
cd data
qdrant
```

```bash
cd backend
uv run scripts/fastembed_server.py
```

### 启动webui服务

```bash
cd backend
bash scripts/start-openwebui.sh
```


## 其他选择

### 使用docker启动向量数据库

```
sudo docker pull docker.1ms.run/qdrant/qdrant:latest
sudo docker run -p 6333:6333 -v $(pwd)/qdrant_data:/qdrant/storage docker.1ms.run/qdrant/qdrant:latest
```
可以访问`http://:6333/dashboard`管理向量数据库


