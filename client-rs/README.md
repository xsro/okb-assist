# OKB-Assist 客户端工具

命令行工具，向 OKB-Assist 上传文献，并同步个人网盘（`zotero` / `dir` / `sync` 三个子命令）。

## 配置文件（`~/.okb/client.json`）

默认从 `~/.okb/client.json` 读服务地址与令牌，不必每条命令都写 `--base-url` / `--token`：

```json
{
  "url": "http://192.168.1.162:5001",
  "token": "okb_xxxxxxxx",
  "storage_root": "D:\\Zotero\\storage"
}
```

- `url`（也接受 `base_url`）服务地址，缺省 `http://192.168.1.162:5001`；
- `token` 令牌，优先级：`--token` > `OKB_ASSIST_TOKEN` 环境变量 > `client.json` > 旧版 `~/.okb_assist_token`；
- `storage_root` 仅 `zotero` 子命令使用，可被 `--storage-root` 覆盖。

文件不存在或字段缺失时用命令行参数/内置默认值；JSON 写错只打一条警告，不阻断运行。

**索引缓存**：每次成功拉到服务端的 doi-hash 全量索引，都会把原始响应原样写入
`~/.okb/client/docs.json`（原子写，先写 `.tmp` 再 rename），方便离线查看或被其他工具复用。
内容就是接口返回的 `{count, with_doi, with_hash, generated_at, documents:[...]}`（4 千篇左右约 700 KB）。

**DOI → 哈希记录**：客户端算过/传过的文件哈希会按 DOI 记到 `~/.okb/client/doi-hash.json`
（原子写：先写 `doi-hash.json.tmp` 再 rename）。文件内容是 JSON 对象，key 为归一化小写 DOI，value 为 sha256 hex：

```json
{
  "10.1000/xyz123": "9f2c…（64 位）"
}
```

- 写入时机：`zotero` 流程真正算出本地哈希时（`--dup-doi-action hash|health-hash`），
  以及 `dir` 流程上传成功时（DOI 优先取服务端回传值、没有则用本地从 Info 字典提取的值；哈希优先取服务端算的 `file_hash`）；
- 读取时机：只有加 `--reuse-hash`（目前仅 `zotero` 子命令）才读，命中就直接复用记录里的哈希，
  **完全不读本地 PDF**，省掉几 GB 库每次重算的时间；未命中仍按原逻辑计算并记回文件。
- 注意：记录不含文件大小/mtime，**本地 PDF 被替换后旧记录会过期**（会误判为 hash 一致）。
  换过文件就删掉对应条目，或别加 `--reuse-hash`（默认行为就是重新计算）。

## 子命令

### `zotero` — Zotero CSV 导入

读取 Zotero 导出的 CSV 文件，与服务端比对 DOI，交互式上传。

比对前客户端与服务端都会归一化 DOI（去掉 `https://doi.org/` / `doi:` 前缀、尾部标点，比对时忽略大小写），
所以 Zotero 里的写法差异不会导致误判为「新建」。仍建议加 `--update-meta`，让 DOI 真正写入服务端库中
（否则下次比对只能靠文件哈希去重）。

比对方式是**一次拉全量索引**：请求 `GET /assist/api/documents/doi-hash-index`
拿到服务端全部文献的 `{id, doi, doi_key, file_hash, pdf_health}`，整库比对在本地完成，
不再按 DOI 逐条打 `by-doi`（旧实现几百条文献就是几百次往返）。只有该接口不可用时才回退到逐条查询，
`dir` 子命令同样优先用这份索引，失败才回退到逐条 `by-doi` / 逐文件 `diff-hashes`。

**本地哈希按需计算**（不再先把整库算完）：只有「服务器已有 PDF、且策略确实需要比对哈希」的条目
才在上传前读文件算 sha256，并当场打印哈希前 16 位与**单项耗时**：

