# okb MCP 使用反馈与改进建议（含落地进度）

> 提交方：终端用户（科研写作 / 文献综述场景）
> 涉及工具：`list_documents`、`search_info`、`get_document_abstract`、`get_document_info`、`grep_search`、`read_markdown`、`get_toc`、`get_stats`
> 实测任务：从 7 540 条文献中筛出 **233 篇 IEEE T-RO**，对 92 篇有 Markdown 的全文做平台/建模关键词统计，产出 6 万字调研报告
> 首次提交：2026-11-18 ｜ 最近更新：2026-11-18（P0 三项已实现，见各节 ✅ 与文末 §10）

## 进度总览

| 编号 | 事项 | 状态 | 落地位置 |
|---|---|---|---|
| §1 | `grep_search` 返回 match 级结果 | ✅ 已完成 | `services/grep_search.rs`、`mcp_server.rs`、`routers/documents.rs` |
| §2 | `list_documents` 期刊/年份过滤 + 分面统计 | ✅ 已完成 | `mcp_server.rs`（`list_documents`/`get_facets`）、`routers/documents.rs`（`/stats/facets`） |
| §3 | 批量/导出 Markdown + MCP 令牌只读 REST | 🟡 已完成主体（异步导出任务未做） | `mcp_server.rs`（`read_markdown_batch`）、`routers/documents.rs`（`/markdown/*`）、`main.rs`（只读白名单） |
| §4 | 正则语义文档化 + `whole_word`/`case_sensitive` | 🟡 参数与文档已补；`fields` 白名单仍未文档化 | `mcp-tools.md` |
| §5 | 元数据/文本质量（期刊归一化、`title_raw`、图片 base、References 标记） | ⬜ 未做 | 入库/解析层 |
| §6 | 混合检索、引用图、`section_ranges` | ⬜ 未做 | 规划 |

---

## 0. 先说好的部分

以下能力在本次任务中表现很好，建议保持：

1. `get_document_abstract` 稳定、快（233 次调用总耗时约 2 秒），中英文摘要齐全。
2. `read_markdown` 的 `section`/`sections` 按标题抽取非常实用，省掉大量解析工作。
3. `list_documents` 的 `md_state` / `pdf_state` / `pdf_health` 文件事实过滤，让我能准确知道“哪些文献有全文”，而不是反复试错。
4. `get_document_info` 返回 `temp_downloadable_pdf_url`（免鉴权、带有效期）这个设计很务实。
5. MCP 本身稳定，233+92 次连续调用无失败、无限流。

下面按“影响任务完成度”排序提建议。

---

## 1. P0：`grep_search` 的返回粒度不足以支撑结构化统计

### 1.1 现象

实测调用：

```json
{"query":"Franka|KUKA|UR5|Baxter|PR2|ANYmal|Crazyflie|Husky|TurtleBot|Pioneer 3",
 "doc_ids":"<233 个 T-RO id>", "limit":5, "max_context_chars":200, "context":0}
```

返回的第 2 条：

```json
{"id":1295,
 "content":"Robot navigation can also be framed as a satisfiability problem. The goal is to find a policy ...",
 "truncated":true}
```

**这条 content 里根本不含任何被查询的词**——片段取自命中行附近的窗口，但没有保证覆盖命中词本身，也没有给出偏移/行号。于是：

- 无法知道 1295 究竟命中了 `PR2` 还是 `Baxter`（实际是正文里一句 "simulation with a PR2 robot"）；
- 无法知道命中了多少次；
- 无法区分“实验段使用该平台”和“参考文献里出现同名作者”。

对我的任务（“每篇论文用了哪些机器人平台”）来说，这个返回**在信息量上等价于布尔值**，却又要付出一次分页往返。

### 1.2 建议

**（a）返回 match 级结果，而不是 doc 级结果。**

```jsonc
// 请求
{"query":"Franka|KUKA|Panda", "mode":"match",          // match | doc
 "doc_ids":"96,1562,7369", "context_lines":1,
 "max_matches_per_doc":5, "include_sentence":true,
 "strip_references":true, "strip_author_bios":true}

// 响应（字段示意，行号/偏移仅作占位）
{"total_docs":3, "total_matches":11,
 "results":[{"doc_id":96, "title":"...", "year":2023,
   "matches":[{"line":412, "offset":18342,
               "sentence":"two Panda arms with two-finger gripper end-effectors are stacking six boxes",
               "section":"VI. Experiments",
               "matched":"Panda"}]}]}
```

