#!/usr/bin/env python3
"""
将 CSV 中的 zip_path 按模板重命名，输出为 bash 脚本（包含 mv 命令）。

从 md_filename 提取 id（如 "1.md" → "1"），替换模板中的 {id} 生成新路径。
若原 zip_path 为空或与原路径一致，跳过。
输出一个 bash 脚本文件，每行形如:
  mv "/原路径/xxx.zip" "/新路径/xxx.zip"
"""

import argparse
import csv
import os
import sys


def main():
    parser = argparse.ArgumentParser(description="生成重命名 zip 文件的 bash 脚本")
    parser.add_argument("csv_file", help="输入 CSV 文件路径（含 md_filename, zip_path 列）")
    parser.add_argument(
        "--template", type=str,
        default="/media/orangepi/CCSICC/okb-knowledge/pdfs/{id}/{id}.zip",
        help="新路径模板，{id} 会被替换为文档 id（默认: %(default)s）"
    )
    parser.add_argument("--max", type=int, default=0, help="最大检查条目数（0 表示不限制）")
    parser.add_argument("--progress", action="store_true", help="显示进度条（需要 tqdm）")
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

    # ── 准备输出 bash 脚本 ──────────────────────────────────────
    base, ext = os.path.splitext(args.csv_file)
    out_path = base + "_rename.sh"
    out_file = open(out_path, "w", encoding="utf-8", newline="\n")
    out_file.write("#!/usr/bin/env bash\n")
    out_file.write("# 自动生成的重命名脚本 - 将 zip_path 更新为新路径\n")
    out_file.write(f"# 模板: {args.template}\n")
    out_file.write(f"# 来源: {os.path.basename(args.csv_file)}\n")
    out_file.write("set -e\n\n")

    # ── 进度条 ──────────────────────────────────────────────────
    use_tqdm = args.progress and total > 0
    if use_tqdm:
        try:
            from tqdm import tqdm
            iterator = tqdm(rows, unit="row", desc="Generating")
        except ImportError:
            print("!! tqdm 未安装，回退到普通输出模式", file=sys.stderr)
            use_tqdm = False
    if not use_tqdm:
        iterator = rows

    cmd_count = 0
    scan_count = 0
    total_to_scan = len(rows)

    for row in iterator:
        scan_count += 1
        md_filename = row.get("md_filename", "").strip()
        zip_path = row.get("zip_path", "").strip()

        if not md_filename or not zip_path:
            continue

        # 从 "1.md" 提取 id
        doc_id = os.path.splitext(md_filename)[0]

        # 替换模板中的 {id}
        new_zip_path = args.template.replace("{id}", doc_id)

        # 原路径与新路径一致，跳过
        if zip_path == new_zip_path:
            continue

        # 输出 mv 命令（路径用引号包裹以防空格）
        out_file.write(f'mv "{zip_path}" "{new_zip_path}"\n')
        cmd_count += 1

        if not use_tqdm and scan_count % 500 == 0:
            print(f"==> 已检查 {scan_count}/{total_to_scan}", file=sys.stderr)

    out_file.close()

    # 设置可执行权限
    os.chmod(out_path, 0o755)

    print(f"\n完成！检查 {scan_count} 条，生成 {cmd_count} 条 mv 命令")
    print(f"输出文件: {out_path}")
    print(f"执行: bash {out_path}")


if __name__ == "__main__":
    main()