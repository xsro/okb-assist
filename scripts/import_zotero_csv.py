#!/usr/bin/env python3
"""
Zotero CSV 导入脚本

读取 Zotero 导出的 CSV 文件，逐条上传到 OKB-Assist 服务。

匹配模式:
    doi   (默认) — 优先用 DOI 匹配，无 DOI 时回退到文件哈希
    hash        — 仅用文件 SHA256 哈希匹配

使用方法:
    python scripts/import_zotero_csv.py <csv_file> [选项]

示例:
    python scripts/import_zotero_csv.py ~/zotero_export.csv
    python scripts/import_zotero_csv.py ~/zotero_export.csv --match-mode hash
    python3 scripts/import_zotero_csv.py data\我的文库.csv --base-url http://192.168.1.100:5001 --token change-me
"""

import argparse
import csv
import hashlib
import json
import os
import sys
from pathlib import Path

import httpx


# ──────────────────────────────────────────────
# Hash 计算（与服务端 app/utils.py 保持一致）
# ──────────────────────────────────────────────

def calculate_file_hash(file_path: str) -> str:
    """计算文件 SHA256 哈希（SHA256 + 4096 字节缓冲区）。"""
    sha256_hash = hashlib.sha256()
    with open(file_path, "rb") as f:
        for byte_block in iter(lambda: f.read(4096), b""):
            sha256_hash.update(byte_block)
    return sha256_hash.hexdigest()


# ──────────────────────────────────────────────
# 文件路径解析
# ──────────────────────────────────────────────

def parse_file_paths(file_field: str) -> list[str]:
    """
    解析 Zotero file 字段，返回存在的文件路径列表。

    支持格式:
      - /path/to/file.pdf
      - /path/to/a.pdf;/path/to/b.pdf
      - /path/to/file.pdf:application/pdf:1234
    """
    if not file_field:
        return []

    paths = []
    for part in file_field.split(";"):
        part = part.strip()
        if not part:
            continue
        path = part.strip()
        if path and os.path.exists(path):
            paths.append(path)
    return paths


def get_newest_file(file_paths: list[str]) -> str | None:
    """返回修改时间最新的文件路径。"""
    valid = [p for p in file_paths if os.path.exists(p)]
    return max(valid, key=os.path.getmtime) if valid else None


# ──────────────────────────────────────────────
# API 调用
# ──────────────────────────────────────────────

def _headers(token: str = None) -> dict:
    h = {"Content-Type": "application/json"}
    if token:
        h["X-Token"] = token
    return h


def check_exists_by_doi(base_url: str, doi: str, token: str = None) -> dict | None:
    """通过 DOI 查询文档是否存在。"""
    try:
        with httpx.Client(timeout=30) as client:
            resp = client.get(
                f"{base_url}/assist/api/documents/by-doi/{doi}",
                headers=_headers(token),
            )
            if resp.status_code == 200:
                return resp.json()
            if resp.status_code != 404:
                print(f"  警告: DOI 查询失败: {resp.status_code}")
    except Exception as e:
        print(f"  警告: DOI 查询失败: {e}")
    return None


def check_exists_by_hash(base_url: str, file_hash: str, token: str = None) -> dict | None:
    """通过文件哈希查询文档是否存在。"""
    try:
        with httpx.Client(timeout=30) as client:
            resp = client.get(
                f"{base_url}/assist/api/documents/by-hash/{file_hash}",
                headers=_headers(token),
            )
            if resp.status_code == 200:
                return resp.json()
            if resp.status_code != 404:
                print(f"  警告: 哈希查询失败: {resp.status_code}")
    except Exception as e:
        print(f"  警告: 哈希查询失败: {e}")
    return None


def upload_document(base_url: str, file_path: str, token: str = None) -> dict | None:
    """上传 PDF 文件到服务器。"""
    try:
        headers = {}
        if token:
            headers["X-Token"] = token
        with httpx.Client(timeout=120) as client:
            with open(file_path, "rb") as f:
                resp = client.post(
                    f"{base_url}/assist/api/documents/upload",
                    files={"file": (os.path.basename(file_path), f, "application/pdf")},
                    headers=headers,
                )
            if resp.status_code == 200:
                return resp.json()
            else:
                print(f"  上传失败: {resp.status_code} - {resp.text[:200]}")
    except Exception as e:
        print(f"  上传失败: {e}")
    return None


def update_document_metadata(base_url: str, doc_id: int, metadata: dict, token: str = None) -> bool:
    """更新文档元数据。"""
    try:
        with httpx.Client(timeout=30) as client:
            resp = client.put(
                f"{base_url}/assist/api/documents/{doc_id}",
                json=metadata,
                headers=_headers(token),
            )
            if resp.status_code == 200:
                return True
            print(f"  更新元数据失败: {resp.status_code}")
    except Exception as e:
        print(f"  更新元数据失败: {e}")
    return False


# ──────────────────────────────────────────────
# Zotero CSV 解析
# ──────────────────────────────────────────────

TYPE_MAP = {
    "journalArticle": "article",
    "book": "book",
    "conferencePaper": "conference",
    "thesis": "thesis",
    "report": "report",
    "webpage": "webpage",
}