关键字段：`matched`（实际命中的文本）、`offset/line`、`sentence`（命中词所在的完整句子）、`section`（最近的 Markdown 标题）。

**（b）提供 `match_count` 与 `count_only` 模式。**
我需要“某词在 92 篇中出现的篇数”这类统计；现在只能把每篇全文拉下来自己数。若支持 `count_only:true`，服务端直接返回 `{doc_id: count}` 映射，能省掉 8 MB 传输。

**（c）支持按小节排除。**
很多误命中来自 `## References` 和作者简介（见 §1.3）。建议默认 `strip_references:true`，或在返回里标注 `section` 让客户端自己过滤。

**（d）分页语义说明。**
当前是 `last_id` 游标、按 doc ID 升序；请明确：**同一文档有多个命中时是否只返回一条？** 如果是，请改成“每文档多命中”或用 `match_count` 补齐，否则“命中次数”这类统计无法在服务端完成。

### 1.3 真实误命中案例（说明为什么需要 `section`/`sentence`）

| 查询词 | 文档 | 实际命中内容 | 结论 |
|---|---|---|---|
| `Allegro` | 7502 | “Hall-effect current sensors (ACS714, **Allegro** Microsystems Inc.)” | 传感器厂商，非机器人 |
| `Barrett` | 3240/1073/174 | 参考文献 “C. R. Wylie and L. R. **Barrett**, Advanced Engineering Mathematics” | 作者名 |
| `Pioneer` | 7502 | “SPIE Nanoengineering **Pioneer** Award in 2011” | 奖项名 |
| `Digit` | 多处 | `Digital` 的前缀匹配 | 子串误命中 |
| `ROV` | 多处 | `prove` / `provide` 的子串 | 子串误命中 |

其中 `Digit`/`ROV` 这类是正则边界问题（我这边已用 `(?<![A-Za-z])...(?![A-Za-z])` 修复），但 `Allegro`/`Barrett`/`Pioneer` 只有看到完整句子才能判断。**如果 grep 返回 `sentence`，这类任务就不需要下载全文。**

### 1.4 ✅ 已完成（2026-11-18）

**实现**：`backend-rs/src/services/grep_search.rs` 重写为 v2（`GrepQuery` + `MatchMode`），MCP 与 REST 两个入口同步升级。

**每文档返回**：

| 字段 | 说明 |
|---|---|
| `match_count` | 该文档内**全部**命中数（不受 `max_matches_per_doc` 截断） |
| `matches[].line` | 1-indexed 行号 |
| `matches[].column` | 该行内 1-indexed **字符**列号 |
| `matches[].matched` | 实际命中的文本（可判断是哪个别名命中） |
| `matches[].sentence` | 命中所在完整句子（`include_sentence=false` 时不返回） |
| `matches[].section` | 命中位置上方最近的 Markdown 标题 |
| `first_line` / `content` | 兼容旧字段（合并后的上下文块） |
| `total_matches` | 外层汇总（本页所有文档的 `match_count` 之和） |

**新增参数**：`mode=doc|match`、`max_matches_per_doc`（默认 5）、`include_sentence`（默认 true）、`strip_references`（默认 false）、`count_only`、`whole_word`、`case_sensitive`。

**参考文献识别**：`References` / `References and Notes` / `VI. References` / `6 References` / `8: Bibliography` / `参考文献`；并避免把 `Reference Frames and Notation` 这类正文小节误判为参考文献。

**与原建议的差异**：

- 未实现 `strip_author_bios`：作者简介没有稳定的小节结构，改为返回 `section` + `sentence`，由客户端判断；
- 用 `line/column` 而非全局 `offset`：Markdown 没有稳定页/字符坐标，行号更可靠；
- 分页语义明确：仍按 doc ID 升序游标；`limit` 是**文档数**上限，同一文档的多条命中在 `matches[]` 中返回。

**测试/文档**：`services::grep_search::tests` 6 个用例（句子切片、参考文献识别、词边界、截断计数）；`frontend/public/docs/mcp-tools.md` 已更新。

---

## 2. P0：缺少“按期刊/年份的枚举与聚合”能力

### 2.1 现象

