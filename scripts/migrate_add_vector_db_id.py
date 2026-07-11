#!/usr/bin/env python3
"""
数据库迁移脚本：添加 vector_db_id 列到 documents 表
"""
import sqlite3
import sys
from pathlib import Path

DB_PATH = Path(__file__).parent.parent / "okb_assist.db"


def migrate():
    if not DB_PATH.exists():
        print(f"数据库文件不存在: {DB_PATH}")
        sys.exit(1)

    conn = sqlite3.connect(DB_PATH)
    cursor = conn.cursor()

    # 检查列是否已存在
    cursor.execute("PRAGMA table_info(documents)")
    columns = [col[1] for col in cursor.fetchall()]

    if "vector_db_id" in columns:
        print("列 vector_db_id 已存在，跳过迁移")
        conn.close()
        return

    # 添加新列
    print("正在添加 vector_db_id 列...")
    cursor.execute("ALTER TABLE documents ADD COLUMN vector_db_id VARCHAR(50)")

    # 将现有记录的 vector_db_id 设置为默认值 "default"
    cursor.execute("UPDATE documents SET vector_db_id = 'default' WHERE vector_db_id IS NULL")
    updated = cursor.rowcount
    print(f"已更新 {updated} 条记录的 vector_db_id 为 'default'")

    conn.commit()
    conn.close()
    print("迁移完成!")


if __name__ == "__main__":
    migrate()
