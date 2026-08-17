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
import pathlib
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

def parse_file_paths(file_field: str, storage_root: str | None = None) -> list[str]:
    """
    解析 Zotero file 字段，返回存在的文件路径列表。

    支持格式:
      - /path/to/file.pdf
      - /path/to/a.pdf;/path/to/b.pdf
      - /path/to/file.pdf:application/pdf:1234

    storage_root: 可选，用于把 Windows Zotero 路径映射为本地 storage 文件夹。
      例如 Windows 路径 C:\\Users\\X\\Zotero\\storage\\HASH\\file.pdf
      会尝试映射为 {storage_root}/HASH/file.pdf。
    """
    if not file_field:
        return []

    paths = []
    for part in file_field.split(";"):
        part = part.strip()
        if not part:
            continue
        # 去掉 Zotero 附加的 :type:size 后缀
        path = part
        candidates = [path]
        if storage_root and not os.path.exists(path):
            # Map Windows Zotero path -> local storage_root (storage_root == the Zotero 'storage' folder)
            p = pathlib.PureWindowsPath(path)
            if len(p.parts) > 5:  # C:\Users\X\Zotero\storage\HASH\file.pdf
                rel = os.path.join(*p.parts[5:])
                candidates.append(os.path.join(storage_root, rel))
        for c in candidates:
            if c and os.path.exists(c) and c.endswith("pdf"):
                paths.append(c)
                break
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
    "journalarticle": "journalArticle",
    "journal article": "journalArticle",
    "article": "journalArticle",
    "book": "book",
    "booksection": "bookSection",
    "book section": "bookSection",
    "conferencepaper": "conferencePaper",
    "conference paper": "conferencePaper",
    "conference proceedings": "conferencePaper",
    "proceedingsarticle": "conferencePaper",
    "proceedings article": "conferencePaper",
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
    "newspaperarticle": "journalArticle",
    "newspaper article": "journalArticle",
    "magazinearticle": "journalArticle",
    "magazine article": "journalArticle",
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


def print_doc_detail(d: dict) -> None:
    """打印单篇文献的可读详情。"""
    meta = d.get("meta", {}) or {}
    print(f"  ── {d.get('title') or '(无标题)'} ──")
    authors = meta.get("authors")
    if authors:
        try:
            authors = json.loads(authors)
        except (json.JSONDecodeError, TypeError):
            pass
    if authors:
        print(f"  作者: {', '.join(authors) if isinstance(authors, list) else authors}")
    if meta.get("year"):
        print(f"  年份: {meta['year']}")
    if meta.get("journal"):
        print(f"  期刊/来源: {meta['journal']}")
    if d.get("doi"):
        print(f"  DOI: {d['doi']}")
    if meta.get("language"):
        print(f"  语言: {meta['language']}")
    if meta.get("doc_type"):
        print(f"  文献类型: {meta['doc_type']}")
    abstract = meta.get("abstract")
    if abstract:
        abstract = abstract if len(abstract) <= 300 else abstract[:300] + "..."
        print(f"  摘要: {abstract}")
    print(f"  文件: {d.get('file_path')}")


def upload_one(d: dict, args, token: str | None) -> bool:
    """上传单篇文献。返回是否成功。"""
    if args.dry_run:
        print(f"  [dry-run] 跳过上传: {d['doi']} ({d['file_path']})")
        return True
    try:
        res = upload_document(args.base_url, d["file_path"], token)
        if not res:
            print(f"  上传失败 {d['doi']}: 无返回结果")
            return False
        doc_id = res["id"]
        if args.update_meta:
            update_document_metadata(args.base_url, doc_id, d["meta"], token)
        print(f"  已上传: {d['doi']} -> id={doc_id}")
        return True
    except Exception as e:
        print(f"  上传失败 {d['doi']}: {e}")
        return False


