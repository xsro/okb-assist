# MinerU v1 REST API 服务调研报告

> 调研时间：2025 年 7 月  
> 数据来源：MinerU 官方仓库 `mineru/parser/api_server.py` 及相关模块  
> 调研方式：通过代理直读 GitHub 源码

---

## 概述

MinerU v1 REST API 是一个基于 **FastAPI** 构建的文档解析服务。它遵循 OpenAI 风格的分片上传→提交任务→轮询结果的异步工作流。支持 4 个解析**档位（tier）**、7 种**输出格式**、4 种**文件来源**和完整的**任务生命周期管理**。

服务启动方式：

```bash
mineru-kit api-server --host 0.0.0.0 --port 8000 --tier standard
```

---

## 1. 基础信息

| 项目 | 值 |
|------|-----|
| 基础路径 | `/v1` |
| 框架 | FastAPI (Uvicorn) |
| Swagger 文档 | `/docs` (默认启用，可通过 `MINERU_API_ENABLE_FASTAPI_DOCS=0` 关闭) |
| OpenAPI JSON | `/openapi.json` |
| ReDoc | `/redoc` |

### 命令行选项

| 选项 | 默认值 | 说明 |
|------|--------|------|
| `--host` | `127.0.0.1` | 监听地址 |
| `--port` | `8000` | 监听端口 |
| `--tier` | `standard` | 服务能力档位：flash / basic / standard |
| `--concurrency` | `1` | 最大并发解析任务数 |
| `--no-flash` | 否 | 禁用 Flash tier |
| `--no-advanced` | 否 | 禁用 Advanced tier |
| `--preload-models` | 否 | 启动时预加载 VLM 和本地 Hybrid 模型 |
| `--api-key` | 无 | API Key，设置后客户端需传入 `Authorization: Bearer <key>` |
| `--upload-dir` | 自动创建的临时目录 | 上传和解析产物存储目录 |
| `--allow-local-source` | 否 | 允许 local 来源读取服务器文件系统的路径 |
| `--allow-http-source` | 否 | 允许 url 来源使用 HTTP（HTTPS 始终允许） |
| `--url-timeout` | `60` | URL 来源下载超时（秒） |
| `--max-inline-bytes` | `1MB` | inline 来源最大大小（字节） |
| `--max-url-bytes` | `200MB` | URL 来源最大大小（字节） |
| `--disable-image-analysis` | 否 | 禁用 Hybrid 后端图像分析 |
| `--vlm-server-url` | 全局配置 | 远程 VLM 服务 URL |
| `--vlm-api-key` | 全局配置 | 远程 VLM API Key |
| `--vlm-model` | 全局配置 | VLM 模型名 |
| `--vlm-http-timeout` | `600` | VLM HTTP 超时（秒） |
| `--vlm-max-concurrency` | `100` | VLM 推理并发数 |

---

## 2. 全部 API 端点总览

| 分组 | 方法 | 路径 | 说明 |
|------|------|------|------|
| **Health** | GET | `/v1/health` | 健康检查 |
| **Models** | GET | `/v1/models` | 列出所有可用 VLM 模型 |
| | GET | `/v1/models/{model}` | 获取单个模型信息 |
| **Tiers** | GET | `/v1/tiers` | 列出所有可用解析档位 |
| **Uploads** | POST | `/v1/uploads` | 创建上传会话 |
| | GET | `/v1/uploads/{upload_id}` | 查询上传会话状态 |
| | PUT | `/v1/uploads/{upload_id}/content` | 上传文件内容（二进制） |
| | POST | `/v1/uploads/{upload_id}/complete` | 完成上传（生成 File 对象） |
| | POST | `/v1/uploads/{upload_id}/cancel` | 取消上传 |
| **Files** | GET | `/v1/files` | 列出文件 |
| | GET | `/v1/files/{file_id}` | 获取文件元数据 |
| | GET | `/v1/files/{file_id}/content` | 下载解析输出文件内容 |
| | DELETE | `/v1/files/{file_id}` | 删除文件 |
| **Jobs** | POST | `/v1/parse/jobs` | 创建解析任务 |
| | GET | `/v1/parse/jobs/{job_id}` | 查询任务状态和结果 |
| | GET | `/v1/parse/jobs` | 列出任务 |
| | DELETE | `/v1/parse/jobs/{job_id}` | 取消任务 |
| **Usage** | GET | `/v1/usage` | 查询用量和限制 |

---

## 3. 端点详解

