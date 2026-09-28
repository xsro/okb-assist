#!/usr/bin/env python3
"""
查找原 CSV 中 zip_path 为空、但在本地 pdf_folder/{id}/ 下 zip 中找到了对应图片的条目。

输入 CSV 格式: md_filename, first_image, zip_path, image_in_zip_path
  - 只处理 zip_path 为空（远程无 zip）的行
输出 CSV 格式: md_filename, first_image, zip_path, image_in_zip_path
  - zip_path: 本地找到的 zip 文件路径
  - image_in_zip_path: 图片在 zip 内的路径
  - 仅输出本地找到了匹配 zip 的条目
"""

import argparse
import csv
import glob
import os
import sys
import zipfile


def main():
    parser = argparse.ArgumentParser(
        description="从原 CSV 中找出缺少 zip、但在本地 pdf_folder 中找到的条目"
    )
    parser.add_argument("csv_file", help="输入 CSV 文件路径（含 md_filename, first_image, zip_path, image_in_zip_path 列）")
    parser.add_argument("pdf_folder", help="PDF 根目录，按 {pdf_folder}/{id}/ 下所有 zip 查找")
    parser.add_argument("--progress", action="store_true", help="显示进度条（需要 tqdm）")
    parser.add_argument("--max", type=int, default=0, help="最大扫描条目数（0 表示不限制）")
    args = parser.parse_args()

    # ── 读取 CSV ────────────────────────────────────────────────
    rows: list[dict[str, str]] = []
    with open(args.csv_file, encoding="utf-8-sig") as f:
        reader = csv.DictReader(f)
        for r in reader:
            rows.append(r)

    total = len(rows)
    if args.max and args.max < total:
        rows = rows[:args.max]

    # ── 准备输出文件 ────────────────────────────────────────────
    base, ext = os.path.splitext(args.csv_file)
    out_path = base + "_rescued" + ext
    out_file = open(out_path, "w", encoding="utf-8-sig", newline="")
    writer = csv.writer(out_file)
    writer.writerow(["md_filename", "first_image", "zip_path", "image_in_zip_path"])

    # ── 进度条 ──────────────────────────────────────────────────
    use_tqdm = args.progress and total > 0
    if use_tqdm:
        try:
            from tqdm import tqdm
            iterator = tqdm(rows, unit="row", desc="Scanning")
        except ImportError:
            print("!! tqdm 未安装，回退到普通输出模式", file=sys.stderr)
            use_tqdm = False
    if not use_tqdm:
        iterator = rows

    rescued_count = 0
    scan_count = 0
    total_to_scan = len(rows)

    for row in iterator:
        scan_count += 1
        md_filename = row["md_filename"].strip()
        first_image = row["first_image"].strip()
        orig_zip_path = row.get("zip_path", "").strip()

        # 只处理原 CSV 中 zip_path 为空的条目
        if orig_zip_path:
            continue

        if not md_filename or not first_image:
            continue

        # 从 "1.md" 提取 id = "1"
        doc_id = os.path.splitext(md_filename)[0]

        # 列出 {pdf_folder}/{id}/ 下所有 zip 文件
        zip_dir = os.path.join(args.pdf_folder, doc_id)
        zip_files = glob.glob(os.path.join(zip_dir, "*.zip"))

        for zip_path_raw in zip_files:
            zip_path = zip_path_raw.replace("\\", "/")
            try:
                with zipfile.ZipFile(zip_path_raw) as zf:
                    all_files = zf.namelist()
                    matched = [
                        name for name in all_files
                        if os.path.basename(name) == first_image or name == first_image
                    ]
                    if matched:
                        for image_in_zip in matched:
                            writer.writerow([md_filename, first_image, zip_path, image_in_zip])
                            rescued_count += 1
            except (zipfile.BadZipFile, PermissionError, OSError) as e:
                if not use_tqdm:
                    print(f"!! {md_filename}: 无法读取 {zip_path} - {e}", file=sys.stderr)
                continue

        if not use_tqdm and scan_count % 500 == 0:
            print(f"==> 已扫描 {scan_count}/{total_to_scan}", file=sys.stderr)

    out_file.close()

    print(f"\n完成！扫描 {scan_count} 条（其中原缺失 zip 条目待查），找到 {rescued_count} 个可找回条目")
    print(f"输出文件: {out_path}")


if __name__ == "__main__":
    main()