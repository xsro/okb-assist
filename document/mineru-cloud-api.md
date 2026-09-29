# MinerU 官方云 API 文档

> 编写时间：2026-09-29  
> 数据来源：mineru.net 官方云 API 实测  
> API 版本：1.0.0（自部署 V1 API 版本为 4.x.x）

---

## 概述

MinerU 官方云 API 提供与自部署 V1 API **基本一致**的接口（创建上传 → 上传文件 → 完成上传 → 提交任务 → 轮询 → 下载结果），但在端点路径、请求参数、认证方式和输出格式等方面存在若干**关键差异**。

---

## 与自部署 V1 API 的差异对照

| 项目 | 官方云 API | 自部署 V1 API |
|------|-----------|---------------|
| **API 基础路径** | `https://mineru.net/api/v1` | `http://host:port/v1` |
| **API 版本号** | `1.0.0` | `4.x.x` |
| **认证方式** | Bearer Token（必填） | 可选 `--api-key` |
| **上传 URL** | 预签名 OSS URL（不同源） | 同源相对路径 |
| **purpose 字段** | **必需**（`"parse"`） | 可选，默认为 `"parse"` |
| **支持的 tier** | 仅 `standard` | `flash`/`basic`/`standard`/`advanced` |
| **文件来源** | `file_id`、`url` | `file_id`、`url`、`inline`、`local` |
| **输出格式** | 7 种（含 `docx`/`latex`/`html`） | 4 种（不含 `docx`/`latex`/`html`）|
| **Webhook** | 支持 | 不支持 |
| **文件下载** | HTTP 302 跳转到阿里云 CDN | 直接返回内容 |
| **重定向处理** | 需跟踪 302 到 CDN 链接 | 同源重定向 |

---

## API 端点总览

| 方法 | 路径 | 说明 |
|------|------|------|
| GET | `/api/v1/health` | 健康检查 |
| GET | `/api/v1/models` | 列出可用 VLM 模型 |
| GET | `/api/v1/tiers` | 列出可用解析档位 |
| POST | `/api/v1/uploads` | 创建上传会话 |
| PUT | `{upload_url}` | 上传文件内容 |
| POST | `/api/v1/uploads/{id}/complete` | 完成上传 |
| POST | `/api/v1/parse/jobs` | 创建解析任务 |
| GET | `/api/v1/parse/jobs/{id}` | 查询任务状态 |
| GET | `/api/v1/files/{id}/content` | 下载结果文件 |
| GET | `/api/v1/usage` | 查询用量 |

> **注意**：所有 API 端点前缀为 `https://mineru.net`（不带 `/api/v1` 前缀的路径无效）。

---

## 详细端点说明

### 1. 健康检查

```bash
curl -s "https://mineru.net/api/v1/health" \
  -H "Authorization: Bearer sk-xxxxxxxx"
```

**响应示例：**
```json
{
  "status": "ok",
  "version": "1.0.0",
  "features": {
    "webhook": true,
    "output_formats": ["markdown","middle_json","content_list","structured_content","zip","docx","latex","html"],
    "sources": ["file_id", "url"]
  }
}
```

### 2. 模型与档位

仅支持 `MinerU2.5-Pro-2605-1.2B`（VLM）模型，仅 `standard` 档位：

```bash
curl -s "https://mineru.net/api/v1/tiers" -H "Authorization: Bearer sk-xxxxxxxx"
```

```json
{
  "object": "list",
  "data": [{
    "id": "standard",
    "object": "tier",
    "description": "VLM-based high-accuracy parsing tier.",
    "current_model": "MinerU2.5-Pro-2605-1.2B"
  }]
}
```

### 3. 创建上传

```bash
curl -s -X POST "https://mineru.net/api/v1/uploads" \
  -H "Authorization: Bearer sk-xxxxxxxx" \
  -H "Content-Type: application/json" \
  -d '{
    "filename": "document.pdf",
    "bytes": 1845615,
    "mime_type": "application/pdf",
    "purpose": "parse",
    "sha256sum": "f0e66ac106d66471a758f41f..."
  }'
```

**关键差异：** `purpose` 字段**必需**（自部署版可选）。

**响应（新文件）：**
```json
{
  "id": "upload_xxxxx",
  "status": "pending",
  "upload_url": "https://mineru.oss-cn-shanghai.aliyuncs.com/...",
  "upload_method": "PUT",
  "upload_headers": {"Content-Type": "application/pdf"}
}
```

**响应（文件已存在——秒传命中）：**
上传时提供 `sha256sum`，若文件已存在则直接返回 `completed` 状态，内含 `file.id`：

```json
{
  "id": "upload_xxxxx",
  "status": "completed",
  "file": {"id": "file-xxxxx", "bytes": 1845615}
}
```

此时**跳过**上传和完成步骤，直接使用 `file.id` 提交任务。

### 4. 上传文件

预签名 URL 自带鉴权，**不要**附加 API Key：

