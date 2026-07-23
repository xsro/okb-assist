"""
基于系统 grep 的轻量全文搜索服务。

适用于嵌入式设备，无需向量数据库和 embedding 模型，
直接使用 grep 搜索 markdown 文件内容。
"""

import asyncio
import re
from pathlib import Path

from app.config import get_settings


async def grep_search(
    query: str,
    context_lines: int = 2,
    limit: int = 10,
    doc_ids: list[int] | None = None,
) -> list[dict]:
    """使用系统 grep 搜索文档的 markdown 内容。

    Args:
        query: 搜索关键词（支持正则）
        context_lines: 匹配行前后的上下文行数
        limit: 最大返回结果数
        doc_ids: 限定搜索的文档 ID 列表，None 表示搜索全部

    Returns:
        [{document_id, content, file_path}, ...]
    """
    settings = get_settings()
    uploads_dir = Path(settings.uploads_folder)

    if not uploads_dir.exists():
        return []

    # 构建搜索路径
    if doc_ids:
        search_paths = []
        for did in doc_ids:
            doc_dir = uploads_dir / str(did)
            if doc_dir.exists():
                search_paths.append(str(doc_dir))
        if not search_paths:
            return []
    else:
        search_paths = [str(uploads_dir)]

    # 构建 grep 命令
    cmd = [
        "grep",
        "-rn",              # 递归，显示行号
        "-i",               # 忽略大小写
        f"-C{context_lines}",  # 上下文行数
        "--include=*.md",   # 只搜索 markdown 文件
        query,
        *search_paths,
    ]

    try:
        proc = await asyncio.create_subprocess_exec(
            *cmd,
            stdout=asyncio.subprocess.PIPE,
            stderr=asyncio.subprocess.PIPE,
        )
        stdout, stderr = await proc.communicate()
    except FileNotFoundError:
        # grep 不存在
        return []

    if proc.returncode not in (0, 1):
        # grep 错误（0=找到，1=未找到，其他=错误）
        return []

    output = stdout.decode("utf-8", errors="replace")
    if not output.strip():
        return []

    return _parse_grep_output(output, limit)


def _parse_grep_output(output: str, limit: int) -> list[dict]:
    """解析 grep -rn -C 的输出，按文件分组。

    输出格式示例：
        uploads/42/42.md-41-## Abstract
        uploads/42/42.md:42:We propose a novel transformer architecture...
        uploads/42/42.md-43-for natural language processing.
        --
        uploads/99/99.md-10-# Introduction
        uploads/99/99.md:11:Transformers have revolutionized NLP...
        uploads/99/99.md-12-by enabling attention mechanisms.
    """
    results = []
    current_file = None
    current_lines = []
    seen_files = set()

    for line in output.split("\n"):
        if line == "--":
            # 分隔符，保存当前块
            if current_file and current_lines:
                doc_id = _extract_doc_id(current_file)
                if doc_id and doc_id not in seen_files:
                    seen_files.add(doc_id)
                    results.append({
                        "document_id": doc_id,
                        "content": "\n".join(current_lines),
                        "file_path": current_file,
                    })
                    if len(results) >= limit:
                        return results
            current_lines = []
            continue

        # 解析文件路径和内容：path:linenum:content 或 path-linenum-content
        match = re.match(r"^(.+?)[\:\-](\d+)[\:\-](.*)$", line)
        if match:
            filepath = match.group(1)
            # 如果是新文件，保存旧块
            if current_file and filepath != current_file and current_lines:
                doc_id = _extract_doc_id(current_file)
                if doc_id and doc_id not in seen_files:
                    seen_files.add(doc_id)
                    results.append({
                        "document_id": doc_id,
                        "content": "\n".join(current_lines),
                        "file_path": current_file,
                    })
                    if len(results) >= limit:
                        return results
                current_lines = []

            current_file = filepath
            current_lines.append(match.group(3))
        else:
            # 非匹配行（上下文行没有行号前缀的情况）
            if line.strip():
                current_lines.append(line)

    # 处理最后一块
    if current_file and current_lines:
        doc_id = _extract_doc_id(current_file)
        if doc_id and doc_id not in seen_files:
            results.append({
                "document_id": doc_id,
                "content": "\n".join(current_lines),
                "file_path": current_file,
            })

    return results[:limit]


def _extract_doc_id(file_path: str) -> int | None:
    """从文件路径中提取文档 ID。

    路径格式: uploads/{id}/{id}.md
    """
    match = re.search(r"/(\d+)/\1\.md$", file_path)
    if match:
        return int(match.group(1))

    # 备用：从路径中提取数字目录名
    match = re.search(r"/(\d+)/[^/]+\.md$", file_path)
    if match:
        return int(match.group(1))

    return None
