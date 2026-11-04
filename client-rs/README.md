# OKB-Assist 客户端工具

命令行工具，用于向 OKB-Assist 服务上传文献。

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
- `--upload-interval <SECONDS>` 每次上传之间的间隔秒数（默认 `1`；可写小数如 `0.5`，`0` 表示不等待）。
  新建上传与「更新 PDF」共用同一个计数，两次上传之间才等待，首个上传不等待

示例：
```
okb-client zotero data\我的文库.csv --base-url http://192.168.1.162:5001 --token xxx
okb-client zotero data\我的文库.csv --storage-root D:\Zotero\storage
okb-client zotero data\我的文库.csv --update-meta   # 更新 PDF 同时更新元数据
okb-client zotero data\我的文库.csv --dup-doi-action hash        # DOI 相同但 PDF 不同则覆盖
okb-client zotero data\我的文库.csv --dup-doi-action health-hash # 只修复不健康的 PDF
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

```
okb-client dir <目录路径> [选项]
```

选项：
- `--base-url <URL>`         服务地址（缺省取 `~/.okb/client.json` 的 `url`）
- `--token <TOKEN>`          访问令牌（缺省取 `~/.okb/client.json` 的 `token`）
- `--dry-run`                试运行，不实际上传
- `-r, --recursive`          递归搜索子目录
- `--process`                上传后触发解析流水线（默认不触发）
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