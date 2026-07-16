#!/usr/bin/env bash
# 列出 uploads 文件夹中缺少 PDF 文件的任务 ID
# 用法: bash scripts/list_missing_pdf.sh

UPLOADS_DIR="$(dirname "$0")/../uploads"

if [ ! -d "$UPLOADS_DIR" ]; then
    echo "uploads 目录不存在: $UPLOADS_DIR"
    exit 1
fi

total=0
missing=0

for dir in "$UPLOADS_DIR"/*/; do
    id=$(basename "$dir")
    # 跳过非数字目录
    [[ "$id" =~ ^[0-9]+$ ]] || continue

    total=$((total + 1))
    pdf="$dir/${id}.pdf"
    if [ ! -f "$pdf" ]; then
        echo "$id"
        missing=$((missing + 1))
    fi
done

>&2 echo "---"
>&2 echo "共 $total 个任务，其中 $missing 个缺少 PDF"