### 3.1 健康检查

**`GET /v1/health`**

无需认证，返回服务器运行状态和功能特性。

**响应体：**
```json
{
  "status": "ok",
  "version": "4.x.x",
  "features": {
    "webhook": false,
    "output_formats": ["markdown", "middle_json", "structured_content", "zip"],
    "sources": ["file_id", "url", "inline"]
  }
}
```

### 3.2 模型列表

**`GET /v1/models`**

无需认证，返回当前服务器可用的 VLM 模型列表。

**`GET /v1/models/{model}`**

获取单个模型信息，需 Path 参数 `model`。

**响应示例：**
```json
{
  "object": "list",
  "data": [
    { "id": "MinerU2.5-Pro-2605-1.2B", "object": "model", "created": 1719000000, "owned_by": "mineru" }
  ]
}
```

### 3.3 档位列表

**`GET /v1/tiers`**

无需认证，返回服务器支持的解析档位及其当前模型。

**响应示例：**
```json
{
  "object": "list",
  "data": [
    { "id": "flash",  "description": "Fast local text extraction.", "current_model": "flash" },
    { "id": "basic",  "description": "Basic parsing with local lightweight models.", "current_model": "hybrid-basic" },
    { "id": "standard", "description": "Standard parsing for most documents.", "current_model": "MinerU2.5-Pro-2605-1.2B" },
    { "id": "advanced", "description": "Advanced parsing for difficult documents.", "current_model": "MinerU2.5-Pro-2605-1.2B" }
  ]
}
```

**四个档位的含义：**

| 档位 (Tier) | 说明 | 适用场景 |
|--------------|------|----------|
| `flash` | 快速本地文本提取 | 预览、索引、Office/EPUB/HTML |
| `basic` | 基础 OCR + 轻量模型 | 简单文档、扫描件 |
| `standard` | 标准解析（VLM 辅助） | 大多数文档 |
| `advanced` | 高级解析（更高 VLM 算力） | 复杂排版、表格、公式 |

服务启动 `--tier` 与可接受的请求 tier 关系：

| 服务 tier | 接受的请求 tier |
|-----------|----------------|
| `flash` | flash |
| `basic` | flash, basic |
| `standard` | flash, basic, standard, advanced |

### 3.4 上传端点

#### 创建上传会话

**`POST /v1/uploads`**

| 参数 | 类型 | 必需 | 说明 |
|------|------|------|------|
| `filename` | string | 是 | 文件名 |
| `bytes` | int | 是 | 文件大小（字节） |
| `mime_type` | string | 是 | MIME 类型 |
| `purpose` | string | 否 | 用途：`parse`（默认）、`input_image` |
| `sha256sum` | string | 否 | 文件 SHA-256（用于去重）|
| `expires_after` | object | 否 | 过期配置：`{anchor: "created_at", seconds: 3600~2592000}` |

**响应：**
- 若提供 `sha256sum` 且 blob 已存在 → 直接返回 `completed` 状态，带文件引用
- 否则返回 `pending` 状态，包含 `upload_url` 和 `upload_headers`，客户端需 PUT 到该 URL

#### 上传文件内容

**`PUT /v1/uploads/{upload_id}/content`**

以 `application/octet-stream` 上传文件原始字节。

#### 完成上传

**`POST /v1/uploads/{upload_id}/complete`**

| 参数 | 类型 | 必需 | 说明 |
|------|------|------|------|
| `sha256sum` | string | 否 | 可选，与上传数据校验一致 |

#### 取消上传

**`POST /v1/uploads/{upload_id}/cancel`**

### 3.5 文件端点

#### 列出文件

**`GET /v1/files`**

| 查询参数 | 类型 | 默认 | 说明 |
|----------|------|------|------|
| `after` | string | null | 分页游标（上一页 last_id） |
| `limit` | int | 100 | 每页数量（1~1000） |
| `order` | string | desc | 排序：`asc` / `desc` |
| `purpose` | string | null | 过滤用途：`parse` / `parse_output` / `input_image` |

#### 获取文件

**`GET /v1/files/{file_id}`**

返回文件元数据。

#### 下载文件内容

**`GET /v1/files/{file_id}/content`**

仅可下载 `purpose=parse_output` 的文件（解析输出产物），源文件不可下载。

#### 删除文件

**`DELETE /v1/files/{file_id}`**

### 3.6 解析任务端点

#### 创建解析任务（核心端点）

**`POST /v1/parse/jobs`**