`list_documents` 没有 `journal` 过滤参数，只有 `query`（标题/作者模糊匹配）和 `doc_type`。为了拿到 233 篇 T-RO，我把全库 7 540 条按 `page_size=100` 翻了 **76 页**，在客户端按 `journal` 字段过滤。

`search_info` 同样没有 journal 过滤；`grep_search` 有 `journal` 参数（很好），但它只能返回“命中的文档”，不能返回“该期刊的全部文档”。

### 2.2 建议

1. `list_documents` 增加 `journal`（支持精确/前缀/模糊）与 `year_from/year_to`；
2. 增加**分面统计**端点，直接回答“按期刊分布”“按年份分布”：

```jsonc
// GET /stats/facets?field=journal&limit=100
{"field":"journal","total":7540,
 "buckets":[{"value":"IEEE TRANSACTIONS ON ROBOTICS","count":233}, ...]}

// GET /stats/facets?field=year&q=journal:"IEEE TRANSACTIONS ON ROBOTICS"
{"buckets":[{"value":2026,"count":20}, ...]}
```

3. `page_size` 上限从 100 放宽到 500–1000，或提供游标；7 540 条翻 76 页对客户端和服务端都是浪费。

> 收益：本次任务第 1 步（枚举期刊文献）可从 76 次调用降为 1–2 次。

### 2.3 ✅ 已完成（2026-11-18）

- `list_documents` 新增 `journal`（默认不区分大小写子串）、`journal_exact`、`year_from`、`year_to`；
- `page_size` 上限 100 → **500**（REST 侧原本就是 500）；
- `search_info` 补 `journal`；年份过滤统一为 `CAST(NULLIF(TRIM(year),'') AS INTEGER)`，顺带修掉 `year=''` 的老库记录被静默丢弃的问题；
- 新 MCP 工具 **`get_facets`**：`field` ∈ `journal|year|doc_type|source|language|pdf_health|category`，支持 `query`/`journal`/`year_from`/`year_to`/`limit`；
- 新 REST 端点 **`GET /assist/api/stats/facets?field=journal&limit=100`**（MCP 令牌可只读访问）。

**说明**：`year` 分桶的 `value` 是字符串；`total` 是**返回分桶**的计数之和（`limit` 截断时小于全库）。

**未做**：期刊名归一化 / `issn` 字段（§5，属 P1）；按相关度排序（仍按 doc ID）。

---

## 3. P0：全文批量获取缺少“批量/导出”通道

### 3.1 现象

- `read_markdown` 只能单篇调用；92 篇需要 92 次调用（我按 10 篇一批并发，约 2.4 秒、8.2 MB，尚可接受，但规模再大就不行）。
- `get_document_info` 里的 `https://xsro20.xyz/assist/markdown/{id}` 是**前端页面**（返回 HTML），不是原始 Markdown；`/assist/api/documents/{id}/markdown` 返回 `401 mcp_only_token`——MCP 令牌不能访问 REST API。

### 3.2 建议

1. **允许同一令牌访问只读 REST**（或发一个 `read:api` scope 的配套 token），至少开放：
   - `GET /api/documents/{id}/markdown`（`text/markdown`）
   - `GET /api/documents/{id}/markdown?raw=1`
2. **批量接口**：

```jsonc
// MCP: read_markdown_batch
{"ids":[96,1562,7369], "line_count":20000, "format":"text"}
// 或 REST
// GET /api/markdown/export?journal=...&year_from=2020&format=ndjson.gz
// 每行: {"id":96,"title":"...","year":2023,"markdown":"..."}
```

3. **异步导出任务**：语料上万篇时，返回 `job_id` + 轮询/下载链接，避免长连接超时。
4. 明确“哪些 id 被跳过”：`doc_ids` 里如果有未解析 Markdown 的文档，目前是静默跳过；请返回 `skipped:[{id, reason:"markdown_not_generated"}]`，或提供“触发解析”的接口（现在只能提示用户“先去前端 parse”）。

> 收益：92 篇从 92 次调用变为 1 次导出；也避免用“下载全文再检索”这种笨办法。

### 3.3 ✅ 已完成（2026-11-18，异步导出任务除外）

