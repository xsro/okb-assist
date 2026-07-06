#!/usr/bin/env python3
"""删除标题重复文献中 ID 较大的条目，保留每组中 ID 最小的。

用法:
    # 预览（不实际删除）
    python scripts/dedup_by_title.py --dry-run

    # 实际删除
    python scripts/dedup_by_title.py

    # 指定服务地址和 token
    python scripts/dedup_by_title.py --base-url http://localhost:5001 --token YOUR_TOKEN
"""

import argparse
import sys
import time

import httpx


def main():
    parser = argparse.ArgumentParser(description="删除标题重复文献中 ID 较大的条目")
    parser.add_argument("--base-url", default="http://localhost:5001", help="服务地址")
    parser.add_argument("--token", default="", help="认证 token（LAN 内可省略）")
    parser.add_argument("--dry-run", action="store_true", help="仅预览，不实际删除")
    args = parser.parse_args()

    headers = {}
    if args.token:
        headers["X-Token"] = args.token

    client = httpx.Client(base_url=args.base_url, headers=headers, timeout=30)

    # 1. 获取重复分组
    print("正在查询标题重复文献...")
    resp = client.get("/assist/api/documents/similar-titles")
    resp.raise_for_status()
    data = resp.json()

    groups = data["groups"]
    total_groups = data["total_groups"]
    total_docs = data["total_documents"]

    if not groups:
        print("未发现标题重复的文献，无需处理。")
        return

    # 2. 计算要删除的文档
    to_delete = []  # (group_title, doc_id)
    to_keep = []    # (group_title, doc_id)

    for group in groups:
        docs = group["documents"]
        sorted_docs = sorted(docs, key=lambda d: d["id"])
        keep = sorted_docs[0]
        remove = sorted_docs[1:]

        sample_title = keep.get("title") or group["normalized_title"]
        to_keep.append((sample_title, keep["id"]))
        for d in remove:
            to_delete.append((sample_title, d["id"]))

    # 3. 打印汇总
    print(f"\n共 {total_groups} 组重复，涉及 {total_docs} 条文献")
    print(f"将保留 {len(to_keep)} 条，删除 {len(to_delete)} 条\n")

    print("=" * 60)
    print(f"{'操作':<6} {'ID':<8} {'标题'}")
    print("-" * 60)

    for title, doc_id in to_keep:
        print(f"{'保留':<6} {doc_id:<8} {title[:44]}")
    for title, doc_id in to_delete:
        print(f"{'删除':<6} {doc_id:<8} {title[:44]}")

    print("=" * 60)

    if args.dry_run:
        print(f"\n[预览模式] 以上为预览结果，未执行删除。去掉 --dry-run 参数以实际执行。")
        return

    # 4. 确认
    confirm = input(f"\n确认删除 {len(to_delete)} 条文献？(y/N): ").strip().lower()
    if confirm != "y":
        print("已取消。")
        return

    # 5. 执行删除
    print("\n开始删除...")
    success = 0
    failed = 0

    for title, doc_id in to_delete:
        try:
            resp = client.delete(f"/assist/api/documents/{doc_id}")
            if resp.status_code == 200:
                success += 1
                print(f"  ✓ 已删除 #{doc_id}")
            else:
                failed += 1
                print(f"  ✗ 删除 #{doc_id} 失败: HTTP {resp.status_code}")
        except Exception as e:
            failed += 1
            print(f"  ✗ 删除 #{doc_id} 异常: {e}")

        # 避免请求过快
        time.sleep(0.05)

    print(f"\n完成: 成功 {success} 条，失败 {failed} 条")


if __name__ == "__main__":
    main()
