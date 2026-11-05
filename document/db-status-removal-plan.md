# 移除数据库文献状态字段（设计与 TODO）

> 目标：彻底删除 `documents.status`（以及同源的 `status_message` / `progress`），
> 文献状态不再由数据库状态机维护，而是由**从文件系统同步到数据库的文件事实**
> （`pdf_size` / `md_size` / `zip_size`）+ **进程内运行时状态**推导。
>
"> 状态：**已实现（后端 + 前端）**，实现记录见文末「附录：落地情况」。
> 全部决策已定案（见「二、已定决策」），无待决项。

## 变更记录

| 版本 | 结论 |
|---|---|
| v1 | 状态由**实时 stat** 推导；筛选走「缓存列预筛 + stat 精判」，需要评估全量 stat 的成本（原 D3） |
| v2 | 状态改由**已同步到数据库的文件事实列**推导；`list_documents` **不再支持状态筛选**，改为按文件事实筛选（`has_md` / `has_pdf` / `has_zip`）。列表与统计请求**零 stat**，原 D3 的「全量 stat 成本」问题消失 |
| **v3** | ① 文件事实改为**三态**：`未同步` / `已同步且确定不存在` / `存在` —— 新增 `fs_synced_at` 列，消除 `size IS NULL` 的二义性；② `pdf_health` 纳入列表筛选（`pdf_health=ok,warning,error,missing,unchecked`） |
| **v4** | **全面不做兼容**：MCP `list_documents` / `search_documents` 的 `status` 参数直接删除（不保留翻译层）；`/admin/documents/status` 端点直接删除（不做 410 过渡）；REST `status_filter` 直接删除 |
| **v5（当前）** | 新增 **4.6 严格参数校验**：MCP `tools/call` 收到 schema 未声明的参数（如旧 `status`）直接返回 `isError` + 可用参数清单，不再静默忽略 |

## 一、为什么可以直接删

`documents.status` 目前把三类信息混在一个字段里：

| 类别 | 取值 | 新归属 |
|---|---|---|
| **持久事实** | `uploaded`（有 PDF 无 MD）、`markdown_done`（有 MD） | 由 `md_size` / `pdf_size` 列派生 |
| **瞬时运行态** | `parsing` + `progress` + `status_message`（MinerU 正在跑） | 进程内运行时注册表（+ 可选 sidecar） |
| **历史错误** | `error` + `status_message` | 同上，重启即丢 |

关键前提：**数据库里早就有一份「文件事实」了** —— `pdf_size` / `md_size` / `zip_size` 三列由
`documents.rs:439` `sync_doc_file_sizes()` 在**每个写路径**上同步（见 3.2），
并且 `POST /assist/api/admin/sync-file-sizes` 提供了全量/补缺修复入口。
因此状态不需要实时 stat 也能算准，只要这三列准。

### 顺带修掉的历史坑

`AGENTS.md` 记录过：「PDF 被替换后状态退回 `uploaded`，但 Markdown 早已解析完成」，
为此专门加了运维端点 `POST /assist/api/admin/documents/status`
（`admin.rs:1130` `fix_document_status`）。派生模型下这种不一致**结构性地不可能发生**
（状态永远等于文件事实），因此本次顺带清掉下面两处「状态补丁」。

### 顺带清掉的两处「状态补丁」

| 补丁 | 位置 | 处置 |
|---|---|---|
| `POST /assist/api/admin/documents/status` | `admin.rs:1130` `fix_document_status` + `:1110` `StatusFixBody` | **直接删除**（D6），不做过渡端点 |
| `reset_status`（替换 PDF 时把状态改回 `uploaded`） | `documents.rs:2439` `persist_pdf_for_doc` 的参数与 SQL 分支、`on_duplicate=replace` 的重置语义、`POST /documents/:id/pdf` 的 multipart 字段 | **直接删除**（D7）：状态字段删掉之后，它既无意义也无法表达 |

## 二、已定决策

| 决策项 | 结论 |
|---|---|
| 状态枚举 | **保留 4 个取值** `uploaded` / `parsing` / `markdown_done` / `error`，仅作为**派生/展示值**，前端徽章不变 |
| 状态来源 | `md_size` / `pdf_size` / `zip_size`（已同步）+ 进程内运行时状态；**不再实时 stat** |
| 必删列 | `documents.status` |
| 必删参数 | REST `list_documents` 的 `status_filter`、`sort_by=status`、搜索语法里的 `status:` 前缀与 `STATUS_ALIASES`；**MCP `list_documents` / `search_documents` 的 `status` 参数** |
| **兼容策略** | **不做任何兼容/过渡/弃用层**：旧参数、旧端点、旧翻译逻辑全部直接删除；旧参数传入 MCP 时**显式报错**而不是静默忽略（见 4.6） |
| 保留项 | `mineru_task_id`（外部任务指针，重启续跑的唯一依据）、`pdf_health` / `pdf_health_message`、`pdf_size` / `md_size` / `zip_size`（**升级为文件事实的唯一来源**） |
| 索引状态 | `document_vector_index.status` 本次不动（D4） |
| **新增列** | `documents.fs_synced_at TEXT` —— NULL = **从未同步过**；非空 = 上次同步时间。让 size 列从二态变**三态** |
| **健康筛选** | `pdf_health` 纳入列表筛选（`ok` / `warning` / `error` / `missing` / `unchecked`），字段本身不删不改 |

### 逐条决策（D1–D7）

| # | 问题 | 结论 |
|---|---|---|
| **D1** | 解析失败（`error`）的持久范围 | **内存为准 + sidecar 文件镜像**（方案 B）。重启后错误消失，文档回到「待解析」；sidecar 只为保留「上次失败原因」 |
| **D2** | `status_message` / `progress` 是否一并删除 | **一并删**。两者都能派生（见 3.3），留着必然再次漂移 |
| **D3** | ~~带 `status_filter` 的列表实现深度~~ | **作废**。v2 不再按状态筛选，列表/统计全部走同步列的 SQL，无需全量 stat，`read_dir` 快路径也不需要 |
| **D4** | `document_vector_index.status` | **不动**。改为探测向量库要每个库一次 count 查询，代价高，留 Phase 4 |
| **D5** | 迁移是否留备份表 | **保留 `documents_status_legacy` 一个版本**（`DROP COLUMN` 不可逆，留表可回滚） |
| **D6** | `/assist/api/admin/documents/status` 端点 | **直接删除**（`fix_document_status` + `StatusFixBody`），不做 410 过渡 |
| **D7** | 替换 PDF 时的「重置状态」开关（`reset_status`） | **直接彻底删除**：参数、SQL 分支、`on_duplicate=replace` 的重置语义、multipart 字段解析全部移除，**不做改名兼容**。重置解析的能力收敛为「清空解析产物」（`POST /pipeline/reset/{id}`，只做文件操作） |

