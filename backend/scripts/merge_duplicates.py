#!/usr/bin/env python3
"""合并重复文档。

按归一化标题分组，每组保留 ID 最小的文档（旧文档），
从同组其他文档（新文档）合并信息和文件到旧文档，然后删除新文档。

合并规则:
  - 元数据: 旧文档为空的字段，用新文档的值填充
  - PDF:    旧文档无 PDF 且新文档有 PDF，则复制
  - MD:     旧文档无 markdown 且新文档有 markdown，则复制
  - 状态:   取状态最高的（indexed > meta_done > markdown_done > uploaded）

用法:
    python scripts/merge_duplicates.py --dry-run       # 仅预览
    python scripts/merge_duplicates.py                 # 实际执行
"""

import argparse
import os
import re
import shutil
from collections import defaultdict
from pathlib import Path

from app.config import get_settings
from app.database import SessionLocal
from app.models import Document, DocStatus
from app.paths import get_pdf_path, get_markdown_path, get_asset_path

settings = get_settings()
UPLOADS_DIR = Path(settings.uploads_folder)

# 状态优先级
STATUS_RANK = {
    DocStatus.uploaded: 0,
    DocStatus.parsing: 1,
    DocStatus.markdown_done: 2,
    DocStatus.extracting: 3,
    DocStatus.meta_done: 4,
    DocStatus.indexing: 5,
    DocStatus.indexed: 6,
    DocStatus.error: -1,
}

# 可合并的元数据字段
META_FIELDS = [
    "title", "authors", "year", "doi", "source", "journal",
    "keywords", "abstract", "category", "doc_type", "language",
    "title_en", "authors_en", "keywords_en", "abstract_en", "journal_en",
]


def normalize(title: str) -> str:
    t = title.lower().strip()
    t = re.sub(r'[^a-z0-9一-鿿]+', '', t)
    return t


def higher_status(a: DocStatus, b: DocStatus) -> DocStatus:
    return a if STATUS_RANK.get(a, 0) >= STATUS_RANK.get(b, 0) else b


def main():
    parser = argparse.ArgumentParser(description="合并重复文档")
    parser.add_argument("--dry-run", action="store_true", help="仅预览")
    args = parser.parse_args()

    db = SessionLocal()
    try:
        # 查找重复分组
        docs = db.query(Document).filter(
            Document.title.isnot(None), Document.title != ""
        ).all()

        groups = defaultdict(list)
        for doc in docs:
            key = normalize(doc.title)
            if key:
                groups[key].append(doc)

        dup_groups = {k: v for k, v in groups.items() if len(v) >= 2}
        if not dup_groups:
            print("未发现重复文档。")
            return

        total_groups = len(dup_groups)
        total_docs = sum(len(v) for v in dup_groups.values())
        to_delete = 0

        print(f"发现 {total_groups} 组重复，共 {total_docs} 条文档\n")

        for key, group_docs in sorted(dup_groups.items(), key=lambda x: -len(x[1])):
            # 按 ID 排序，最小的为保留目标
            group_docs.sort(key=lambda d: d.id)
            keeper = group_docs[0]
            others = group_docs[1:]

            sample_title = keeper.title or key
            print(f"📋 {sample_title[:60]}  (保留 #{keeper.id}, 合并 {len(others)} 条)")

            for src in others:
                merged_items = []

                # 合并元数据
                for field in META_FIELDS:
                    old_val = getattr(keeper, field, None)
                    new_val = getattr(src, field, None)
                    if not old_val and new_val:
                        setattr(keeper, field, new_val)
                        merged_items.append(field)

                # 合并状态
                if STATUS_RANK.get(src.status, 0) > STATUS_RANK.get(keeper.status, 0):
                    keeper.status = src.status
                    keeper.status_message = src.status_message
                    keeper.progress = src.progress
                    merged_items.append("status")

                # 合并 PDF 文件（路径由 system.json 推导）
                src_pdf_abs = get_pdf_path(src.id)
                keeper_pdf_abs = get_pdf_path(keeper.id)
                if not os.path.exists(keeper_pdf_abs) and os.path.exists(src_pdf_abs):
                    os.makedirs(os.path.dirname(keeper_pdf_abs), exist_ok=True)
                    shutil.copy2(src_pdf_abs, keeper_pdf_abs)
                    merged_items.append("pdf")

                # 合并 markdown 文件（路径由 system.json 推导）
                src_md_abs = get_markdown_path(src.id)
                keeper_md_abs = get_markdown_path(keeper.id)
                if not os.path.exists(keeper_md_abs) and os.path.exists(src_md_abs):
                    os.makedirs(os.path.dirname(keeper_md_abs), exist_ok=True)
                    shutil.copy2(src_md_abs, keeper_md_abs)
                    merged_items.append("markdown")

                # 合并 qdrant_collection
                if not keeper.qdrant_collection and src.qdrant_collection:
                    keeper.qdrant_collection = src.qdrant_collection
                    merged_items.append("qdrant")

                if merged_items:
                    print(f"  ← #{src.id}: 合并 {', '.join(merged_items)}")
                else:
                    print(f"  ← #{src.id}: 无新信息")

                to_delete += 1

            if not args.dry_run:
                # 删除被合并的文档
                for src in others:
                    # 删除解析临时目录
                    src_dir = UPLOADS_DIR / str(src.id)
                    if src_dir.exists():
                        shutil.rmtree(src_dir, ignore_errors=True)
                    # 删除推导出的规范文件
                    for p in (get_pdf_path(src.id), get_markdown_path(src.id), get_asset_path(src.id)):
                        if os.path.exists(p):
                            try:
                                os.remove(p)
                            except OSError:
                                pass
                    # 删除 Qdrant 点
                    if src.qdrant_collection:
                        try:
                            from app.services.qdrant import delete_document_points
                            delete_document_points(0, src.id)
                        except Exception:
                            pass
                    db.delete(src)

        if args.dry_run:
            print(f"\n[预览模式] 将删除 {to_delete} 条重复文档。去掉 --dry-run 以执行。")
        else:
            db.commit()
            print(f"\n完成: 合并并删除了 {to_delete} 条重复文档。")

    finally:
        db.close()


if __name__ == "__main__":
    main()
