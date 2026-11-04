# OKB-Assist 客户端工具

命令行工具，用于向 OKB-Assist 服务上传文献。

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
- `--base-url <URL>`         服务地址（默认 http://192.168.1.162:5001）
- `--token <TOKEN>`          访问令牌
- `--storage-root <PATH>`    Zotero storage 文件夹本地路径
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
- `--base-url <URL>`         服务地址（默认 http://192.168.1.162:5001）
- `--token <TOKEN>`          访问令牌
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
3. `~/.okb_assist_token` 文件内容

## 构建

```
cargo build --release
```