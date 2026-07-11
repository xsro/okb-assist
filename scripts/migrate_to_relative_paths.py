#!/usr/bin/env python3
"""
数据库迁移脚本：将 file_path 和 markdown_path 从绝对路径转换为相对于 uploads_folder 的路径。
"""
import sqlite3
import sys
from pathlib import Path

DB_PATH = Path(__file__).parent.parent / "okb_assist.db"
UPLOADS_FOLDER = Path(__file__).parent.parent / "uploads"


def migrate():
    if not DB_PATH.exists():
        print(f"数据库文件不存在: {DB_PATH}")
        sys.exit(1)

    conn = sqlite3.connect(DB_PATH)
    cursor = conn.cursor()

    # 获取所有文档
    cursor.execute("SELECT id, file_path, markdown_path FROM documents")
    rows = cursor.fetchall()

    updated_count = 0
    skipped_count = 0
    errors = []

    for row in rows:
        doc_id, file_path, markdown_path = row
        new_file_path = file_path
        new_markdown_path = markdown_path
        changed = False

        # 转换 file_path
        if file_path:
            try:
                abs_path = Path(file_path).resolve()
                rel_path = abs_path.relative_to(UPLOADS_FOLDER.resolve())
                if str(rel_path) != file_path:
                    new_file_path = str(rel_path)
                    changed = True
            except ValueError:
                # 路径不在 uploads_folder 下，跳过
                errors.append({"id": doc_id, "field": "file_path", "error": f"路径不在 uploads 目录下: {file_path}"})
            except Exception as e:
                errors.append({"id": doc_id, "field": "file_path", "error": str(e)})

        # 转换 markdown_path
        if markdown_path:
            try:
                abs_path = Path(markdown_path).resolve()
                rel_path = abs_path.relative_to(UPLOADS_FOLDER.resolve())
                if str(rel_path) != markdown_path:
                    new_markdown_path = str(rel_path)
                    changed = True
            except ValueError:
                errors.append({"id": doc_id, "field": "markdown_path", "error": f"路径不在 uploads 目录下: {markdown_path}"})
            except Exception as e:
                errors.append({"id": doc_id, "field": "markdown_path", "error": str(e)})

        # 更新数据库
        if changed:
            try:
                cursor.execute(
                    "UPDATE documents SET file_path = ?, markdown_path = ? WHERE id = ?",
                    (new_file_path, new_markdown_path, doc_id)
                )
                updated_count += 1
            except Exception as e:
                errors.append({"id": doc_id, "error": f"更新失败: {str(e)}"})
        else:
            skipped_count += 1

    conn.commit()
    conn.close()

    print(f"迁移完成!")
    print(f"  - 更新: {updated_count} 条记录")
    print(f"  - 跳过: {skipped_count} 条记录 (已经是相对路径)")
    if errors:
        print(f"  - 错误: {len(errors)} 条记录")
        for err in errors[:10]:  # 只显示前10个错误
            print(f"    - ID {err.get('id')}: {err.get('error')}")
        if len(errors) > 10:
            print(f"    ... 还有 {len(errors) - 10} 个错误")


if __name__ == "__main__":
    print(f"数据库路径: {DB_PATH}")
    print(f"uploads 目录: {UPLOADS_FOLDER}")
    print()
    migrate()