> **D7 的两个直接后果**：
> 1. 替换 PDF **永不影响**解析阶段 —— 已解析文档换扫描件后仍是 `markdown_done`（派生模型的正确语义）；
> 2. 想重解析必须走「清空解析产物」（删除 Markdown + 资源包），这是一次破坏性文件操作，UI 上要二次确认。

## 三、可见状态：由同步列派生

### 3.1 文件事实的载体与三态语义

三列 size 已存在（`models.rs:107-109` → `Document.pdf_size/md_size/zip_size`），但 **NULL 有二义性**：
既可能是「还没同步过」，也可能是「同步过、文件确实不存在」。本次新增一列消除它：

| 列 | 语义 |
|---|---|
| `fs_synced_at`（**新增**） | 最近一次 `sync_doc_file_sizes()` 的时间；`NULL` = **从未同步过** |
| `pdf_size` / `md_size` / `zip_size` | 同步时 stat 到的大小；`NULL`（或 `0`）= 同步时**文件不存在** |

于是每个文件有三种状态（以 Markdown 为例）：

| 状态 | 判定 | 含义 |
|---|---|---|
| `unknown` 未同步 | `fs_synced_at IS NULL` | 数据库不知道文件在不在（迁移前老行 / 崩溃遗留 / 刚插入还没 stat） |
| `absent` 不存在 | `fs_synced_at IS NOT NULL AND NOT (md_size > 0)` | **确定**没有 Markdown |
| `present` 存在 | `md_size > 0` | 有 Markdown |

- 「0 字节文件」按 `absent` 处理（`md_size > 0` 为假），与现有 `file_size()` 语义一致。
- 三个文件**共享同一个** `fs_synced_at`（一次 `sync_doc_file_sizes()` 同时 stat 三个文件），不需要三个时间戳。
- `pdf_health` 是**另一条维度**（内容健康度，要真正解析 PDF 才能得到），不复用 size 的三态；
  它自己的「未检查」就是 `pdf_health IS NULL`。

> 为什么不加三个 `md_state/pdf_state/zip_state` 枚举列：枚举列需要额外维护、且与 size 天然冗余；
> 一个时间戳就能把「没同步」区分出来，三态在 SQL 里用表达式白名单拼（见 4.1）。

### 3.2 谁负责让这些列保持准确（现有 10 处 + 新增 2 处 + 1 处语义调整）

`sync_doc_file_sizes()` 的调用点（已存在）：

| 位置 | 场景 |
|---|---|
| `documents.rs:1397` | 上传落盘后 |
| `documents.rs:1515` | 按路径登记（`register`）复制 PDF 后 |
| `documents.rs:2454` | `persist_pdf_for_doc`（替换/覆盖 PDF） |
| `documents.rs:2832` | 整篇 Markdown 覆盖写 |
| `documents.rs:2955` | 上传解析结果（`parse-result`） |
| `documents.rs:3803` | 替换资源包 / PDF 相关 |
| `documents.rs:3870` | `replace-asset` |
| `pipeline.rs:519` | `finish_parse_result`（MinerU 解析完成搬文件后） |
| `admin.rs:1084` | `POST /admin/sync-file-sizes` 全量/补缺 |

**需要补的 3 处**：

1. `documents.rs:439` `sync_doc_file_sizes()` 自身要写 `fs_synced_at = now`（否则三态永远停在 `unknown`）；
2. `pipeline.rs:2250` `reset`（改造后的「清空解析产物」）删完 MD/ZIP 后必须调用一次，
   否则 `md_size` 残留会让文档继续显示 `markdown_done`；
3. BibTeX 导入落库路径（`documents.rs:1865` 一带的 `INSERT`）—— 现在插入后不 stat，`fs_synced_at` 会是
   `NULL`，文档先显示「未同步」。导入时就知道没有 PDF，应直接同步一次
   （得到三个 `NULL` + 时间戳 = 三个文件全部 `absent`）。

**语义变化的一处**：`POST /admin/sync-file-sizes?only_missing=1`（`admin.rs:1084` 一带）现在的判定是
「任一 size 为 `NULL`」，但 `NULL` 从此表示「文件不存在」而不再等于「缺失」。
改为 `fs_synced_at IS NULL`（真正的未同步），`AGENTS.md` 里对 `only_missing` 的描述同步更新。

### 3.3 派生规则（纯函数，唯一入口）

```rust
// services/file_state.rs
pub struct DocState {
    pub status: DocStatus,
    pub message: Option<String>,
    pub progress: f64,
    /// 文件事实是否已同步过（false = 状态未知，UI 可显示灰色「未同步」标）
    pub synced: bool,
}

/// 输入全部来自数据库列 + 进程内运行时状态，**零 IO**
pub fn derive_state(
    fs_synced_at: Option<&str>,
    pdf_size: Option<i64>,
    md_size: Option<i64>,
    rt: Option<&ActiveTask>,       // 进程内活动任务
    last_error: Option<&str>,      // 进程内 / sidecar
    mineru_task_id: Option<&str>,
) -> DocState
```

| # | 条件 | status | message | progress |
|---|---|---|---|---|
| 1 | 有活动任务 | `parsing` | 任务实时文案 | 任务实时进度 |
| 2 | 有错误记录 | `error` | 错误原文 | 0 |
| 3 | `md_size > 0` | `markdown_done` | `PDF 解析完成` | 100 |
| 4 | `mineru_task_id` 非空 | `parsing` | `正在恢复上次解析任务…` | 0 |
| 5 | `fs_synced_at` 为空（**未同步**） | `uploaded` | `文件状态未同步`（`synced = false`） | 0 |
| 6 | `pdf_size > 0` | `uploaded` | `待解析` | 0 |
| 7 | 其他（已同步，PDF **确定**不存在） | `uploaded` | `PDF 缺失，请先上传 PDF` | 0 |

- **规则 5 与 7 的区别就是要的「没同步」vs「文件不存在」**：前者是「不知道」，后者是「确定没有」。
  两者 status 都是 `uploaded`（枚举不变），靠 `message` 与 `synced` 标记区分。

