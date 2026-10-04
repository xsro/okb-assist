# MinerU 官方云 API 文档

> 编写时间：2026-09-29  
> 数据来源：mineru.net 官方云 API 实测 + API 管理文档  

---

## 概述

MinerU 官方云提供**三种 API 接口**，覆盖不同使用场景。OKB-Assist 通过 `config.json` 中 `mineru[].type` 字段选择使用哪种接口：

| type 值 | API 版本 | 名称 | 认证 | 适用场景 |
|---------|---------|------|------|---------|
| `local` | V1（自部署） | 自部署 MinerU 服务 | 可选 API Key | 本地/内网部署 |
| `official` | **V4 精准解析** | 精准解析 API | Bearer Token | 高精度解析，需 Token |
| `official-lightweight` | **V1 Agent 轻量** | Agent 轻量解析 API | IP 限频（免登录） | AI Agent 工作流 |

各 API 的详细对比见下方对照表，完整 API **文档来源**：[https://mineru.net/apiManage/docs](https://mineru.net/apiManage/docs)

---

## 三种 API 对比

| 对比维度 | 🎯 V4 精准解析 API (`official`) | ⚡ V1 Agent 轻量 API (`official-lightweight`) | 🏠 自部署 V1 API (`local`) |
|---------|------|------|------|
| **需要 Token** | ✅ 是 | ❌ 否（IP 限频） | 可选 `--api-key` |
| **接口地址** | `/api/v4/extract/task` 或 `/api/v4/file-urls/batch` | `/api/v1/agent/parse/url` 或 `/api/v1/agent/parse/file` | `/v1/uploads` → `/v1/parse/jobs` |
| **模型版本** | pipeline/vlm/MinerU-HTML | 固定 pipeline 轻量模型 | 由服务端 tier 决定 |
| **文件大小** | ≤ 200MB | ≤ 10MB | 由部署配置决定 |
| **页数限制** | ≤ 200 页 | ≤ 20 页 | 无限制 |
| **批量支持** | ✅ ≤ 200 个 | ❌ 单文件 | ✅ |
| **输出格式** | ZIP（含 Markdown/JSON）可导出 docx/html/latex | 仅 Markdown（CDN 链接） | Markdown + ZIP |
| **调用方式** | 异步（提交 → 轮询） | 异步（提交 → 轮询） | 异步（提交 → 轮询） |
| **文件上传** | 预签名 OSS URL 直传 | multipart 上传 | 预签名 URL 或同源上传 |
| **结果查询** | `GET /api/v4/extract/task/batch/{batch_id}` | `GET /api/v1/agent/parse/{task_id}` | `GET /v1/parse/jobs/{job_id}` |
| **结果下载** | ZIP 包（`full_zip_url`） | Markdown 文本（CDN URL） | 通过 file_id 下载 |
| **图片资源** | ZIP 内 `images/` 目录 | 无独立图片 | ZIP 内 `images/` 目录 |

---

## API 端点总览

### 🎯 V4 精准解析 API（type=official）

| 方法 | 路径 | 说明 |
|------|------|------|
| POST | `/api/v4/file-urls/batch` | 获取预签名上传 URL（上传后系统自动提交解析）|
| PUT | `{预签名URL}` | 上传文件到 OSS |
| GET | `/api/v4/extract/task/batch/{batch_id}` | 查询 batch 中所有任务状态 |
| GET | `/api/v4/extract/task/{task_id}` | 查询单个任务状态 |
| POST | `/api/v4/extract/task` | 按 URL 提交单文件解析 |
| POST | `/api/v4/extract/task/batch` | 按 URL 批量提交解析 |

> base URL: `https://mineru.net`，需 Bearer Token 认证

### ⚡ V1 Agent 轻量解析 API（type=official-lightweight）

| 方法 | 路径 | 说明 |
|------|------|------|
| POST | `/api/v1/agent/parse/url` | 按 URL 提交解析 |
| POST | `/api/v1/agent/parse/file` | multipart 上传文件并解析 |
| GET | `/api/v1/agent/parse/{task_id}` | 查询任务状态与结果 |

> base URL: `https://mineru.net`，无需 Token，IP 限频

### 🏠 自部署 V1 API（type=local）

| 方法 | 路径 | 说明 |
|------|------|------|
| GET | `/v1/health` | 健康检查 |
| POST | `/v1/uploads` | 创建上传会话 |
| PUT | `{upload_url}` | 上传文件内容 |
| POST | `/v1/uploads/{id}/complete` | 完成上传 |
| POST | `/v1/parse/jobs` | 创建解析任务 |
| GET | `/v1/parse/jobs/{id}` | 查询任务状态 |
| GET | `/v1/files/{id}/content` | 下载结果文件 |

> base URL: `http://host:port`，可选 API Key 认证

> **注意**：官方云 API 端点前缀统一为 `https://mineru.net`。自部署 API 不带 `/api` 前缀。

---

## 详细端点说明

### 🎯 V4 精准解析 API 工作流

V4 精准解析 API 用于需要高精度解析的场景，支持表格、公式、复杂排版。

#### 文件上传 + 自动解析

**Step 1：获取预签名上传 URL**

```bash
curl -s -X POST "https://mineru.net/api/v4/file-urls/batch" \
  -H "Authorization: Bearer sk-xxxxxxxx" \
  -H "Content-Type: application/json" \
  -d '{
    "files": [{"name": "document.pdf", "data_id": "my_doc_001"}],
    "model_version": "vlm"
  }'
```

请求参数：

| 参数 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `files[].name` | string | 是 | 文件名 |
| `files[].data_id` | string | 否 | 自定义数据 ID（用于后续追踪）|
| `model_version` | string | 否 | `pipeline`/`vlm`(推荐)/`MinerU-HTML`，默认 `pipeline` |
| `is_ocr` | bool | 否 | 启用 OCR，默认 false |
| `enable_formula` | bool | 否 | 启用公式识别，默认 true |
| `enable_table` | bool | 否 | 启用表格识别，默认 true |
| `language` | string | 否 | 文档语言，默认 `ch` |
| `page_ranges` | string | 否 | 页码范围，如 `"2,4-6"` |
| `extra_formats` | array | 否 | 额外导出格式，如 `["docx","html"]` |

**响应：**
```json
{
  "code": 0,
  "data": {
    "batch_id": "batch_xxxxx",
    "file_urls": ["https://mineru.oss-cn-shanghai.aliyuncs.com/..."]
  },
  "msg": "ok"
}
```

**Step 2：上传文件到预签名 URL**

```bash
curl -s -X PUT "{file_urls[0]}" --data-binary "@document.pdf"
```

> 上传后**系统自动提交解析任务**，无需额外调用提交接口。

**Step 3：查询 batch 任务状态**

```bash
curl -s "https://mineru.net/api/v4/extract/task/batch/{batch_id}" \
  -H "Authorization: Bearer sk-xxxxxxxx"
```

**响应（处理中）：**
```json
{
  "code": 0,
  "data": {
    "tasks": [{
      "task_id": "47726b6e-46ca-4bb9-******",
      "state": "running",
      "extract_progress": {
        "extracted_pages": 1,
        "total_pages": 10,
        "start_time": "2025-01-20 11:43:20"
      }
    }]
  },
  "msg": "ok"
}
```

**响应（完成）：**
```json
{
  "code": 0,
  "data": {
    "tasks": [{
      "task_id": "47726b6e-46ca-4bb9-******",
      "state": "done",
      "full_zip_url": "https://cdn-mineru.openxlab.org.cn/pdf/xxxx.zip"
    }]
  },
  "msg": "ok"
}
```

也可通过单个 task_id 查询：

```bash
curl -s "https://mineru.net/api/v4/extract/task/{task_id}" \
  -H "Authorization: Bearer sk-xxxxxxxx"
```

**状态流转：** `pending` → `running`/`extract` → `done`/`failed`

**Step 4：下载结果 ZIP**

```bash
curl -sL "{full_zip_url}" -o result.zip
```

ZIP 内容结构：
```
result.zip
├── full.md                              # Markdown 文件（图片引用 images/xxx.jpg）
├── images/
│   ├── f23f0f611d925b15d978ccbcec539f4ede4d7fbca81810df38f11e95a0cf3799.jpg
│   ├── 40d67da4ca83d789a83bf55f0f1fc3f0e615c8728e5ea53b20f38494b2476f37.jpg
│   └── ...
├── layout.json                          # 布局数据
├── {uuid}_origin.pdf                    # 原始 PDF 副本
├── {uuid}_model.json                    # 模型元数据
├── {uuid}_content_list.json             # 内容列表
└── {uuid}_content_list_v2.json          # 内容列表 v2
```

#### 按 URL 提交（文件已在云端）

如果文件已有可公开访问的 URL，可直接提交：

```bash
curl -s -X POST "https://mineru.net/api/v4/extract/task" \
  -H "Authorization: Bearer sk-xxxxxxxx" \
  -H "Content-Type: application/json" \
  -d '{
    "url": "https://example.com/document.pdf",
    "model_version": "vlm"
  }'
```

**响应：**
```json
{
  "code": 0,
  "data": {"task_id": "a90e6ab6-44f3-4554-b4***"},
  "msg": "ok"
}
```

然后通过 `GET /api/v4/extract/task/{task_id}` 轮询结果。

---

### ⚡ V1 Agent 轻量解析 API 工作流

轻量 API 无需 Token，适合 AI Agent 场景。文件 ≤ 10MB，≤ 20 页，输出纯 Markdown。

#### 上传文件解析

```bash
curl -s -X POST "https://mineru.net/api/v1/agent/parse/file" \
  -F "file=@document.pdf"
```

**响应：**
```json
{
  "code": 0,
  "data": {"task_id": "agent_task_xxxxx"},
  "msg": "ok"
}
```

#### 按 URL 提交

```bash
curl -s -X POST "https://mineru.net/api/v1/agent/parse/url" \
  -H "Content-Type: application/json" \
  -d '{"url": "https://example.com/document.pdf"}'
```

#### 查询结果

```bash
curl -s "https://mineru.net/api/v1/agent/parse/{task_id}"
```

**响应（完成）：**
```json
{
  "code": 0,
  "data": {
    "task_id": "agent_task_xxxxx",
    "state": "done",
    "url": "https://cdn-mineru.openxlab.org.cn/result/xxx.md"
  },
  "msg": "ok"
}
```

#### 下载 Markdown

```bash
curl -sL "{url}" -o result.md
```

> 轻量 API 输出为纯 Markdown，**不包含独立图片文件**。

---

### 🏠 自部署 V1 API 工作流

参见 `document/mineru4/mineru-api-server-report.md` 获取完整工作流说明。

简要流程：

1. `POST /v1/uploads` — 创建上传会话
2. `PUT /v1/uploads/{id}/content` — 上传文件内容
3. `POST /v1/uploads/{id}/complete` — 完成上传获得 file_id
4. `POST /v1/parse/jobs` — 创建解析任务，获得 job_id
5. `GET /v1/parse/jobs/{job_id}` — 轮询至 `completed`
6. `GET /v1/files/{file_id}/content` — 下载输出文件

```bash
# 健康检查
curl -s "http://host:port/v1/health"
```

注意：自部署 API 返回的输出文件内容可能为 gzip 压缩，需解压后使用。

---

## 完整工作流示例

### 🎯 V4 精准解析 API 示例

```bash
#!/bin/bash
# V4 精准解析 API：文件上传 → 自动解析 → 查询 → 下载 ZIP

API_URL="https://mineru.net/api/v4"
API_KEY="sk-xxxxxxxx"
PDF_FILE="$1"
FILENAME=$(basename "$PDF_FILE")

# 1. 获取预签名上传 URL
BATCH_RESP=$(curl -s -X POST "${API_URL}/file-urls/batch" \
  -H "Authorization: Bearer ${API_KEY}" \
  -H "Content-Type: application/json" \
  -d "{\"files\":[{\"name\":\"${FILENAME}\"}],\"model_version\":\"vlm\"}")

BATCH_ID=$(echo "$BATCH_RESP" | python3 -c "import sys,json;print(json.load(sys.stdin)['data']['batch_id'])")
UPLOAD_URL=$(echo "$BATCH_RESP" | python3 -c "import sys,json;print(json.load(sys.stdin)['data']['file_urls'][0])")
echo "Batch ID: $BATCH_ID"

# 2. 上传文件
echo "上传文件中..."
curl -s -X PUT "$UPLOAD_URL" --data-binary "@$PDF_FILE" -o /dev/null -w "HTTP %{http_code}\n"

# 3. 系统自动解析，轮询结果
echo "等待解析结果..."
for i in $(seq 1 60); do
  STATUS_RESP=$(curl -s "${API_URL}/extract/task/batch/${BATCH_ID}" \
    -H "Authorization: Bearer ${API_KEY}")
  STATE=$(echo "$STATUS_RESP" | python3 -c "
import sys,json
d=json.load(sys.stdin)
tasks=d.get('data',{}).get('tasks',[])
if tasks: print(tasks[0].get('state','unknown'))
else: print('pending')
")
  echo "[$i] $STATE"
  if [ "$STATE" = "done" ]; then
    ZIP_URL=$(echo "$STATUS_RESP" | python3 -c "
import sys,json
d=json.load(sys.stdin)
print(d['data']['tasks'][0].get('full_zip_url',''))
")
    echo "下载结果..."
    curl -sL "$ZIP_URL" -o "${FILENAME%.pdf}.zip"
    unzip -o "${FILENAME%.pdf}.zip" -d "${FILENAME%.pdf}_result/"
    echo "完成！结果保存到 ${FILENAME%.pdf}_result/"
    exit 0
  fi
  [ "$STATE" = "failed" ] && echo "解析失败" && exit 1
  sleep 5
done
```

### ⚡ V1 Agent 轻量 API 示例

```bash
#!/bin/bash
# Agent 轻量 API：上传文件 → 轮询 → 下载 Markdown

API_URL="https://mineru.net/api/v1/agent"
PDF_FILE="$1"
FILENAME=$(basename "$PDF_FILE")

# 1. 上传并提交解析
TASK_RESP=$(curl -s -X POST "${API_URL}/parse/file" \
  -F "file=@${PDF_FILE}")
TASK_ID=$(echo "$TASK_RESP" | python3 -c "import sys,json;print(json.load(sys.stdin)['data']['task_id'])")
echo "Task ID: $TASK_ID"

# 2. 轮询结果
for i in $(seq 1 30); do
  STATUS_RESP=$(curl -s "${API_URL}/${TASK_ID}")
  STATE=$(echo "$STATUS_RESP" | python3 -c "import sys,json;print(json.load(sys.stdin)['data']['state'])")
  echo "[$i] $STATE"
  if [ "$STATE" = "done" ]; then
    MD_URL=$(echo "$STATUS_RESP" | python3 -c "import sys,json;print(json.load(sys.stdin)['data']['url'])")
    curl -sL "$MD_URL" -o "${FILENAME%.pdf}.md"
    echo "完成！Markdown 保存到 ${FILENAME%.pdf}.md"
    exit 0
  fi
  [ "$STATE" = "failed" ] && echo "解析失败" && exit 1
  sleep 3
done
```

### 🏠 自部署 V1 API 示例

参见 `document/mineru4/parse-api-test.md` 获取完整示例脚本。

---

## 图片处理说明

### V4/V1 标准 API 的 ZIP 包图片

V4 精准解析 API 和 V1 标准 API 的结果 ZIP 包结构相同：

```
result.zip
├── full.md                   # Markdown（图片引用 images/hash.jpg）
├── images/                   # 独立图片文件（SHA256 哈希命名）
└── layout.json / *_model.json / *_content_list.json  # 结构化数据
```

### 轻量 API 的图片处理

Agent 轻量 API 输出**纯 Markdown**，无独立图片文件。如果 Markdown 中包含图片，它们以 `data:image/...;base64,...` 格式内嵌或引用为 CDN URL。

### 推荐的图片存储策略

```python
import zipfile, os

with zipfile.ZipFile("output.zip") as z:
    # 1. 提取 full.md
    z.extract("full.md", target_dir)
    
    # 2. 提取所有图片
    for name in z.namelist():
        if name.startswith("images/") and not name.endswith("/"):
            z.extract(name, target_dir)
    
    # 3. 打包为 images.zip（供前端按名引用）
    images = [n for n in z.namelist() if n.startswith("images/")]
    if images:
        with zipfile.ZipFile(os.path.join(target_dir, "images.zip"), "w") as imgz:
            for name in images:
                imgz.writestr(os.path.basename(name), z.read(name))
```

### 与 OKB-Assist 的集成

OKB-Assist `backend-rs/src/services/mineru.rs` 已完整支持三种 API：

| 能力 | V1 自部署 | V4 精准解析 | V1 轻量 |
|------|---------|-----------|--------|
| base URL 自动适配 | — | ✅ `/api` 前缀 | ✅ `/api` 前缀 |
| purpose 字段 | — | ✅ 自动添加 | N/A |
| 预签名 URL 处理 | ✅ | ✅ | N/A（multipart）|
| 302 重定向处理 | ✅ | N/A（直接下载）| N/A |
| ZIP 提取+图片打包 | ✅ | ✅ | N/A |
| multipart 上传 | N/A | N/A | ✅ |

配置示例（三选一，由 `enabled` 控制使用哪个）：

```json
{
  "mineru": [
    {
      "name": "官方云 API 精准解析",
      "type": "official",
      "url": "https://mineru.net",
      "key": "sk-xxxxxxxx",
      "tier": "vlm",
      "enabled": true
    },
    {
      "name": "官方云 API Agent 轻量",
      "type": "official-lightweight",
      "url": "https://mineru.net",
      "tier": "standard",
      "enabled": false
    },
    {
      "name": "本地服务",
      "type": "local",
      "url": "http://192.168.1.185:8002",
      "key": "keyxxx",
      "tier": "standard",
      "enabled": false
    }
  ]
}
```

> 注意：V1 Agent 轻量 API 无需 `key` 字段，V4 精准解析 API 的 `tier` 可选 `pipeline`/`vlm`/`MinerU-HTML`。

---

## 错误处理

### V4/V1 标准 API 错误码

| HTTP 状态码 | 说明 |
|------------|------|
| 400 | 请求参数错误（如缺少 `purpose` 字段）|
| 401 | 无效或缺少 API Key |
| 404 | 资源不存在 |
| 409 | 资源冲突（上传已完成等） |
| 413 | 文件超出大小限制 |
| 503 | 服务不可用 |

V4 API 的 JSON 错误格式（`code` 非零）：

```json
{
  "code": 400,
  "msg": "field \"purpose\" is not set",
  "trace_id": "c876cd60b202f2396de1f9e39a1b0172"
}
```

### 轻量 API 错误码

| code | msg | 说明 |
|------|-----|------|
| 400 | invalid parameters | 请求参数错误 |
| 401 | unauthorized | 请求频率过高被限频 |
| 404 | task not found | 任务 ID 不存在 |
| 500 | internal error | 服务端错误 |

---

## 限制说明

| 限制项 | V4 精准解析 | V1 轻量 | 自部署 |
|--------|------------|--------|--------|
| 文件大小上限 | 200MB | 10MB | 由部署配置 |
| 页数上限 | 200 页 | 20 页 | 无限制 |
| 批量提交 | ≤ 200 文件 | 单文件 | 取决于部署 |
| 文件来源 | URL + 预签名上传 | URL + multipart | file_id/url/inline/local |
| Token 认证 | ✅ 必需 | ❌ 无需（IP 限频） | 可选 |
| 文件有效期 | 24 小时 | 24 小时 | 取决于部署 |
| 每日免费额度 | 1000 页高优先级 | 无限制（IP 限频） | 无限制 |

---

## 参考链接

- [MinerU API 管理文档](https://mineru.net/apiManage/docs)（完整 API 说明）
- [MinerU V1 HTTP API 官方文档](https://opendatalab.github.io/MinerU/usage/http_api/)（自部署）
- [MinerU GitHub 仓库](https://github.com/opendatalab/MinerU)
- [MinerU 官网](https://mineru.net)
- OKB-Assist: `document/mineru4/mineru-api-server-report.md`（自部署 API 详细调研）
- OKB-Assist: `document/mineru4/parse-api-test.md`（自部署 API 测试报告）
- OKB-Assist: `backend-rs/src/services/mineru.rs`（完整实现）