# 高级搜索（Web of Science 风格检索式）

OKB-Assist 的搜索框与「高级搜索」面板使用 **Web of Science 风格的检索式**：`标签=值` + 布尔运算符 + 括号分组。
检索式由后端 `backend-rs/src/services/query_parser.rs` 编译成 SQL（无额外搜索服务），
前端 `frontend/src/utils/wosQuery.ts` 只做实时校验与语法高亮。

旧的 GitHub 风格写法（`field:value`、`-field:value`）**仍然完全兼容**，`TAG=value` 与 `tag:value` 等价。

## 快速入门

```
TS=transformer AND PY=2020-2023
AU=Vaswani* AND (TI=attention OR AB=transformer)
DO=10.1093/nar/gkab123
PDF=present AND HEALTH=ok
```

## 字段标签

标签大小写不敏感；**值含空格时必须用引号包起来**：`SO="IEEE TRANSACTIONS ON AUTOMATIC CONTROL"`。

| WoS 标签 | 旧写法（等价） | 含义 | 示例 |
|---|---|---|---|
| `TS=` | — | **主题**：标题 + 作者关键词 + 摘要（中英双语列） | `TS=(transformer OR "deep learning")` |
| `TI=` | `title:` | 标题 | `TI=attention`、`TI="attention is all you need"` |
| `AU=` | `author:` / `authors:` | 作者 | `AU=Vaswani*` |
| `SO=` | `journal:` | 期刊 / 会议名 | `SO="IEEE TRANSACTIONS ON …"` |
| `AB=` | `abstract:` | 摘要 | `AB=pre-training` |
| `AK=` | `keyword:` / `keywords:` | 作者关键词 | `AK=reinforcement` |
| `DO=` | `doi:` | DOI | `DO=10.1000/xyz123` |
| `DT=` | `type:` / `doc_type:` | 文献类型（Zotero 标准名，同义词自动归一） | `DT=journalArticle`、`DT=article` |
| `PY=` | `year:` | 出版年份（支持范围与比较，见下） | `PY=2023`、`PY=2020-2023`、`PY>2020` |
| `LA=` | `lang:` / `language:` | 语言 | `LA=en` |
| `SRC=` | `source:` | 来源 | `SRC=arxiv` |
| `FN=` | `filename:` | PDF 文件名 | `FN=paper.pdf` |
| `CAT=` | `category:` | 分类 | `CAT=AI` |
| `ALL=` | — | **全部元数据列**（不写标签时的默认行为） | `ALL=transformer` |
| `ID=` | `id:` | 文献 ID（支持范围与比较） | `ID=1-30`、`ID>100` |
| `PDF=` | `pdf:` | PDF 文件三态 | `PDF=present` |
| `MD=` | `md:` | Markdown 文件三态 | `MD=absent` |
| `ZIP=` | `zip:` | 资源包三态 | `ZIP=present` |
| `HEALTH=` | `health:` | PDF 健康 | `HEALTH=warning,error` |
| `filesize_pdf=` | 同左 | PDF 大小（支持单位与范围） | `filesize_pdf>1MB` |
| `filesize_md=` | 同左 | Markdown 大小 | `filesize_md>10KB` |
| `filesize_zip=` | 同左 | 资源包大小 | `filesize_zip=100KB-2MB` |
| `SORT=` | `sort:` | 排序字段（不参与筛选） | `SORT=PY` |
| `ORDER=` | `order:` | 排序方向 `asc` / `desc` | `ORDER=desc` |

> **不要用 `status:`** —— 文献状态由「文件事实 + 运行时」推导，库里没有这一列。
> 想看「已解析」用 `MD=present`，想看「待解析」用 `PDF=present AND MD=absent`。

## 布尔运算与括号

| 写法 | 含义 |
|---|---|
| `A AND B` / `A B` | 同时满足（**相邻条件默认就是 AND**） |
| `A OR B` | 满足其一 |
| `NOT A` / `-A` | 排除（`-PY:2020` 与 `NOT PY=2020` 等价） |
| `( … )` | 分组，可任意嵌套；`AND` 优先级高于 `OR` |
| `TAG=( … )` | 分组内裸值套用该标签：`TS=(transformer OR attention)`、`PY=(2020 OR 2021)` |
| `NEAR/n` / `SAME` | 暂未实现词距判定，**按 AND 处理** |

