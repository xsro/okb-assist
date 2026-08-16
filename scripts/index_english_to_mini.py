#!/usr/bin/env python3
"""遍历英文文献，将存在且非空的markdown文件通过API提交到mini索引。

用法:
    python scripts/index_english_to_mini.py [--dry-run] [--limit N] [--base-url URL]

参数:
    --dry-run       只打印将要处理的文档，不实际索引
    --limit N       最多处理N个文档（用于测试）
    --base-url URL  后端API地址（默认 http://127.0.0.1:5001）
"""

import argparse
import json
import os
import sqlite3
import time

import requests


DB_PATH = os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__))), "okb_assist.db")


def get_english_docs_without_mini_index():
    """查询未索引到mini的英文文献。"""
    conn = sqlite3.connect(DB_PATH)
    cursor = conn.cursor()

    cursor.execute("""
        SELECT d.id, d.title
        FROM documents d
        WHERE d.language = 'en'
          AND NOT EXISTS (
              SELECT 1 FROM document_vector_index dvi
              WHERE dvi.document_id = d.id
                AND dvi.vector_db_id = 'mini'
                AND dvi.status = 'indexed'
          )
        ORDER BY d.id
    """)

    rows = cursor.fetchall()
    conn.close()
    return rows


def check_markdown_valid(doc_id: int) -> bool:
    """检查文档 markdown 文件（由 system.json 推导）是否存在且非空。"""
    try:
        from app.paths import get_markdown_path
    except Exception:
        # 兜底：直接读取 system.json 推导
        system_json = os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__))), "system.json")
        with open(system_json, "r") as f:
            template = json.load(f).get("markdown_path", "")
        abs_path = template.replace("{id}", str(doc_id))
    else:
        abs_path = get_markdown_path(doc_id)

    if not abs_path or not os.path.exists(abs_path):
        return False

    try:
        return os.path.getsize(abs_path) > 0
    except Exception:
        return False


def submit_index(doc_id: int, base_url: str, token: str = None) -> bool:
    """通过API提交索引任务。"""
    url = f"{base_url}/assist/api/pipeline/{doc_id}/index"
    params = {"vector_db_id": "mini"}
    headers = {}
    if token:
        headers["Authorization"] = f"Bearer {token}"

    try:
        resp = requests.post(url, params=params, headers=headers, timeout=10)
        if resp.status_code == 200:
            return True
        else:
            print(f"  [ERROR] HTTP {resp.status_code}: {resp.text[:200]}")
            return False
    except Exception as e:
        print(f"  [ERROR] 请求失败: {e}")
        return False


def main():
    parser = argparse.ArgumentParser(description="索引英文文献到mini向量数据库")
    parser.add_argument("--dry-run", action="store_true", help="只打印，不实际索引")
    parser.add_argument("--limit", type=int, default=0, help="最多处理N个文档")
    parser.add_argument("--base-url", default="http://127.0.0.1:5001", help="后端API地址")
    parser.add_argument("--token", default=None, help="认证token")
    args = parser.parse_args()

    docs = get_english_docs_without_mini_index()
    print(f"找到 {len(docs)} 个未索引到mini的英文文献")

    if args.limit > 0:
        docs = docs[:args.limit]
        print(f"限制处理前 {args.limit} 个")

    # 过滤有效文档
    valid_docs = []
    for doc_id, title in docs:
        if check_markdown_valid(doc_id):
            valid_docs.append((doc_id, title))
        else:
            print(f"  跳过 ID {doc_id}: markdown文件不存在或为空")

    print(f"有效文档: {len(valid_docs)} 个")

    if args.dry_run:
        print("\n[DRY RUN] 将要提交索引的文档:")
        for doc_id, title in valid_docs:
            print(f"  ID {doc_id}: {title[:70]}")
        return

    # 提交索引
    success = 0
    failed = 0

    for i, (doc_id, title) in enumerate(valid_docs, 1):
        print(f"[{i}/{len(valid_docs)}] 提交 ID {doc_id}: {title[:50]}...")
        if submit_index(doc_id, args.base_url, args.token):
            print(f"  ✓ 已提交")
            success += 1
        else:
            print(f"  ✗ 失败")
            failed += 1

        # 避免请求过快
        if i % 10 == 0:
            time.sleep(1)

    print(f"\n完成! 成功提交: {success}, 失败: {failed}")


if __name__ == "__main__":
    main()
