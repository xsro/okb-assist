#!/usr/bin/env python3
"""
将 CSV 中的 zip_path 与模板生成的新路径比较，输出不一致的条目。

输入 CSV 格式: md_filename, first_image, zip_path, image_in_zip_path
从 md_filename 提取 id（如 "1.md" → "1"），替换模板中的 {id} 生成新路径。
若原 zip_path 与新路径一致，跳过；否则输出该行（含新旧路径）。
"""

import argparse
import csv
import os
import sys


def main():
    parser = argparse.ArgumentParser(description="将 CSV 中的 zip_path 按模板重命名并输出不一致条目")
    parser.add_argument("csv_file", help="输入 CSV 文件路径（含 md_filename, zip_path 列）")
    parser.add_argument(
        "--template", type=str,
        default="media/orangepi/CCSICC/okb-knowledge/pdfs/{id}/{id}.zip",
        help="新路径模板，{id} 会被替换为文档 id（默认: %(default)s）"
    )
    parser.add_argument("--max", type=int, default=0, help="最大检查条目数（0 表示不限制）")
    parser.add_argument("--progress", action="store_true", help="显示进度条（需要 tqdm）")
    args = parser.parse_args()

    # ── 读取 CSV ────────────────────────────────────────────────
    rows: list[dict[str, str]] = []
    with open(args.csv_file, encoding="utf-8-sig") as f:
        reader = csv.DictReader(f)
        fieldnames = reader.fieldnames
        for r in reader:
            rows.append(r)

    total = len(rows)
    if args.max and args.max < total:
        rows = rows[:args.max]

    # ── 准备输出文件 ────────────────────────────────────────────
    base, ext = os.path.splitext(args.csv_file)
    out_path = base + "_renamed" + ext
    out_file = open(out_path, "w", encoding="utf-8-sig", newline="")
    out_fields = ["md_filename", "first_image", "zip_path", "new_zip_path", "image_in_zip_path"]
    writer = csv.writer(out_file)
    writer.writerow(out_fields)

    # ── 进度条 ──────────────────────────────────────────────────
    use_tqdm = args.progress and total > 0
    if use_tqdm:
        try:
            from tqdm import tqdm
            iterator = tqdm(rows, unit="row", desc="Checking")
        except ImportError:
            print("!! tqdm 未安装，回退到普通输出模式", file=sys.stderr)
            use_tqdm = False
    if not use_tqdm:
        iterator = rows

    changed_count = 0
    scan_count = 0
    total_to_scan = len(rows)

    for row in iterator:
        scan_count += 1
        md_filename = row.get("md_filename", "").strip()
        first_image = row.get("first_image", "").strip()
        zip_path = row.get("zip_path", "").strip()
        image_in_zip = row.get("image_in_zip_path", "").strip()

        if not md_filename:
            continue

        # 从 "1.md" 提取 id = "1"
        doc_id = os.path.splitext(md_filename)[0]

        # 替换模板中的 {id}
        new_zip_path = args.template.replace("{id}", doc_id)

        # 原路径与新路径一致，跳过
        if zip_path == new_zip_path:
            continue

        writer.writerow([md_filename, first_image, zip_path, new_zip_path, image_in_zip])
        changed_count += 1

        if not use_tqdm and scan_count % 500 == 0:
            print(f"==> 已检查 {scan_count}/{total_to_scan}", file=sys.stderr)

    out_file.close()

    print(f"\n完成！检查 {scan_count} 条，不一致 {changed_count} 条")
    print(f"输出文件: {out_path}")


if __name__ == "__main__":
    main()