| `--dup-doi-action` | 什么时候算本地哈希 |
|---|---|
| `skip`（默认） | 完全不读文件（DOI 已存在就跳过，哈希用不上） |
| `hash` | 服务器有 PDF 的条目才算 |
| `health-hash` | 服务器有 PDF 且 PDF 不健康（`warning`/`error`/`missing`）才算 |

服务器没有该 DOI（= 新建）或服务器没有 PDF 时都不算哈希，直接进入新建/更新分支。

**多个附件的取舍**：一个 Zotero 条目挂了多个 PDF 时，不再只按「最新修改时间」选（会把别篇论文当成本条目附件传上去），
而是按「文件名与标题/作者词的匹配数」选，并打印全部候选与得分：

```
⚠ x3sm5ek9 (10.1007/978-3-319-28664-8) 有 2 个 PDF 附件，按标题/作者匹配挑选（不再只看修改时间）:
    [匹配 0 词] 1-s2.0-S0016003223006038-main.pdf
    [匹配 6 词] Brogliato - 2016 - Nonsmooth Mechanics Models, Dynamics and Control.pdf
    -> 选用: Brogliato - 2016 - Nonsmooth Mechanics Models, Dynamics and Control.pdf
```

得分全是 0 时会回退到「最新修改时间」并提示可能选错。

**409 去重命中**：服务端按文件内容 sha256 / DOI 拦下时，客户端会打印库里已有文档的
`id` / DOI / 文件名、本条记录的 DOI 与文件，并分情况给建议：

- `kind=hash`：本地这份 PDF 的**内容**已在库中（登记成了另一篇）。常见原因是 Zotero 条目挂错附件，
  或库里那条记录装错了 PDF。多个附件时会列出全部候选，提示去 Zotero / CSV 里删掉错的；
- `kind=doi`：DOI 已存在，`--dup-doi-action hash` 可改走「比对哈希，不同则覆盖已有 PDF」。
- 特例：已有记录的 DOI 与我们上传的**一致**（说明上一次请求其实已在超时后落库）→ 直接按成功处理，
  不报错也不会新建重复记录。

**上传失败自动重试**：发送阶段报 `error sending request`（超时/断链）会自动重试一次；
仍失败时提示「该请求可能已在服务端完成，重跑同一命令核对」。实测慢链路或服务端卡顿时，
一条 2 MB 的上传曾耗时 6 分钟，旧版 `120s` 基线会先把请求掰断（表现就是 `error sending request`）。

```
okb-client zotero <csv_file> [选项]
```

选项：
- `--base-url <URL>`         服务地址（缺省取 `~/.okb/client.json` 的 `url`）
- `--token <TOKEN>`          访问令牌（缺省取 `~/.okb/client.json` 的 `token`）
- `--storage-root <PATH>`    Zotero storage 文件夹本地路径（缺省取 `~/.okb/client.json` 的 `storage_root`）
- `--dry-run`                试运行，不实际上传
- `--update-meta`            同时更新元数据和原始 CSV 信息（默认仅替换 PDF）
- `--dup-doi-action <MODE>`  DOI 已存在时的处理方式（默认 `skip`）：
  - `skip` 直接跳过；
  - `hash` 比对文件 sha256，不一致（或服务端没有 PDF）则覆盖已有 PDF；
  - `health-hash` 先看已有 PDF 的 `pdf_health`：`ok` **或从未检查过（`NULL`）** 都算健康 → 跳过；只有 `warning`/`error`/`missing` 才比对哈希，不一致则覆盖
- `--keys <KEY...>`         只处理这些 Zotero 条目：取 CSV 的 `Key` 列，可重复（`--keys A --keys B`）
  或用逗号分隔（`--keys A,B`），大小写不敏感。过滤在 DOI/附件判定之前执行，
  阶段一之前会报告「CSV 里找不到的 key」与「命中但因缺 DOI/缺 PDF 被跳过的条目」
