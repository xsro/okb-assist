#!/usr/bin/env python3
"""
扫描 Markdown 文件夹中的 .md 文件，提取第一个引用的图片文件名，
然后在 PDF 资产文件夹的所有 zip 包中查找该图片，输出 CSV 报告。

用法:
    python scripts/find_md_images.py --md-folder <path> --pdf-folder <path> [--max-md <N>] [--output <path>]

输出 CSV 列:
    md_filename, first_image, zip_path, image_in_zip_path
"""

import argparse
import csv
import os
import re
import zipfile
from pathlib import Path


def extract_first_image(md_path: Path) -> str | None:
    """从 .md 文件中提取第一个图片引用的文件名（basename）。"""
    # MinerU 格式: ![](images/xxx.jpg)
    pattern = re.compile(r'!\[.*?\]\(([^)]+)\)')
    try:
        with open(md_path, 'r', encoding='utf-8', errors='replace') as f:
            for line in f:
                m = pattern.search(line)
                if m:
                    ref = m.group(1).strip()
                    # 取 basename，例如 "images/page_1_image_8.jpg" → "page_1_image_8.jpg"
                    return os.path.basename(ref)
    except Exception as e:
        print(f"  [WARN] 读取 {md_path} 失败: {e}")
    return None


def build_zip_index(pdf_folder: Path, progress: bool = False) -> dict[str, list[tuple[str, str]]]:
    """
    扫描 pdf_folder 下所有 .zip 文件，建立 {filename: [(zip_path, internal_path), ...]} 索引。
    """
    index: dict[str, list[tuple[str, str]]] = {}
    zip_files = sorted(pdf_folder.rglob('*.zip'))
    print(f"发现 {len(zip_files)} 个 zip 文件，正在建立索引...")

    for i, zip_path in enumerate(zip_files, 1):
        if progress:
            print(f"  [进度] 索引 zip [{i}/{len(zip_files)}]: {zip_path}")
        try:
            with zipfile.ZipFile(zip_path, 'r') as zf:
                for entry in zf.namelist():
                    # 跳过目录项
                    if entry.endswith('/'):
                        continue
                    basename = os.path.basename(entry)
                    index.setdefault(basename, []).append(
                        (str(zip_path.resolve()), entry)
                    )
        except Exception as e:
            print(f"  [WARN] 读取 {zip_path} 失败: {e}")

    return index


def main():
    parser = argparse.ArgumentParser(
        description='扫描 md 文件中的第一张图片，在 zip 包中定位其位置，输出 CSV'
    )
    parser.add_argument(
        '--md-folder', '-m',
        required=True,
        help='Markdown 文件所在目录'
    )
    parser.add_argument(
        '--pdf-folder', '-p',
        required=True,
        help='PDF 资产文件夹（含 {id}/{id}.zip 压缩包）'
    )
    parser.add_argument(
        '--max-md', '-n',
        type=int,
        default=0,
        help='最大扫描的 md 文件数（0 表示不限制）'
    )
    parser.add_argument(
        '--output', '-o',
        default='md_images_report.csv',
        help='输出 CSV 文件路径（默认: md_images_report.csv）'
    )
    parser.add_argument(
        '--progress', '-g',
        action='store_true',
        help='打印处理进度信息'
    )
    args = parser.parse_args()

    md_folder = Path(args.md_folder)
    pdf_folder = Path(args.pdf_folder)

    if not md_folder.is_dir():
        print(f"[ERROR] md 文件夹不存在: {md_folder}")
        return 1
    if not pdf_folder.is_dir():
        print(f"[ERROR] pdf 文件夹不存在: {pdf_folder}")
        return 1

    # 1. 建立 zip 文件索引
    zip_index = build_zip_index(pdf_folder, progress=args.progress)
    print(f"索引构建完成，共 {len(zip_index)} 个不同文件名。")

    # 2. 扫描 md 文件
    md_files = sorted(md_folder.glob('*.md'))
    if args.max_md > 0:
        md_files = md_files[:args.max_md]

    print(f"待处理 md 文件数: {len(md_files)}")
    if args.progress:
        print()  # 空行分隔索引阶段和扫描阶段

    rows = []
    no_image_count = 0
    found_count = 0
    not_found_count = 0

    for idx, md_path in enumerate(md_files, 1):
        md_name = md_path.name
        if args.progress:
            print(f"[进度] 扫描 md [{idx}/{len(md_files)}]: {md_name}", end='', flush=True)
        first_image = extract_first_image(md_path)

        zip_path = ''
        image_in_zip = ''

        if first_image is None:
            no_image_count += 1
            if args.progress:
                print(f" → 无图片")
        else:
            # 在 zip 索引中查找
            matches = zip_index.get(first_image, [])
            if matches:
                zip_path, image_in_zip = matches[0]
                found_count += 1
                if args.progress:
                    print(f" → 找到图片: {first_image} @ {os.path.basename(zip_path)}")
            else:
                not_found_count += 1
                if args.progress:
                    print(f" → 未找到图片: {first_image}")

        img_display = first_image if first_image else '无图片'
        rows.append([md_name, img_display, zip_path, image_in_zip])

    # 3. 输出 CSV
    output_path = Path(args.output)
    with open(output_path, 'w', newline='', encoding='utf-8-sig') as f:
        writer = csv.writer(f)
        writer.writerow(['md_filename', 'first_image', 'zip_path', 'image_in_zip_path'])
        writer.writerows(rows)

    print(f"\n报告已生成: {output_path.resolve()}")
    print(f"  共处理 {len(rows)} 个 md 文件")
    print(f"    - 无图片:          {no_image_count}")
    print(f"    - 图片已找到:      {found_count}")
    print(f"    - 图片未找到:      {not_found_count}")


if __name__ == '__main__':
    exit(main())