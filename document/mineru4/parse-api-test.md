# MinerU V1 API 解析测试报告

> 测试时间：2026-09-28  
> 测试对象：`http://192.168.1.185:8002`（mineru-kit router 模式，MinerU 4.0.5）  
> 测试文档：学术论文 PDF（9页，1.8MB，3466.pdf）  
> 测试目标：验证上传 → 解析 → 下载完整工作流，对比 flash / standard 档位效果

---

## 1. 健康检查

```bash
curl -s http://192.168.1.185:8002/v1/health
```

```json
{
  "status": "ok",
  "version": "4.0.5",
  "features": {
    "webhook": false,
    "output_formats": ["markdown", "middle_json", "structured_content", "zip"],
    "sources": ["file_id", "inline", "url"]
  }
}
```

## 2. 模型与档位

```bash
# 可用模型
curl -s http://192.168.1.185:8002/v1/models
```

| 模型 ID | 用途 |
|---------|------|
| MinerU-Flash | flash 档位（本地快速文本提取） |
| Hybrid-Basic | basic 档位（基础 OCR + 轻量模型） |
| MinerU2.5-Pro-2605-1.2B | standard / advanced 档位（VLM 辅助） |
| MinerU-HTML | HTML / DOCX / LaTeX 输出 |

```bash
# 可用档位
curl -s http://192.168.1.185:8002/v1/tiers
```

| 档位 | 当前模型 | 说明 |
|------|----------|------|
| flash | flash | 快速本地文本提取（预览、索引、Office文档） |
| basic | hybrid-basic | 基础 OCR + 轻量模型（简单文档/扫描件） |
| standard | MinerU2.5-Pro-2605-1.2B | 标准解析，VLM 辅助（大多数文档） |
| advanced | MinerU2.5-Pro-2605-1.2B | 高级解析，更高 VLM 算力（复杂排版/表格/公式） |

> 注：standard/advanced 需要配置 VLM 后端（`--vlm-server-url`）或使用 `--preload-models`；flash 档位无需 VLM。

---

## 3. 完整解析工作流（5 步）

### 3.1 创建上传会话

```bash
curl -s -X POST http://192.168.1.185:8002/v1/uploads \
  -H "Content-Type: application/json" \
  -d '{"filename":"test.pdf","bytes":1845615,"mime_type":"application/pdf"}'
```

必要字段：`filename`, `bytes`, `mime_type`  
可选字段：`sha256sum`（启用秒传去重）, `purpose`, `expires_after`

**响应示例：**
```json
{
  "id": "upload_xxx",
  "status": "pending",
  "upload_url": "/v1/uploads/upload_xxx/content",
  "upload_method": "PUT",
  "upload_headers": {"Content-Type": "application/pdf"}
}
```

### 3.2 上传文件内容

```bash
UPLOAD_ID="upload_xxx"
curl -s -X PUT "http://192.168.1.185:8002/v1/uploads/${UPLOAD_ID}/content" \
  -H "Content-Type: application/pdf" \
  --data-binary "@test.pdf"
```

### 3.3 完成上传

```bash
curl -s -X POST "http://192.168.1.185:8002/v1/uploads/${UPLOAD_ID}/complete" \
  -H "Content-Type: application/json" \
  -d '{}'
```

**响应示例：**
```json
{
  "status": "completed",
  "file": {"id": "file-xxx", "bytes": 1845615}
}
```

### 3.4 创建解析任务

```bash
FILE_ID="file-xxx"
curl -s -X POST http://192.168.1.185:8002/v1/parse/jobs \
  -H "Content-Type: application/json" \
  -d '{
    "files": [{"source": {"type": "file_id", "file_id": "'"${FILE_ID}"'"}}],
    "tier": "standard",
    "output_formats": ["markdown", "zip"]
  }'
```

**请求参数说明：**