- `--debug`                  显示解析出的元数据详情
- `--reuse-hash`             复用 `~/.okb/client/doi-hash.json` 里记录的 DOI→哈希：命中则跳过重新读文件算哈希
  （需要比对哈希的策略才有意义：`--dup-doi-action hash` / `health-hash`）。文件没换过时才可靠
- `--upload-timeout <SECONDS>` 单次上传请求的超时（默认 `0` = 自动：`300s + 每 MB 30s`，上限 2 小时）。
  慢链路或服务端卡顿时会看到 `error sending request`，调大这个值即可
- `--extract-meta`            上传时让服务端做 PDF 元数据启发式抽取（默认**不开**，详见上方「PDF 元数据抽取」）
- `--no-progress`             不显示上传进度（默认实时刷新已发送 / 总大小、速度、剩余时间）
- `--upload-interval <SECONDS>` 每次上传之间的间隔秒数（默认 `1`；可写小数如 `0.5`，`0` 表示不等待）。
  新建上传与「更新 PDF」共用同一个计数，两次上传之间才等待，首个上传不等待
- `-y, --yes`                不询问：新建全部选 `a`、更新全部选 `a`、末尾「确认执行」自动通过，用于脚本 / 定时任务
  （与 `--dry-run` 同时使用时仍然不会上传）

示例：
```
okb-client zotero data\我的文库.csv --base-url http://192.168.1.162:5001 --token xxx
okb-client zotero data\我的文库.csv --storage-root D:\Zotero\storage
okb-client zotero data\我的文库.csv --update-meta   # 更新 PDF 同时更新元数据
okb-client zotero data\我的文库.csv --dup-doi-action hash        # DOI 相同但 PDF 不同则覆盖
okb-client zotero data\我的文库.csv --dup-doi-action health-hash # 只修复不健康的 PDF
okb-client zotero data\我的文库.csv --dup-doi-action hash --reuse-hash  # 复用上次算好的哈希，不重读 PDF
okb-client zotero data\我的文库.csv --keys NTRNTRLJ,URB2Q2D5   # 只处理这两条
okb-client zotero data\我的文库.csv -y             # 不询问，新建与更新全部直接执行
```

### `dir` — 目录 PDF 比对上传

扫描指定目录中的 PDF，**先从 PDF 的 Info 字典提取 DOI** 与服务端比对，提不到 DOI 的文件才计算 SHA256 按文件内容比对。
两份清单（需要上传 / 不需要上传）打印出来**等用户确认后才开始上传**。

流程图：扫描目录 → 拉一次服务端全量索引 → 逐个「读 Info 字典取 DOI → 比对（无 DOI 才算哈希）」→
分组打印待上传清单 → 确认（`y`/`s`/`n`）→ 逐个上传。

**比对规则**（按优先级）：

| 本地情况 | 服务端情况 | 结论 |
|---|---|---|
| Info 字典里有 DOI | 库里没有这个 DOI | 上传（新建记录） |
| Info 字典里有 DOI | 有这个 DOI，但条目**没有文件**（`file_hash` 为空或 `pdf_health = missing`） | 上传（补进那条已有记录，清单里标 `→ 补入 id=N`） |
| Info 字典里有 DOI | 有这个 DOI 且条目已带 PDF | 不上传（跳过时附上已有 `id` 与服务端哈希前 12 位） |
| 提不到 DOI（CNKI / ReaderEx 导出这类只写了 `Producer` 的） | 哈希已在库中 | 不上传 |
| 提不到 DOI | 哈希不在库中 | 上传（新建记录） |
| 提不到 DOI | 哈希比对请求失败 | 本次不动它，清单单独列「未能比对」，可重跑 |

