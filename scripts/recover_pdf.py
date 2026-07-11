#!/usr/bin/env python3
"""从磁盘恢复的文件夹中匹配并恢复 PDF 文件。

通过 SHA256 哈希匹配数据库中的文档记录，
将匹配到的 PDF 复制到 uploads/{id}/{id}.pdf，并更新数据库路径。

用法:
    python scripts/recover_pdf.py --dry-run       # 仅预览
    python scripts/recover_pdf.py                  # 实际执行
"""

import argparse
import hashlib
import os
import shutil

from app.config import get_settings
from app.database import SessionLocal
from app.models import Document
from app.utils import to_relative_path

settings = get_settings()
UPLOADS_DIR = settings.uploads_folder
RECOVERED_DIR = "/home/a422/recovered"


def file_sha256(path: str) -> str:
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def main():
    parser = argparse.ArgumentParser(description="从恢复文件夹恢复 PDF 文件")
    parser.add_argument("--dry-run", action="store_true", help="仅预览，不实际复制")
    parser.add_argument("--recovered-dir", default=RECOVERED_DIR, help="恢复文件夹路径")
    args = parser.parse_args()

    # 1. 扫描恢复文件夹中的所有 PDF
    print(f"扫描恢复文件夹: {args.recovered_dir}")
    pdf_files = []
    for root, dirs, files in os.walk(args.recovered_dir):
        for f in files:
            if f.lower().endswith(".pdf"):
                pdf_files.append(os.path.join(root, f))
    print(f"找到 {len(pdf_files)} 个 PDF 文件\n")

    # 2. 从数据库读取所有文档的哈希
    db = SessionLocal()
    try:
        docs = db.query(Document).all()
        # hash -> doc
        hash_map: dict[str, Document] = {}
        for d in docs:
            if d.file_hash:
                hash_map[d.file_hash] = d
        print(f"数据库文档数: {len(docs)}, 有哈希: {len(hash_map)}\n")

        # 3. 计算恢复文件的哈希并匹配
        matched = []   # (pdf_path, doc)
        unmatched = 0

        for i, pdf_path in enumerate(pdf_files):
            print(f"\r  哈希匹配中: {i+1}/{len(pdf_files)}", end="", flush=True)
            try:
                h = file_sha256(pdf_path)
                doc = hash_map.get(h)
                if doc:
                    matched.append((pdf_path, doc))
                else:
                    unmatched += 1
            except Exception:
                unmatched += 1

        print(f"\n\n匹配成功: {len(matched)}, 未匹配: {unmatched}")

        if not matched:
            print("没有匹配的文件，退出。")
            return

        # 4. 打印预览
        print("\n" + "=" * 70)
        print(f"{'操作':<6} {'ID':<6} {'原文件名':<50} {'大小'}")
        print("-" * 70)
        for pdf_path, doc in matched[:20]:
            size_mb = os.path.getsize(pdf_path) / 1024 / 1024
            fname = doc.filename[:48] if doc.filename else "-"
            print(f"{'复制':<6} {doc.id:<6} {fname:<50} {size_mb:.1f}MB")
        if len(matched) > 20:
            print(f"  ... 还有 {len(matched) - 20} 个文件")
        print("=" * 70)

        if args.dry_run:
            print(f"\n[预览模式] 共 {len(matched)} 个文件待恢复。去掉 --dry-run 以执行。")
            return

        # 5. 执行复制
        print(f"\n开始复制 {len(matched)} 个 PDF 文件...")
        success = 0
        failed = 0

        for pdf_path, doc in matched:
            doc_dir = os.path.join(UPLOADS_DIR, str(doc.id))
            os.makedirs(doc_dir, exist_ok=True)
            dest = os.path.join(doc_dir, f"{doc.id}.pdf")

            try:
                shutil.copy2(pdf_path, dest)
                # 存储相对路径
                doc.file_path = to_relative_path(dest)
                success += 1
                print(f"\r  已恢复 {success}/{len(matched)}", end="", flush=True)
            except Exception as e:
                failed += 1
                print(f"\n  ✗ id={doc.id} 失败: {e}")

        db.commit()
        print(f"\n\n恢复完成: 成功 {success}, 失败 {failed}")

    finally:
        db.close()


if __name__ == "__main__":
    main()