```bash
curl -s -X PUT "{upload_url}" \
  -H "Content-Type: application/pdf" \
  --data-binary "@document.pdf"
```

### 5. 完成上传

```bash
curl -s -X POST "https://mineru.net/api/v1/uploads/{upload_id}/complete" \
  -H "Authorization: Bearer sk-xxxxxxxx" \
  -H "Content-Type: application/json" \
  -d '{}'
```

### 6. 提交解析任务

```bash
curl -s -X POST "https://mineru.net/api/v1/parse/jobs" \
  -H "Authorization: Bearer sk-xxxxxxxx" \
  -H "Content-Type: application/json" \
  -d '{
    "files": [{"source": {"type": "file_id", "file_id": "file-xxxxx"}}],
    "tier": "standard",
    "output_formats": ["markdown", "zip"]
  }'
```

**注意事项：**
- `tier` 仅支持 `standard`
- 始终请求 `zip` 格式以获取图片（markdown 中引用本地相对路径）

### 7. 轮询任务

```bash
curl -s "https://mineru.net/api/v1/parse/jobs/{job_id}" \
  -H "Authorization: Bearer sk-xxxxxxxx"
```

**状态流转：** `queued` → `running` → `completed`/`failed`/`canceled`

### 8. 下载结果

文件下载会返回 **302 重定向**到阿里云 CDN，必须跟踪重定向：

```bash
# 下载 markdown
curl -sL "https://mineru.net/api/v1/files/{file_id}/content" \
  -H "Authorization: Bearer sk-xxxxxxxx" \
  -o output.md

# 下载 zip（含图片）
curl -sL "https://mineru.net/api/v1/files/{zip_file_id}/content" \
  -H "Authorization: Bearer sk-xxxxxxxx" \
  -o output.zip
```

ZIP 包内容结构：

```
output.zip
├── full.md                              # Markdown 文件
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

**关键差异：**
- 官方云返回 **HTTP 302** 到 CDN，而非直接返回内容
- 必须使用 `-L`（跟随重定向）或手动跟踪 302
- 重定向到 CDN 后**无需** API Key（预签名 URL）
- ZIP 中图片以 **SHA256 哈希值**命名（如 `images/f23f0f61...jpg`）
- markdown 中图片引用为 `images/{hash}.{ext}` 相对路径

---

## 完整工作流示例

```bash
#!/bin/bash
# MinerU 官方云 API 完整示例

API_URL="https://mineru.net/api/v1"
API_KEY="sk-xxxxxxxx"
PDF_FILE="$1"
FILENAME=$(basename "$PDF_FILE")
BYTES=$(stat -c%s "$PDF_FILE")
SHA256=$(sha256sum "$PDF_FILE" | awk '{print $1}')

# 1. 创建上传
UPLOAD=$(curl -s -X POST "${API_URL}/uploads" \
  -H "Authorization: Bearer ${API_KEY}" \
  -H "Content-Type: application/json" \
  -d "{\"filename\":\"${FILENAME}\",\"bytes\":${BYTES},\"mime_type\":\"application/pdf\",\"purpose\":\"parse\",\"sha256sum\":\"${SHA256}\"}")

# 检查是否为秒传
STATUS=$(echo "$UPLOAD" | python3 -c "import sys,json;print(json.load(sys.stdin).get('status',''))")
if [ "$STATUS" = "completed" ]; then
  FILE_ID=$(echo "$UPLOAD" | python3 -c "import sys,json;print(json.load(sys.stdin)['file']['id'])")
else
  UPLOAD_ID=$(echo "$UPLOAD" | python3 -c "import sys,json;print(json.load(sys.stdin)['id'])")
  UPLOAD_URL=$(echo "$UPLOAD" | python3 -c "import sys,json;print(json.load(sys.stdin)['upload_url'])")

  # 2. 上传文件
  curl -s -X PUT "$UPLOAD_URL" -H "Content-Type: application/pdf" --data-binary "@$PDF_FILE"

  # 3. 完成上传
  COMPLETE=$(curl -s -X POST "${API_URL}/uploads/${UPLOAD_ID}/complete" \
    -H "Authorization: Bearer ${API_KEY}" -H "Content-Type: application/json" -d '{}')
  FILE_ID=$(echo "$COMPLETE" | python3 -c "import sys,json;print(json.load(sys.stdin)['file']['id'])")
fi

# 4. 提交解析任务 (tier 仅支持 standard)
JOB=$(curl -s -X POST "${API_URL}/parse/jobs" \
  -H "Authorization: Bearer ${API_KEY}" -H "Content-Type: application/json" \
  -d "{\"files\":[{\"source\":{\"type\":\"file_id\",\"file_id\":\"${FILE_ID}\"}}],\"tier\":\"standard\",\"output_formats\":[\"markdown\",\"zip\"]}")
JOB_ID=$(echo "$JOB" | python3 -c "import sys,json;print(json.load(sys.stdin)['job_id'])")

