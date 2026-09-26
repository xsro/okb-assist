#!/usr/bin/env python3
"""
从 OKB-Assist 文献服务器中获取待解析（uploaded/error）的文献，
从 MinerU 服务器获取解析结果并下载到本地，
然后询问用户是否将结果上传回文献服务器。

用法:
    # 使用默认参数（192.168.1.122:5001 + token 60b8be...）
    python scripts/mineru_result_fetcher.py

    # 指定自定义参数
    python scripts/mineru_result_fetcher.py \
        --base-url http://192.168.1.122:5001 \
        --token 60b8be6583ac41abb3af63e01151bd86 \
        --mineru-url http://192.168.1.185:8002 \
        --output-dir data/mineru_result
"""

import argparse
import json
import os
import sys
import zipfile
import io

import requests


# ── 默认参数 ──────────────────────────────────────────────────────

DEFAULT_BASE_URL = "http://192.168.1.122:5001"
DEFAULT_TOKEN = "60b8be6583ac41abb3af63e01151bd86"
DEFAULT_MINERU_URL = "http://192.168.1.185:8002"
DEFAULT_OUTPUT_DIR = "data/mineru_result"

# 要查询的文献状态（代码中为 "uploaded" 和 "error"，与用户说的"uploading"、"failed"对应）
QUERY_STATUSES = ["uploaded", "error"]

# ── 辅助函数 ──────────────────────────────────────────────────────


def okb_headers(token: str) -> dict:
    """构造 OKB-Assist API 请求头"""
    return {"X-Token": token, "Accept": "application/json"}


def get_documents(base_url: str, token: str) -> list[dict]:
    """获取状态的文献列表"""
    url = f"{base_url.rstrip('/')}/assist/api/documents/"
    params = {
        "page_size": 500,
        "status_filter": ",".join(QUERY_STATUSES),
    }
    resp = requests.get(url, params=params, headers=okb_headers(token), timeout=30)
    resp.raise_for_status()
    data = resp.json()
    return data.get("items", [])


def get_document_info(base_url: str, token: str, doc_id: int) -> dict | None:
    """尝试从 /assist/api/documents/{id}/info/ 获取附加信息（可能包含 mineru_task_id）"""
    url = f"{base_url.rstrip('/')}/assist/api/documents/{doc_id}/info/"
    try:
        resp = requests.get(url, headers=okb_headers(token), timeout=15)
        if resp.status_code == 200:
            return resp.json().get("info", {})
    except requests.RequestException:
        pass
    return None


def get_individual_document(base_url: str, token: str, doc_id: int) -> dict | None:
    """获取单个文献的完整详情"""
    url = f"{base_url.rstrip('/')}/assist/api/documents/{doc_id}/"
    try:
        resp = requests.get(url, headers=okb_headers(token), timeout=15)
        if resp.status_code == 200:
            return resp.json()
    except requests.RequestException:
        pass
    return None


def query_mineru_status(mineru_url: str, task_id: str) -> dict:
    """查询 MinerU 任务状态"""
    url = f"{mineru_url.rstrip('/')}/tasks/{task_id}"
    resp = requests.get(url, timeout=15)
    if resp.status_code == 200:
        return resp.json()
    return {"status": "unknown"}


def download_mineru_result(mineru_url: str, task_id: str, output_path: str) -> str | None:
    """
    从 MinerU 下载解析结果压缩包并解压。
    返回解压后的 .md 文件路径，失败时返回 None。
    """
    url = f"{mineru_url.rstrip('/')}/tasks/{task_id}/result"
    resp = requests.get(url, timeout=120)
    if resp.status_code != 200:
        print(f"  ⚠ 下载结果失败: HTTP {resp.status_code}")
        return None

    os.makedirs(output_path, exist_ok=True)

    # 解压 zip
    try:
        with zipfile.ZipFile(io.BytesIO(resp.content)) as zf:
            zf.extractall(output_path)
            # 查找 .md 文件
            md_files = [f for f in zf.namelist() if f.endswith(".md")]
            if md_files:
                return os.path.join(output_path, md_files[0])
        # 如果循环完成但没找到 md 文件
        print(f"  ⚠ 压缩包中未找到 .md 文件")
        return None
    except zipfile.BadZipFile:
        print(f"  ⚠ 下载内容不是有效的 ZIP 文件")
        return None


def get_mineru_task_id_from_info(info: dict) -> str | None:
    """从 info JSON 中提取 mineru_task_id（可能有多种命名）"""
    for key in ("mineru_task_id", "task_id", "mineru_id"):
        val = info.get(key)
        if val and isinstance(val, str) and val.strip():
            return val.strip()
    return None


def upload_parse_result(base_url: str, token: str, doc_id: int, md_path: str) -> bool:
    """将解析结果（Markdown）上传回 OKB-Assist 文献服务器"""
    try:
        with open(md_path, "r", encoding="utf-8") as f:
            content = f.read()
    except Exception as e:
        print(f"  ⚠ 读取 Markdown 文件失败: {e}")
        return False

    url = f"{base_url.rstrip('/')}/assist/api/documents/{doc_id}/parse-result/"
    payload = {"content": content}
    resp = requests.post(url, json=payload, headers=okb_headers(token), timeout=30)
    if resp.status_code in (200, 201):
        print(f"  ✅ 上传成功，文献已标记为 markdown_done")
        return True
    else:
        print(f"  ⚠ 上传失败: HTTP {resp.status_code} - {resp.text[:200]}")
        return False


# ── 主流程 ────────────────────────────────────────────────────────