- 规则 6 覆盖 BibTeX 导入的「只有条目没有 PDF」（`AGENTS.md` 易错点 24），`pdf_health = missing` 不变。
- 规则 2 的错误在重启后消失 → 回到规则 5/6，即「可以重新解析」。
- **`doc_to_out` 不再做 stat**：`pdf_size` / `md_size` / `zip_size` 直接返回列值
  （现在是 `documents.rs:455` 里现算的 `file_size()`，与筛选条件不同源会造成
  「筛出 `has_md=1` 的行却显示 `md_size` 为空」的自相矛盾）。

### 3.4 新鲜度策略（v2 的唯一代价）

同步列不反映**带外**文件改动（手工放/删 `.md`、外部删 PDF），直到下一次 sync。兜底四层：

| 层 | 做法 | 目的 |
|---|---|---|
| 1 | 所有写路径 `sync_doc_file_sizes()`（3.2） | 正常流程 100% 准确 |
| 2 | **启动后后台全量 sync 一次**（`tokio::spawn`，不阻塞启动） | 修复带外改动、崩溃遗留、迁移后偏差 |
| 3 | `POST /assist/api/admin/sync-file-sizes`（已有端点） | 运维手动修复 |
| 4 | **单篇详情 `GET /documents/:id` 实时 stat 并回写** | 详情页永远准确（单篇 3 次 stat，成本可忽略）；列表页用缓存可接受 |

> 第 4 层是刻意的不对称设计：**列表/筛选/统计用同步列，单篇详情用实时值并回写**。
> 用户点进详情页就会把该篇同步准，列表也随之更新。

启动全量 sync 会写 `fs_synced_at`，因此**首次启动完成后「未同步」集合应为空**（可直接当迁移验收项，
见第九节）。迁移刚完成、后台 sync 尚未跑完的窗口里，老行会短暂显示「文件状态未同步」，
而不是给出一个错误的结论。

## 四、筛选与排序 API 变更（核心）

### 4.1 REST：删 `status_filter`，加文件事实筛选 + 健康筛选

```diff
 GET /assist/api/documents
-  &status_filter=uploaded,markdown_done
+  &md_state=present             # present / absent / unknown（可逗号多选）
+  &pdf_state=absent             # 已同步且确定没有 PDF
+  &synced=1                     # 只列出已同步过的
+  &has_md=1                     # 语法糖：md_state=present
+  &pdf_size_min=1048576         # 补齐既有前端语法（见 4.4）
+  &pdf_health=warning,error     # 健康筛选（unchecked = 未检查）
```

`ListQuery`（`documents.rs:316`）新增字段，全部映射为 **SQL 表达式白名单**（禁止拼接原始列名）：

| 参数 | SQL 片段 | 说明 |
|---|---|---|
| `md_state` / `pdf_state` / `zip_state` | `present` → `COALESCE(md_size,0) > 0`；`absent` → `fs_synced_at IS NOT NULL AND NOT (COALESCE(md_size,0) > 0)`；`unknown` → `fs_synced_at IS NULL` | 逗号多选 = OR |
| `synced` | `1` → `fs_synced_at IS NOT NULL`；`0` → `IS NULL` | 正交参数，可与任意 state 组合 |
| `has_md` / `has_pdf` / `has_zip` | `1` → `COALESCE(x,0) > 0`；`0` → `NOT (COALESCE(x,0) > 0)`（**含未同步**） | 语法糖；要表达「确定不存在」用 `md_state=absent` |
| `pdf_size_min` / `pdf_size_max` | `COALESCE(pdf_size,0) >= ?` / `<= ?` | 未同步/不存在按 0 计 |
| `md_size_min` / `md_size_max`、`zip_size_*` | 同上 | |
| `pdf_health` | `ok/warning/error/missing` → `pdf_health IN (…)`；`unchecked` → `pdf_health IS NULL` | 与 state 正交，可组合 |

布尔类参数接受 `1/0`、`true/false`、`yes/no`（非法值 400）；**同类内 OR，跨类 AND**。

### 4.2 排序

- **删除** `sort_by=status`（`SORTABLE_COLUMNS` 里的 `("status","status")` 一行，`documents.rs:547`）。
- **新增** `md_size` / `pdf_size` / `zip_size` 可排序（同样走白名单）。
- 默认排序仍是 `created_at DESC`。

### 4.3 批量选择与阶段守卫（不再 stat，直接 SQL）

| 场景 | 新条件 |
|---|---|
| 批量解析（`pipeline.rs:1426`、`:1838`） | `COALESCE(md_size,0) = 0 AND COALESCE(pdf_size,0) > 0`，再并上运行时错误重试集 |
| 批量提取（`pipeline.rs:1454`） | `COALESCE(md_size,0) > 0` |
| 批量索引（`pipeline.rs:1482`、`:1838`） | `COALESCE(md_size,0) > 0 AND id NOT IN (已 indexed 的)` |
| 单篇 `parse` 守卫（`pipeline.rs:2069`） | `md_size = 0/空` 且 `pdf_size > 0` |
| 单篇 `extract` / `index` 守卫（`:2100` / `:2158`） | `md_size > 0` |
| `batch_status` / `admin` 统计 / MCP `stats` | 直接用 SQL 聚合（`SUM(COALESCE(md_size,0)>0)`），**零 stat** |

> 「选文档去跑阶段」这类**决策路径**要求列必须准；由 3.2 的写时同步 + 3.4 的启动全量 sync 保证。
> 若某篇的列确实过期，最坏结果是「白跑一次解析」（`do_parse_impl` 已有「PDF 不存在就跳过」的守卫），
> 不会损坏数据。

> **未同步的行不会被批量选中**：`fs_synced_at IS NULL` 时 `pdf_size` / `md_size` 也必为 NULL，
> 天然不满足 `pdf_size > 0`。它们要等启动全量 sync / 详情页 / 手工 sync 之后才进入候选集，
> 避免拿「不知道」去跑阶段。

### 4.4 搜索语法（`frontend/src/utils/searchParser.ts`）

| 现状 | 处置 |
|---|---|
| `{ prefix: 'status', field: 'status', type: 'status' }` + `STATUS_ALIASES`（上传/解析中/完成/错误） | **删除**；`?q=status:done` 不再被解析 |
| `filesize_pdf` / `filesize_md` / `filesize_zip`（`field: pdf_size` 等） | **保留并让它真正生效** —— 后端 `ListQuery` 目前**根本没有这些参数**（axum 静默忽略未知 query），这是「`search-filesize-plan.md` 提到但未实现」的缺口，本次顺手补齐（4.1） |
| — | **新增** 三态前缀：`md:present` / `md:absent` / `md:unknown`（同样 `pdf:` / `zip:`），映射到 `md_state` / `pdf_state` / `zip_state` |
| — | **新增** 健康前缀：`health:error` / `health:unchecked` → `pdf_health`（可逗号多选） |

