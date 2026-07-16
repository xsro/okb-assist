#!/usr/bin/env python3
"""
Zotero CSV 导入脚本

读取 Zotero 导出的 CSV 文件，逐条上传到 OKB-Assist 服务。
尽可能填满所有文献元数据字段。

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
import re
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
        # 去掉 Zotero 附加的 :type:size 后缀
        path = part.split(":")[0].strip() if ":" in part else part.strip()
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
# Zotero CSV 字段名映射（兼容不同导出格式）
# ──────────────────────────────────────────────

# Zotero CSV 列名 → 内部 key（小写匹配）
COLUMN_ALIASES = {
    "title": "title",
    "author": "author",
    "publication year": "year",
    "date": "date",
    "doi": "doi",
    "journal": "journal",
    "publication title": "journal",
    "abstract note": "abstract",
    "abstract": "abstract",
    "tags": "tags",
    "type": "type",
    "item type": "type",
    "publisher": "publisher",
    "place": "place",
    "volume": "volume",
    "issue": "issue",
    "pages": "pages",
    "isbn": "isbn",
    "issn": "issn",
    "url": "url",
    "language": "language",
    "edition": "edition",
    "series": "series",
    "series number": "series_number",
    "conference name": "conference_name",
    "proceedings title": "proceedings_title",
    "book title": "book_title",
    "editor": "editor",
    "translator": "translator",
    "access date": "access_date",
    "rights": "rights",
    "file attachments": "file",
    "file": "file",
    "link attachments": "link",
    "extra": "extra",
    "call number": "call_number",
    "archive": "archive",
    "archive location": "archive_location",
    "library catalog": "library_catalog",
    "retrieved": "retrieved",
}


def normalize_row(row: dict) -> dict:
    """将 Zotero CSV 列名统一为小写 key。"""
    result = {}
    for col, val in row.items():
        key = COLUMN_ALIASES.get(col.strip().lower(), col.strip().lower())
        result[key] = val.strip() if val else ""
    return result


# ──────────────────────────────────────────────
# Zotero 类型映射
# ──────────────────────────────────────────────

TYPE_MAP = {
    "journalarticle": "article",
    "journal article": "article",
    "article": "article",
    "book": "book",
    "booksection": "book_section",
    "book section": "book_section",
    "conferencepaper": "conference",
    "conference paper": "conference",
    "conference proceedings": "conference",
    "proceedingsarticle": "conference",
    "proceedings article": "conference",
    "thesis": "thesis",
    "dissertation": "thesis",
    "report": "report",
    "technicalreport": "report",
    "technical report": "report",
    "webpage": "webpage",
    "document": "document",
    "presentation": "presentation",
    "manuscript": "manuscript",
    "patent": "patent",
    "newspaperarticle": "article",
    "newspaper article": "article",
    "magazinearticle": "article",
    "magazine article": "article",
    "preprint": "preprint",
    "review": "review",
}


# ──────────────────────────────────────────────
# 语言映射
# ──────────────────────────────────────────────

LANG_MAP = {
    "en": "en", "eng": "en", "english": "en",
    "zh": "zh", "chi": "zh", "chinese": "zh", "中文": "zh",
    "ja": "ja", "jpn": "ja", "japanese": "ja", "日文": "ja",
    "fr": "fr", "fre": "fr", "french": "fr",
    "de": "de", "ger": "de", "german": "de",
    "ru": "ru", "rus": "ru", "russian": "ru",
    "ko": "ko", "kor": "ko", "korean": "ko", "韩文": "ko",
    "es": "es", "spa": "es", "spanish": "es",
    "it": "es", "ita": "it", "italian": "it",
    "pt": "pt", "por": "pt", "portuguese": "pt",
    "ar": "ar", "ara": "ar", "arabic": "ar",
}


def normalize_language(lang: str) -> str | None:
    """标准化语言代码。"""
    if not lang:
        return None
    lang = lang.strip().lower()
    if lang in LANG_MAP:
        return LANG_MAP[lang]
    # 尝试取前两位
    if len(lang) >= 2 and lang[:2] in LANG_MAP:
        return LANG_MAP[lang[:2]]
    return None


# ──────────────────────────────────────────────
# 作者解析
# ──────────────────────────────────────────────

def parse_authors(author_field: str) -> list[str]:
    """解析 Zotero 作者字段，支持多种格式。"""
    if not author_field:
        return []

    authors = []
    # Zotero 导出通常用 ; 分隔多个作者
    for a in author_field.split(";"):
        a = a.strip()
        if not a:
            continue
        # "Last, First" → "First Last"
        if "," in a:
            parts = a.split(",", 1)
            last, first = parts[0].strip(), parts[1].strip()
            if first and last:
                authors.append(f"{first} {last}")
            elif last:
                authors.append(last)
        else:
            authors.append(a)
    return authors


# ──────────────────────────────────────────────
# Zotero CSV 解析
# ──────────────────────────────────────────────

def parse_zotero_row(row: dict) -> dict:
    """从 Zotero CSV 行提取尽可能完整的元数据。"""
    r = normalize_row(row)
    meta = {}

    # ── 基本信息 ──
    if r.get("title"):
        meta["title"] = r["title"]

    # 作者
    authors = parse_authors(r.get("author", ""))
    if authors:
        meta["authors"] = json.dumps(authors, ensure_ascii=False)

    # 年份：优先用 Publication Year，其次从 Date 字段提取
    year = None
    if r.get("year"):
        try:
            year = int(r["year"])
        except ValueError:
            pass
    if year is None and r.get("date"):
        m = re.search(r"(\d{4})", r["date"])
        if m:
            year = int(m.group(1))
    if year:
        meta["year"] = year

    if r.get("doi"):
        meta["doi"] = r["doi"]

    # ── 期刊 / 来源 ──
    # 对于会议论文，优先用 proceedings_title / conference_name
    item_type = r.get("type", "").lower()
    journal = r.get("journal", "")
    if not journal and "conference" in item_type:
        journal = r.get("proceedings_title", "") or r.get("conference_name", "")
    if not journal:
        journal = r.get("book_title", "")
    if journal:
        meta["journal"] = journal

    if r.get("abstract"):
        meta["abstract"] = r["abstract"]

    # ── 标签 → 关键词 ──
    if r.get("tags"):
        tags = [t.strip() for t in r["tags"].split(";") if t.strip()]
        if tags:
            meta["keywords"] = json.dumps(tags, ensure_ascii=False)

    # ── 文档类型 ──
    if item_type:
        meta["doc_type"] = TYPE_MAP.get(item_type, r["type"])

    # ── 语言 ──
    lang = normalize_language(r.get("language", ""))
    if lang:
        meta["language"] = lang

    # ── 来源/出版信息 ──
    # 组合 Publisher + Place + Edition 为 source 字段
    source_parts = []
    if r.get("publisher"):
        source_parts.append(r["publisher"])
    if r.get("place"):
        source_parts.append(r["place"])
    if r.get("edition"):
        source_parts.append(f"Edition: {r['edition']}")
    if r.get("series"):
        series_info = r["series"]
        if r.get("series_number"):
            series_info += f" #{r['series_number']}"
        source_parts.append(f"Series: {series_info}")
    if source_parts:
        meta["source"] = "; ".join(source_parts)

    # ── 卷号/期号/页码 → 拼接到 journal 末尾 ──
    vol_issue_pages = []
    if r.get("volume"):
        vol_issue_pages.append(f"Vol.{r['volume']}")
    if r.get("issue"):
        vol_issue_pages.append(f"No.{r['issue']}")
    if r.get("pages"):
        vol_issue_pages.append(f"pp.{r['pages']}")
    if vol_issue_pages and meta.get("journal"):
        meta["journal"] = f"{meta['journal']}, {', '.join(vol_issue_pages)}"
    elif vol_issue_pages:
        meta["journal"] = ", ".join(vol_issue_pages)

    # ── ISBN/ISSN → 存入 source 末尾 ──
    id_parts = []
    if r.get("isbn"):
        id_parts.append(f"ISBN:{r['isbn']}")
    if r.get("issn"):
        id_parts.append(f"ISSN:{r['issn']}")
    if id_parts:
        existing = meta.get("source", "")
        meta["source"] = f"{existing} ({', '.join(id_parts)})" if existing else ", ".join(id_parts)

    # ── URL ──
    # 不单独存字段，但如果有 DOI 而没有 URL，保留 URL 作为补充
    if r.get("url") and not r.get("doi"):
        # 无 DOI 时把 URL 存入 extra 信息
        existing = meta.get("source", "")
        meta["source"] = f"{existing} URL:{r['url']}" if existing else f"URL:{r['url']}"

    # ── 编辑/译者 → 存入 source ──
    contrib_parts = []
    if r.get("editor"):
        editors = [e.strip() for e in r["editor"].split(";") if e.strip()]
        if editors:
            contrib_parts.append(f"Editors: {', '.join(editors)}")
    if r.get("translator"):
        contrib_parts.append(f"Translator: {r['translator']}")
    if contrib_parts:
        existing = meta.get("source", "")
        meta["source"] = f"{existing}; {', '.join(contrib_parts)}" if existing else ", ".join(contrib_parts)

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
    parser.add_argument("--save-original", action="store_true", default=False,
                        help="额外保存原始 CSV 行数据到 source 字段（默认关闭，避免覆盖结构化信息）")
    parser.add_argument("--update-meta", action="store_true", default=True,
                        help="上传后自动更新元数据（默认开启）")
    parser.add_argument("--no-update-meta", action="store_false", dest="update_meta",
                        help="不更新元数据，仅上传文件")
    parser.add_argument("--debug", action="store_true", help="显示解析出的元数据详情")

    args = parser.parse_args()
    token = read_token(args.token)

    print(f"服务地址: {args.base_url}")
    print(f"CSV 文件: {args.csv_file}")
    print(f"匹配模式: {args.match_mode}")
    print(f"保存原始: {'是' if args.save_original else '否'}")
    print(f"Token: {'*' * 8 if token else '未设置'}")
    print()

    if not os.path.exists(args.csv_file):
        print(f"错误: CSV 文件不存在: {args.csv_file}")
        sys.exit(1)

    with open(args.csv_file, "r", encoding="utf-8-sig") as f:
        rows = list(csv.DictReader(f))

    print(f"共 {len(rows)} 条记录")
    print()

    stats = {"total": len(rows), "uploaded": 0, "skipped": 0, "failed": 0, "duplicate": 0, "meta_updated": 0}

    for i, row in enumerate(rows, 1):
        print(f"[{i}/{len(rows)}] ", end="")

        normalized = normalize_row(row)
        title = normalized.get("title", "") or normalized.get("file", "").split("/")[-1] or "未知标题"
        print(f"{title[:60]}")

        csv_doi = normalized.get("doi", "")

        # ── DOI 模式: 无 DOI 直接跳过 ──
        if args.match_mode == "doi" and not csv_doi:
            print(f"  跳过: 无 DOI")
            stats["skipped"] += 1
            continue

        # ── 解析文件路径（两种模式都需要，注册时要用）──
        file_field = normalized.get("file", "")
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

        # ── 解析元数据 ──
        metadata = parse_zotero_row(row)

        if args.debug and metadata:
            print(f"  [DEBUG] 元数据字段: {list(metadata.keys())}")
            for k, v in metadata.items():
                val_str = str(v)
                if len(val_str) > 80:
                    val_str = val_str[:80] + "..."
                print(f"    {k}: {val_str}")

        if args.save_original:
            # 将原始数据存为 JSON，追加到 source 字段末尾（不覆盖已有值）
            original_json = json.dumps(row, ensure_ascii=False)
            existing_source = metadata.get("source", "")
            if existing_source:
                metadata["source"] = f"{existing_source} | CSV:{original_json}"
            else:
                metadata["source"] = f"CSV:{original_json}"

        # ── 试运行 ──
        if args.dry_run:
            print(f"  [试运行] 将上传并更新 {len(metadata)} 个字段")
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
        if args.update_meta and metadata:
            if update_document_metadata(args.base_url, doc_id, metadata, token):
                print(f"  已更新 {len(metadata)} 个元数据字段")
                stats["meta_updated"] += 1
            else:
                print(f"  警告: 元数据更新失败")

        stats["uploaded"] += 1

    # ── 统计 ──

    print()
    print("=" * 50)
    print("导入完成!")
    print(f"  总计: {stats['total']}")
    print(f"  上传: {stats['uploaded']}")
    print(f"  元数据更新: {stats['meta_updated']}")
    print(f"  重复: {stats['duplicate']}")
    print(f"  跳过: {stats['skipped']}")
    print(f"  失败: {stats['failed']}")


if __name__ == "__main__":
    main()
