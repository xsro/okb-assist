#!/usr/bin/env python3
"""从 Qdrant 恢复 markdown 文件。

从 Qdrant 的 documents_0 集合中读取所有文档的文本分块，
按 document_id 分组、按 chunk_index 排序后拼接为 markdown，
写入 uploads/{id}/{id}.md，并更新数据库记录。
"""

import os
import sys

from app.config import get_settings
from app.database import SessionLocal
from app.models import Document
from app.services.qdrant import get_qdrant_client

settings = get_settings()
UPLOADS_DIR = settings.uploads_folder

BATCH_SIZE = 500  # Qdrant scroll batch size


def main():
    client = get_qdrant_client()
    collection = f"{settings.qdrant_collection}_0"

    # 1. Scroll all points, group by document_id
    print("正在从 Qdrant 读取所有分块...")
    chunks: dict[int, list] = {}  # document_id -> [(chunk_index, text)]
    offset = None
    total_read = 0

    while True:
        result = client.scroll(
            collection_name=collection,
            limit=BATCH_SIZE,
            offset=offset,
            with_payload=True,
            with_vectors=False,
        )
        points, next_offset = result

        for point in points:
            meta = point.payload.get("metadata", {})
            doc_id = meta.get("document_id")
            chunk_index = meta.get("chunk_index", 0)
            text = point.payload.get("text", "")
            if doc_id is not None:
                chunks.setdefault(doc_id, []).append((chunk_index, text))

        total_read += len(points)
        print(f"\r  已读取 {total_read} 个分块...", end="", flush=True)

        if next_offset is None:
            break
        offset = next_offset

    print(f"\n共读取 {total_read} 个分块，涉及 {len(chunks)} 个文档")

    # 2. Reconstruct markdown and write files
    os.makedirs(UPLOADS_DIR, exist_ok=True)

    db = SessionLocal()
    success = 0
    failed = 0

    try:
        all_docs = db.query(Document).all()
        doc_map = {d.id: d for d in all_docs}

        for doc_id, chunk_list in sorted(chunks.items()):
            # Sort by chunk_index
            chunk_list.sort(key=lambda x: x[0])
            markdown = "\n\n".join(text for _, text in chunk_list)

            # Write file: uploads/{id}/{id}.md
            doc_dir = os.path.join(UPLOADS_DIR, str(doc_id))
            os.makedirs(doc_dir, exist_ok=True)
            md_path = os.path.join(doc_dir, f"{doc_id}.md")

            try:
                with open(md_path, "w", encoding="utf-8") as f:
                    f.write(markdown)

                # Update database
                doc = doc_map.get(doc_id)
                if doc:
                    doc.markdown_path = md_path

                success += 1
                print(f"\r  已恢复 {success} 个文档...", end="", flush=True)
            except Exception as e:
                failed += 1
                print(f"\n  ✗ 文档 {doc_id} 恢复失败: {e}")

        db.commit()
    finally:
        db.close()

    print(f"\n\n恢复完成: 成功 {success}，失败 {failed}")
    print(f"文件目录: {os.path.abspath(UPLOADS_DIR)}/")


if __name__ == "__main__":
    main()
