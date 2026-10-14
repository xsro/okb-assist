# 文献元数据修复方法

## 适用场景

当 OKB-Assist 系统中文献的 `title`、`authors`、`journal`、`doi` 等字段因 MinerU 解析错误而出现以下问题时：

- **标题错误**：标题为期刊名、会议名、ISSN/CN 号、分类号、乱码等
- **作者错误**：作者为 "CNKI"、期刊名、作者简介、论文标题等
- **期刊缺失**：`journal` 字段为空
- **DOI 缺失**：`doi` 字段为空
- **作者不完整**：仅有部分作者，缺少合著者

## 修复流程

### 1. 获取文献列表

```bash
curl -s -H "X-Token: <token>" "https://<host>/assist/api/documents/?id_min=<start>&id_max=<end>"
```

### 2. 逐篇获取 Markdown 原文

```bash
curl -s -H "X-Token: <token>" "https://<host>/assist/api/documents/{id}/markdown/?raw=1"
```

从 Markdown 前 30 行提取以下信息：

| 提取项 | Markdown 中的位置 |
|--------|-------------------|
| 正确标题 | `# 标题` 或 `题目：标题` |
| 英文标题 | `# English Title` |
| 作者列表 | 标题下方的作者行（含上标数字标记） |
| 期刊名 | 引用格式 `[J]. 期刊名, 年份, 卷(期):页码` |
| DOI | `DOI：10.xxx/...` |
| 关键词 | `关键词：...` |
| 年份 | 引用格式中的年份 |

### 3. 从文件名补充信息

当 Markdown 为空或信息不全时，从文件名提取：

```
文件名格式：<标题>_<第一作者>.pdf
```

### 4. 构造修正 JSON

```json
{
  "title": "正确标题",
  "authors": "[\"作者1\",\"作者2\",\"作者3\"]",
  "journal": "期刊名",
  "source": "期刊名",
  "year": 2026,
  "doi": "10.xxxx/xxxx"
}
```

> **关键**：`authors` 和 `keywords` 必须是 JSON 字符串（双引号包裹的数组字符串），不能是原生 JSON 数组。例如：`"[\"A\",\"B\"]"` 而不是 `["A","B"]`。

### 5. 通过 PUT 请求提交修正

```bash
curl -s -X PUT -H "X-Token: <token>" -H "Content-Type: application/json" -d @fix_{id}.json "https://<host>/assist/api/documents/{id}"
```

### 6. 验证修正结果

```bash
curl -s -H "X-Token: <token>" "https://<host>/assist/api/documents/{id}"
```

## 常见错误模式

### 标题类错误

| 错误模式 | 示例 | 原因 |
|----------|------|------|
| 期刊名 | `指挥控制与仿真 Command Control & Simulation` | MinerU 提取了页眉/页脚的期刊名 |
| ISSN/CN 号 | `ISSN 1004-731X,CN 11-3092/V` | 提取了期刊标识 |
| 会议名 | `中国电子学会第五届电磁频谱学术大会论文集` | 提取了会议名称 |
| 分类号 | `中2分类号 : V355` | 提取了中图分类号 |
| 乱码 | `138 Ö‚»Ó¿ØÖ˘Óº•´Õæ` | 编码错误 |
| 页码/卷号 | `48(7):161-166. 标题` | 提取了引用信息 |
| 作者附加 | `标题 □ 李 腾,吴 昊,马 娜` | 提取了作者姓名 |

### 作者类错误

| 错误模式 | 示例 | 原因 |
|----------|------|------|
| "CNKI" | `["CNKI"]` | 提取了来源标识 |
| 期刊名 | `["现代导航"]` | 提取了期刊名 |
| 论文标题 | `["融合大模型的智能化车辆管理系统构建与效能评估"]` | 提取了标题 |
| 作者简介 | `["作者简介 李鹏程","男","山东潍坊",...]` | 提取了作者简介栏 |
| 会议名 | `["中国电子学会第五届电磁频谱学术大会论文集"]` | 提取了会议名称 |

## 修复原则

1. **只修正明显错误的字段**：已正确的字段保持不变
2. **从 Markdown 提取权威信息**：以 Markdown 正文中的标题、作者、引用格式为准
3. **文件名作为辅助**：Markdown 为空时从文件名推断标题和第一作者
4. **期刊名从引用格式提取**：优先使用 Markdown 中的引用格式 `[J]. 期刊名`
5. **DOI 从 Markdown 提取**：优先使用 `DOI：10.xxx` 格式
6. **中文标题保留原文**：不做翻译或改写

## 批量修复技巧

- 使用 `write` 工具先将 JSON 写入本地文件，再用 `curl -d @file` 提交，避免中文字符编码问题
- 批量并行提交 PUT 请求，每批 6-10 个
- 修复后用 grep 快速验证关键字段

## 相关文件

- API 文档：`backend-rs/src/routers/documents.rs`
- 数据模型：`backend-rs/src/models.rs`
- 配置：`backend-rs/system.json`（token 鉴权）