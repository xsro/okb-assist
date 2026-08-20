# OKB-Assist 工具脚本

## import_zotero_csv.py

从 Zotero 导出的 CSV 文件导入文献到 OKB-Assist。

### 使用方法

```bash
python scripts/import_zotero_csv.py <csv_file> [选项]
```

### 参数

| 参数 | 说明 | 默认值 |
|------|------|--------|
| `csv_file` | Zotero 导出的 CSV 文件路径 | 必需 |
| `--base-url` | OKB-Assist 服务地址 | `http://localhost:5001` |
| `--token` | 访问令牌 | 从环境变量读取 |
| `--dry-run` | 试运行，不实际上传 | `false` |
| `--skip-existing` | 跳过已存在的文件 | `true` |
| `--save-original` | 保存原始 CSV 数据到 source 字段 | `true` |

### 示例

```bash
# 基本用法
python scripts/import_zotero_csv.py ~/zotero_export.csv

# 指定服务地址和令牌
python scripts/import_zotero_csv.py ~/zotero_export.csv \
    --base-url http://192.168.1.183:5001 \
    --token your-token

# 试运行（不实际上传）
python scripts/import_zotero_csv.py ~/zotero_export.csv --dry-run

# 不保存原始数据
python scripts/import_zotero_csv.py ~/zotero_export.csv --no-save-original
```

### Token 配置

可以通过以下方式设置 token（按优先级）：

1. 命令行参数: `--token your-token`
2. 环境变量: `export OKB_ASSIST_TOKEN=your-token`
3. 配置文件: `~/.okb_assist_token`

### Zotero CSV 格式

脚本支持 Zotero 导出的标准 CSV 格式，常见字段映射：

| Zotero 字段 | OKB-Assist 字段 |
|-------------|----------------|
| Title | title |
| Author | authors |
| Publication Year | year |
| DOI | doi |
| Journal | journal |
| Abstract Note | abstract |
| Tags | keywords |
| Type | doc_type |
| Publisher | source |
| File Attachments | file_path |

### 文件路径处理

Zotero 的 `File Attachments` 字段格式：
- 单个文件: `/path/to/file.pdf`
- 多个文件: `/path/to/file1.pdf;/path/to/file2.pdf`
- 带附件信息: `/path/to/file.pdf:application/pdf:1234`

脚本会自动：
1. 按分号分割多个路径
2. 验证文件是否存在
3. 选择最新的文件
4. 计算 SHA256 哈希
5. 检查是否已存在

### 原始数据保存

当 `--save-original` 启用时（默认），CSV 中的所有字段会以 JSON 格式保存到 `source` 字段：

```json
{
  "Title": "Example Paper",
  "Author": "Doe, John; Smith, Jane",
  "Publication Year": "2024",
  "DOI": "10.1234/example",
  ...
}
```
