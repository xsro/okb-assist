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
  以及 `dir` 流程上传成功、服务端返回了 `doi` 时（取服务端算的 `file_hash`）；
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
`dir` 子命令同样优先用这份索引，失败才回退到 `diff-hashes`。

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

示例：
```
okb-client zotero data\我的文库.csv --base-url http://192.168.1.162:5001 --token xxx
okb-client zotero data\我的文库.csv --storage-root D:\Zotero\storage
okb-client zotero data\我的文库.csv --update-meta   # 更新 PDF 同时更新元数据
okb-client zotero data\我的文库.csv --dup-doi-action hash        # DOI 相同但 PDF 不同则覆盖
okb-client zotero data\我的文库.csv --dup-doi-action health-hash # 只修复不健康的 PDF
okb-client zotero data\我的文库.csv --dup-doi-action hash --reuse-hash  # 复用上次算好的哈希，不重读 PDF
okb-client zotero data\我的文库.csv --keys NTRNTRLJ,URB2Q2D5   # 只处理这两条
```

### `dir` — 目录 PDF 批量比对上传

扫描指定目录中的 PDF 文件，**上传前逐个**计算 SHA256 哈希、与服务端比对，服务器已有的文件自动跳过，
其余交互式上传。

流程图：扫描目录 → 拉一次服务端哈希索引 → 列出全部本地 PDF → 选择（`a`/`s`/`q`）→
逐个「`[i/N]` 计算哈希 → 比对 → 上传（或跳过）」。

- 不会先把整个目录的哈希算完再上传：算一个、比一个、传一个，进度与上传实时可见；
- 哈希比对优先用一次性拉到的全量索引（本地比对），索引拿不到时回退为逐文件 `diff-hashes`；
  回退模式下单次比对请求失败会打印「本文件未上传（可重试）」，不会默默传重复文件；
- `--dry-run` 仍会算哈希并比对，输出会列出「服务器已存在，跳过」与「服务器没有该文件（dry-run）」。
- 上传成功且服务端返回了 DOI 时，会把「DOI → 哈希」写进 `~/.okb/client/doi-hash.json`（供 `zotero --reuse-hash` 复用）。
  `dir` 流程在算哈希前并不知道 DOI，所以这里**只记录、不复用**。

```
okb-client dir <目录路径> [选项]
```

选项：
- `--base-url <URL>`         服务地址（缺省取 `~/.okb/client.json` 的 `url`）
- `--token <TOKEN>`          访问令牌（缺省取 `~/.okb/client.json` 的 `token`）
- `--dry-run`                试运行，不实际上传
- `-r, --recursive`          递归搜索子目录
- `--process`                上传后触发解析流水线（默认不触发）
- `--upload-timeout <SECONDS>` 单次上传请求的超时（默认 `0` = 自动：`300s + 每 MB 30s`，上限 2 小时）
- `--extract-meta`            上传时让服务端做 PDF 元数据启发式抽取（默认**不开**，详见上方「PDF 元数据抽取」）
- `--no-progress`             不显示上传进度（默认实时刷新已发送 / 总大小、速度、剩余时间）
- `--upload-interval <SECONDS>` 每次上传之间的间隔秒数（默认 `1`；可写小数如 `0.5`，`0` 表示不等待），
  适合服务端限速或不想一次压满带宽的场景

示例：
```
okb-client dir D:\Papers --base-url http://192.168.1.162:5001 -r
okb-client dir ./downloads --dry-run
okb-client dir D:\Papers -r --upload-interval 3   # 每次上传之间等 3 秒
okb-client dir D:\Papers -r --upload-interval 0   # 尽可能快（默认是 1 秒）
okb-client dir D:\Papers --dry-run                # 算哈希 + 比对，只列出哪些会传
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

上传时服务端会先用启发式规则从 PDF 里抽题名 / 作者 / 年份 / DOI / 摘要等（`pdf_meta`）。那一步要起几个 `mutool` 子进程，
其中 `mutool draw -F text` 会把**整份文档**正文过一遍，大文件/扫描件能慢到几十秒——这正是上传停在「等待服务端处理」的主因。

客户端两个子命令**默认跳过**这一步（请求里带 `extract_meta=false`）：记录先建好（列表里显示文件名），不抽元数据。
想保留旧行为加 `--extract-meta`；已上传的记录可以事后补：

```
POST /assist/api/pipeline/extract-pdf-meta/:id     # 单篇重新抽取（文档管理页有按钮）
```

注意：跳过抽取后，新建记录只有文件名 + 文件哈希（`dir` 流程），题名/作者/DOI 都是空；
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