- **MCP `read_markdown_batch`**：`ids`、`full`、`line_count`、`raw`、`include_hash`、`budget_chars`（默认 1,000,000）；
  返回 `items[]`（`id/total_lines/content/truncated/hash?`）+ `skipped[{id,reason}]` + `used_chars`；
  超预算的文档返回 `truncated=true` 与单篇读取提示，不再静默截断。
- **REST 批量**：`GET /assist/api/markdown/batch?ids=1,2,3&full=1&raw=1&include_hash=1`；
  `POST /assist/api/markdown/batch`（body 传 ids，按“读语义 POST”放行 MCP 令牌）。
- **REST 导出**：`GET /assist/api/markdown/export?journal=...&year_from=...&ids=...&limit=...`，
  流式返回 `application/x-ndjson`，每行 `{id,title,year,journal,lines,content}`。
- **MCP 令牌只读放行**（`main.rs` 的 `is_mcp_readonly`）：
  `GET/HEAD` → `/assist/api/documents*`、`/assist/api/markdown/*`、`/assist/api/stats/*`；
  `POST` → 仅 `/assist/api/markdown/batch`；其余写操作仍返回 `401 {"code":"mcp_only_token"}`（文案已说明只读范围）。
- **备注**：`/assist/api/documents/{id}/markdown` 本身一直存在，本次只是解开了 MCP 令牌的 401。

**未做**：

- 异步导出任务（`job_id` + 轮询）——上万篇时仍可能撞反向代理超时；
- `format=ndjson.gz` 服务端压缩（当前依赖反向代理/客户端 gzip）。

---

## 4. P1：`grep_search` 的正则语义与能力边界需要文档化

我实测到的行为，建议写进工具描述：

| 项 | 现状（实测/推测） | 建议 |
|---|---|---|
| 正则引擎 | 未说明（PCRE/Python re/RE2？） | 明确引擎与不支持的特性（lookbehind、反向引用等） |
| 大小写 | 默认大小写不敏感？ | 给出 `case_sensitive` 参数并说明默认值 |
| 词边界 | 无 `\b` 说明；`Digit` 命中 `Digital` 取决于引擎 | 提供 `whole_word` 便捷参数 |
| 多行匹配 | 未说明 | 明确 `.` 是否跨行、如何匹配跨行段落 |
| 结果排序 | 按 doc ID 升序 | 建议支持按相关度/年份；至少文档化 |
| 截断 | `max_context_chars` 默认 500，可能截掉命中词 | 保证窗口以命中词为中心（见 §1.2） |

顺带：`search_info` / `list_documents` 的 `fields` 白名单没有文档；我传 `['id','has_md']` 时不确定是否生效。请给出可用字段列表（`id/title/authors/year/journal/doi/doc_type/has_md/...`）。

**🟡 进展（2026-11-18）**：`case_sensitive`、`whole_word` 已实现并在 `mcp-tools.md` 中补了参数说明；`grep_search` 返回 `matched/sentence/section` 后，“为什么命中”已可判断。
**仍未做**：正则引擎/多行匹配的精确语义说明；`fields` 白名单清单；按相关度排序。

---

## 5. P1：元数据与文本质量

1. **期刊名不统一**：同一本刊出现 `IEEE TRANSACTIONS ON ROBOTICS` 与 `IEEE Transactions on Robotics`（大小写/全大写混用），还有缩写 `IEEE Trans. Robot.`。建议入库时归一化，并额外提供 `journal_abbrev`/`issn` 字段，便于精确过滤。
2. **Markdown 覆盖率偏低**：T-RO 仅 92/233（39%）；全库 4 326/7 540（57%）。建议：
   - 在 `get_stats` 里按期刊给出 `md_state` 分布；
   - 提供“按期刊批量触发解析”的管理接口；
   - 解析失败时返回失败原因（而非仅 `markdown_not_generated`）。
3. **标题中的 LaTeX 需无损保留**：如 `3-R $\underline{\hbox{P}}$R`。我这边因为做全文档 `}}`→`}` 的清洗，一度把它改坏；如果服务端在入库时就做有损清洗，客户端将无法恢复。建议明确清洗规则，或保留 `title_raw` 字段。
4. **图片相对路径**：正文是 `![](images/xxx.jpg)`，客户端无法解析。建议提供 `asset_base_url`，或把图片作为资源（`resources/list`）挂出来。
5. **参考文献段落请显式标记**：如果能输出 `<section id="references">` 或统一的 `## References` 标题，§1.2 的“排除引用误命中”就能自动化。
6. **`get_document_abstract` 返回的是 JSON 字符串**（`{"abstract":..., "keywords":...}` 被塞进 `text`），建议直接用 MCP structured content 返回 `abstract/keywords/language` 三个字段；现在客户端要二次 `JSON.parse`，且不同文档结构不一致（有的 `abstract` 为 `null`）。

