#!/usr/bin/env python3
"""
Client script to register PDF files from a directory to OKB-Assist server.
PDF files are NOT copied - only their paths are recorded.
"""

import os
import sys
import json
import argparse
import requests
from pathlib import Path


def find_pdfs(directory: str) -> list[str]:
    """Find all PDF files in directory recursively."""
    pdf_files = []
    for root, dirs, files in os.walk(directory):
        for file in files:
            if file.lower().endswith('.pdf'):
                pdf_files.append(os.path.join(root, file))
    return sorted(pdf_files)


def register_pdf(base_url: str, file_path: str) -> dict:
    """Register a PDF file with the server."""
    url = f"{base_url}/assist/api/documents/register"
    data = {"file_path": file_path}
    response = requests.post(url, json=data)
    response.raise_for_status()
    return response.json()


def start_pipeline(base_url: str, doc_id: int) -> dict:
    """Start full pipeline for a document."""
    url = f"{base_url}/assist/api/pipeline/{doc_id}/process"
    response = requests.post(url)
    response.raise_for_status()
    return response.json()


def get_status(base_url: str, doc_id: int) -> dict:
    """Get document processing status."""
    url = f"{base_url}/assist/api/pipeline/{doc_id}/status"
    response = requests.get(url)
    response.raise_for_status()
    return response.json()


def main():
    parser = argparse.ArgumentParser(description="Register PDF files to OKB-Assist")
    parser.add_argument(
        "--dir",
        default="/mnt/hdd/open-webui/assist-data/uploads",
        help="Directory containing PDF files"
    )
    parser.add_argument(
        "--server",
        default="http://localhost:5000",
        help="OKB-Assist server URL"
    )
    parser.add_argument(
        "--process",
        action="store_true",
        help="Start processing pipeline after registration"
    )
    parser.add_argument(
        "--list-only",
        action="store_true",
        help="Only list PDF files, don't register"
    )
    parser.add_argument(
        "--dry-run",
        action="store_true",
        help="Show what would be done without doing it"
    )

    args = parser.parse_args()

    # Find PDFs
    print(f"扫描目录: {args.dir}")
    pdf_files = find_pdfs(args.dir)
    print(f"找到 {len(pdf_files)} 个 PDF 文件")

    if args.list_only:
        for i, pdf in enumerate(pdf_files, 1):
            print(f"  {i}. {pdf}")
        return

    # Register each PDF
    results = []
    for i, pdf_path in enumerate(pdf_files, 1):
        filename = os.path.basename(pdf_path)
        print(f"\n[{i}/{len(pdf_files)}] 处理: {filename}")

        if args.dry_run:
            print(f"  [DRY RUN] 将注册: {pdf_path}")
            continue

        try:
            # Register
            doc = register_pdf(args.server, pdf_path)
            print(f"  ✓ 注册成功: ID={doc['id']}, 状态={doc['status']}")
            results.append({"file": pdf_path, "id": doc["id"], "status": "registered"})

            # Start pipeline if requested
            if args.process and doc["status"] == "uploaded":
                result = start_pipeline(args.server, doc["id"])
                print(f"  ✓ 处理已提交: {result['detail']}")
                results[-1]["status"] = "processing"

        except requests.exceptions.RequestException as e:
            print(f"  ✗ 错误: {e}")
            results.append({"file": pdf_path, "error": str(e)})

    # Summary
    print("\n" + "=" * 50)
    print("处理完成!")
    print(f"总计: {len(pdf_files)} 个文件")
    print(f"成功: {len([r for r in results if 'error' not in r])} 个")
    print(f"失败: {len([r for r in results if 'error' in r])} 个")

    if args.process:
        print("\n处理任务已提交，可通过以下方式查看进度:")
        print(f"  - 监控页面: {args.server}/assist/monitor")
        print(f"  - 文献列表: {args.server}/assist/")


if __name__ == "__main__":
    main()