| 参数 | 类型 | 必需 | 默认 | 说明 |
|------|------|------|------|------|
| `files[].source.type` | string | 是 | — | 来源：`file_id`/`url`/`inline`/`local` |
| `files[].source.file_id` | string | 取决于 type | — | 已上传文件的 ID |
| `files[].page_range` | string | 否 | `all` | 页码范围，如 `"1-5,8"` |
| `tier` | string | 否 | 服务默认 | `flash`/`basic`/`standard`/`advanced` |
| `output_formats` | array | 否 | `["markdown"]` | 输出格式列表 |

### 3.5 轮询任务状态

```bash
curl -s http://192.168.1.185:8002/v1/parse/jobs/job_xxx
```

**状态流转：** `queued` → `running` → `completed` / `failed` / `canceled`

### 3.6 下载结果

```bash
# 获取输出的 markdown（gzip 压缩）
curl -s "http://192.168.1.185:8002/v1/files/file-xxx/content" | gunzip -c > output.md

# 获取输出的 zip 包
curl -s "http://192.168.1.185:8002/v1/files/file-yyy/content" -o output.zip
```

---

## 4. 测试结果对比

| 指标 | Flash | Standard（VLM） |
|------|-------|-----------------|
| 解析耗时 | 14.2s | 31.3s |
| Markdown 输出大小 | 932KB | 691KB |
| $$ LaTeX 公式块数 | 0（公式被渲染为 base64 图片） | **16**（正确提取为 LaTeX） |
| 内嵌图片数 | 24（含公式图片） | 15（公式用 LaTeX 表示） |
| Section 标题 | 24 | 19（结构更精简） |
| 模型 | flash（本地） | MinerU2.5-Pro-2605-1.2B |

### 质量分析

**Flash 档位**适合：
- 快速文本提取、索引构建
- Office/EPUB/HTML 文件解析
- 纯文本为主的简单 PDF

**Standard 档位（VLM）**适合：
- 学术论文（数学公式密集型）
- 复杂排版、表格、多栏文档
- 需要高质量 LaTeX 公式输出的场景

---

## 5. VLM 后端配置说明

### 配置方式

MinerU 4.0.5 通过以下命令行参数配置 VLM 推理后端：

| 参数 | 默认值 | 说明 |
|------|--------|------|
| `--preload-models` | 未启用 | 启动时预加载 VLM 和本地 Hybrid 模型 |
| `--vlm-server-url` | 全局配置 | 远程 VLM 服务 URL（兼容 OpenAI API） |
| `--vlm-api-key` | 全局配置 | VLM 服务的 API Key |
| `--vlm-model` | 全局配置 | VLM 模型名 |
| `--vlm-http-timeout` | 600s | VLM HTTP 超时 |
| `--vlm-max-concurrency` | 100 | VLM 推理并发数 |

### 方案一：本地 GPU 加载

```bash
docker run --gpus all --shm-size 32g \
  -d -p 8002:8002 --ipc=host \
  --name mineru-router \
  mineru4 \
  /bin/bash -c "mineru-kit router --host 0.0.0.0 --port 8002 --local-gpus 0,1 --preload-models"
```

### 方案二：连接外部 VLM 服务

```bash
docker run --gpus all --shm-size 32g \
  -d -p 8002:8002 --ipc=host \
  --name mineru-router \
  mineru4 \
  /bin/bash -c "mineru-kit router \
    --host 0.0.0.0 --port 8002 --local-gpus 0,1 \
    --vlm-server-url http://192.168.1.185:11434/v1 \
    --vlm-api-key your-key \
    --vlm-model MinerU2.5-Pro-2605-1.2B"
```

### 验证 VLM 是否生效

```bash
# 检查模型列表（确认 VLM 模型存在）
curl -s http://192.168.1.185:8002/v1/models | python3 -m json.tool

# 提交 standard 解析任务测试
curl -s -X POST http://192.168.1.185:8002/v1/parse/jobs \
  -H "Content-Type: application/json" \
  -d '{"files":[{"source":{"type":"url","url":"https://arxiv.org/pdf/2401.00001.pdf"}}],
       "tier":"standard","output_formats":["markdown"]}'
```