def parse_zotero_row(row: dict) -> dict:
    """从 Zotero CSV 行提取元数据。"""
    meta = {}

    if row.get("Title"):
        meta["title"] = row["Title"].strip()

    if row.get("Author"):
        authors = []
        for a in row["Author"].split(";"):
            a = a.strip()
            if "," in a:
                last, first = a.split(",", 1)
                authors.append(f"{first.strip()} {last.strip()}")
            else:
                authors.append(a)
        meta["authors"] = json.dumps(authors, ensure_ascii=False)

    if row.get("Publication Year"):
        try:
            meta["year"] = int(row["Publication Year"].strip())
        except ValueError:
            pass

    if row.get("DOI"):
        meta["doi"] = row["DOI"].strip()

    if row.get("Journal"):
        meta["journal"] = row["Journal"].strip()

    if row.get("Abstract Note"):
        meta["abstract"] = row["Abstract Note"].strip()

    if row.get("Tags"):
        tags = [t.strip() for t in row["Tags"].split(";") if t.strip()]
        meta["keywords"] = json.dumps(tags, ensure_ascii=False)

    if row.get("Type"):
        meta["doc_type"] = TYPE_MAP.get(row["Type"].strip(), row["Type"].strip())

    if row.get("Publisher"):
        meta["source"] = row["Publisher"].strip()

    return meta


# ──────────────────────────────────────────────
# 主流程
# ──────────────────────────────────────────────

def read_token(args_token: str | None) -> str | None:
    """按优先级读取 token: 参数 > 环境变量 > 文件。"""
    if args_token:
        return args_token
    token = os.environ.get("OKB_ASSIST_TOKEN")
    if token:
        return token
    token_file = Path.home() / ".okb_assist_token"
    if token_file.exists():
        return token_file.read_text().strip()
    return None


def main():
    parser = argparse.ArgumentParser(description="导入 Zotero CSV 到 OKB-Assist")
    parser.add_argument("csv_file", help="Zotero 导出的 CSV 文件路径")
    parser.add_argument("--base-url", default="http://localhost:5001", help="OKB-Assist 服务地址")
    parser.add_argument("--token", default=None, help="访问令牌")
    parser.add_argument("--match-mode", choices=["doi", "hash"], default="doi",
                        help="匹配模式: doi (默认，仅 DOI 匹配，无 DOI 则跳过) | hash (仅哈希匹配)")
    parser.add_argument("--dry-run", action="store_true", help="试运行，不实际上传")
    parser.add_argument("--no-skip", action="store_true", help="不跳过已存在的文件，强制重新注册")
    parser.add_argument("--save-original", action="store_true", default=True,
                        help="保存原始 CSV 数据到 source 字段")

    args = parser.parse_args()
    token = read_token(args.token)

    print(f"服务地址: {args.base_url}")
    print(f"CSV 文件: {args.csv_file}")
    print(f"匹配模式: {args.match_mode}")
    print(f"Token: {'*' * 8 if token else '未设置'}")
    print()

    if not os.path.exists(args.csv_file):
        print(f"错误: CSV 文件不存在: {args.csv_file}")
        sys.exit(1)

    with open(args.csv_file, "r", encoding="utf-8-sig") as f:
        rows = list(csv.DictReader(f))

    print(f"共 {len(rows)} 条记录")
    print()

    stats = {"total": len(rows), "uploaded": 0, "skipped": 0, "failed": 0, "duplicate": 0}

    for i, row in enumerate(rows, 1):
        print(f"[{i}/{len(rows)}] ", end="")

        title = row.get("Title", "").strip() or row.get("Filename", "").strip() or "未知标题"
        print(f"{title[:50]}...")

        csv_doi = (row.get("DOI") or "").strip()

        # ── DOI 模式: 无 DOI 直接跳过 ──
        if args.match_mode == "doi" and not csv_doi:
            print(f"  跳过: 无 DOI")
            stats["skipped"] += 1
            continue

        # ── 解析文件路径（两种模式都需要，注册时要用）──
        file_field = row.get("File Attachments", "") or row.get("File", "") or row.get("file", "")
        newest_file = get_newest_file(parse_file_paths(file_field))
        if not newest_file:
            print(f"  跳过: 无有效文件路径")
            stats["skipped"] += 1
            continue
        print(f"  文件: {newest_file}")

        # ── 匹配已有文档 ──
        if not args.no_skip:
            existing = None
            if args.match_mode == "doi":
                print(f"  DOI: {csv_doi}")
                existing = check_exists_by_doi(args.base_url, csv_doi, token)
            else:
                file_hash = calculate_file_hash(newest_file)
                print(f"  哈希: {file_hash[:16]}...")
                existing = check_exists_by_hash(args.base_url, file_hash, token)

            if existing:
                print(f"  跳过: 已存在 (ID: {existing['id']})")
                stats["duplicate"] += 1
                continue

        # ── 试运行 ──
        if args.dry_run:
            print(f"  [试运行] 将上传")
            stats["uploaded"] += 1
            continue

        # ── 上传文档 ──
        result = upload_document(args.base_url, newest_file, token)
        if not result:
            stats["failed"] += 1
            continue

        doc_id = result["id"]
        print(f"  已上传: ID={doc_id}")

        # ── 更新元数据 ──
        metadata = parse_zotero_row(row)
        if args.save_original:
            metadata["source"] = json.dumps(row, ensure_ascii=False)

        if metadata:
            if update_document_metadata(args.base_url, doc_id, metadata, token):
                print(f"  已更新元数据")
            else:
                print(f"  警告: 元数据更新失败")

        stats["uploaded"] += 1

    # ── 统计 ──

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
