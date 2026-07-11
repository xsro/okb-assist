#!/usr/bin/env python3
"""
数据库迁移脚本：创建 document_vector_index 表，并迁移现有数据。

将现有的 qdrant_collection 和 vector_db_id 数据迁移到新表中。
"""
import sqlite3
import sys
from pathlib import Path
from datetime import datetime

DB_PATH = Path(__file__).parent.parent / "okb_assist.db"


def migrate():
    if not DB_PATH.exists():
        print(f"数据库文件不存在: {DB_PATH}")
        sys.exit(1)

    conn = sqlite3.connect(DB_PATH)
    cursor = conn.cursor()

    # 1. 检查新表是否已存在
    cursor.execute("""
        SELECT name FROM sqlite_master
        WHERE type='table' AND name='document_vector_index'
    """)
    if cursor.fetchone():
        print("表 document_vector_index 已存在，跳过创建")
    else:
        # 创建新表
        print("创建 document_vector_index 表...")
        cursor.execute("""
            CREATE TABLE document_vector_index (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                document_id INTEGER NOT NULL,
                vector_db_id VARCHAR(50) NOT NULL,
                collection_name VARCHAR(100),
                status VARCHAR(20) DEFAULT 'pending',
                error_message TEXT,
                created_at DATETIME,
                updated_at DATETIME,
                FOREIGN KEY (document_id) REFERENCES documents(id) ON DELETE CASCADE,
                UNIQUE(document_id, vector_db_id)
            )
        """)
        print("表创建成功")

    # 2. 迁移现有数据
    print("\n迁移现有索引数据...")

    # 查找已有索引的文档（状态为 indexed 且有 qdrant_collection）
    cursor.execute("""
        SELECT id, vector_db_id, qdrant_collection
        FROM documents
        WHERE status = 'indexed'
        AND qdrant_collection IS NOT NULL
        AND qdrant_collection != ''
    """)
    indexed_docs = cursor.fetchall()
    print(f"找到 {len(indexed_docs)} 个已索引的文档")

    migrated = 0
    skipped = 0
    now = datetime.utcnow().isoformat()

    for doc_id, vector_db_id, collection_name in indexed_docs:
        # 默认使用 "default" 如果 vector_db_id 为空
        db_id = vector_db_id if vector_db_id else "default"

        # 检查是否已存在记录
        cursor.execute("""
            SELECT id FROM document_vector_index
            WHERE document_id = ? AND vector_db_id = ?
        """, (doc_id, db_id))

        if cursor.fetchone():
            skipped += 1
            continue

        # 插入新记录
        cursor.execute("""
            INSERT INTO document_vector_index
            (document_id, vector_db_id, collection_name, status, created_at, updated_at)
            VALUES (?, ?, ?, 'indexed', ?, ?)
        """, (doc_id, db_id, collection_name, now, now))
        migrated += 1

    conn.commit()

    print(f"\n迁移完成:")
    print(f"  - 新增: {migrated} 条索引记录")
    print(f"  - 跳过: {skipped} 条（已存在）")

    # 3. 显示统计信息
    cursor.execute("SELECT COUNT(*) FROM document_vector_index")
    total = cursor.fetchone()[0]
    cursor.execute("SELECT status, COUNT(*) FROM document_vector_index GROUP BY status")
    stats = cursor.fetchall()

    print(f"\n索引统计 (共 {total} 条):")
    for status, count in stats:
        print(f"  - {status}: {count}")

    conn.close()


if __name__ == "__main__":
    print(f"数据库路径: {DB_PATH}")
    print()
    migrate()