- **需要上传的文件才计算哈希**：DOI 已经能定位到带 PDF 的条目就不读文件算 sha256，比旧版「逐个先算全目录哈希」快；
- DOI 取自 Info 字典（`lopdf::Document::load_metadata`，只解析文件结构与 Info 字典、**不抽取正文**），
  整本字典的字符串值都扫一遍；命中多个时按键名可信度取：**带 `article` 的 DOI 键 > 纯 `/doi` > 其他含 `doi` 的键
  （`/WPS-JOURNALDOI` 这类刊级 DOI）> 其余字段**，`(ISSN)` / `(ISBN)` 形态的刊级/书级标识直接跳过
  （WPS 导出的 PDF 常同时写 `/WPS-ARTICLEDOI` 与 `/WPS-JOURNALDOI`，后者会把同刊文章并成一条记录）；
  排序后再扫，所以同一个文件每次比对出的 DOI 都一样；
  扫描规则与服务端 `pdf_meta::scan_doi` 对齐（含「`10.3969/j. issn.…`」这种被空格折断的接续）；
  **同一个值两边算出来一样**，但字典里有多个 DOI 键时客户端可能和服务端不同 —— 服务端
  `doi_from_info_dict()` 按字典顺序取第一个命中（`custom` 是无序 map，可能拿到刊级 DOI），客户端按上面的键名排序取。
- 分组清单每组默认最多列 200 条，未列出的同样在编号序列里、确认后一样会上传；
- 确认提示：`y` 全部上传、`s` 逐个挑选（每个文件问一次 `y/N`）、其他任意输入 = 取消且不上传任何文件；
  提示单独占一行、输入在下一行（避免半截提示在某些终端里看不见）；
  **stdin 不是终端时不提问**（管道 / 重定向 / 后台执行），直接按取消处理并提示加 `-y`，不会卡在等待输入上；
  加 `-y` 就不会停在提示上，清单打印完直接全部上传（无人值守场景）；
- 服务端仍会做自己的哈希/DOI 去重（409），清单结论只是「要不要传」，不是最终判定；
  清单说「条目缺 PDF」但服务端回 409 `kind=doi` 时按「索引已过期」处理，计入「服务端去重」而不是失败；
- 哈希比对优先用一次性拉到的全量索引（本地比对），索引拿不到时回退为逐文件 `diff-hashes`、DOI 回退为逐条 `by-doi`；
  回退模式下单次比对请求失败会列入「未能比对」，不会默默传重复文件；
- `--dry-run` 走完提取 + 比对 + 打印清单，停在确认之前，不会上传。
- 上传成功后把「DOI → 哈希」写进 `~/.okb/client/doi-hash.json`（DOI 优先取服务端回传值、没有就用本地提取值），供 `zotero --reuse-hash` 复用。

```
okb-client dir <目录路径> [选项]
```

选项：
- `--base-url <URL>`         服务地址（缺省取 `~/.okb/client.json` 的 `url`）
- `--token <TOKEN>`          访问令牌（缺省取 `~/.okb/client.json` 的 `token`）
- `--dry-run`                提取 DOI、比对并打印清单，不上传
- `-r, --recursive`          递归搜索子目录
- `--process`                上传后触发解析流水线（默认不触发）
- `--upload-timeout <SECONDS>` 单次上传请求的超时（默认 `0` = 自动：`300s + 每 MB 30s`，上限 2 小时）
- `--extract-meta`            上传时让服务端做 PDF 元数据启发式抽取（默认**不开**，详见上方「PDF 元数据抽取」）
- `--no-progress`             不显示上传进度（默认实时刷新已发送 / 总大小、速度、剩余时间）
- `--upload-interval <SECONDS>` 每次上传之间的间隔秒数（默认 `1`；可写小数如 `0.5`，`0` 表示不等待），
  适合服务端限速或不想一次压满带宽的场景
- `-y, --yes`                不询问：清单照常打印，之后直接全部上传（等价于确认时按 `y`），用于脚本 / 定时任务