### 4.5 MCP 工具：直接删 `status` 参数

`mcp_server.rs` 的 `list_documents` / `search_documents` **删除 `status` 入参、schema 字段与
`append_list_filters()`（`:842`）里的 `AND status IN (…)` 拼接**，不做翻译层、不做弃用期。
改为与 REST 一致的文件事实参数（同名同义）：

| 新参数 | 取值 |
|---|---|
| `md_state` / `pdf_state` / `zip_state` | `present` / `absent` / `unknown`（逗号多选） |
| `synced` | `0` / `1` |
| `pdf_health` | `ok` / `warning` / `error` / `missing` / `unchecked`（逗号多选） |
| `has_md` / `has_pdf` / `has_zip` | `0` / `1` |

工具描述（`mcp_server.rs:113` / `:81` 的 `description`）里的「filter by status」文案同步改写，避免模型继续传旧参数；`sort_by` 描述里的 `status` 也删，`append_order_clause`（`:370`）排序白名单一并移除。

### 4.6 MCP 严格参数校验：未知参数直接报错

旧客户端不会“默默失去筛选”，而是在 `tools/call` 就拿到明确错误。

**实现位置**：`mcp_server.rs:189` `call_tool()` **最前面**（`match name` 之前），对全部 9 个工具生效：

```rust
/// 白名单来源 = 该工具 inputSchema.properties 的键（单一真相，不会漂移）
fn reject_unknown_args(name: &str, args: &Value) -> Result<(), Value>
```

- 只拒绝 `args` **声明了但 schema 里没有**的键；`args` 不是对象、或某键值为 `null` 时**跳过**
  （部分客户端会把所有字段都填上 `null`，不应误伤）。
- 错误体沿用现有形状（`isError: true` + `Self::err("invalid_argument", …)`）：

```json
{ "isError": true, "content": [{"type": "text", "text":
  "未知参数 `status`。可用参数：query, doc_type, page, page_size, limit, fields, sort_by, sort_order。
   状态筛选已移除：请改用 md_state / pdf_state / zip_state（present|absent|unknown）、
   synced（0|1）、pdf_health（ok|warning|error|missing|unchecked）" }] }
```

- **错误文案必须带可用参数清单**：`call_tool()` 同时被 `routers/chat.rs` 的工具循环复用，
  错误会回给模型 → 模型能自我纠正（否则只会反复重试同一个旧参数）。
- 保留一个 `REMOVED_ARGS` 小表，仅用于**生成有针对性的提示**（不是兼容层，不会让旧参数生效）：
  `status` → 指向 `md_state` / `synced` / `pdf_health`；`reset_status` → 指向 `pipeline reset`。
- **前置条件（重要）**：契约必须以**实现真正读取的键**为准核对一遍 9 个工具，
  否则会把正在使用的参数误判为未知。已核对的漂移点：`list_documents` 的 `limit` 已在 schema 里（安全）；
  `search_info` 的 `offset` / `max_results` / `year_from` / `year_to` 均已在 schema 里。

## 五、后端改造清单

### 5.1 新增与改造的端点

| 位置 | 内容 |
|---|---|
| `services/file_state.rs`（新） | `DocState` / `derive_state()` / 运行时注册表（`LAST_ERROR`、活动任务读取） |
| `routers/admin.rs`（新） | `GET /assist/api/admin/file-state/reconcile?fix=0`：报告「同步列 vs 实时 stat」的差异清单；`fix=1` 时对差异文档调用 `sync_doc_file_sizes`（取代人工改状态） |
| `routers/pipeline.rs:2250` `reset`（改造） | 「清空解析产物」：删 `markdown_path` + `asset_path`、`image_cache::invalidate`、清运行时错误与 `mineru_task_id`，**最后调用 `sync_doc_file_sizes`**；`target_status` 参数删除（D7） |

### 5.2 逐点改动