---

## 6. 注意事项

1. **Output 使用 gzip 压缩**：下载 markdown 时需解压（`gunzip -c` 或 `gzip.decompress()`）
2. **内存存储**：Upload/File/Job 元数据全部在内存中，进程重启即丢失
3. **无 Webhook**：仅支持轮询获取结果
4. **`sha256sum` 去重**：上传时提供 SHA-256 可启用秒传（文件已存在时自动跳过上传和解析）
5. **端口差异**：router 模式默认端口 8002，api-server 模式默认端口 8000

---

## 7. 测试脚本（供后续重复使用）

```bash
#!/bin/bash
# MinerU API 测试脚本
URL="http://192.168.1.185:8002"
PDF_FILE="$1"
TIER="${2:-flash}"

if [ -z "$PDF_FILE" ]; then
  echo "Usage: $0 <pdf-file> [tier]"
  exit 1
fi

BYTES=$(stat -c%s "$PDF_FILE")
FILENAME=$(basename "$PDF_FILE")

# Step 1: 创建上传
echo "=== 1. 创建上传 ==="
UPLOAD=$(curl -s -X POST "$URL/v1/uploads" \
  -H "Content-Type: application/json" \
  -d "{\"filename\":\"$FILENAME\",\"bytes\":$BYTES,\"mime_type\":\"application/pdf\"}")
UPLOAD_ID=$(echo "$UPLOAD" | python3 -c "import sys,json;print(json.load(sys.stdin)['id'])")
echo "Upload ID: $UPLOAD_ID"

# Step 2: 上传文件
echo "=== 2. 上传文件 ==="
curl -s -X PUT "$URL/v1/uploads/${UPLOAD_ID}/content" \
  -H "Content-Type: application/pdf" \
  --data-binary "@$PDF_FILE" > /dev/null
echo "OK"

# Step 3: 完成上传
echo "=== 3. 完成上传 ==="
COMPLETE=$(curl -s -X POST "$URL/v1/uploads/${UPLOAD_ID}/complete" \
  -H "Content-Type: application/json" -d '{}')
FILE_ID=$(echo "$COMPLETE" | python3 -c "import sys,json;print(json.load(sys.stdin)['file']['id'])")
echo "File ID: $FILE_ID"

# Step 4: 创建解析任务
echo "=== 4. 创建解析任务 (tier=$TIER) ==="
JOB=$(curl -s -X POST "$URL/v1/parse/jobs" \
  -H "Content-Type: application/json" \
  -d "{\"files\":[{\"source\":{\"type\":\"file_id\",\"file_id\":\"$FILE_ID\"}}],\"tier\":\"$TIER\",\"output_formats\":[\"markdown\",\"zip\"]}")
JOB_ID=$(echo "$JOB" | python3 -c "import sys,json;print(json.load(sys.stdin)['job_id'])")
echo "Job ID: $JOB_ID"

# Step 5: 轮询
echo "=== 5. 等待任务完成 ==="
for i in $(seq 1 60); do
  STATUS=$(curl -s "$URL/v1/parse/jobs/$JOB_ID")
  STATE=$(echo "$STATUS" | python3 -c "import sys,json;print(json.load(sys.stdin)['status'])")
  echo "  [$i] Status: $STATE"
  if [ "$STATE" = "completed" ] || [ "$STATE" = "failed" ]; then break; fi
  sleep 2
done

# Step 6: 下载
echo "=== 6. 下载结果 ==="
MD_ID=$(echo "$STATUS" | python3 -c "
import sys,json
r=json.load(sys.stdin)
mf=r['files'][0]['output_files']['markdown']
print(mf['file_id'])
")
curl -s "$URL/v1/files/${MD_ID}/content" | gunzip -c > "${FILENAME%.pdf}_${TIER}.md"
echo "Saved to ${FILENAME%.pdf}_${TIER}.md"
```