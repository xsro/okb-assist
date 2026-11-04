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

比对方式是**一次拉全量索引**：客户端先算完本地哈希，再请求 `GET /assist/api/documents/doi-hash-index`
拿到服务端全部文献的 `{id, doi, doi_key, file_hash, pdf_health}`，整库比对在本地完成，
不再按 DOI 逐条打 `by-doi`（旧实现几百条文献就是几百次往返）。只有该接口不可用时才回退到逐条查询，
`dir` 子命令同样优先用这份索引，失败才回退到 `diff-hashes`。

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

扫描指定目录中的 PDF 文件，计算 SHA256 哈希，与服务端比对后交互式上传。

```
okb-client dir <目录路径> [选项]
```

选项：
- `--base-url <URL>`         服务地址（缺省取 `~/.okb/client.json` 的 `url`）
- `--token <TOKEN>`          访问令牌（缺省取 `~/.okb/client.json` 的 `token`）
- `--dry-run`                试运行，不实际上传
- `-r, --recursive`          递归搜索子目录
- `--process`                上传后触发解析流水线（默认不触发）

示例：
```
okb-client dir D:\Papers --base-url http://192.168.1.162:5001 -r
okb-client dir ./downloads --dry-run
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