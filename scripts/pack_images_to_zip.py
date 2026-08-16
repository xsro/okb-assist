#!/usr/bin/env python3
"""
将已有文档的 images/ 目录打包为 images.zip

遍历所有文档，将 uploads/{id}/images/ 打包为 system.json 定义的资源 zip
（markdown_asset_path，如 /.../pdfs/{id}/{id}.zip），验证 zip 完整性后删除 images/ 目录。

使用方法:
    python scripts/pack_images_to_zip.py [选项]

示例:
    python scripts/pack_images_to_zip.py --dry-run
    python scripts/pack_images_to_zip.py --limit 10
    python scripts/pack_images_to_zip.py --force
"""

import argparse
import os
import shutil
import sys
import zipfile
from pathlib import Path

import httpx

try:
    from app.paths import get_asset_path
except Exception:
    get_asset_path = None


# ──────────────────────────────────────────────
# API 调用
# ──────────────────────────────────────────────

def _headers(token: str = None) -> dict:
    h = {"Content-Type": "application/json"}
    if token:
        h["X-Token"] = token
    return h


def list_documents(base_url: str, page: int = 1, page_size: int = 50, token: str = None) -> dict | None:
    """获取文档列表（分页）。"""
    try:
        with httpx.Client(timeout=30) as client:
            resp = client.get(
                f"{base_url}/assist/api/documents/",
                params={"page": page, "page_size": page_size},
                headers=_headers(token),
            )
            if resp.status_code == 200:
                return resp.json()
    except Exception as e:
        print(f"  获取文档列表失败: {e}")
    return None


# ──────────────────────────────────────────────
# Token 读取
# ──────────────────────────────────────────────

def read_token(args_token: str | None) -> str | None:
    if args_token:
        return args_token
    token = os.environ.get("OKB_ASSIST_TOKEN")
    if token:
        return token
    token_file = Path.home() / ".okb_assist_token"
    if token_file.exists():
        return token_file.read_text().strip()
    return None


# ──────────────────────────────────────────────
# 打包逻辑
# ──────────────────────────────────────────────

def pack_images_dir(images_dir: str, zip_path: str) -> tuple[bool, int, str]:
    """
    将 images/ 目录打包为 images.zip。
    返回 (成功, 文件数, 错误信息)。
    """
    if not os.path.isdir(images_dir):
        return False, 0, "images/ 目录不存在"

    # 收集所有图片文件
    image_files = []
    for f in os.listdir(images_dir):
        fp = os.path.join(images_dir, f)
        if os.path.isfile(fp) and any(f.lower().endswith(ext) for ext in ['.png', '.jpg', '.jpeg', '.gif', '.svg']):
            image_files.append(fp)

    if not image_files:
        return False, 0, "无图片文件"

    # 创建 zip
    try:
        with zipfile.ZipFile(zip_path, 'w', zipfile.ZIP_STORED) as zf:
            for img_path in image_files:
                zf.write(img_path, os.path.basename(img_path))
    except Exception as e:
        return False, 0, f"创建 zip 失败: {e}"

    # 验证 zip 完整性
    try:
        with zipfile.ZipFile(zip_path, 'r') as zf:
            bad = zf.testzip()
            if bad is not None:
                os.remove(zip_path)
                return False, 0, f"zip 校验失败: {bad}"
            zip_count = len(zf.namelist())
    except Exception as e:
        if os.path.exists(zip_path):
            os.remove(zip_path)
        return False, 0, f"zip 验证失败: {e}"

    # 校验文件数一致
    if zip_count != len(image_files):
        os.remove(zip_path)
        return False, 0, f"文件数不匹配: 目录 {len(image_files)} vs zip {zip_count}"

    return True, zip_count, ""


# ──────────────────────────────────────────────
# 主流程
# ──────────────────────────────────────────────

