#!/usr/bin/env python3
"""
将数据库中所有文献类型统一为 Zotero 标准类型名称。

运行方式:
    python scripts/normalize_doc_types.py [--dry-run]

映射规则:
    article / journalArticle        → journalArticle
    conference / conferencePaper / inproceedings → conferencePaper
    thesis / 硕士学位论文 / 博士学位论文   → thesis
    book                            → book
    bookSection                     → bookSection
    preprint                        → preprint
    report                          → report
    webpage                         → webpage
    其他未知值                       → document
"""

import sys
import os

# 将项目根目录加入 path
sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

from app.database import SessionLocal
from app.models import Document

# ── 映射表：各种写法 → Zotero 标准类型 ──
TYPE_NORMALIZE = {
    # article 系列
    "article": "journalArticle",
    "journalarticle": "journalArticle",
    "journal article": "journalArticle",
    "journalArticle": "journalArticle",

    # conference 系列
    "conference": "conferencePaper",
    "conferencepaper": "conferencePaper",
    "conference paper": "conferencePaper",
    "conferencepaper": "conferencePaper",
    "inproceedings": "conferencePaper",
    "conference proceedings": "conferencePaper",
    "proceedingsarticle": "conferencePaper",
    "proceedings article": "conferencePaper",

    # thesis 系列
    "thesis": "thesis",
    "dissertation": "thesis",
    "硕士学位论文": "thesis",
    "博士学位论文": "thesis",
    "mastersthesis": "thesis",
    "phdthesis": "thesis",

    # book 系列
    "book": "book",
    "booksection": "bookSection",
    "book section": "bookSection",
    "bookSection": "bookSection",

    # 其他常见类型
    "preprint": "preprint",
    "report": "report",
    "technicalreport": "report",
    "technical report": "report",
    "webpage": "webpage",
    "document": "document",
    "presentation": "presentation",
    "manuscript": "manuscript",
    "patent": "patent",
    "review": "review",
    "newspaperarticle": "journalArticle",
    "newspaper article": "journalArticle",
    "magazinearticle": "journalArticle",
    "magazine article": "journalArticle",
}


def normalize_type(raw: str) -> str:
    """将任意 doc_type 值标准化为 Zotero 类型。"""
    if not raw:
        return ""
    raw = raw.strip()
    # 先精确匹配
    if raw in TYPE_NORMALIZE:
        return TYPE_NORMALIZE[raw]
    # 小写匹配
    lower = raw.lower()
    if lower in TYPE_NORMALIZE:
        return TYPE_NORMALIZE[lower]
    # 无法识别的归为 document
    return "document"


def main():
    dry_run = "--dry-run" in sys.argv

    db = SessionLocal()
    try:
        # 查询所有有 doc_type 的文档
        docs = db.query(Document).filter(
            Document.doc_type.isnot(None),
            Document.doc_type != "",
        ).all()

        print(f"共 {len(docs)} 条有类型的文献")
        print()

        changes = []
        for doc in docs:
            old_type = doc.doc_type.strip()
            new_type = normalize_type(old_type)
            if old_type != new_type:
                changes.append((doc.id, old_type, new_type))

        # 统计
        from collections import Counter
        type_changes = Counter()
        for _, old, new in changes:
            type_changes[(old, new)] += 1

        print("需要修改的映射:")
        for (old, new), count in type_changes.most_common():
            print(f"  {old:30s} → {new:20s}  ({count} 条)")
        print()
        print(f"共需修改 {len(changes)} 条记录")

        if dry_run:
            print("\n[试运行] 未实际修改")
            return

        if not changes:
            print("无需修改")
            return

        # 执行修改
        for doc_id, old_type, new_type in changes:
            db.query(Document).filter(Document.id == doc_id).update({"doc_type": new_type})

        db.commit()
        print(f"\n已修改 {len(changes)} 条记录")

    finally:
        db.close()


if __name__ == "__main__":
    main()