**⬜ 状态（2026-11-18）**：以上 6 条均未改动。其中第 5 条（References 标记）在 §1 的实现里做了**启发式**兜底：`grep_search` 现在能识别常见参考文献标题，但 Markdown 本身仍没有显式小节标记。

---

## 6. P2：搜索/检索体验

1. **语义检索缺失**：`search_info` 是元数据模糊匹配，`grep_search` 是字面正则。若要做“找讨论接触力建模的论文”，只能自己拼正则。建议提供可选的向量/混合检索（哪怕是可选的 embedding 索引）。
2. **`grep_search` 结果没有“为什么命中”**：命中多个词时无法知道命中了哪几个（见 §1）。建议返回 `matched_terms:["Panda","KUKA"]`。
3. **缺少“相似文献/引用关系”**：调研场景很需要“这篇引用了哪些库内文献”“哪些库内文献引用了它”。若 PDF 已解析，参考文献抽取 + `cites` 图会非常有价值。
4. **`get_toc` 很好用，但建议补充 `section_ranges`**（起止行），这样客户端可以直接按小节切片，不必二次解析。

**⬜ 状态（2026-11-18）**：以上 4 条均未改动，留作后续规划。

---

## 7. 优先级汇总

| 优先级 | 建议 | 状态 | 影响 |
|---|---|---|---|
| P0 | `grep_search` 返回 match 级结果（`matched/line/column/sentence/section`） | ✅ 已完成 | 无需下载全文即可做结构化统计；直接消除 §1.3 类误判 |
| P0 | `list_documents` 支持 `journal` 过滤 + `/stats/facets` | ✅ 已完成 | 期刊枚举从 76 次调用降到 1 次 |
| P0 | 只读 REST + 批量/导出 Markdown（或 `read_markdown_batch`） | 🟡 主体完成（异步导出未做） | 92 次调用 → 1 次；支撑大规模综述 |
| P1 | 正则语义文档化 + `whole_word`/`case_sensitive` | 🟡 参数/文档完成，语义说明未补齐 | 减少客户端试错 |
| P1 | 期刊名归一化、`title_raw`、`asset_base_url`、References 标记 | ⬜ 未做 | 数据可信度与可复现性 |
| P1 | `get_document_abstract` 结构化返回 | ⬜ 未做 | 去掉客户端 `JSON.parse` 与字段不一致 |
| P2 | 混合检索、引用图、`section_ranges` | ⬜ 未做 | 提升调研效率 |

---

## 8. 附：本次任务的实测数据（供评估收益）

| 动作 | 调用次数 | 耗时 | 数据量 |
|---|---:|---:|---:|
| 全库枚举（7 540 条，`page_size=100`） | 76 | 43 s | ~1 MB 元数据 |
| 拉取 233 篇摘要 | 233 | 2 s | 0.36 MB |
| 拉取 92 篇全文（10 篇/批并发） | 10（含 92 次子调用） | 2.4 s | 8.2 MB |
| 全部 233 次 `grep_search` 等价枚举（估算，若不用全文） | 40–80 | — | 每次 10–20 条 doc 级结果，无法计数 |

结论很清楚：**改造前 `grep_search` 的信息密度不足以替代“取回全文本地扫描”**，而全文获取又缺少批量通道。§1、§2、§3 三项落地后（见 §10），同类综述任务的调用次数可从 **约 320 次降到 3–5 次**：

```text
get_facets(field="journal")                                  # 1 次，拿到期刊规范名
list_documents(journal="…", page_size=500)                   # 1 次，枚举该刊全部文献
grep_search(query="…", strip_references=true, mode="match")  # 按需分页，直接拿 matched/section
read_markdown_batch(ids=[…]) 或 /assist/api/markdown/export   # 1 次，替代 92 次单篇拉取
```

---

## 9. 复现脚本（客户端侧已做的兜底）