| 参数 | 类型 | 必需 | 默认 | 说明 |
|------|------|------|------|------|
| `files` | array | 是 | — | 待解析文件列表（1~100 个） |
| `tier` | string | 否 | 服务默认 | 解析档位：`flash`/`basic`/`standard`/`advanced` |
| `ocr_mode` | string | 否 | `auto` | OCR 模式：`auto`/`txt`/`ocr` |
| `output_formats` | array | 否 | `["markdown"]` | 输出格式列表 |
| `callback` | object | 否 | null | Webhook 回调配置（本地服务不支持） |

**`files[].source` 支持四种来源：**

| 来源类型 | type 值 | 参数 | 说明 |
|----------|---------|------|------|
| 已上传文件 | `file_id` | `file_id` | 引用已上传的文件 |
| URL 下载 | `url` | `url` | 从 URL 下载 |
| 内联数据 | `inline` | `name`, `data` | Base64 编码内联数据（≤1MB） |
| 本地路径 | `local` | `path` | 服务器文件系统路径（需 `--allow-local-source`） |

**`files[].page_range` 可选参数：**

指定要解析的页码范围（仅 PDF 支持），格式示例：
- `"1-5,8"` — 第 1~5 页和第 8 页
- `"r3-r1"` — 倒序第 3 到第 1 页
- `"all"` — 全部页面

**`output_formats` 可选值：**

| 输出格式 | 默认支持 | 需 API Key | 说明 |
|----------|----------|------------|------|
| `markdown` | 是 | 否 | Markdown 文本 |
| `middle_json` | 是 | 否 | 中间 JSON（结构化文档模型） |
| `structured_content` | 是 | 否 | 结构化内容 |
| `zip` | 是 | 否 | 自包含 ZIP 包（含图片） |
| `html` | 是 | 是 | HTML 渲染输出 |
| `latex` | 是 | 是 | LaTeX 渲染输出 |
| `docx` | 是 | 是 | DOCX 渲染输出 |

**响应：** `202 Accepted`，返回 `JobAsyncResponse`，包含 `job_id` 和初始状态 `queued`。

#### 查询任务状态

**`GET /v1/parse/jobs/{job_id}`**

返回 `JobAsyncResponse`，包含：

| 字段 | 说明 |
|------|------|
| `job_id` | 任务 ID |
| `status` | 状态：`queued`/`running`/`completed`/`partial`/`failed`/`canceled` |
| `created_at` | 创建时间（ISO-8601 UTC） |
| `started_at` | 开始时间 |
| `finished_at` | 完成时间 |
| `tier` | 使用的解析档位 |
| `output_formats` | 输出格式 |
| `access_level` | 访问级别：`anonymous`/`registered` |
| `progress` | 进度：`{completed, failed, total}` |
| `files[]` | 每个文件的结果 |
| `files[].status` | 文件状态：`queued`/`running`/`completed`/`failed` |
| `files[].output_files` | 输出文件引用（含 file_id） |
| `files[].parse` | 解析信息（耗时、版本号） |
| `files[].error` | 错误详情（失败时） |
| `links.self` | 任务自引用链接 |
| `links.cancel` | 取消操作链接 |

#### 列出任务

**`GET /v1/parse/jobs`**

| 查询参数 | 类型 | 默认 | 说明 |
|----------|------|------|------|
| `status` | string | null | 按状态过滤（逗号分隔） |
| `limit` | int | 20 | 每页数量（1~100） |
| `after` | string | null | 分页游标 |
| `order` | string | desc | 排序：`asc`/`desc` |
| `created_after` | string | null | 创建时间下限（ISO-8601） |

#### 取消任务

**`DELETE /v1/parse/jobs/{job_id}`**

只能取消 `queued` 或 `running` 状态的任务。

### 3.7 用量端点

**`GET /v1/usage`**

| 字段 | 说明 |
|------|------|
| `access_level` | 访问级别 |
| `billing_period.start` | 计费周期开始（服务启动时间） |
| `current.pages_processed` | 已处理的页数 |
| `current.files_processed` | 已处理的文件数 |
| `current.jobs_created` | 已创建的任务数 |
| `limits.max_pages_per_file` | 每文件最大页数（1000） |
| `limits.max_file_size_bytes` | 最大文件大小（200MB） |
| `limits.max_files_per_job` | 每任务最大文件数（100） |
| `limits.max_concurrent_jobs` | 最大并发任务数（由 `--concurrency` 设置） |