| 文件:行 | 现状 | 改法 |
|---|---|---|
| `models.rs:110` | `Document.status` | 删除；同时删 `status_message`（`:111`）与 `progress`（`:112`），删 `Document::doc_status()`（`:126`）；**保留** `DocStatus` 枚举 |
| `models.rs` | — | 新增 `DocState`（可放 `services/file_state.rs`） |
| `database.rs:103` | `status TEXT DEFAULT 'uploaded'` | 建表语句删三列、**加 `fs_synced_at TEXT`**；迁移见第六节；**新增索引** `idx_documents_md_size` / `idx_documents_pdf_size` / `idx_documents_fs_synced_at` / `idx_documents_pdf_health` |
| `documents.rs:439` `sync_doc_file_sizes` | 只写三个 size + `updated_at` | 额外写 `fs_synced_at = now`（3.2 第 1 项）|
| `documents.rs:1865` 一带 BibTeX 导入 `INSERT` | 不 stat | 插入后调一次 `sync_doc_file_sizes`（三个 size 得 NULL + 时间戳 = 全部 `absent`） |
| `documents.rs:409` `DOC_COLUMNS` | 含 `status, status_message, progress` | 去掉三列；**同步改** `pipeline.rs` 与 `mcp_server.rs:23` 的另外两份 |
| `documents.rs:316` `ListQuery` | `status_filter` | 删 `status_filter`；加 `md_state` / `pdf_state` / `zip_state` / `synced` / `pdf_health` / `has_*` + 6 个 size 边界（4.1） |
| `documents.rs:455` `doc_to_out` | `status` 取列、`*_size` 现 stat | `status/message/progress/synced` 走 `derive_state`；`*_size` **改为返回列值**；新增 `fs_synced_at` / `has_md` / `has_pdf` / `has_zip` |
| `documents.rs:602` | `status IN (…)` 条件 | 删除，换成 4.1 的表达式白名单 |
| `documents.rs:542` `SORTABLE_COLUMNS` | 含 `("status","status")` | 删该行；加 `md_size` / `pdf_size` / `zip_size` |
| `documents.rs:2225` / `:2319` | 搜索结果补充 `status` | 派生后填入 |
| `documents.rs:712` `get_document` | 走 `doc_to_out` | 详情页：先 `stat` 一次并回写同步列（3.4 第 4 层），再 `doc_to_out` |
| `documents.rs:1328` / `:1485` | `Document { status: "uploaded".into(), … }` | 删三个字段 |
| `documents.rs:2439` `persist_pdf_for_doc` | `reset_status` 决定是否 `status='uploaded'` | **参数与 SQL 分支整体删除**；`on_duplicate=replace` 不再重置解析阶段（D7）；`POST /documents/:id/pdf` 收到该字段时忽略并 `warn` |
| `documents.rs:2979` `upload_parse_result` | `:3006` 把 `uploaded/parsing/error` 升级为 `markdown_done` | 整段删除（写 MD + `sync_doc_file_sizes` 后状态自然成立） |
| `pipeline.rs:216` `update_doc_status` | 写 DB | 改为 `set_runtime_state()`：只写内存（+ sidecar），不碰 DB |
| `pipeline.rs:243` `count_status` | `SELECT COUNT(*) WHERE status = ?` | 改为基于同步列的聚合 SQL |
| `pipeline.rs:331-472` `do_parse_impl` | 十几次 `update_doc_status` | 全换 `track_task_*` + 失败写错误集合；成功路径不写 DB |
| `pipeline.rs:479` `finish_parse_result` | `UPDATE … status='markdown_done', progress=100` | 只搬文件 + `sync_doc_file_sizes` + bump `updated_at` |
| `pipeline.rs:1426` / `:1454` / `:1482` / `:1838` | 批量选择按 status | 4.3 的 SQL 条件 |
| `pipeline.rs:1726` `batch_status` | 5 条 `COUNT(*)` | 同步列聚合 SQL（保持单次请求 1 条 SQL，零 stat） |
| `pipeline.rs:1936` / `:1982` | 按 md 是否存在猜目标状态 | 只清运行时错误记录 |
| `pipeline.rs:2028` `batch_promote_ready` | `status='markdown_done' AND title/authors 非空` | `md_size>0 AND title/authors 非空` |
| `pipeline.rs:2069` / `:2100` / `:2158` / `:2318` | 阶段守卫 | 4.3 |
| `pipeline.rs:2211` `doc_status` | 读 `doc.status` | 派生；响应字段名保持不变（`status` / `status_message` / `progress` / `has_markdown` / `is_indexed`） |
| `pipeline.rs:2346` `stop` | `UPDATE … status='error'` | 置运行时 `cancelled` + `track_task_end` |
| `admin.rs:410` | 按 status 循环计数 | 同步列聚合 SQL；`pdf_health_counts` / `pdf_health_unchecked` 保持不变 |
| `admin.rs:1084` `sync-file-sizes?only_missing=1` | 判定「任一 size 为 NULL」 | 改为 `fs_synced_at IS NULL`（真正的未同步），见 3.2 |
| `admin.rs:1110` / `:1130` | `StatusFixBody` / `fix_document_status` | **整体删除**（含路由注册 `admin.rs:62` 两行），不做 410 |
| `mcp_server.rs:23` / `:281` / `:539` / `:736` / `:842` / `:113` | `DOC_COLUMNS` / 输出 / 统计 / `status` 筛选与工具描述 | 派生 + 4.5（删 `status` 入参与 schema，换文件事实参数） |
| `mcp_server.rs:189` `call_tool` | 无参数校验 | 新增 `reject_unknown_args()`（白名单来自 `inputSchema.properties`），放在 `match name` 之前（4.6） |
| `mcp_server.rs:370` `append_order_clause` | 排序白名单含 `status` | 删 `status`，保留 `title` / `authors` / `year` / `id` / `journal` / `doc_type` / `created_at` / `updated_at` / `md_size` / `pdf_size` |
| `main.rs`（启动处） | — | 启动后 `tokio::spawn` 一次后台全量 sync（3.4 第 2 层），打 `info!` 记录耗时与条数 |

### 5.3 `updated_at` 语义补偿（容易漏）

以前「状态变化」会顺带 bump `updated_at`，现在改为在**文件落盘成功时显式 bump**：
`finish_parse_result`、`upload_parse_result`、`replace-markdown`、`pipeline reset`。
`sync_doc_file_sizes` 本身已在 `UPDATE` 里写 `updated_at`，多数路径无需额外处理 —— 逐个确认即可。

## 六、数据库迁移

`sqlx 0.7` → `libsqlite3-sys 0.27.0` bundled SQLite 为 3.45.x，`ALTER TABLE … DROP COLUMN`（3.35+）可用；
`status` 上无索引、无视图、无触发器，可安全删除。

在 `database.rs::init()` 的「轻量迁移」区追加（幂等，复用现成的 `has_column()`）：

```rust
// 1) 备份一个版本，便于回滚（D5）
if Self::has_column(&self.pool, "documents", "status").await? {
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS documents_status_legacy AS \
         SELECT id, status, status_message, progress, datetime('now') AS archived_at FROM documents",
    ).execute(&self.pool).await?;
}

// 2) 逐列删除
for column in ["status", "status_message", "progress"] {
    if Self::has_column(&self.pool, "documents", column).await? {
        sqlx::query(&format!("ALTER TABLE documents DROP COLUMN {}", column))
            .execute(&self.pool).await?;
        tracing::info!("数据库迁移：documents 表删除列 {}", column);
    }
}

// 3) 新增三态时间戳列（旧行为 NULL = 未同步，启动全量 sync 会补齐）
if !Self::has_column(&self.pool, "documents", "fs_synced_at").await? {
    sqlx::query("ALTER TABLE documents ADD COLUMN fs_synced_at TEXT")
        .execute(&self.pool).await?;
    tracing::info!("数据库迁移：documents 表新增列 fs_synced_at（全部旧行 = 未同步）");
}

// 4) 文件事实列建索引（新筛选/排序/only_missing 都会用到）
for ddl in [
    "CREATE INDEX IF NOT EXISTS idx_documents_md_size ON documents(md_size)",
    "CREATE INDEX IF NOT EXISTS idx_documents_pdf_size ON documents(pdf_size)",
    "CREATE INDEX IF NOT EXISTS idx_documents_fs_synced_at ON documents(fs_synced_at)",
    "CREATE INDEX IF NOT EXISTS idx_documents_pdf_health ON documents(pdf_health)",
] {
    sqlx::query(ddl).execute(&self.pool).await?;
}
```

- 建表语句（新库）同步删三列、加 `fs_synced_at`、补四个索引。
- **`fs_synced_at` 不需要回填**：保持 NULL = 「未同步」，由启动全量 sync（3.4 第 2 层）自动变成真实值。
  这样迁移与陈旧数据修复走同一条路径，不需要一次性写脚本。
- 启动时打印 `SELECT sqlite_version()`；`< 3.35` 时跳过删列并 `warn`（代码已不读写这三列，属安全降级）。
- 回滚：从 `documents_status_legacy` 重建列（`ALTER TABLE ADD COLUMN` + `UPDATE … FROM`），再回滚代码。

## 七、前端改造