本次任务中客户端为绕开上述限制写的脚本，可直接作为接口设计的参考实现：

- `document/TRO/extract.py`：平台/类别/建模关键词三套正则，含 `(?<![A-Za-z])...(?![A-Za-z])` 词边界与误命中过滤；
- `document/TRO/build_doc.py`：从 JSON 生成带统计表格的 Markdown 报告；
- `document/TRO/TRO-robot-models-survey.md`：最终产出（233 篇逐篇分类 + 32 个具名平台 + 92 篇全文关键词统计）。

~~如需要，我可以把上述脚本改写成“如果 okb 支持 §1.2 的 match 级返回，客户端代码可以减少约 70%”的对照版本。~~
→ 2026-11-18：§1 的 match 级返回已实现，上述“下载全文再正则”的兜底可逐步退役（保留作离线复算/审计）。

---

## 10. 实现记录与验收（2026-11-18）

### 10.1 改动清单

| 文件 | 内容 |
|---|---|
| `backend-rs/src/services/grep_search.rs` | 重写为 v2（`GrepQuery`/`MatchMode`，match 级结果、句子/小节、参考文献过滤、词边界、只计数） |
| `backend-rs/src/mcp_server.rs` | `grep_search` 新参数与返回；`list_documents` 期刊/年份过滤、`page_size≤500`；`search_info` 加 `journal`；新增 `get_facets`、`read_markdown_batch` |
| `backend-rs/src/routers/documents.rs` | REST `grep-search` 新参数；新增 `/assist/api/stats/facets`、`/assist/api/markdown/batch`（GET/POST）、`/assist/api/markdown/export`（NDJSON 流） |
| `backend-rs/src/main.rs` | `is_mcp_readonly`：MCP 令牌可读 `documents*` / `markdown/*` / `stats/*`，写操作仍拒绝 |
| `backend-rs/Cargo.toml` / `Cargo.lock` | 新增直接依赖 `tokio-stream`（导出流式响应） |
| `frontend/public/docs/mcp-tools.md` | 新参数、`matches[]` 字段、`get_facets`、`read_markdown_batch`、MCP 令牌只读 API 一节 |

### 10.2 冒烟验证（独立临时实例，独立 SQLite）

| 用例 | 结果 |
|---|---|
| `grep_search(strip_references=1)` | `match_count=1`，`## VI. References` 内的同名命中被排除，`section` 正确 |
| `grep_search(mode=match)` | 返回 `sentence`（完整句）与 `column` |
| `grep_search(count_only=1)` | 只返回 `{id, match_count}`，`total_matches` 正确 |
| `grep_search(whole_word=1, regex=0)` | `Digit` 不再命中 `Digital` |
| `get_facets(field=journal)` | `IEEE Transactions on Robotics: 3`、`Automatica: 1` |
| `list_documents(journal=Automatica)` | 只返回 1 篇，`total=1` |
| `read_markdown_batch(budget_chars=200)` | 首篇完整、次篇 `truncated=true`，`skipped` 正确 |
| `GET/POST /assist/api/markdown/batch`（MCP 令牌） | 200，返回 `items/hash` |
| `GET /assist/api/markdown/export?journal=...`（MCP 令牌） | 200，`application/x-ndjson`，行数正确 |
| `PUT /assist/api/documents/{id}/markdown`（MCP 令牌） | 401 `mcp_only_token`（符合预期） |
| `tools/list` | 含 `get_facets`、`read_markdown_batch` |

### 10.3 测试

- `cargo test`：**216 passed / 1 failed / 1 ignored**；
- 失败项为 `static_files::tests::解析默认值`（Windows 路径分隔符 `\` vs `/`，本次未改动该文件，属既有平台相关问题）；
- 新增用例：`services::grep_search::tests`（6 个）、`mcp_readonly_tests`（1 个）。

### 10.4 剩余待办

1. 异步导出任务（`job_id` + 轮询/下载，避免长连接超时）＋ 服务端 gzip；
2. `fields` 白名单文档化；正则多行/引擎语义说明；
3. 期刊名归一化（含 `journal_abbrev`/`issn`）；
4. `get_document_abstract` 结构化返回（`abstract/keywords/language`）；
5. `get_toc` 的 `section_ranges`；引用关系图；
6. Markdown 覆盖率与失败原因（`get_stats` 按期刊给 `md_state` 分布）。
