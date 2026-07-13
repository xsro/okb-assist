#!/usr/bin/env python3
"""
从数据库 source 字段（Zotero JSON）更新文档元数据

从 source 字段中解析并补充以下字段：
  - Title → title
  - Author → authors（JSON list，"Last, First" → "First Last"）
  - Publication Year → year
  - DOI → doi
  - Item Type → doc_type
  - Publication Title → journal
  - Abstract Note → abstract
  - Tags → keywords（JSON list）
  - Language → language（优先使用 Zotero 语言字段，其次根据期刊名判断）

使用后端 API 更新。

使用方法:
    python scripts/update_from_source.py [选项]

示例:
    python scripts/update_from_source.py --base-url http://192.168.1.185:5001 --token we-love-control-and-network
    python scripts/update_from_source.py --dry-run
    python scripts/update_from_source.py --force  # 覆盖已有值
"""

import argparse
import json
import os
import re
import sys
from pathlib import Path

import httpx


# ──────────────────────────────────────────────
# Zotero Item Type → doc_type 映射
# ──────────────────────────────────────────────

TYPE_MAP = {
    "journalArticle": "article",
    "conferencePaper": "conference",
    "book": "book",
    "bookSection": "book",
    "thesis": "thesis",
    "report": "report",
    "preprint": "preprint",
    "webpage": "webpage",
}

# Zotero Language → 标准语言代码映射
LANG_MAP = {
    "chinese": "zh",
    "zh": "zh",
    "english": "en",
    "en": "en",
    "japanese": "ja",
    "ja": "ja",
    "korean": "ko",
    "ko": "ko",
    "french": "fr",
    "fr": "fr",
    "german": "de",
    "de": "de",
    "russian": "ru",
    "ru": "ru",
    "spanish": "es",
    "es": "es",
    "portuguese": "pt",
    "pt": "pt",
}


# ──────────────────────────────────────────────
# 中文检测
# ──────────────────────────────────────────────

def is_chinese(text: str) -> bool:
    """判断文本是否包含中文字符（至少有 2 个中文字符视为中文）。"""
    if not text:
        return False
    chinese_chars = re.findall(r'[一-鿿]', text)
    return len(chinese_chars) >= 2


def detect_language(pub_title: str, zotero_lang: str = "") -> str | None:
    """
    检测语言。优先使用 Zotero 提供的 Language 字段，
    兜底根据 Publication Title 判断中英文。
    """
    # 优先用 Zotero 语言字段
    if zotero_lang:
        lang = zotero_lang.strip().lower()
        if lang in LANG_MAP:
            return LANG_MAP[lang]
        # 尝试匹配前缀，如 "zh-CN" → "zh"
        for key, val in LANG_MAP.items():
            if lang.startswith(key):
                return val

    # 兜底：根据 Publication Title 判断
    if pub_title:
        return "zh" if is_chinese(pub_title) else "en"

    return None


# ──────────────────────────────────────────────
# 作者名解析
# ──────────────────────────────────────────────

def parse_authors(author_str: str) -> list[str] | None:
    """
    解析 Zotero 作者字段。
    格式: "Doe, John; Smith, Jane" → ["John Doe", "Jane Smith"]
    """
    if not author_str or not author_str.strip():
        return None
    authors = []
    for a in author_str.split(";"):
        a = a.strip()
        if not a:
            continue
        if "," in a:
            last, first = a.split(",", 1)
            authors.append(f"{first.strip()} {last.strip()}")
        else:
            authors.append(a)
    return authors if authors else None


# ──────────────────────────────────────────────
# 关键词解析
# ──────────────────────────────────────────────

def parse_keywords(tags_str: str) -> list[str] | None:
    """
    解析 Zotero Tags 字段。
    格式: "tag1; tag2; tag3" → ["tag1", "tag2", "tag3"]
    """
    if not tags_str or not tags_str.strip():
        return None
    tags = [t.strip() for t in tags_str.split(";") if t.strip()]
    return tags if tags else None


# ──────────────────────────────────────────────
# API 调用
# ──────────────────────────────────────────────

