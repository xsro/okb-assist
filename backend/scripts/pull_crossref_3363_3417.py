"""触发已有 Crossref 抓取流程，补全文档 3363 至 3417 的元数据。

本脚本复用应用自带的 `app.routers.pipeline._run_crossref(doc_id)` 函数
（定义于 app/routers/pipeline.py:1271），逐篇触发 Crossref 元数据补全：

- `_run_crossref` 是 async 函数，仅接受 doc_id 一个参数；
- 它会自行打开数据库会话，按 DOI（无 DOI 时按题名）查询 Crossref；
- 仅填充文档中为空的元数据字段，不会改变 doc.status；
- 既无 DOI 也无标题的文档会被该函数静默跳过。

运行方式（必须在仓库根目录执行，因为 import app 会读取 system.json）：

    cd /home/orangepi/sys/okb-assist
    .venv/bin/python scripts/pull_crossref_3363_3417.py

可选：指定自定义区间（含端点）：

    .venv/bin/python scripts/pull_crossref_3363_3417.py 3363 3417
"""

import sys
import os
import asyncio

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
if ROOT not in sys.path:
    sys.path.insert(0, ROOT)

from app.routers import pipeline


def main():
    default_start = 3363
    default_end = 3417

    if len(sys.argv) >= 3:
        start = int(sys.argv[1])
        end = int(sys.argv[2])
    else:
        start = default_start
        end = default_end

    for doc_id in range(start, end + 1):
        try:
            asyncio.run(pipeline._run_crossref(doc_id))
        except Exception as exc:
            print(f"[ERROR] doc_id={doc_id}: {exc}")


if __name__ == "__main__":
    main()
