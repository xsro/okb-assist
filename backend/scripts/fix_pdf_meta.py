#!/usr/bin/env python3
"""修复因 PDF 元数据提取失败而 title 为空（或退化为文件名）的文档记录。

背景：旧版 ``extract_pdf_metadata`` 只读取经典 Info 字典的 ``/Title``，
当 PDF 缺失 Info /Title 时，``doc.title`` 会被提交为 NULL；部分文档在导入
时退化为“文件名（去 .pdf）”作为标题。本脚本用改进版提取算法重新读取源
PDF，尝试从 XMP / 正文推断真实标题并回填：

- title：直接覆盖（修复 NULL 标题 与 “文件名即标题” 两类失败记录）；
- authors / year / doi / keywords / abstract：仅在当前字段为空时补全。

PDF 位于外部挂载盘，由 ``app.paths.get_pdf_path(doc_id)`` 解析；若文件
不存在则跳过并计入 “PDF 缺失”，不会报错。

默认直接落库并提交；加 --dry-run 仅统计与预览，不写库、不提交。

有两种修复范围：

- 默认（无 --retitle）：仅修复“标题为空（NULL / 空串）”或“标题退化为
  文件名 stem”的失败记录；已被错误写成“期刊引用条”的记录保持不变。
- --retitle：重新提取并覆盖标题。命中范围扩大为“标题为 NULL”或“标题
  看起来像期刊引用条（``_looks_like_citation`` 为真）”的文档；用改进版
  推断得到的真实论文标题覆盖旧值，从而修复那批被误题成卷期年号的记录。
  注意：--retitle 也会覆盖 NULL，因此两类错误一并修复。

PDF 位于外部挂载盘，由 ``app.paths.get_pdf_path(doc_id)`` 解析；若文件
不存在则跳过并计入 “PDF 缺失”，不会报错。

--dry-run 在两种模式下均安全（仅预览、显式回滚、绝不提交）。

用法：
    uv run python scripts/fix_pdf_meta.py --dry-run            # 试运行（仅空标题/文件名退化）
    uv run python scripts/fix_pdf_meta.py                      # 真正修复并提交（仅空标题/文件名退化）
    uv run python scripts/fix_pdf_meta.py --dry-run --retitle  # 试运行预览（含期刊引用条误题记录）
    uv run python scripts/fix_pdf_meta.py --retitle            # 重新提取并覆盖（真实修复并提交）
"""
import argparse
import json
from pathlib import Path

from app.database import SessionLocal
from app.models import Document
from app.paths import get_pdf_path
from app.services.pdf_meta import (
    extract_pdf_metadata,
    normalize_doi,
    _looks_like_filename,
    _looks_like_citation,
)


def _norm(s: str) -> str:
    """与 app.services.pdf_meta._norm_filename 一致的归一化（用于判断 title==文件名）。"""
    s = s.strip().lower()
    if s.endswith(".pdf"):
        s = s[:-4]
    for ch in "._- ":
        s = s.replace(ch, "")
    return s


