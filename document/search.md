# 高级搜索功能

OKB-Assist 提供 GitHub 风格的搜索语法，支持多字段筛选、范围查询、逻辑组合等高级特性。所有搜索通过前端解析后映射为后端 SQL 查询，无需额外的搜索服务。

## 快速入门

在首页顶部搜索框中直接输入查询语句：

```
transformer attention
```

搜索 `transformer` 和 `attention` 两个关键词（匹配标题、作者、文件名）。常规文本搜索行为不变。

## 语法参考

### 字段限定搜索

使用 `field:value` 语法在指定字段中搜索：

| 语法 | 说明 | 示例 |
|------|------|------|
| `title:关键词` | 按标题搜索 | `title:attention` |
| `author:姓名` | 按作者搜索 | `author:vaswani` |
| `authors:姓名` | 同 author | `authors:sun` |
| `keyword:术语` | 按关键词搜索 | `keyword:transformer` |
| `keywords:术语` | 同 keyword | `keywords:reinforcement` |
| `abstract:文本` | 按摘要搜索 | `abstract:pre-training` |
| `journal:刊名` | 按期刊搜索（模糊匹配） | `journal:Nature` |
| `doi:DOI` | 按 DOI 搜索 | `doi:10.1000/xyz123` |
| `source:来源` | 按来源搜索 | `source:arxiv` |
| `filename:文件名` | 按 PDF 文件名搜索 | `filename:paper.pdf` |
| `category:分类` | 按分类搜索 | `category:AI` |
| `type:类型` | 按文献类型筛选 | `type:article` |
| `doc_type:类型` | 同 type | `doc_type:book` |
| `lang:语言` | 按语言筛选 | `lang:en` |
| `language:语言` | 同 lang | `language:zh` |
| `status:状态` | 按文档状态筛选（见下方状态值） | `status:done` |
| `sort:字段` | 排序字段 | `sort:year` |
| `order:方向` | 排序方向（asc 升序 / desc 降序） | `order:desc` |

**排序字段可选值**：`title`、`authors`、`year`、`id`、`doc_type`、`status`、`journal`、`doi`、`category`、`created_at`（登记时间）、`updated_at`（更新时间）

**状态值说明**：

| 查询值 | 含义 | 后端映射 |
|--------|------|---------|
| `uploaded` | 已上传未解析 | `uploaded` |
| `parsing` | 解析中 | `parsing` |
| `done` | 解析完成 | `markdown_done` |
| `error` | 解析出错 | `error` |

### 年份范围查询

`year:` 支持多种数值语法：

| 语法 | 含义 | 示例 |
|------|------|------|
| `year:2023` | 精确匹配 2023 年 | `year:2023` |
| `year:>2020` | 年份大于 2020 | `year:>2020` |
| `year:>=2020` | 年份大于等于 2020 | `year:>=2020` |
| `year:<2020` | 年份小于 2020 | `year:<2020` |
| `year:<=2020` | 年份小于等于 2020 | `year:<=2020` |
| `year:2020-2023` | 年份在 2020 到 2023 之间（含） | `year:2020-2023` |

### 精确短语

用双引号 `"` 包裹的文本会保留空格作为一个整体搜索：

```
title:"attention is all you need"
"deep reinforcement learning"
```

### 否定条件

用 `-` 前缀排除匹配项：

```
transformer -year:2020          ← 排除 2020 年的 transformer 论文
author:vaswani -title:attention ← 排除标题含 attention 的 vaswani 论文
```

### 逻辑或

用 `OR`（大写）连接两个条件，满足其一即返回：

```
year:2023 OR year:2024
title:transformer OR title:attention
```

### 组合查询

空格分隔多个条件 = AND 逻辑：

```
transformer title:attention year:>2018 status:done
```

含义：搜索 `transformer` 关键词，标题含 `attention`，2018 年后，已解析完成的文档。

## 输入提示

- 点击搜索框（内容为空时）会弹出**语法帮助面板**，列出所有支持的高级语法
- 输入时 `field:` 前缀会**自动高亮**为蓝色，`-` 否定前缀为红色，`OR` 为橙色，引号短语为绿色
- 输入后搜索框下方会显示**活跃筛选条件 chips**，点击 × 或「清除全部」可一键重置

## 与旧界面控件的关系

| 旧控件 | 新方案 |
|--------|--------|
| 搜索范围面板（复选框） | 已移除，由 `field:` 语法完全替代 |
| 类型筛选下拉 | 已移除，由 `type:` 语法替代 |
| 排序下拉 | 已移除，由 `sort:` 语法替代 |
| 升降序切换 | 已移除，由 `order:` 语法替代 |

## 技术实现

### 解析流程

```
用户输入 → searchParser.ts（分词 → 字段识别 → 范围解析）→ SearchApiParams
        ↓
   后端 list_documents()（SQL LIKE + 条件过滤）→ 结果列表
```

### 前端文件

| 文件 | 职责 |
|------|------|
| `frontend/src/utils/searchParser.ts` | 查询语法解析器（分词、字段映射、范围解析、OR 分割） |
| `frontend/src/views/HomeView.vue` | 搜索 UI（输入框、高亮、帮助面板、chips、调用解析器发起请求） |

### 后端文件

| 文件 | 职责 |
|------|------|
| `backend-rs/src/routers/documents.rs` | `ListQuery` 结构体、`list_documents()` SQL 构建（含 `year_min`/`year_max`/`authors` 条件） |

### 注意事项

- OR 查询目前在前端被解析标记，但因后端 SQL 原生未支持 OR 语法，含 OR 的查询退化为基础字段匹配（按第一组条件搜索）
- `year:>` 和 `year:<` 使用不包含边界（即 `>2020` → `year >= 2021`），`year:>=` 和 `year:<=` 包含边界
- 否定条件（`-year:2020`）目前标记在前端 ParsedQuery 中，后端暂未实现排除逻辑
- 所有搜索均为 SQL `LIKE %keyword%` 模糊匹配，不是全文索引

## 常见搜索场景

```bash
# 查找 2022 年以后 Nature 上的 transformer 论文
  transformer journal:Nature year:>2021

# 查找某个作者近年的文章（按年份升序排列）
  author:lecun year:2019-2024 sort:year order:asc

# 查找所有解析完成的书籍
  type:book status:done

# 查找摘要中包含 pre-training 的 2023 年文章（按标题排序）
  abstract:pre-training year:2023 type:article sort:title

# 排除某些年份
  transformer -year:2021 -year:2022

# 按更新时间降序排列（最新在前）
  sort:updated_at order:desc

# 按类型分组查看
  sort:doc_type order:asc
```