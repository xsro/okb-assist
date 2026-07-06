#!/usr/bin/env python3
"""
Zotero CSV 导入脚本

读取 Zotero 导出的 CSV 文件，逐条上传到 OKB-Assist 服务。

使用方法:
    python scripts/import_zotero_csv.py <csv_file> [--base-url URL] [--token TOKEN]

示例:
    python scripts/import_zotero_csv.py ~/zotero_export.csv
    python scripts/import_zotero_csv.py ~/zotero_export.csv --base-url http://192.168.1.183:5001 --token your-token
"""

import argparse
import csv
import hashlib
import json
import os
import sys
from datetime import datetime
from pathlib import Path

import httpx


def calculate_file_hash(file_path: str) -> str:
    """计算文件 SHA256 哈希"""
    sha256_hash = hashlib.sha256()
    with open(file_path, "rb") as f:
        for byte_block in iter(lambda: f.read(4096), b""):
            sha256_hash.update(byte_block)
    return sha256_hash.hexdigest()


def parse_file_paths(file_field: str) -> list[str]:
    """
    解析 file 字段，返回文件路径列表

    Zotero 导出的 file 字段格式可能是：
    - 单个路径: /path/to/file.pdf
    - 多个路径: /path/to/file1.pdf;/path/to/file2.pdf
    - 带附件: /path/to/file.pdf;/path/to/file.pdf:application/pdf:1234
    """
    if not file_field:
        return []

    paths = []
    # 按分号分割
    parts = file_field.split(";")

    for part in parts:
        part = part.strip()
        if not part:
            continue

        # 移除可能的 MIME 类型和 ID 后缀 (如 :application/pdf:1234)
        if ":" in part:
            # 只取第一个冒号前的部分（路径可能包含冒号，如 Windows 路径）
            # 但对于 Unix 路径，直接取第一部分
            path = part.split(":")[0]
        else:
            path = part

        path = path.strip()
        if path and os.path.exists(path):
            paths.append(path)

    return paths


def get_newest_file(file_paths: list[str]) -> str | None:
    """获取最新的文件路径"""
    if not file_paths:
        return None

    # 按修改时间排序，返回最新的
    valid_paths = [p for p in file_paths if os.path.exists(p)]
    if not valid_paths:
        return None

    return max(valid_paths, key=os.path.getmtime)


def check_hash_exists(base_url: str, file_hash: str, token: str = None) -> dict | None:
    """检查哈希是否已存在"""
    headers = {}
    if token:
        headers["X-Token"] = token

    try:
        with httpx.Client(timeout=30) as client:
            response = client.get(
                f"{base_url}/assist/api/documents/",
                params={"page_size": 10000},
                headers=headers,
            )

            if response.status_code == 200:
                data = response.json()
                for doc in data.get("items", []):
                    if doc.get("file_hash") == file_hash:
                        return doc
    except Exception as e:
        print(f"  警告: 检查哈希失败: {e}")

    return None


def register_document(base_url: str, file_path: str, token: str = None) -> dict | None:
    """注册文档（不复制文件）"""
    headers = {"Content-Type": "application/json"}
    if token:
        headers["X-Token"] = token

    try:
        with httpx.Client(timeout=30) as client:
            response = client.post(
                f"{base_url}/assist/api/documents/register",
                json={"file_path": file_path, "force": False},
                headers=headers,
            )

            if response.status_code == 200:
                return response.json()
            elif response.status_code == 409:
                # 重复文件
                detail = response.json().get("detail", {})
                print(f"  文件已存在 (ID: {detail.get('existing_id')})")
                return None
            else:
                print(f"  注册失败: {response.status_code} - {response.text[:200]}")
                return None
    except Exception as e:
        print(f"  注册失败: {e}")
        return None


def update_document_metadata(base_url: str, doc_id: int, metadata: dict, token: str = None) -> bool:
    """更新文档元数据"""
    headers = {"Content-Type": "application/json"}
    if token:
        headers["X-Token"] = token

    try:
        with httpx.Client(timeout=30) as client:
            response = client.put(
                f"{base_url}/assist/api/documents/{doc_id}",
                json=metadata,
                headers=headers,
            )

            if response.status_code == 200:
                return True
            else:
                print(f"  更新元数据失败: {response.status_code}")
                return False
    except Exception as e:
        print(f"  更新元数据失败: {e}")
        return False


