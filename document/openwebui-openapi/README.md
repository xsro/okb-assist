# OKB-Assist OpenAPI Tool Server

通过 OpenAPI 协议将 OKB-Assist 知识库连接到 OpenWebUI。

## ⚠️ 重要配置说明

OpenWebUI 需要的是 **OpenAPI Schema URL**，格式为：
```
http://your-server:port/openapi.json
```

**不是** `/assist/openapi/` 路径！

## OpenWebUI 配置步骤

### 1. 启动 OKB-Assist 服务

```bash
cd /home/a422/repo/okb-assist
python main.py
# 或
uvicorn main:app --host 0.0.0.0 --port 8000
```

### 2. 验证 OpenAPI Schema

在浏览器中访问：
```
http://your-server:8000/openapi.json
```

应该能看到 JSON格式的 OpenAPI schema。

### 3. 在 OpenWebUI 中配置

1. 登录 OpenWebUI 管理后台
2. 进入 **Settings** → **Connections**
3. 在 **OpenAPI Servers** 部分点击 **Add**
4. 填写配置：
   - **Name**: `OKB-Assist` (自定义名称)
   - **URL**: `http://your-server:8000/openapi.json`
   
   例如：`http://192.168.1.162:8000/openapi.json`

5. 点击 **Save**

## 功能端点

配置完成后，OpenWebUI 可以调用以下工具：

| 端点 | 功能 | 参数 |
|------|------|------|
| `GET /assist/openapi/search` | 语义搜索知识库 | `q` (查询), `limit` (数量) |
| `GET /assist/openapi/grep-search` | 全文搜索（grep） | `q` (查询), `limit`, `context` |
| `GET /assist/openapi/documents` | 列出文献列表 | `status`, `doc_type`, `page`, `page_size` |
| `GET /assist/openapi/documents/{id}` | 获取文献详情（含链接） | `doc_id` |
| `GET /assist/openapi/stats` | 获取统计信息 | 无 |
| `GET /assist/openapi/doc-types` | 获取文献类型列表 | 无 |

## 使用示例

在 OpenWebUI 对话中：

```
用户：搜索关于"滑模控制"的文献
AI：[自动调用 search 端点] 找到5条相关结果...

用户：列出已索引的文献
AI：[自动调用 documents 端点] 共1571篇文献...

用户：获取ID为710的文献详情和链接
AI：[自动调用 documents/710 端点] 文献详情...
```

## 常见问题

### Q: 连接失败怎么办？

1. **检查 URL 格式**
   - ❌ 错误：`http://192.168.1.162:8081/assist/openapi/`
   - ✅ 正确：`http://192.168.1.162:8000/openapi.json`

2. **检查服务是否运行**
   ```bash
   curl http://localhost:8000/openapi.json
   ```

3. **检查端口是否正确**
   - OKB-Assist 默认端口：8000
   - 检查 main.py 或启动命令中的端口配置

4. **检查防火墙**
   - 确保端口未被防火墙阻止

### Q: OpenWebUI 显示 "No tools available"

确保 OpenAPI schema 中包含正确的路由：
```bash
curl http://your-server:8000/openapi.json | grep "/openapi/"
```

应该看到 `/assist/openapi/search` 等路由。

## API 详细说明

### 语义搜索

```
GET /assist/openapi/search?q={query}&limit={limit}
```

**示例响应：**
```json
{
  "query": "滑模控制",
  "results": [
    {
      "document_id": 710,
      "title": "Fuzzy Adaptive Disturbance-Observer-Based Robust Tracking Control...",
      "authors": "ZhongYi Chu, Jing Cui, FuChun Sun",
      "year": 2014,
      "journal": "IEEE Systems Journal",
      "content": "...",
      "score": 0.85
    }
  ],
  "total": 1
}
```

### 列出文献

```
GET /assist/openapi/documents?status=indexed&page=1&page_size=10
```

### 获取详情（含链接）

```
GET /assist/openapi/documents/{doc_id}
```

**示例响应：**
```json
{
  "id": 710,
  "title": "Fuzzy Adaptive Disturbance-Observer-Based Robust Tracking Control...",
  "authors": "ZhongYi Chu, Jing Cui, FuChun Sun",
  "year": 2014,
  "doc_type": "journalArticle",
  "journal": "IEEE Systems Journal",
  "doi": "10.1109/JSYST.2014.2345678",
  "abstract": "...",
  "status": "indexed",
  "detail_page": "http://localhost:8000/redirect/710",
  "pdf_download": "http://localhost:8000/assist/api/documents/710/pdf",
  "markdown_content": "http://localhost:8000/assist/api/documents/710/markdown"
}
```

### 统计信息

```
GET /assist/openapi/stats
```

**示例响应：**
```json
{
  "total_documents": 1571,
  "status_counts": {
    "indexed": 704,
    "meta_done": 1513,
    "markdown_done": 58,
    "error": 13
  },
  "indexed_count": 704
}
```