```
(TI=attention OR TI=transformer) AND PY>=2019 AND NOT DT=review
TS=(transformer OR "deep learning") AND AU=Vaswani*
```

## 不写标签（自由文本）

不写标签的裸值等于 `ALL=`，**但按空格切开后是多个独立条件，彼此是 AND，而不是一个短语**：

```
prescribed time control
≡ ALL=prescribed AND ALL=time AND ALL=control
≡ (14 列 LIKE '%prescribed%') AND (14 列 LIKE '%time%') AND (14 列 LIKE '%control%')
```

- 每个词单独在**全部元数据列**上做 `LIKE '%词%'`，任一列命中即算该词命中：
  `title`、`i18n.en.title`、`authors`、`i18n.en.authors`、`journal`、`i18n.en.journal`、`keywords`、
  `i18n.en.keywords`、`abstract`、`i18n.en.abstract`、`doi`、`source`、`filename`、`category`；
- 三个词**不需要相邻、也不需要落在同一列**（`prescribed` 可以在标题里、`time` 在摘要里、`control` 在关键词里，照样命中）；
- 所以 `prescribed time control` 是「三词各自出现」的宽泛检索，词越常见（`time`、`control`）结果越松；
- **要按短语查必须加引号**：`"prescribed time control"`（只产生一条 `%prescribed time control%` 的 14 列 OR）；
- 要缩小范围就显式指定字段：`TI=prescribed AND TI="time control"`、`TS=(prescribed AND "time control")`。

> 一句话：空格的语义是 **AND 分词**，不是「连起来一起匹配」。

## 通配符与匹配方式

| 符号 | 含义 | 例 |
|---|---|---|
| `*` | 任意个字符 | `AU=Vaswani*` → 以 Vaswani 开头的作者 |
| `?` | 单个字符 | `TI=trans?ormer` |
| `$` | 0 或 1 个字符（近似为 `*`） | `TI=colour$` |

- **不含通配符**时按「包含」匹配（两侧自动补 `%`）：`TI=attention` = 标题含 attention；
- **含通配符**时按原样匹配（`ATT*` 是前缀匹配，不再自动补后侧 `%`）；
- `%`、`_`、`\` 会被转义（SQL 侧 `ESCAPE '\'`），所以搜索 `50%` 这类字面值也能精确命中；
- 短语用引号：`TI="attention is all you need"`（作为一个整体匹配）；不加引号则按空格拆成多个 AND 条件，见上节「不写标签（自由文本）」；
- 所有文本检索都是 SQL `LIKE` 模糊匹配，不是全文索引。

## 数值、范围与大小

| 语法 | 含义 |
|---|---|
| `PY=2023` | 精确 2023 |
| `PY=2020-2023` | 2020 ~ 2023（含两端） |
| `PY>2020` / `PY>=2020` | 大于 / 大于等于（`>` 自动换算为 `>=2021`） |
| `PY<2020` / `PY<=2020` | 小于 / 小于等于（`<` 自动换算为 `<=2019`） |
| `ID=1-30` | 文献 ID 区间 |
| `filesize_pdf>1MB` | 文件大小支持 `B` / `KB` / `MB` / `GB`，如 `filesize_md=10KB-2MB` |

## 文件事实三态与 PDF 健康

| 查询值 | 含义 | 后端条件 |
|---|---|---|
| `present` | 文件存在 | `size > 0` |
| `absent` | 已同步且确定没有该文件 | `fs_synced_at` 非空且 `size` 为空/0 |
| `unknown` | 还没从磁盘同步过（状态未知） | `fs_synced_at IS NULL` |

`HEALTH=` 取 `ok` / `warning` / `error` / `missing` / `unchecked`；`unchecked` 匹配 `pdf_health IS NULL`。
两者都支持逗号多选（= OR）：`PDF=present,absent`、`HEALTH=warning,error`。

## 排序