def main():
    parser = argparse.ArgumentParser(description="导入 Zotero CSV 到 OKB-Assist（交互式 diff-then-upload）")
    parser.add_argument("csv_file", help="Zotero 导出的 CSV 文件路径")
    parser.add_argument("--base-url", default="http://192.168.1.122:5001", help="OKB-Assist 服务地址")
    parser.add_argument("--token", default=None, help="访问令牌")
    parser.add_argument("--storage-root", default=None,
                        help="Zotero storage 文件夹的本地路径，用于把 Windows 附件路径映射到本地")
    parser.add_argument("--dry-run", action="store_true", help="试运行，不实际上传")
    parser.add_argument("--update-meta", action="store_true", default=True,
                        help="上传后自动更新元数据（默认开启）")
    parser.add_argument("--no-update-meta", action="store_false", dest="update_meta",
                        help="不更新元数据，仅上传文件")
    parser.add_argument("--debug", action="store_true", help="显示解析出的元数据详情")

    args = parser.parse_args()
    token = read_token(args.token)

    print(f"服务地址: {args.base_url}")
    print(f"CSV 文件: {args.csv_file}")
    print(f"Storage root: {args.storage_root or '(未设置)'}")
    print(f"Token: {'*' * 8 if token else '未设置'}")
    print()

    if not os.path.exists(args.csv_file):
        print(f"错误: CSV 文件不存在: {args.csv_file}")
        sys.exit(1)

    # ── 1. 读取 CSV，构建本地可上传文档列表 ──
    with open(args.csv_file, "r", encoding="utf-8-sig") as f:
        rows = list(csv.DictReader(f))

    local_docs = []
    for row in rows:
        nrow = normalize_row(row)
        doi = (nrow.get("doi") or "").strip()
        if not doi:
            continue  # DOI-based flow; skip rows w/o DOI
        file_field = nrow.get("file", "")
        paths = parse_file_paths(file_field, storage_root=args.storage_root)
        file_path = get_newest_file(paths)
        if not file_path:
            continue  # not locally uploadable
        meta = parse_zotero_row(row)
        local_docs.append({"doi": doi, "file_path": file_path, "title": meta.get("title"), "meta": meta})

    if not local_docs:
        print("没有本地可上传的文件（或全部缺少 DOI）。")
        return

    # ── 2. 与服务器做 diff ──
    dois = [d["doi"] for d in local_docs]
    try:
        with httpx.Client(timeout=120) as client:
            resp = client.post(
                f"{args.base_url}/assist/api/documents/diff-dois",
                json={"dois": dois},
                headers={"X-Token": token, "Content-Type": "application/json"},
            )
        if resp.status_code != 200:
            print(f"错误: diff-dois 请求失败: {resp.status_code} - {resp.text[:200]}")
            return
        diff = resp.json()
    except Exception as e:
        print(f"错误: 无法连接服务器进行 diff: {e}")
        return

    missing = set(diff.get("missing", []))
    to_upload = [d for d in local_docs if d["doi"] in missing]

    total = len(local_docs)
    present_count = diff.get("present_count", len(local_docs) - len(to_upload))
    vacant_count = len(to_upload)

    print(f"本地可上传（含 DOI 且有文件）: {total}")
    print(f"服务器已存在: {present_count}")
    print(f"服务器空缺（待上传）: {vacant_count}")
    print()
    for i, d in enumerate(to_upload, 1):
        print(f"  [{i}] {d['doi']}  {d['title']}")

    if not to_upload:
        print("无空缺文献，无需上传。")
        return

    # ── 3. 交互选择 ──
    choice = input("请选择 [a] 全部上传  [s] 逐个上传  [q] 退出: ").strip().lower()
    if choice == "q":
        return
    if choice == "a":
        uploaded = 0
        failed = 0
        for d in to_upload:
            if upload_one(d, args, token):
                uploaded += 1
            else:
                failed += 1
    elif choice == "s":
        uploaded = 0
        failed = 0
        for d in to_upload:
            print_doc_detail(d)
            ans = input("上传这篇？[y/N]: ").strip().lower()
            if ans == "y":
                if upload_one(d, args, token):
                    uploaded += 1
                else:
                    failed += 1
            else:
                print("  跳过。")
    else:
        print("无效选择，退出。")
        return

    # ── 4. 最终统计 ──
    print()
    print("=" * 50)
    print(f"已上传: {uploaded}")
    print(f"失败: {failed}")


if __name__ == "__main__":
    main()