示例：
```
okb-client dir D:\Papers --base-url http://192.168.1.162:5001 -r
okb-client dir ./downloads --dry-run
okb-client dir D:\Papers -r --upload-interval 3   # 每次上传之间等 3 秒
okb-client dir D:\Papers -r --upload-interval 0   # 尽可能快（默认是 1 秒）
okb-client dir D:\Papers --dry-run                # 只提取 + 比对 + 打清单
okb-client dir D:\Papers -r -y                    # 不询问，清单里的全部直接上传
```

输出片段：

```
比对完成，用时 17.2 s。

==================================================
比对结果：8 个文件，4 个需要上传（4 个提取不到 DOI，改按文件哈希比对）

── 新建 —— 库中没有这个 DOI（4 个 · 17.3 MB）
  [  1] 10.1109/TASE.2026.3661266 · 6.0 MB · D:\Papers\1016.pdf
  [  2] 10.1109/TAC.2013.2286897 · 1.4 MB · D:\Papers\3472.pdf
  …
不需要上传（4）：同 DOI 条目已有 PDF 2 个 / 哈希已在库中 2 个

确认上传这 4 个文件？[y] 全部上传  [s] 逐个挑选  [n] 取消:
```

### `sync` —— 个人网盘同步

按 `~/.okb/client.json` 的 `sync` 段，在**本地目录**与**个人网盘**（`scope=private`）之间双向/单向同步。
这个子命令**没有任何参数**：服务地址、令牌、本地目录、网盘目录、同步方向全部来自配置文件。

```
okb-client sync
```

配置（`personal` 是个人网盘，`public` 是公有文件夹数组，可同时配多个目标）：

```json
{
  "url": "http://192.168.1.162:5001",
  "token": "okb_xxxxxxxx",
  "sync": {
    "personal": {
      "disk": "D:\\liuch586\\Sync\\okb-disk",
      "remote": "sync",
      "direction": "two-way"
    },
    "public": [
      { "folder": "团队资料", "disk": "D:\\liuch586\\Sync\\team", "direction": "download" },
      { "folder": "pf_1a2b3c4d", "disk": "D:\\liuch586\\Sync\\public-docs", "remote": "release" }
    ]
  }
}
```

- `sync.personal` 可以省略（只同步公有文件夹时）；
- `sync.public` 是数组，每项是一个公有文件夹的同步目标；`folder` 匹配**名称或 id**（名称不区分大小写），
  匹配不到会报错并列出当前账号可访问的公有文件夹；
- 公有文件夹是共享的，`direction` **缺省是 `download`（只读镜像）**；写 `upload` / `two-way` 才会上传，
  且要求账号对该文件夹有写权限 —— 只读文件夹即使写了 `two-way` 也会自动降级为单向下载并在表头提示。

| 字段 | 必填 | 说明 |
|---|---|---|
| `personal.disk`（或平铺的 `sync.local`） | 配了个人网盘就必填 | 本地目录；不存在会自动创建（只同步公有文件夹时可省略整个 `personal`） |
| `personal.remote`（或 `sync.remote`） | 否 | 个人网盘里的子目录，缺省 = 网盘根目录；`/a/b` 会自动建出来 |
| `personal.direction`（或 `sync.direction`） | 否 | `two-way`（默认，双向）/ `upload`（只写网盘）/ `download`（只写本地） |
| `personal.conflict`（或 `sync.conflict`） | 否 | 冲突策略：`newer`（默认，取较新一方）/ `strict`（**严格模式**：只报告、不改动文件） |
| `public[].folder` | 是 | 公有文件夹名称或 id（`pf_xxx`） |
| `public[].disk` | 是 | 该文件夹对应的本地目录 |
| `public[].remote` | 否 | 文件夹内的子目录（缺省 = 文件夹根） |
| `public[].direction` | 否 | **缺省 `download`**；写 `upload` / `two-way` 需有写权限，否则自动降级为 `download` |
| `public[].conflict` | 否 | 同 `personal.conflict` |