def _headers(token: str = None) -> dict:
    h = {"Content-Type": "application/json"}
    if token:
        h["X-Token"] = token
    return h


def list_documents(base_url: str, page: int = 1, page_size: int = 50, token: str = None) -> dict | None:
    """获取文档列表（分页）。"""
    try:
        with httpx.Client(timeout=30) as client:
            resp = client.get(
                f"{base_url}/assist/api/documents/",
                params={"page": page, "page_size": page_size},
                headers=_headers(token),
            )
            if resp.status_code == 200:
                return resp.json()
    except Exception as e:
        print(f"  获取文档列表失败: {e}")
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
            print(f"  更新失败: {resp.status_code} - {resp.text[:200]}")
    except Exception as e:
        print(f"  更新失败: {e}")
    return False


# ──────────────────────────────────────────────
# Token 读取
# ──────────────────────────────────────────────

def read_token(args_token: str | None) -> str | None:
    if args_token:
        return args_token
    token = os.environ.get("OKB_ASSIST_TOKEN")
    if token:
        return token
    token_file = Path.home() / ".okb_assist_token"
    if token_file.exists():
        return token_file.read_text().strip()
    return None


# ──────────────────────────────────────────────
# 主流程
# ──────────────────────────────────────────────

def main():
    parser = argparse.ArgumentParser(description="从 source 字段更新文档元数据")
    parser.add_argument("--base-url", default="http://localhost:5001", help="OKB-Assist 服务地址")
    parser.add_argument("--token", default=None, help="访问令牌")
    parser.add_argument("--dry-run", action="store_true", help="试运行，不实际更新")
    parser.add_argument("--force", action="store_true", help="覆盖已有值（默认仅填充空值）")
    parser.add_argument("--limit", type=int, default=0, help="最多处理 N 条文档（0=全部）")

    args = parser.parse_args()
    token = read_token(args.token)

    print(f"服务地址: {args.base_url}")
    print(f"Token: {'*' * 8 if token else '未设置'}")
    print(f"模式: {'试运行' if args.dry_run else '覆盖' if args.force else '仅填充空值'}")
    print()

    # ── 分页获取所有文档 ──
    all_docs = []
    page = 1
    page_size = 100
    while True:
        result = list_documents(args.base_url, page=page, page_size=page_size, token=token)
        if not result or not result.get("items"):
            break
        all_docs.extend(result["items"])
        if len(all_docs) >= result.get("total", 0):
            break
        page += 1

    if args.limit > 0:
        all_docs = all_docs[:args.limit]

    print(f"共获取 {len(all_docs)} 个文档")
    print()

    stats = {
        "total": len(all_docs),
        "updated": 0,
        "skipped_no_source": 0,
        "skipped_no_change": 0,
        "failed": 0,
        "fields": {
            "title": 0, "authors": 0, "year": 0, "doi": 0,
            "doc_type": 0, "journal": 0, "abstract": 0, "keywords": 0, "language": 0,
        },
    }

    for i, doc in enumerate(all_docs, 1):
        doc_id = doc["id"]
        title = (doc.get("title") or "")[:50]
        source = doc.get("source") or ""

        print(f"[{i}/{len(all_docs)}] ID={doc_id} {title}...")

        # ── 跳过非 JSON source ──
        if not source or not source.startswith("{"):
            print(f"  跳过: source 非 JSON 格式")
            stats["skipped_no_source"] += 1
            continue

        # ── 解析 source JSON ──
        try:
            source_data = json.loads(source)
        except json.JSONDecodeError:
            print(f"  跳过: source JSON 解析失败")
            stats["skipped_no_source"] += 1
            continue

        # ── 提取字段 ──
        item_type = source_data.get("Item Type", "").strip()
        pub_title = source_data.get("Publication Title", "").strip()
        zotero_lang = source_data.get("Language", "").strip()
        title = source_data.get("Title", "").strip()
        author_str = source_data.get("Author", "").strip()
        pub_year = source_data.get("Publication Year", "").strip()
        doi = source_data.get("DOI", "").strip()
        abstract = source_data.get("Abstract Note", "").strip()
        tags = source_data.get("Tags", "").strip()

        new_doc_type = TYPE_MAP.get(item_type, item_type if item_type else None)
        new_journal = pub_title if pub_title else None
        new_language = detect_language(pub_title, zotero_lang)
        new_title = title if title else None
        new_authors = parse_authors(author_str)
        new_year = None
        if pub_year:
            try:
                new_year = int(pub_year)
            except ValueError:
                pass
        new_doi = doi if doi else None
        new_abstract = abstract if abstract else None
        new_keywords = parse_keywords(tags)

        # ── 决定更新内容 ──
        update = {}
        current_doc_type = doc.get("doc_type") or ""
        current_journal = doc.get("journal") or ""
        current_language = doc.get("language") or ""
        current_title = doc.get("title") or ""
        current_authors = doc.get("authors") or ""
        current_year = doc.get("year")
        current_doi = doc.get("doi") or ""
        current_abstract = doc.get("abstract") or ""
        current_keywords = doc.get("keywords") or ""

        if new_title and (args.force or not current_title):
            update["title"] = new_title
        if new_authors and (args.force or not current_authors):
            update["authors"] = json.dumps(new_authors, ensure_ascii=False)
        if new_year is not None and (args.force or not current_year):
            update["year"] = new_year
        if new_doi and (args.force or not current_doi):
            update["doi"] = new_doi
        if new_doc_type and (args.force or not current_doc_type):
            update["doc_type"] = new_doc_type
        if new_journal and (args.force or not current_journal):
            update["journal"] = new_journal
        if new_abstract and (args.force or not current_abstract):
            update["abstract"] = new_abstract
        if new_keywords and (args.force or not current_keywords):
            update["keywords"] = json.dumps(new_keywords, ensure_ascii=False)
        if new_language and (args.force or not current_language):
            update["language"] = new_language

        if not update:
            print(f"  跳过: 无需更新")
            stats["skipped_no_change"] += 1
            continue

        # ── 显示变更 ──
        changes = []
        if "title" in update:
            changes.append(f"title: {current_title[:30] or '(空)'} → {update['title'][:30]}")
        if "authors" in update:
            changes.append(f"authors: {current_authors[:30] or '(空)'} → {update['authors'][:30]}")
        if "year" in update:
            changes.append(f"year: {current_year or '(空)'} → {update['year']}")
        if "doi" in update:
            changes.append(f"doi: {current_doi[:30] or '(空)'} → {update['doi'][:30]}")
        if "doc_type" in update:
            changes.append(f"doc_type: {current_doc_type or '(空)'} → {update['doc_type']}")
        if "journal" in update:
            changes.append(f"journal: {current_journal or '(空)'} → {update['journal']}")
        if "abstract" in update:
            changes.append(f"abstract: (空) → {update['abstract'][:30]}...")
        if "keywords" in update:
            changes.append(f"keywords: {current_keywords[:30] or '(空)'} → {update['keywords'][:30]}")
        if "language" in update:
            changes.append(f"language: {current_language or '(空)'} → {update['language']}")
        print(f"  更新: {'; '.join(changes)}")

        # ── 执行更新 ──
        if args.dry_run:
            stats["updated"] += 1
            for field in update:
                stats["fields"][field] = stats["fields"].get(field, 0) + 1
            continue

        if update_document_metadata(args.base_url, doc_id, update, token):
            stats["updated"] += 1
            for field in update:
                stats["fields"][field] = stats["fields"].get(field, 0) + 1
        else:
            stats["failed"] += 1

    # ── 统计 ──
    print()
    print("=" * 50)
    print("完成!")
    print(f"  总计: {stats['total']}")
    print(f"  更新: {stats['updated']}")
    print(f"  跳过(无source): {stats['skipped_no_source']}")
    print(f"  跳过(无变更): {stats['skipped_no_change']}")
    print(f"  失败: {stats['failed']}")
    print()
    print("各字段更新次数:")
    for field, count in stats["fields"].items():
        if count > 0:
            print(f"  {field}: {count}")


if __name__ == "__main__":
    main()
