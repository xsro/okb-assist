#!/usr/bin/env python3
"""将 documents.source 的值改为 'zotero'，并把原值备份到 markdowns 目录。

- 原值非空时写入 {id}.txt
- 原值可解析为 JSON 时，额外写入格式化后的 {id}.json
- source 统一更新为 'zotero'

操作前会自动对数据库做一次带时间戳的备份。
"""
import argparse
import json
import os
import shutil
import sqlite3
import sys
from datetime import datetime

DEFAULT_DB = "/home/orangepi/sys/okb-assist/okb_assist.db"
DEFAULT_OUT = "/home/orangepi/sys/okb-knowledge/markdowns"
NEW_SOURCE = "zotero"


def main():
    parser = argparse.ArgumentParser(description="将 documents.source 改为 'zotero' 并备份原值")
    parser.add_argument("--db", default=DEFAULT_DB, help="okb_assist.db 路径")
    parser.add_argument("--out", default=DEFAULT_OUT, help="原值保存目录")
    parser.add_argument("--dry-run", action="store_true", help="只统计不改动数据库/不写文件")
    args = parser.parse_args()

    if not os.path.isfile(args.db):
        print(f"数据库不存在: {args.db}", file=sys.stderr)
        return 1

    os.makedirs(args.out, exist_ok=True)

    conn = sqlite3.connect(args.db)
    conn.row_factory = sqlite3.Row

    rows = conn.execute("SELECT id, source FROM documents").fetchall()
    total = len(rows)

    written_txt = 0
    written_json = 0
    skipped_empty = 0

    for row in rows:
        doc_id = row["id"]
        original = row["source"]

        # 原值非空才保存备份文件
        if original is not None and str(original).strip() != "":
            raw = str(original)
            txt_path = os.path.join(args.out, f"{doc_id}.txt")
            if not args.dry_run:
                with open(txt_path, "w", encoding="utf-8") as f:
                    f.write(raw)
            written_txt += 1

            # 尝试解析为 JSON
            try:
                parsed = json.loads(raw)
            except (json.JSONDecodeError, ValueError):
                parsed = None

            if parsed is not None:
                json_path = os.path.join(args.out, f"{doc_id}.json")
                if not args.dry_run:
                    with open(json_path, "w", encoding="utf-8") as f:
                        json.dump(parsed, f, ensure_ascii=False, indent=2)
                written_json += 1
        else:
            skipped_empty += 1

        if not args.dry_run:
            conn.execute(
                "UPDATE documents SET source = ? WHERE id = ?", (NEW_SOURCE, doc_id)
            )

    if not args.dry_run:
        conn.commit()

        # 备份数据库
        ts = datetime.now().strftime("%Y%m%d_%H%M%S")
        backup = f"{args.db}.bak.{ts}"
        shutil.copy2(args.db, backup)
        print(f"数据库已备份至: {backup}")

    conn.close()

    print(f"处理完成 (dry_run={args.dry_run})")
    print(f"  总行数        : {total}")
    print(f"  写入 .txt     : {written_txt}")
    print(f"  写入 .json    : {written_json}")
    print(f"  原值为空跳过  : {skipped_empty}")
    print(f"  输出目录      : {args.out}")

    return 0


if __name__ == "__main__":
    sys.exit(main())
