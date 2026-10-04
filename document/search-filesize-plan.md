# 文件大小筛选 — 实施计划

## 目标

在高级搜索中支持按 PDF、Markdown、ZIP 文件的大小进行筛选，语法如：

```
filesize_pdf:>0          # PDF 文件存在（>0 字节）
filesize_md:>0           # MD 文件存在
filesize_zip:>0          # ZIP 文件存在
filesize_pdf:>10KB       # PDF 大于 10KB
filesize_md:<=1MB        # MD 小于等于 1MB
filesize_zip:=0          # ZIP 文件不存在（大小为 0）
filesize_pdf:1024-5120   # PDF 在 1KB~5KB 之间
```

支持单位：`B`, `KB`, `MB`, `GB`（大小写不敏感）。

## 涉及文件

| 层 | 文件 | 改动 |
|----|------|------|
| 前端解析 | `frontend/src/utils/searchParser.ts` | 新增 `filesize_pdf` / `filesize_md` / `filesize_zip` 字段定义 |
| 前端 UI | `frontend/src/components/QueryBuilder.vue` | 搜索面板新增文件大小筛选表单域 |
| 后端解析 | `backend-rs/src/services/query_parser.rs` | 新增 `filesize_pdf/md/zip` 字段映射 + 数值解析支持单位后缀 |
| 后端 SQL | `backend-rs/src/services/query_parser.rs` | `build_where_clause()` 中处理新的数值字段 |
| 后端搜索 | `backend-rs/src/routers/documents.rs` | `search_info` 中的排序列表可能需扩展 |

## 实施步骤

### Step 1：定义单位解析工具函数

后端 `query_parser.rs` 中新增 `parse_file_size()` 函数：

```
输入 "10KB"  → 输出 10240
输入 "1.5MB" → 输出 1572864
输入 ">10KB" → 输出 运算符 + 10240
输入 "=0"    → 输出 运算符 = + 0
```

支持的运算符：`>`, `>=`, `<`, `<=`, `=`（精确等于），`min-max` 范围。

正则模式：`^(>=?|<=?|=)?\s*(\d+(?:\.\d+)?)\s*(B|KB|MB|GB)?$`

### Step 2：后端 — 扩展 query_parser.rs

1. 在 `resolve_field()` 中增加：

```rust
"filesize_pdf" => Some(FieldInfo::number("pdf_size")),
"filesize_md"  => Some(FieldInfo::number("md_size")),
"filesize_zip" => Some(FieldInfo::number("zip_size")),
```

2. 修改 `parse_range()` → 调用 `parse_file_size()` 将带单位的字符串转为字节数再解析范围。

3. `build_where_clause()` 中已有的数值字段处理逻辑（`year` 分支）可复用，需扩展为通用数值字段处理（不仅限于 `year`）。

### Step 3：前端 — 扩展 searchParser.ts

1. 在 `SEARCH_FIELD_DEFS` 中增加：

```ts
{ prefix: 'filesize_pdf', field: 'pdf_size', type: 'number', label: 'PDF 大小' },
{ prefix: 'filesize_md',  field: 'md_size',  type: 'number', label: 'MD 大小' },
{ prefix: 'filesize_zip', field: 'zip_size', type: 'number', label: 'ZIP 大小' },
```

2. `parsedQueryToParams()` 中扩展数值字段处理（不仅限于 `year`），将 `pdf_size`/`md_size`/`zip_size` 的 range 条件放到 `q` 参数中传递给后端。

### Step 4：前端 — 扩展 QueryBuilder.vue

在搜索面板中新增「文件大小」区域，包含三个输入行，每行一个下拉选择运算符 + 数值输入 + 单位选择：

```
文件大小：
  PDF: [≥] [10] [KB]
  MD:  [>]  [0]  [B]
  ZIP: [=]  [0]  [B]
```

运算符选项：`>`, `≥`, `<`, `≤`, `=`, 范围

### Step 5：后端 — 扩展 search-info 的 SQL 生成

`build_where_clause()` 中目前只有 `year` 走数值分支。需要改为通用分支：

```rust
// 通用数值范围过滤（不再仅限 year）
for (column, range) in &q.range_filters {
    if let Some(min) = range.min {
        if let Some(max) = range.max {
            if min == max {
                conditions.push(format!("{} = ?", column));
                binds.push(min.to_string());
            } else {
                conditions.push(format!("{} >= ? AND {} <= ?", column, column));
                binds.push(min.to_string());
                binds.push(max.to_string());
            }
        } else {
            conditions.push(format!("{} >= ?", column));
            binds.push(min.to_string());
        }
    } else if let Some(max) = range.max {
        conditions.push(format!("{} <= ?", column));
        binds.push(max.to_string());
    }
}
```

### Step 6：前后端联调验证

测试用例：

| 查询 | 预期 |
|------|------|
| `filesize_pdf:>0` | 仅返回有 PDF 文件的文档 |
| `filesize_md:=0` | 仅返回无 MD 文件的文档 |
| `filesize_zip:>10KB` | ZIP 包大于 10KB 的文档 |
| `filesize_pdf:>=1MB filesize_md:>0` | PDF ≥ 1MB 且 MD 存在的文档 |
| `filesize_pdf:512KB-2MB` | PDF 在 512KB~2MB 之间的文档 |

## 注意事项

1. 数据库中的 `pdf_size` / `md_size` / `zip_size` 是 `Option<i64>`（可为 NULL），NULL 时视为不存在。`>0` 可有效区分存在与不存在。
2. 后端 `parse_range()` 当前返回 `Option<RangeBound>`（`min`/`max` 都是 `Option<i64>`），`=` 运算符可表达为 `min == max == value`。
3. 前端 `parsedQueryToParams()` 目前仅将 `year` 转为 `year_min`/`year_max` 参数。对 `pdf_size` 等字段，需要将 range 条件序列化到 `q` 参数字符串中（如 `pdf_size:>=1024`），让后端解析。或者新增独立参数 `pdf_size_min`/`pdf_size_max`。推荐后者（更干净）。
4. `search-info` 入口已使用 `query_parser.rs` 解析 + `build_where_clause()` 构建 SQL，新增字段后自动生效，无需修改 `search_info` 函数本身，只需确保 `build_where_clause()` 正确处理新字段。