| 文件:行 | 改动 |
|---|---|
| `types/document.ts:5` / `:28` | `DocStatus` 四值**不变**；`Document` 保留 `status` / `status_message`（派生值），新增 `synced: boolean` / `fs_synced_at` / `has_md` / `has_pdf` / `has_zip`；`pdf_health` 早已存在 |
| `api/documents.ts:23` / `:26` | 删 `status_filter`；加 `md_state` / `pdf_state` / `zip_state` / `synced` / `pdf_health` / `has_*` + size 边界；`sort_by` 去掉 `'status'`、加 `'md_size'` / `'pdf_size'` |
| `utils/searchParser.ts:50-56` | 删 `status` 前缀与 `STATUS_ALIASES`；`filesize_*` 保留（后端本次补齐）；新增 `md:` / `pdf:` / `zip:`（`present/absent/unknown`）与 `health:` 前缀 |
| `components/QueryBuilder.vue` | 「状态」筛选项替换为两组：①「文件」多选：有 PDF / 有 Markdown / 有资源包 / **确定缺 PDF** / **确定缺 Markdown** / **未同步**；②「PDF 健康」多选：`ok` / `warning` / `error` / `missing` / `未检查` |
| `views/HomeView.vue:352` / `:369` | 筛选 chips 与排序标签同步（`状态: …` → `文件: 有 Markdown` / `健康: error`） |
| `views/DocManageView.vue:165-175` | 「设置状态」下拉**删除**；替换为 `清空解析产物（删除 Markdown + 资源包）` → `POST /pipeline/reset/{id}`（**二次确认**）与 `清除错误记录`；「文件大小」区增加「同步于 `<fs_synced_at>` / 未同步」与「立即同步」按钮（调 `sync-file-sizes` 单篇或详情 `?refresh=1`） |
| `api/pipeline.ts:59-62` / `:118` / `:127` | `resetDocument(id, target)` 去掉 `target_status`，改名 `clearParseOutput(id)` |
| `components/StatusBadge.vue` | 不变；新增 `synced === false` 时显示灰色「未同步」小标（比误导性的「PDF 缺失」准确） |
| `views/DetailView.vue` / `AdminView.vue` | 零改动（`status` / `status_counts` 字段名不变） |
| `stores/pipeline.ts:43` | 3 秒轮询保持 —— 后端 `batch_status` 已改为单条聚合 SQL，零 stat |

## 八、分阶段 TODO

每阶段可独立合并、可单独回滚。

### P0 —— 只读并行（不改现有行为）

- [ ] `services/file_state.rs`：`DocState`（含 `synced`）/ `derive_state()`（输入为 `fs_synced_at` + size 列 + 运行时）
- [ ] `database.rs`：加 `fs_synced_at` 列 + 四个索引（第六节第 3/4 步，先加列再删列）
- [ ] `sync_doc_file_sizes` 写 `fs_synced_at`；BibTeX 导入落库后调一次
- [ ] `DocumentOut` 增加 `derived_status` / `derived_message` / `synced` / `fs_synced_at` / `has_md` / `has_pdf` / `has_zip`（与旧 `status` **并存**）
- [ ] `doc_to_out` 双写这两套字段（`*_size` 仍先保留现算，便于对比）
- [ ] `GET /admin/file-state/reconcile`：输出「同步列 vs 实时 stat」差异清单（分「未同步 / 不一致 / 一致」三桶）→ 迁移基线

### P1 —— 切换读路径

- [ ] `doc_to_out` 的 `*_size` 改为返回列值（与筛选同源，3.3）
- [ ] `ListQuery` 删 `status_filter`；加 `md_state` / `pdf_state` / `zip_state` / `synced` / `pdf_health` / `has_*` 与 size 边界；`SORTABLE_COLUMNS` 调整
- [ ] 前端：类型、QueryBuilder（文件 + 健康两组）、searchParser、HomeView chips、`sort_by` 选项
- [ ] `batch_status` / `admin` 统计 / MCP `stats` 改为同步列聚合 SQL
- [ ] `pipeline.rs` 批量选择与守卫改为 4.3 的 SQL 条件
- [ ] MCP `list_documents` / `search_documents`：删 `status` 入参 + schema + `append_list_filters` 对应拼接 + 工具描述，换文件事实参数（4.5）
- [ ] MCP `call_tool()` 新增 `reject_unknown_args()`（白名单从 `inputSchema.properties` 取）+ 未知参数错误文案（4.6）
- [ ] 核对 9 个工具「实现读取的键 ⊆ schema 声明的键」，不一致的补齐 schema

### P2 —— 切换写路径

- [ ] `update_doc_status` → `set_runtime_state`（内存 + sidecar，D1）
- [ ] `system.json` 新增 `runtime_state_path` + `config_manager.rs` 默认值
- [ ] `do_parse_impl` / `finish_parse_result` / `stop` / `reset` / `upload_parse_result` /
      `persist_pdf_for_doc` 改造；补 `updated_at` bump（5.3）
- [ ] **删除 `reset_status`**（D7）：`persist_pdf_for_doc` 参数与 SQL 分支、
      `on_duplicate=replace` 的重置语义、`/documents/:id/pdf` 的字段解析（收到则忽略 + `warn`）
- [ ] `pipeline reset` 改造为「清空解析产物」并调用 `sync_doc_file_sizes`；删 `target_status`
- [ ] `fix_document_status` / `StatusFixBody` / 两条路由注册**整体删除**（D6）
- [ ] `admin.rs:1084` `only_missing` 判定改为 `fs_synced_at IS NULL`
- [ ] `main.rs` 启动后后台全量 sync（3.4 第 2 层）；日志输出「同步 N 条，耗时 X ms」
- [ ] 回归：解析 → 提取 → 索引全流程；进程重启后状态正确回落

### P3 —— 删列与收尾

- [ ] `database.rs` 删列迁移 + 建表语句 + 四个索引（第六节）
- [ ] `models.rs` 删三字段与 `doc_status()`；`DOC_COLUMNS` ×3 收尾
- [ ] `DocumentOut` 去掉并存的旧字段（`derived_status` → `status`）
- [ ] 文档同步：`AGENTS.md`（易错点 12 的加列清单、`fs_synced_at` 三态、状态机段落、`only_missing` 语义、`/admin/documents/status` 段落）、
      `document/search.md`（`status:` → `md:` / `health:` / `filesize_*`）、`document/mcp-tools.md`（status 来源）

## 九、验收矩阵