行为说明：

- **多目标**：每个目标（个人网盘 / 每个公有文件夹）各自独立比对与统计，表头列出各自的 scope、方向、冲突策略，
  末尾再给一份合计。
- **三方比对**：`~/.okb/client/sync-state.json` 记录上次同步完成时两侧的大小与 mtime。
  只有本地变 → 上传；只有网盘变 → 下载；都没变 → 跳过；**两侧都变 → 冲突**（见下）。
  首次同步（无记录）时**大小相同**的文件按已同步处理，大小不同则算「无法判断」（也按冲突处理）。
- **冲突策略**（`personal.conflict`）：
  - `newer`（默认）：比较「本地文件 mtime」与「网盘 `modified`（= 上传到网盘的时间）」，**较新的一方胜出**，
    覆盖另一侧并在终端打印 `⚠ 冲突 ... 采用较新的一方（本地/网盘）`。没有冲突副本，败方内容直接丢失。
  - `strict`（严格模式）：**只报告，不动任何一侧**，打印
    `⚠ 冲突（严格模式，未改动）a.txt: 两侧都改过｜本地 18 B，刚刚修改 · 网盘 22 B，刚刚上传`，
    并在汇总里写「严格模式：未改动，需人工处理」，最后以**非 0 退出码**结束（方便 cron/脚本发现问题）。
    这些文件的旧状态会被保留，所以**下次运行仍会继续报同一个冲突**，不会自己消失。
    处理办法（任选）：
    1. 对比两版内容后让两侧一致，再删掉 `~/.okb/client/sync-state.json`（或其中对应条目）跑一次 ——
       首次同步对「大小相同」的文件按已同步处理并重建基线；
    2. 临时把 `conflict` 改成 `newer` 跑一次，让较新的一方自动覆盖，之后再改回 `strict`。
  - 严格模式只拦「两侧都可能改过」这种无法判断的情况；**能确定方向的变更照常同步**（只有本地变就上传、
    只有网盘变就下载），所以不会把正常同步也一起停掉。
  - 单向同步（`upload` / `download`）没有冲突概念：始终以指定的一侧为准。
- **单向**：`upload` 以本地为准（网盘上多出来的文件不动），`download` 以网盘为准（本地多出来的不动）。
- 上传带 `overwrite=true`，同名直接覆盖（不会生成 `name (1).pdf`）；下载先写 `.<name>.okbsync-part` 再改名，不会留下半截文件。
- **不传播删除**：某一侧删掉的文件，下次同步会从另一侧补回（要删请两侧都删，或直接删网盘页面上的文件后不再同步）。
- 不碰符号链接，跳过 `Thumbs.db` / `desktop.ini` / `.DS_Store` 与 `*.okbsync-part`；空目录按方向补齐。
- 每个文件传输都带进度（`⬆` 上传 / `⬇` 下载，非 TTY 自动降为每 10% 一行）。
- 状态文件是 **v2 结构**，按目标分组：`{"version":2,"targets":{"private:sync":{…},"public:pf_xxx:":{…}}}`。
  旧版（v1，只有个人网盘一份记录）读取时会自动迁移成 `private:<remote>`，无需手动处理。
- 删除 `~/.okb/client/sync-state.json` 会让下次同步重新做全量比对（不会删文件，只是会重新看大小/mtime）。

输出示例：