---

## 4. 认证与安全

### 4.1 API Key 认证

- 设置 `--api-key` 后，除 `/v1/health`、`/v1/models`（及 `/v1/models/{model}`）、`/v1/tiers` 和 OpenAPI 文档端点外，所有请求需在 Header 中传入：
  ```
  Authorization: Bearer <api-key>
  ```
- 未设置 `--api-key` 时，所有端点开放访问。

### 4.2 访问级别

- `anonymous`：服务器未配置 API Key 时的访问级别
- `registered`：服务器配置了 API Key 且已通过认证时的访问级别
- 部分高级输出格式（`html`、`latex`、`docx`）要求 `registered` 级别

---

## 5. 支持的输入文件类型

| 类别 | 扩展名 | 支持的 tier |
|------|--------|-------------|
| PDF | `.pdf` | flash / basic / standard / advanced |
| 图片 | `.png, .jpg, .jpeg, .webp, .gif, .bmp, .tiff, .jp2` | flash / basic / standard / advanced |
| Word | `.doc, .docx` | flash only |
| PowerPoint | `.ppt, .pptx` | flash only |
| Excel | `.xls, .xlsx` | flash only |
| RTF | `.rtf` | flash only |
| OpenDocument | `.odt, .ods, .odp` | flash only |
| EPUB | `.epub` | flash only |
| OFD | `.ofd` | flash only |
| HTML | `.html, .htm, .shtml` | flash only |
| MHTML | `.mhtml, .mht` | flash only |
| CSV/TSV | `.csv, .tsv` | flash only |

---

## 6. 典型工作流

```
1. (可选) 创建上传 → 上传文件 → 完成上传
   POST /v1/uploads
   PUT /v1/uploads/{upload_id}/content
   POST /v1/uploads/{upload_id}/complete

2. (简化) 直接在创建任务时使用 URL 或 inline 来源
   POST /v1/parse/jobs
   {
     "files": [{"source": {"type": "url", "url": "https://..."}, "page_range": "1-10"}],
     "tier": "standard",
     "output_formats": ["markdown", "middle_json"]
   }

3. 轮询任务状态
   GET /v1/parse/jobs/{job_id}

4. 下载输出
   GET /v1/files/{file_id}/content
```

---

## 7. 错误处理

### 错误响应格式

所有错误返回统一的 JSON 结构：

```json
{
  "error": {
    "type": "invalid_request_error",
    "code": "invalid_request",
    "message": "描述信息",
    "param": "files.0.source"
  }
}
```

### 常见错误码

| HTTP 状态码 | 说明 |
|------------|------|
| 400 | 请求参数错误（如不支持的来源类型、文件格式） |
| 401 | 缺少或无效 API Key |
| 403 | 权限不足（如匿名用户请求高级输出格式） |
| 404 | 资源不存在（upload / file / job） |
| 409 | 资源冲突（上传已完成、任务已终止） |
| 413 | 上传大小不匹配或超出限制 |
| 429 | 请求频率过高 |
| 503 | 服务不可用（模型未就绪、tier 不可用、服务器关闭中） |

---

## 8. 架构特点

- **异步非阻塞**：所有解析任务在后台协程中执行，HTTP 请求立即返回 `202 Accepted`
- **信号量并发控制**：由 `--concurrency` 指定最大并发数，默认 1
- **内容寻址存储**：文件以 SHA-256 为键存储，支持自动去重
- **内存存储**：Upload/File/Job 元数据全部在内存中，进程重启即丢失
- **无 Webhook 支持**：本地版不支持回调，需通过轮询获取结果
- **模型预加载**：可选 `--preload-models` 在启动时预热 VLM 和 Hybrid 模型

---

## 9. 与 OKB-Assist 的关联

OKB-Assist 当前通过 MinerU 解析 PDF 时使用的并非此 v1 API，而是直接调用 MinerU 的 Python SDK（`parse_async`）进行解析。本文的调研内容可作为未来升级到 API 模式或集成更多 MinerU 功能的参考。

关键差异：
- MinerU v1 API 支持 4 种文件来源（file_id / url / inline / local），OKB-Assist 目前仅通过本地文件路径调用
- MinerU v1 API 支持 7 种输出格式，OKB-Assist 主要使用 markdown
- MinerU v1 API 支持 4 个解析档位，OKB-Assist 默认使用 standard

---

*本报告基于 MinerU 仓库 master 分支 `mineru/parser/api_server.py` 源码分析生成。*