| 项目 | 手段 |
|---|---|
| 派生正确性 | `GET /admin/file-state/reconcile` 无差异；详情页 stat 回写后列表同步更新 |
| **三态：未同步** | 迁移后立即查询（启动 sync 前）：老行 `synced=false`、页面显示「文件状态未同步」；sync 跑完后 `GET /documents?synced=0` 应返回 0 条 |
| **三态：确定不存在** | `GET /documents?md_state=absent` 只包含已同步且无 MD 的文档；`?md_state=unknown` 只包含未同步的；两个结果互斥且并集 = 全库 |
| **健康筛选** | `GET /documents?pdf_health=error`、`?pdf_health=unchecked` 的 `total` 与 `GET /admin` 的 `pdf_health_counts` / `pdf_health_unchecked` 一致 |
| 文件事实筛选 | `has_md=1`（含未同步的保守判定）与 `md_state=present` 对比差异应只来自未同步行 |
| size 筛选真正生效 | `filesize_md:>0` 从「静默无效」变为生效（P1 回归点） |
| 列表排序 | `sort_by=md_size&sort_order=desc` 正确；`sort_by=status` 被忽略（回退默认） |
| 列表零 stat | P1 后 `strace -c -e trace=stat,newfstatat` 抓一次 `GET /documents`：列表不再有 N 次 stat（仅详情页有） |
| 解析流程 | `POST /pipeline/parse/{id}` → 轮询 `GET /pipeline/status/{id}` 看到 `parsing` → 落 `markdown_done` |
| 重启语义 | 解析中 `Ctrl-C` 重启 → 文档不是永久 `error`，回到「待解析」（且 `synced` 仍为 true，不误判为未同步） |
| 替换 PDF | `POST /documents/{id}/pdf`（已解析文档）后状态仍是 `markdown_done`；带 `reset_status=1` 也**不再**改状态（仅 `warn`）；`on_duplicate=replace` 同样不重置 |
| 清空解析产物 | `POST /pipeline/reset/{id}` 后 MD/ZIP 被删、`image_cache` 失效、`md_size` 归零且 `fs_synced_at` 更新（状态变 `uploaded` + `PDF 缺失` 或 `待解析`） |
| 带外改动 | 手工往 `data/markdowns/` 放一个 `.md` → 列表仍显示「待解析」；访问详情页后变「解析完成」并写回（3.4 第 4 层） |
| 启动 sync | 杀进程 → 带外删几个 PDF → 重启 → 后台 sync 完成日志出现，列表 `has_pdf` 更新 |
| 批量 | `batch/start-parse` / `start-extract` / `start-index` 选中集合与文件事实一致，`batch/status` 计数自洽；**未同步行不会被选中** |
| MCP | `list_documents`（`md_state` / `synced` / `pdf_health`）、`stats` 与 REST 一致；**传旧 `status` 参数 → `isError: true` + 可用参数清单**（4.6） |
| MCP 严格校验 | `tools/call` 传任意未声明参数（如 `{"name":"list_documents","arguments":{"status":"markdown_done"}}`）→ 返回 `isError`，不是空结果；传 schema 已声明的全部参数（含 `null` 值）→ 正常返回 |
| MCP chat 循环 | 在「问答」里让模型查库，旧参数报错后模型能改用新参数重试（错误文案含参数清单） |
| 迁移 | 旧库启动后 `.schema documents` 无三列、有 `fs_synced_at`；`documents_status_legacy` 有全量旧值 |
| 编译 | `cargo check`（嵌入式设备上只做检查，不编译） |

## 十、风险清单

1. **同步列过期 = 状态过期**：这是 v2 的固有取舍。兜底是「写路径同步 + 启动全量 sync + admin 手动 + 详情页实时回写」四层；
   最坏后果是「白跑一次解析」（`do_parse_impl` 有 PDF 缺失守卫），不会损坏数据。
2. **三态不能倒退成二态**：`fs_synced_at` 一旦漏写（新增写路径忘记调 `sync_doc_file_sizes`），
   文档会长时间停在 `unknown`，表现为「永远显示未同步」。**新增任何会动 PDF/MD/ZIP 的写路径时必须同步调用 sync**，
   并可用 `GET /documents?synced=0` 作为常驻哨兵查询。
3. **`only_missing` 语义变更**：现在=「任一 size 为 NULL」，改后=「未同步」。老库里绝大多数行 size 为 NULL（文件不存在），
   若不同步改判定，运维点一次「补充缺失」会把全库重新 stat 一遍（不致命但很慢）。`AGENTS.md` 已文档化该参数，需同步更新。
4. **列表与磁盘不再逐次一致**：`doc_to_out` 改回列值后，列表显示的是「上次同步时」的大小。
   若用户希望列表也实时，需在 `ListQuery` 加 `?live=1`（对当前页 ≤500 行做 stat），**默认关闭**。
5. **破坏性接口变更（本项目有意为之）**：MCP 工具 schema 直接删 `status`（不保留弃用期）；REST 删 `status_filter`。
   - MCP 侧：旧参数会被 **4.6 的严格校验显式拒绝**（`isError` + 替代参数提示），不会静默失去筛选；
   - REST 侧：axum 仍会静默忽略未知 query（改成报错需要自定义 extractor，本次不做），
     外部脚本传 `status_filter` 会变成「不过滤」→ 必须在发布说明里点名。
6. **严格校验会放大 schema/实现漂移的代价**：一旦某工具实现读取了 schema 未声明的键，该参数会被当成未知而报错。
   因此 P1 必须逐个核对 9 个工具的「实现读取键 ⊆ schema 声明键」（已预先核对两处风险点，见 4.6）。
7. **`progress` 丢失**：进度条只在 `parsing` 时展示，`parsing` 一律来自运行时任务 → 对齐；进程重启后进度归零（与现状一致）。
8. **`DROP COLUMN` 不可逆**：靠 `documents_status_legacy`（D5）+ 分阶段合并兜底。
9. **单实例假设**：运行时状态与 session / 锁一样在进程内存，不支持多实例部署（与本仓库现状一致）。
10. **重解析通道变唯一（D7 副作用）**：删除 `reset_status` 后，重解析只能走 `POST /pipeline/reset/{id}`
    （破坏性删 MD + 资源包），**不能**靠替换 PDF 间接重置；UI 必须二次确认，运维手册要写清楚。
11. **迁移窗口的观感**：首启动时全库为「未同步」，后台 sync 跑完前列表会出现一批「文件状态未同步」。
    这是准确的（而不是错的），但需在发布说明里提一句；库小时（几千篇）几秒内就会结束。

## 十一、工作量粗估