```
网盘同步（sync）
  服务地址: http://192.168.1.162:5001
  同步目标: 2 个

── 个人网盘 ──
  本地目录: D:\liuch586\Sync\okb-disk
  网盘目录: /sync（scope=private）
  同步方向: 双向同步（本地 ⇄ 网盘）
  冲突策略: 取较新一方（自动处理）
[/]
  上传 notes.md（12.3 KB）
⬆ 已发送 12.3 KB / 12.3 KB（用时 21 ms，平均 585.7 KB/s）
  → 上传 1（12.3 KB）· 下载 0（0 B）· 跳过 8 · 冲突 0 · 失败 0

── 公有文件夹「团队资料」可写 ──
  本地目录: D:\liuch586\Sync\team
  网盘目录: /（scope=public:pf_1a2b3c4d）
  同步方向: 单向下载（网盘 → 本地，以网盘为准）
  冲突策略: 取较新一方（自动处理）
[/]
  下载 paper.pdf（3.4 MB）
⬇ 已下载 3.4 MB / 3.4 MB（用时 1.2 s，平均 2.8 MB/s）
  → 上传 0（0 B）· 下载 1（3.4 MB）· 跳过 3 · 冲突 0 · 失败 0

==================================================
上传:   1 个（12.3 KB）
下载:   1 个（3.4 MB）
跳过:   11 个（已同步 / 方向不适用）
失败:   0 个
用时:   1.3 s
==================================================
```

## 上传进度

每次真正传输文件（`zotero` / `dir` 子命令上传 PDF、`sync` 子命令上传/下载网盘文件）都会实时显示进度，
计量的是**从本地文件读出发送的字节数与文件总大小**（流式上传，不会为了显示进度把文件读进内存）：

```
        上传 ...
⬆  43% [███████░░░░░░░░░] 7.9 MB / 18.0 MB · 5.5 MB/s · 剩余约 1.8 s
⬆ 已发送 18.0 MB / 18.0 MB（用时 3.6 s，平均 5.0 MB/s）
        成功 -> id=4242
```

- 终端里（stderr 是 TTY）每 200ms 原地刷新一行：百分比、进度条、已发送 / 总大小、实时速度、预计剩余时间；
  字节发完后显示「传输完成，等待服务端处理…」（服务端还要查重 / 抽 PDF 元数据 / 落盘）；
- 重定向到文件或被管道接走时（非 TTY）自动降级为每 10% 打一行，日志里不会出现一堆回车符；
- 上传失败/中断时保留已发送量：`⬆ 上传中断：已发送 3.0 MB / 18.0 MB（16%），用时 17 ms`；
- `--no-progress` 完全关闭（`zotero` / `dir` 都支持）。

## PDF 元数据抽取（默认跳过）

上传时服务端可以按三种模式抽元数据（表单字段 `extract_meta`）：

- `full`：先从 PDF **Info 字典** 拿 DOI → Crossref 查权威元数据；Info 字典里没有 DOI 时退化为启发式抽取（题名 / 作者 / 年份 / 刊名 / 摘要）。启发式那一步要起几个 `mutool` 子进程，
  其中 `mutool draw -F text` 会把**整份文档**正文过一遍，大文件/扫描件能慢到几十秒——这正是上传停在「等待服务端处理」的主因。
- `light`：只读 Info 字典拿 DOI + Crossref，**不抽正文**（最便宜的“带元数据”模式）。
- `false`：完全跳过，记录只有文件名 + 哈希。

客户端两个子命令**默认跳过**（请求里带 `extract_meta=false`）：记录先建好（列表里显示文件名），不抽元数据。
想开完整抽取加 `--extract-meta`（= `extract_meta=full`）；已上传的记录可以事后补：

```
POST /assist/api/pipeline/extract-pdf-meta/:id     # 单篇重新抽取（文档管理页有按钮）
```

注意：跳过抽取后，新建记录只有文件名 + 文件哈希 + 客户端从 Info 字典带到表单里的 `doi`（`dir` 流程），题名/作者都是空；
`zotero` 流程加的 `--update-meta` 不受影响（元数据来自 CSV，与服务端启发式抽取无关）。

## Token 设置

按以下优先级读取：
1. `--token` 命令行参数
2. `OKB_ASSIST_TOKEN` 环境变量
3. `~/.okb/client.json` 的 `token` 字段
4. 旧版 `~/.okb_assist_token` 文件内容

## 构建

```
cargo build --release
```