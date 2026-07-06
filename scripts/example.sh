#!/bin/bash
# Zotero CSV 导入示例

# 1. 从 Zotero 导出 CSV
# 在 Zotero 中选择文件 -> 导出文献库 -> 选择 CSV 格式

# 2. 设置环境变量（可选）
export OKB_ASSIST_TOKEN="your-token-here"
export OKB_ASSIST_URL="http://192.168.1.183:5001"

# 3. 试运行（不实际上传）
python scripts/import_zotero_csv.py ~/zotero_export.csv \
    --base-url $OKB_ASSIST_URL \
    --token $OKB_ASSIST_TOKEN \
    --dry-run

# 4. 正式导入
python scripts/import_zotero_csv.py ~/zotero_export.csv \
    --base-url $OKB_ASSIST_URL \
    --token $OKB_ASSIST_TOKEN

# 5. 跳过已存在的文件（默认开启）
python scripts/import_zotero_csv.py ~/zotero_export.csv \
    --base-url $OKB_ASSIST_URL \
    --token $OKB_ASSIST_TOKEN \
    --skip-existing

# 6. 不保存原始 CSV 数据
python scripts/import_zotero_csv.py ~/zotero_export.csv \
    --base-url $OKB_ASSIST_URL \
    --token $OKB_ASSIST_TOKEN \
    --no-save-original