| 阶段 | 内容 | 估算 |
|---|---|---|
| P0 | `file_state.rs`（含三态）+ `fs_synced_at` 列 + 双写 + reconcile 基线 | 0.5–1 天 |
| P1 | 读路径（列表/三态筛选/健康筛选/排序/统计/批量/MCP 含严格参数校验）+ 前端筛选 UI | 2–2.5 天 |
| P2 | 写路径（运行时注册表 + sidecar + reset 语义 + 启动 sync + `only_missing` 语义） | 1.5–2 天 |
| P3 | 删列迁移 + 索引 + 类型收尾 + 文档同步 | 0.5–1 天 |
| 回归 | 全流程 curl 验收 + 一次真实库迁移演练（含三态与健康筛选） | 0.5–1 天 |

---

## 附录：落地情况（实现记录）

### 实际改动（与计划的对应关系）

| 计划位置 | 落地实现 |
|---|---|
| 3.1 三态 | `documents.fs_synced_at TEXT`（NULL = 未同步），`sync_doc_file_sizes()` 一并写入 |
| 3.2 同步点 | 除原有 10 处外，新增：BibTeX 导入落库（直接写 `fs_synced_at`，三个 size 为 NULL = 全部 absent）、`pipeline reset` 删文件后、`upload_parse_result` |
| 3.3 派生 | `services/file_state.rs`：`DocState` / `RuntimeTask` / `derive_state()`（7 条规则，含「未同步」分支） |
| 3.4 新鲜度 | `main.rs` 启动后 `tokio::spawn` 全量 sync；`GET /documents/:id` 实时 stat 并回写 |
| 4.1 REST 筛选 | `ListQuery` 删 `status_filter`，新增 `md_state`/`pdf_state`/`zip_state`/`synced`/`has_*`/`pdf_health`/6 个 size 边界；非法取值 400 |
| 4.2 排序 | `SORTABLE_COLUMNS` 删 `status`，加 `pdf_size`/`md_size`/`zip_size`/`fs_synced_at` |
| 4.3 批量/守卫 | 批量选择改为 `COALESCE(md_size,0)` / `COALESCE(pdf_size,0)` 条件；单篇门禁（`parse`/`extract`/`index`/`full`）改为**纯文件事实**判断（`has_md()` / `has_pdf()` + `runtime_task(id)` 是否已有任务），不再依赖派生状态；`stage_counts()` 单条聚合 SQL |
| 4.4 搜索语法 | `searchParser.ts` 删 `status:` + `STATUS_ALIASES`，加 `md:`/`pdf:`/`zip:`/`health:`；`filesize_*` 现在真正生效 |
| 4.5 MCP 删参 | `list_documents` schema 删 `status`，加三态/健康参数；`append_list_filters()` 重写 |
| 4.6 MCP 严格校验 | `McpServer::reject_unknown_args()`：白名单取自 `list_tools()` 的 `inputSchema.properties`，`null` 值放行，错误文案含可用参数 + `status` 替代提示 |
| 5.1 端点 | `GET /assist/api/admin/file-state/reconcile?fix=`（替代被删的 `documents/status`）；`pipeline reset` 改为「清空解析产物」 |
| 5.2 逐点 | `models.rs` 删三字段；`database.rs` 建表 + 备份表 + 删列 + 4 个索引；`doc_to_out` 零 stat；`admin`/`mcp` 统计改聚合 |
| D6 | `fix_document_status` / `StatusFixBody` / 两条路由 **直接删除**（实测返回 404） |
| D7 | `persist_pdf_for_doc` 的 `reset_status` 参数与 SQL 分支删除；收到该 multipart 字段时忽略并 `warn` |
| 七、前端 | `types/document.ts`、`api/documents.ts`、`api/pipeline.ts`（`clearParseOutput`）、`searchParser.ts`、`QueryBuilder.vue`（文件事实 + PDF 健康两组）、`DocManageView.vue`（「设置状态」→「文件事实」+「清空解析产物」）、`StatusBadge.vue`（`synced === false` 显示「未同步」） |

### 与计划的偏差

1. **REST 侧未做严格参数校验**：未知 query（如 `status_filter`）仍被 axum 静默忽略（表现为「不过滤」）；只有 MCP 侧会显式 `isError`。若需要 REST 也报错，需自定义 extractor。
2. **reset 端点沿用既有路径**：`POST /assist/api/pipeline/reset/:id` —— 本组端点的约定就是 `动词/:id`（`parse/:id`、`extract/:id`、`index/:id`、`stop/:id`、`full/:id`、`status/:id`），`reset/:id` 与之一致；计划早期写的 `/{id}/reset` 是笔误，已按代码修正，**不改路由**。
3. **`progress` 语义**：`DocState.progress` 由运行时任务给出（页数比或百分比），非运行时为 100/0。
4. **`filesize_*` 的 `NULL` 处理**：size 边界条件用 `COALESCE(x,0)` + `CAST(? AS INTEGER)`（字符串绑定需显式转型，否则 SQLite 按存储类比较会永远为假）。

### 验证结果

| 项 | 结果 |
|---|---|
| `cargo build` | 通过（57 条既有 warning，无 error） |
| `cargo test` | 165 passed / 1 failed，失败项为既有 Windows 路径分隔符问题（`static_files::tests::解析默认值`），与本次改动无关 |
| `pnpm run build` | 通过 |
| 迁移（真实库 3838 篇的副本） | 备份表 3838 行；`status` / `status_message` / `progress` 三列删除成功；`fs_synced_at` 新增 |
| 启动全量同步 | 3838 篇，17.9 s（Windows / 数据文件缺失时最坏情况） |
| 三态筛选 | `md_state=present/absent/unknown`、`synced=0/1`、`has_md` 互斥正确；同步后 `synced=0` 为 0 条 |
| 健康筛选 | `pdf_health=unchecked` / `error` 与 `/admin/stats` 的 `pdf_health_counts` 一致 |
| 详情实时回写 | 手工新建 `data/markdowns/1.md` 后访问详情 → `markdown_done` + `has_md=true` + `md_size=29` |
| `pipeline reset` | 删除 MD、`md_size` 归零、状态回落 `uploaded`（`待解析`） |
| 上传 / BibTeX 导入 | 上传 → `uploaded`+`has_pdf`；BibTeX → `uploaded`+`PDF 缺失`+`synced=true`（不是「未同步」） |
| `parse-result` | 写 MD 后自动 `markdown_done`（无需状态字段） |
| 阶段守卫 | 无 MD 时 `index` / `extract` 返回 400；有 MD 时 200 |
| MCP | 旧 `status` 参数 → `isError: true` + 可用参数清单 + 替代建议；`status: null` 放行；新参数正常返回 |
| 已删端点 | `POST /assist/api/admin/documents/status` → 404 |