def parse_zotero_row(row: dict) -> dict:
    """
    解析 Zotero CSV 行，提取有用的元数据

    Zotero CSV 常见字段:
    - Title: 标题
    - Author: 作者
    - Publication Year: 出版年份
    - DOI: DOI
    - ISBN: ISBN
    - ISSN: ISSN
    - Journal: 期刊
    - Publisher: 出版社
    - Abstract Note: 摘要
    - Tags: 标签
    - Type: 文献类型
    - File Attachments: 文件路径
    """
    metadata = {}

    # 标题
    if row.get("Title"):
        metadata["title"] = row["Title"].strip()

    # 作者 - Zotero 格式通常是 "Last, First; Last, First"
    if row.get("Author"):
        authors = []
        for author in row["Author"].split(";"):
            author = author.strip()
            if "," in author:
                parts = author.split(",", 1)
                authors.append(f"{parts[1].strip()} {parts[0].strip()}")
            else:
                authors.append(author)
        metadata["authors"] = json.dumps(authors, ensure_ascii=False)

    # 年份
    if row.get("Publication Year"):
        try:
            metadata["year"] = int(row["Publication Year"].strip())
        except ValueError:
            pass

    # DOI
    if row.get("DOI"):
        metadata["doi"] = row["DOI"].strip()

    # 期刊
    if row.get("Journal"):
        metadata["journal"] = row["Journal"].strip()

    # 摘要
    if row.get("Abstract Note"):
        metadata["abstract"] = row["Abstract Note"].strip()

    # 标签
    if row.get("Tags"):
        tags = [t.strip() for t in row["Tags"].split(";") if t.strip()]
        metadata["keywords"] = json.dumps(tags, ensure_ascii=False)

    # 文献类型
    type_mapping = {
        "journalArticle": "article",
        "book": "book",
        "conferencePaper": "conference",
        "thesis": "thesis",
        "report": "report",
        "webpage": "webpage",
    }
    if row.get("Type"):
        zotero_type = row["Type"].strip()
        metadata["doc_type"] = type_mapping.get(zotero_type, zotero_type)

    # 来源
    if row.get("Publisher"):
        metadata["source"] = row["Publisher"].strip()

    return metadata


def main():
    parser = argparse.ArgumentParser(description="导入 Zotero CSV 到 OKB-Assist")
    parser.add_argument("csv_file", help="Zotero 导出的 CSV 文件路径")
    parser.add_argument("--base-url", default="http://localhost:5001", help="OKB-Assist 服务地址")
    parser.add_argument("--token", default=None, help="访问令牌")
    parser.add_argument("--dry-run", action="store_true", help="试运行，不实际上传")
    parser.add_argument("--skip-existing", action="store_true", default=True, help="跳过已存在的文件")
    parser.add_argument("--save-original", action="store_true", default=True, help="保存原始 CSV 数据到 source 字段")

    args = parser.parse_args()

    # 读取 token
    token = args.token
    if not token:
        # 尝试从环境变量读取
        token = os.environ.get("OKB_ASSIST_TOKEN")
    if not token:
        # 尝试从 localStorage 模拟的文件读取
        token_file = Path.home() / ".okb_assist_token"
        if token_file.exists():
            token = token_file.read_text().strip()

    print(f"服务地址: {args.base_url}")
    print(f"CSV 文件: {args.csv_file}")
    print(f"Token: {'*' * 8 if token else '未设置'}")
    print()

    # 读取 CSV 文件
    if not os.path.exists(args.csv_file):
        print(f"错误: CSV 文件不存在: {args.csv_file}")
        sys.exit(1)

    with open(args.csv_file, "r", encoding="utf-8-sig") as f:
        reader = csv.DictReader(f)
        rows = list(reader)

    print(f"共 {len(rows)} 条记录")
    print()

    # 统计
    stats = {
        "total": len(rows),
        "uploaded": 0,
        "skipped": 0,
        "failed": 0,
        "duplicate": 0,
    }

    # 逐条处理
    for i, row in enumerate(rows, 1):
        print(f"[{i}/{len(rows)}] ", end="")

        # 获取标题
        title = row.get("Title", "").strip() or row.get("Filename", "").strip() or "未知标题"
        print(f"{title[:50]}...")

        # 解析文件路径
        file_field = row.get("File Attachments", "") or row.get("File", "") or row.get("file", "")
        file_paths = parse_file_paths(file_field)

        if not file_paths:
            print(f"  跳过: 无有效文件路径")
            stats["skipped"] += 1
            continue

        # 获取最新的文件
        newest_file = get_newest_file(file_paths)
        if not newest_file:
            print(f"  跳过: 文件不存在")
            stats["skipped"] += 1
            continue

        print(f"  文件: {newest_file}")

        # 计算哈希
        file_hash = calculate_file_hash(newest_file)
        print(f"  哈希: {file_hash[:16]}...")

        # 检查是否已存在
        if args.skip_existing:
            existing = check_hash_exists(args.base_url, file_hash, token)
            if existing:
                print(f"  跳过: 文件已存在 (ID: {existing['id']})")
                stats["duplicate"] += 1
                continue

        # 试运行模式
        if args.dry_run:
            print(f"  [试运行] 将上传")
            stats["uploaded"] += 1
            continue

        # 注册文档
        result = register_document(args.base_url, newest_file, token)
        if not result:
            stats["failed"] += 1
            continue

        doc_id = result["id"]
        print(f"  已注册: ID={doc_id}")

        # 准备元数据
        metadata = parse_zotero_row(row)

        # 保存原始数据到 source 字段
        if args.save_original:
            metadata["source"] = json.dumps(row, ensure_ascii=False)

        # 更新元数据
        if metadata:
            if update_document_metadata(args.base_url, doc_id, metadata, token):
                print(f"  已更新元数据")
            else:
                print(f"  警告: 元数据更新失败")

        stats["uploaded"] += 1

    # 打印统计
    print()
    print("=" * 50)
    print("导入完成!")
    print(f"  总计: {stats['total']}")
    print(f"  上传: {stats['uploaded']}")
    print(f"  重复: {stats['duplicate']}")
    print(f"  跳过: {stats['skipped']}")
    print(f"  失败: {stats['failed']}")


if __name__ == "__main__":
    main()
