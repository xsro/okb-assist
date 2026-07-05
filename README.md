```bash
uv run main.py --host 0.0.0.0
```


请帮我设计一个用于存储我的所有论文和专著数据的系统，主要功能为：

- 用户上传pdf，用户需要设置token确认上传
- 将上传的pdf通过mineru解析为markdown 
- 将获得的markdown通过ollama提取meta信息，包括类型（例如book，article），标题，年份，作者，摘要，doi，来源，期刊名/会议名、关键词、分类、**主要语言**等信息，并将meta通过 yaml 元数据 的形式添加到markdown中
    - **语言检测**：自动检测文献主要语言（使用 ISO 639-1 代码）
    - **非英文文献**：同时生成原文和英文版本的元数据（标题、作者、摘要、期刊、关键词）
    - **英文文献**：仅生成英文元数据
    - 支持的语言：中文(zh)、日语(ja)、韩语(ko)、法语(fr)、德语(de)、西班牙语(es)、葡萄牙语(pt)、俄语(ru)、阿拉伯语(ar)、意大利语(it)、荷兰语(nl)、波兰语(pl)、泰语(th)、越南语(vi)、土耳其语(tr) 等
- 将meta和markdown分片上传给qdrant。

已有如下的程序
```
MINERU_URL=http://127.0.0.1:8002
MINERU_KEY=key
MINERU_TASKS=3

OLLAMA_URL=http://127.0.0.1:11434
OLLAMA_KEY=
OLLAMA_MODEL=qcwind/qwen3-8b-instruct-Q4-K-M:latest
```

并且我已经运行了`qdrant`

```
docker pull docker.1ms.run/qdrant/qdrant:latest
docker run -p 6333:6333 -v $(pwd)/qdrant_data:/qdrant/storage docker.1ms.run/qdrant/qdrant:latest
```
可以访问`http://:6333/dashboard`管理向量数据库


技术路线使用fastapi+vanilla js，本地数据库用sqlite。向量数据库使用Qdrant，所有的api和页面都放在`/assist/`路径下，需要设计管理页面。

- `/assist/api` 帮我设计api
- `/` 自动跳转到 `/assist/` 
- `/assist/` 显示所有的文献的列表，
    - 可以查看markdown，pdf的路径，或者点击查看markdown文件和pdf文件。
    - 可以点击自动开始处理pdf，分为三个阶段：生成markdown，生成meta和提交到向量数据库。
    - 可以删除文件及相关资源，可以替换pdf，编辑markdown，meta
- `/assist/upload` 上传pdf文件。

## 补充说明

### 认证方式
- 无需登录，所有页面可直接访问
- 仅**上传 PDF** 时需要提供令牌（token）
- 令牌通过环境变量 `UPLOAD_TOKEN` 配置
- 上传时通过 `X-Token` 请求头传递令牌

### 向量化策略
- 使用 **Ollama embedding 模型**（如 `nomic-embed-text`）生成向量
- 采用**固定大小分片**策略上传到 Qdrant

### 配置管理
- 所有配置项（MinerU、Ollama、Qdrant 连接信息）仅从 `.env` 文件读取
- 不提供 UI 配置界面

### 页面结构
- `/assist/` — 文献列表页面（查看、搜索、删除）
- `/assist/upload` — 上传 PDF（需要令牌）
- `/assist/detail/{id}` — 文献详情（查看、编辑、处理）
- `/assist/markdown/{id}` — Markdown 查看器（分页、公式渲染）
- `/assist/monitor` — 任务监控（队列状态、进度跟踪）
- `/assist/admin` — 管理后台（系统统计、Qdrant 状态）

### Open WebUI 集成

配置 Open WebUI 连接 Qdrant 向量数据库：

| 配置项 | 值 |
|--------|-----|
| 名称 | `文献库` |
| 提供商 | `Qdrant` |
| 描述 | `学术论文与专著文献库` |
| Endpoint | `http://<服务器IP>:6333` |
| 超时时间 | `30` |
| API Key / Token | 留空（无认证） |
| 文件集 | `documents_0` |
| Content Field | `payload.text` |
| Vector Field | `vector` (默认) |
| Metadata Field | `payload.metadata` |
| Document ID Field | `id` |

**重要提示：**
- Open WebUI 的 embedding 模型必须与本系统一致：`nomic-embed-text`（维度 768）
- Test Query 示例：`高速动车组控制`

**Qdrant Payload 结构：**
```json
{
  "text": "文档内容片段...",
  "metadata": {
    "document_id": 1,
    "chunk_index": 0,
    "title": "论文标题",
    "authors": ["作者1", "作者2"],
    "year": 2024,
    "doc_type": "article",
    "keywords": ["关键词1", "关键词2"]
  }
}
```