def main():
    parser = argparse.ArgumentParser(description="将 images/ 目录打包为 images.zip")
    parser.add_argument("--base-url", default="http://localhost:5001", help="OKB-Assist 服务地址")
    parser.add_argument("--token", default=None, help="访问令牌")
    parser.add_argument("--uploads-dir", default="uploads", help="uploads 目录路径")
    parser.add_argument("--dry-run", action="store_true", help="试运行，不实际打包")
    parser.add_argument("--force", action="store_true", help="强制重新打包（覆盖已有 zip）")
    parser.add_argument("--limit", type=int, default=0, help="最多处理 N 条文档（0=全部）")

    args = parser.parse_args()
    token = read_token(args.token)

    print(f"服务地址: {args.base_url}")
    print(f"uploads 目录: {args.uploads_dir}")
    print(f"Token: {'*' * 8 if token else '未设置'}")
    print(f"模式: {'试运行' if args.dry_run else '覆盖' if args.force else '仅打包无 zip 的文档'}")
    print()

    # ── 分页获取所有文档 ──
    all_docs = []
    page = 1
    page_size = 100
    while True:
        result = list_documents(args.base_url, page=page, page_size=page_size, token=token)
        if not result or not result.get("items"):
            break
        all_docs.extend(result["items"])
        if len(all_docs) >= result.get("total", 0):
            break
        page += 1

    if args.limit > 0:
        all_docs = all_docs[:args.limit]

    print(f"共获取 {len(all_docs)} 个文档")
    print()

    stats = {
        "total": len(all_docs),
        "packed": 0,
        "skipped_no_images": 0,
        "skipped_has_zip": 0,
        "failed": 0,
        "total_images": 0,
        "total_size_before": 0,
        "total_size_after": 0,
    }

    for i, doc in enumerate(all_docs, 1):
        doc_id = doc["id"]
        doc_dir = os.path.join(args.uploads_dir, str(doc_id))
        images_dir = os.path.join(doc_dir, "images")
        # 资源 zip 目标路径由 system.json 推导（markdown_asset_path）
        if get_asset_path is not None:
            zip_path = get_asset_path(doc_id)
        else:
            zip_path = os.path.join(doc_dir, "images.zip")

        print(f"[{i}/{len(all_docs)}] ID={doc_id}", end="")

        # ── 检查 images/ 目录 ──
        if not os.path.isdir(images_dir):
            print(f" 跳过: 无 images/ 目录")
            stats["skipped_no_images"] += 1
            continue

        image_count = len([f for f in os.listdir(images_dir) if os.path.isfile(os.path.join(images_dir, f))])
        if image_count == 0:
            print(f" 跳过: images/ 为空")
            stats["skipped_no_images"] += 1
            continue

        # ── 检查是否已有 zip ──
        if os.path.exists(zip_path) and not args.force:
            print(f" 跳过: 已有 images.zip")
            stats["skipped_has_zip"] += 1
            continue

        # 计算目录大小
        dir_size = sum(os.path.getsize(os.path.join(images_dir, f)) for f in os.listdir(images_dir) if os.path.isfile(os.path.join(images_dir, f)))

        print(f" ({image_count} 张图片, {dir_size / 1024:.0f} KB)", end="")

        # ── 试运行 ──
        if args.dry_run:
            print(f" [试运行] 将打包")
            stats["packed"] += 1
            stats["total_images"] += image_count
            stats["total_size_before"] += dir_size
            continue

        # ── 打包 ──
        success, count, err = pack_images_dir(images_dir, zip_path)
        if not success:
            print(f" 失败: {err}")
            stats["failed"] += 1
            continue

        # 获取 zip 大小
        zip_size = os.path.getsize(zip_path)

        # 删除 images/ 目录
        shutil.rmtree(images_dir)

        print(f" ✓ {count} 张 → {zip_size / 1024:.0f} KB (节省 {(1 - zip_size / dir_size) * 100:.0f}%)")
        stats["packed"] += 1
        stats["total_images"] += count
        stats["total_size_before"] += dir_size
        stats["total_size_after"] += zip_size

    # ── 统计 ──
    print()
    print("=" * 60)
    print("完成!")
    print(f"  总计文档: {stats['total']}")
    print(f"  已打包: {stats['packed']}")
    print(f"  跳过(无images): {stats['skipped_no_images']}")
    print(f"  跳过(已有zip): {stats['skipped_has_zip']}")
    print(f"  失败: {stats['failed']}")
    print()
    print(f"  图片总数: {stats['total_images']}")
    if stats['total_size_before'] > 0:
        print(f"  打包前大小: {stats['total_size_before'] / 1048576:.1f} MB")
        if not args.dry_run and stats['total_size_after'] > 0:
            print(f"  打包后大小: {stats['total_size_after'] / 1048576:.1f} MB")
            print(f"  节省空间: {(1 - stats['total_size_after'] / stats['total_size_before']) * 100:.1f}%")


if __name__ == "__main__":
    main()