```
SORT=PY ORDER=desc
sort:year order:asc        ← 旧写法
```

排序字段可选：`title`、`authors`、`year`、`id`、`journal`、`doc_type`、`category`、`created_at`、
`updated_at`、`pdf_size`、`md_size`、`zip_size`；也可写标签（`SORT=TI` → `title`、`SORT=AU` → `authors`、`SORT=PY` → `year`）。

## 常见场景

```
# 2020 年以后的 transformer / 深度学习论文
TS=(transformer OR "deep learning") AND PY=2020-2023

# 某作者近年的文章，按年份升序
AU=lecun AND PY=2019-2024 SORT=PY ORDER=asc

# 2022 年以后 Nature 上的 transformer，排除综述
SO=Nature AND TS=transformer AND PY>2021 NOT DT=review

# 已解析完成、PDF 健康有问题的文献
MD=present AND HEALTH=warning,error

# 某本专著的章节（题名里带 chapter 的）
DT=bookSection AND TI=chapter

# 指定 ID 区间按 ID 升序
ID=1-30 AND MD=present SORT=ID ORDER=asc

# 三词各自出现（宽泛）vs 作为整体短语（严格）
prescribed time control
"prescribed time control"
```

## 界面行为

- 搜索框输入时实时高亮：**标签**蓝色、`AND/OR/NOT` 橙色、括号紫色、引号短语绿色、通配符金色；
- 检索式有语法错误时，搜索框下方给出**红色提示与出错字符位置**（后端也会再校验一次，非法检索式返回 400 `code: "query_error"`）；
- 生效了哪些条件直接看检索式本身（高亮 + 语法提示），列表页**不再另外渲染筛选 chips**（解析结果只用于错误提示，面板内才显示「共 N 个条件」）；
- 搜索栏右侧的「高级搜索」按钮打开面板：检索式编辑器 + 字段标签 / 运算符一键插入 + 示例 + 语法说明（第二个 tab 是列管理）。

## 技术实现

```
用户输入 → HomeView 原样发给 /assist/api/documents/search-info/?q=…
        ↓
services/query_parser.rs：分词（lex）→ 递归下降（parse_or → parse_and → parse_not → primary）
        ↓
AST → emit()：Text / Range / Enum / Raw(state) / Health → WHERE 片段 + 绑定值
        ↓
documents.rs::search_info：叠加 ?pdf_health= / ?pdf_state= 等参数 → SQL
```

| 文件 | 职责 |
|---|---|
| `backend-rs/src/services/query_parser.rs` | 检索式词法 / 语法分析、AST → SQL（`compile()`）、语法错误（带字符位置） |
| `backend-rs/src/services/file_state.rs` | `state_condition()`（文件事实三态）、`health_condition()`（PDF 健康，检索式与列表端点共用） |
| `backend-rs/src/routers/documents.rs` | `search_info()` 端点（400 `query_error`）、`list_documents()` 的筛选参数 |
| `frontend/src/utils/wosQuery.ts` | 字段标签表（`WOS_FIELDS`）、解析 / 校验（`parseWosQuery`）、高亮（`highlightWosQuery`）、示例 |
| `frontend/src/components/QueryBuilder.vue` | 高级搜索面板（检索式编辑器 + 标签/运算符插入 + 实时校验 + 语法说明；另一 tab 为列管理） |
| `frontend/src/views/HomeView.vue` | 搜索框、高亮层、语法错误提示、地址栏（`?q=` / `?page=`）同步 |

### 注意事项

- **两份解析器要保持同步**：新增字段标签先改后端 `resolve_tag()`，再补前端 `WOS_FIELDS`；
  前端校验刻意比后端宽松，真正的裁决是后端 400。
- 未知 `TAG=` 会直接报错（400）；未知的旧写法 `tag:value` 仍按自由文本处理（避免 `https://…` 被误判）。
- 字段值里的 `:` / `=` 不影响解析（`DO=10.1000/xyz` 正常）；只有值**以空格结尾**时才需要引号。
- 布尔值都是大写无歧义的关键字；小写 `and` / `or` / `not` 同样识别。