def classify_failed(db, retitle: bool = False):
    """返回待修复文档字典 ``{doc_id: (reason, doc)}``。

    - (a) title IS NULL 或 ''：两种模式都纳入（reason='null'）；
    - 非 retitle 模式（默认）：额外纳入 (b) title 等于文件名 stem
      （归一化相等）的退化记录（reason='filename'）；
    - retitle 模式：额外纳入 (c) title 看起来像“期刊引用条”
      （``_looks_like_citation`` 为真）的误题记录（reason='citation'）。

    两类扩展（b/c）互斥且都不与 (a) 重叠；同一文档只计入一种原因。
    """
    failed = {}

    # (a) SQL 筛选空标题（两种模式均处理）
    null_docs = (
        db.query(Document)
        .filter((Document.title.is_(None)) | (Document.title == ""))
        .all()
    )
    for d in null_docs:
        failed[d.id] = ("null", d)

    for d in db.query(Document).all():
        if d.id in failed:
            continue
        title = d.title or ""
        if not title:
            continue

        if retitle:
            # (c) 期刊引用条误题：纳入以便重新提取覆盖
            if _looks_like_citation(title):
                failed[d.id] = ("citation", d)
        else:
            # (b) 文件名 stem 退化：在 Python 中归一化比较
            filename = d.filename or ""
            if not filename:
                continue
            stem = filename.lower().removesuffix(".pdf")
            if _norm(title) == _norm(stem):
                failed[d.id] = ("filename", d)

    return failed


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--dry-run", action="store_true",
        help="仅统计与预览，不写库、不提交；缺省为真实修复并提交",
    )
    parser.add_argument(
        "--retitle", action="store_true",
        help="重新提取并覆盖标题：修复范围扩大为 NULL 或期刊引用条误题记录",
    )
    args = parser.parse_args()
    dry_run = args.dry_run
    retitle = args.retitle

    db = SessionLocal()
    try:
        failed = classify_failed(db, retitle=retitle)
        total_failed = len(failed)

        processed = 0          # 实际读到 PDF 并参与修复
        skipped_missing = 0    # PDF 文件缺失，跳过
        title_fixed = 0        # title 被改写（非空且不同于原值）
        still_null = 0         # 修复后 title 仍为空
        examples = []          # 前后对比示例

        for doc_id, (reason, doc) in failed.items():
            pdf_path = get_pdf_path(doc_id)
            if not pdf_path or not Path(pdf_path).exists():
                skipped_missing += 1
                continue

            processed += 1
            content = Path(pdf_path).read_bytes()
            meta = extract_pdf_metadata(content, filename=doc.filename)

            old_title = doc.title
            changed_title = None

            # title：根据模式选择覆盖策略
            new_title = meta.get("title")
            if retitle:
                # --retitle：重新提取并覆盖。仅当新标题非期刊引用条、
                # 且与旧标题不同才覆盖（用于修复误题成卷期年号的记录，
                # 同时也覆盖 NULL 标题）。
                if (
                    new_title
                    and not _looks_like_citation(new_title)
                    and new_title != (doc.title or "")
                ):
                    changed_title = new_title
                    if not dry_run:
                        doc.title = new_title
            else:
                # 默认：优先覆盖（修复 NULL 与文件名退化两类）
                if new_title and not _looks_like_filename(new_title, doc.filename):
                    if new_title != (doc.title or ""):
                        changed_title = new_title
                        if not dry_run:
                            doc.title = new_title

            if changed_title is not None:
                title_fixed += 1
            elif not (doc.title or "").strip():
                # 修复后仍为空的（dry-run 下 doc.title 未改，原值即空）
                still_null += 1

            # 仅空补全：authors / year / doi / keywords / abstract
            fills = {}
            if not doc.authors and meta.get("authors"):
                fills["authors"] = json.dumps(meta["authors"], ensure_ascii=False)
            if doc.year is None and meta.get("year"):
                fills["year"] = meta["year"]
            if not doc.doi and meta.get("doi"):
                fills["doi"] = normalize_doi(meta["doi"])
            if not doc.keywords and meta.get("keywords"):
                fills["keywords"] = json.dumps(meta["keywords"], ensure_ascii=False)
            if not doc.abstract and meta.get("abstract"):
                fills["abstract"] = meta["abstract"]

            if not dry_run:
                for k, v in fills.items():
                    setattr(doc, k, v)

            if len(examples) < 10 and (changed_title is not None or fills):
                examples.append({
                    "id": doc_id,
                    "reason": reason,
                    "old_title": old_title,
                    "new_title": changed_title,
                    "fills": fills,
                })

        # 试运行显式回滚，确保绝不做任何落库（即使会话中存在脏数据）
        if dry_run:
            db.rollback()
        else:
            db.commit()

        # ---------------- 摘要 ----------------
        mode_label = ("DRY-RUN" if dry_run else "APPLY") + (" + RETITLE" if retitle else "")
        scope_label = "NULL / 期刊引用条误题" if retitle else "NULL / 文件名退化"
        print("=" * 70)
        print("FIX PDF META — " + mode_label + " MODE")
        print("=" * 70)
        print(f"修复范围                         : {scope_label}")
        print(f"失败记录总数                     : {total_failed}")
        print(f"  其中 PDF 缺失（跳过）          : {skipped_missing}")
        print(f"  实际处理（读到 PDF）           : {processed}")
        print(f"  标题被修复（已改写）           : {title_fixed}")
        print(f"  修复后标题仍为空               : {still_null}")
        print(f"  其它字段补全的文档数           : {sum(1 for e in examples if e['fills'])}")

        if examples:
            print("\n" + "-" * 70)
            print("前后对比示例（最多 5 条）")
            print("-" * 70)
            for e in examples:
                print(f"\nid={e['id']}  原因={e['reason']}")
                print(f"  old_title : {e['old_title']!r}")
                print(f"  new_title : {e['new_title']!r}")
                if e["fills"]:
                    for k, v in e["fills"].items():
                        shown = v if len(str(v)) <= 200 else str(v)[:200] + " ..."
                        print(f"  fill {k} : {shown!r}")

        if dry_run:
            print("\n[DRY-RUN] 未写入数据库，未提交。去掉 --dry-run 执行真实修复。")
        else:
            print("\n[APPLY] 已提交数据库更改。")
    finally:
        db.close()


if __name__ == "__main__":
    main()