def main():
    parser = argparse.ArgumentParser(
        description="从 OKB-Assist 获取待解析文献、下载 MinerU 结果并可选上传。"
    )
    parser.add_argument("--base-url", default=DEFAULT_BASE_URL, help=f"OKB-Assist 服务器地址（默认: {DEFAULT_BASE_URL}）")
    parser.add_argument("--token", default=DEFAULT_TOKEN, help="API 访问令牌")
    parser.add_argument("--mineru-url", default=DEFAULT_MINERU_URL, help=f"MinerU 服务器地址（默认: {DEFAULT_MINERU_URL}）")
    parser.add_argument("--output-dir", default=DEFAULT_OUTPUT_DIR, help=f"本地下载目录（默认: {DEFAULT_OUTPUT_DIR}）")
    args = parser.parse_args()

    print(f"🔍 连接文献服务器: {args.base_url}")
    print()

    # ── 第 1 步：获取待解析文献 ──
    print("📋 正在获取待解析文献（状态: uploaded / error）...")
    try:
        docs = get_documents(args.base_url, args.token)
    except requests.RequestException as e:
        print(f"❌ 获取文献列表失败: {e}")
        sys.exit(1)

    if not docs:
        print("✅ 没有待解析的文献。")
        return

    print(f"   共找到 {len(docs)} 篇待解析文献。")
    print()

    # ── 第 2 步：逐篇检查状态，尝试获取 mineru_task_id ──
    candidates = []  # (doc_id, filename, mineru_task_id, output_path)
    for doc in docs:
        doc_id = doc.get("id")
        filename = doc.get("filename", f"doc_{doc_id}")
        status = doc.get("status", "?")
        print(f"  📄 [ID={doc_id}] {filename} (status={status})")

        # 尝试获取 mineru_task_id
        mineru_task_id = doc.get("mineru_task_id")  # 可能没有

        if not mineru_task_id:
            # 尝试从 info JSON 获取
            info = get_document_info(args.base_url, args.token, doc_id)
            if info:
                mineru_task_id = get_mineru_task_id_from_info(info)

        if not mineru_task_id:
            # 尝试从单个文档详情获取（不同版本可能返回不同字段）
            detail = get_individual_document(args.base_url, args.token, doc_id)
            if detail:
                mineru_task_id = detail.get("mineru_task_id")

        if mineru_task_id:
            print(f"     mineru_task_id: {mineru_task_id}")
        else:
            print(f"     ⚠ 未找到 mineru_task_id，跳过 MinerU 查询")
            continue

        candidates.append((doc_id, filename, mineru_task_id))

    if not candidates:
        print("\n⚠ 所有待解析文献均无 mineru_task_id，无法查询 MinerU。")
        print("  提示：mineru_task_id 目前不通过 API 暴露，可能需要直接读取数据库。")
        print("  你可以手动为各文献设置 mineru_task_id 后重试。")
        return

    print(f"\n📋 共 {len(candidates)} 篇文献有 MinerU 任务 ID，开始查询 MinerU...")

    # ── 第 3 步：查询 MinerU 并下载结果 ──
    downloaded = []  # (doc_id, md_path)
    for doc_id, filename, task_id in candidates:
        output_dir = os.path.join(args.output_dir, str(doc_id))
        md_path = os.path.join(output_dir, f"{doc_id}.md")

        # 检查是否已下载
        if os.path.exists(md_path):
            print(f"  📄 [ID={doc_id}] 结果已存在: {md_path}")
            downloaded.append((doc_id, md_path))
            continue

        # 查询 MinerU 任务状态
        print(f"  📄 [ID={doc_id}] 查询 MinerU 任务 {task_id}... ", end="", flush=True)
        status_data = query_mineru_status(args.mineru_url, task_id)
        status = status_data.get("status", "unknown")
        print(f"状态: {status}")

        if status == "completed":
            print(f"    正在下载解析结果...", end="", flush=True)
            result_md = download_mineru_result(args.mineru_url, task_id, output_dir)
            if result_md:
                # 确保文件名为 {doc_id}.md
                expected_path = os.path.join(output_dir, f"{doc_id}.md")
                if result_md != expected_path:
                    # 重命名或复制
                    import shutil
                    shutil.copy2(result_md, expected_path)
                print(f" ✅ -> {expected_path}")
                downloaded.append((doc_id, expected_path))
            else:
                print(f" ❌ 下载失败")
        elif status == "failed":
            err_msg = status_data.get("err_msg", status_data.get("error", "未知错误"))
            print(f"    ⚠ MinerU 任务失败: {err_msg}")
        elif status in ("processing", "pending"):
            print(f"    ⏳ 任务仍在处理中，跳过")
        else:
            print(f"    ⚠ 未知状态")

    if not downloaded:
        print("\n⚠ 没有可用的解析结果。")
        return

    # ── 第 4 步：询问用户是否上传 ──
    print(f"\n📦 共 {len(downloaded)} 篇文献的解析结果可上传。")
    for doc_id, md_path in downloaded:
        print(f"   - [ID={doc_id}] {md_path}")

    answer = input("\n❓ 是否将这些结果上传回文献服务器？(y/N): ").strip().lower()
    if answer not in ("y", "yes"):
        print("跳过上传。")
        return

    print()
    success_count = 0
    for doc_id, md_path in downloaded:
        print(f"  📤 [ID={doc_id}] 正在上传...", end=" ", flush=True)
        if upload_parse_result(args.base_url, args.token, doc_id, md_path):
            success_count += 1
        else:
            print(f"     ❌ 上传失败")

    print(f"\n✅ 完成！成功上传 {success_count}/{len(downloaded)} 篇文献的解析结果。")


if __name__ == "__main__":
    main()