# 5. 轮询
for i in $(seq 1 60); do
  STATUS=$(curl -s "${API_URL}/parse/jobs/${JOB_ID}" -H "Authorization: Bearer ${API_KEY}")
  STATE=$(echo "$STATUS" | python3 -c "import sys,json;print(json.load(sys.stdin)['status'])")
  echo "[$i] $STATE"
  [ "$STATE" = "completed" ] && break
  [ "$STATE" = "failed" ] && echo "Failed!" && exit 1
  sleep 3
done

# 6. 下载结果（必须使用 -L 跟踪 302 重定向）
MD_ID=$(echo "$STATUS" | python3 -c "
import sys,json
r=json.load(sys.stdin)
print(r['files'][0]['output_files']['markdown']['file_id'])
")
ZIP_ID=$(echo "$STATUS" | python3 -c "
import sys,json
r=json.load(sys.stdin)
print(r['files'][0]['output_files']['zip']['file_id'])
")

curl -sL "${API_URL}/files/${MD_ID}/content" \
  -H "Authorization: Bearer ${API_KEY}" -o "${FILENAME%.pdf}.md"
curl -sL "${API_URL}/files/${ZIP_ID}/content" \
  -H "Authorization: Bearer ${API_KEY}" -o "${FILENAME%.pdf}.zip"

echo "Done: ${FILENAME%.pdf}.md + ${FILENAME%.pdf}.zip"
```

---

## 图片处理说明

官方云 API 的图片处理流程如下：

1. **ZIP 包**中的 Markdown 引用 `images/{hash}.{ext}` 相对路径图片
2. Markdown **独立文件**返回的是纯 Markdown（无 data URI）
3. **图片不包含在独立 Markdown 中**，必须从 ZIP 包提取

### 推荐的图片存储策略

```python
# 伪代码：从 ZIP 提取资源
import zipfile, os

with zipfile.ZipFile("output.zip") as z:
    # 1. 提取 full.md 到目标目录
    z.extract("full.md", target_dir)
    
    # 2. 提取所有图片到同一目录
    for name in z.namelist():
        if name.startswith("images/") and not name.endswith("/"):
            z.extract(name, target_dir)  # → target_dir/images/hash.jpg
    
    # 3. 将图片打包为 images.zip
    images = [n for n in z.namelist() if n.startswith("images/")]
    if images:
        with zipfile.ZipFile(os.path.join(target_dir, "images.zip"), "w") as imgz:
            for name in images:
                imgz.writestr(os.path.basename(name), z.read(name))
```

### 与 OKB-Assist 的集成

OKB-Assist 已在 `backend-rs/src/services/mineru.rs` 中实现对官方云 API 的完整支持：

1. 🟢 **Base URL 自动适配**：`https://mineru.net` → 自动添加 `/api` 前缀为 `https://mineru.net/api/v1/...`
2. 🟢 **purpose 字段自动添加**：官方 API 类型自动在创建上传时添加 `"purpose": "parse"`
3. 🟢 **预签名 URL 处理**：不同源上传 URL 不附加 API Key（安全）
4. 🟢 **302 重定向处理**：文件下载遵循 302 到 CDN 预签名 URL
5. 🟢 **ZIP 提取和图片打包**：从 ZIP 提取 `full.md` 和 `images/` 目录，打包为 `images.zip`

```json
{
  "mineru": [{
    "key": "sk-xxxxxxxx",
    "tier": "standard",
    "task_timeout": 300,
    "type": "official",
    "url": "https://mineru.net"
  }]
}
```

> ⚠️ 官方 API 的 `tier` 仅支持 `standard`，禁用 `model_version`（旧字段名）

---

## 错误处理

| HTTP 状态码 | 说明 |
|------------|------|
| 400 | 请求参数错误（如缺少 `purpose` 字段）|
| 401 | 无效或缺少 API Key |
| 404 | 资源不存在 |
| 409 | 资源冲突（上传已完成等） |
| 413 | 文件超出大小限制 |
| 503 | 服务不可用 |

官方 API 在缺少 `purpose` 字段时的典型错误：
```json
{
  "error": {
    "code": "invalid_request",
    "message": "field \"purpose\" is not set",
    "param": "purpose",
    "type": "invalid_request_error"
  }
}
```

---

## 限制说明

| 限制项 | 值 |
|--------|-----|
| 支持的文件来源 | `file_id`、`url` |
| 支持的解析档位 | `standard`（仅此一个）|
| 最大并发任务数 | 由账户等级决定 |
| 文件有效期 | 上传 24 小时后自动清理 |
| 最大文件大小 | 200MB |

---

## 参考链接

- [MinerU V1 HTTP API 官方文档](https://opendatalab.github.io/MinerU/usage/http_api/)
- [MinerU GitHub 仓库](https://github.com/opendatalab/MinerU)
- [MinerU 官网](https://mineru.net)
- OKB-Assist: `document/mineru4/mineru-api-server-report.md`（自部署 API 详细调研）
- OKB-Assist: `backend-rs/src/services/mineru.rs`（完整实现）