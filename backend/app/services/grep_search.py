"""
基于系统 grep 的轻量全文搜索服务。

适用于嵌入式设备，无需向量数据库和 embedding 模型，
直接使用 grep 搜索 markdown 文件内容。
"""

import asyncio
import re


def parse_doc_ids(s: str | None) -> list[int] | None:
    """将 '1,2,5-100,4' 形式解析为文档 id 列表。

    支持逗号分隔与 lo-hi 区间（含端点，lo>hi 时自动交换）。
    空/None 返回 None；格式非法抛出 ValueError。
    """
    if not s or not s.strip():
        return None
    ids: list[int] = []
    for part in s.split(","):
        part = part.strip()
        if not part:
            continue
        if "-" in part:
            lo_s, _, hi_s = part.partition("-")
            lo = int(lo_s.strip())
            hi = int(hi_s.strip())
            if lo > hi:
                lo, hi = hi, lo
            ids.extend(range(lo, hi + 1))
        else:
            ids.append(int(part))
    return ids


async def grep_search(
    query: str,
    context_lines: int = 2,
    limit: int = 10,
    doc_ids: list[int] | None = None,
    algorithm: str = "full",   # "full"=原算法（全量扫描）；"fast"=元数据预筛候选
    regex: bool = True,        # True=正则（原行为）；False=字面匹配（-F）
    db=None,                   # SQLAlchemy Session，fast 模式预筛需要
) -> list[dict]:
    """使用系统 grep 搜索文档的 markdown 内容。

    搜索路径取自 system.json 推导的规范 markdown 路径（get_markdown_path），
    不再依赖 uploads/<id>/ 暂存目录（解析完成后该目录会被清理，仅保留规范副本）。

    算法选择：
    - "full"（默认）：完全复现原有行为，全量扫描所有 markdown 文件。
    - "fast"：仅当未指定 doc_ids 且提供了 db 时生效，先用元数据（标题/摘要/
      关键词/作者）做 LIKE 预筛，只扫描候选文档；若没有任何元数据命中，则回退到
      全量枚举，保证结果不遗漏。regex=False 时使用 grep -F 做字面匹配。

    Args:
        query: 搜索关键词（支持正则，regex=True 时）
        context_lines: 匹配行前后的上下文行数
        limit: 最大返回结果数
        doc_ids: 限定搜索的文档 ID 列表，None 表示搜索全部
        algorithm: 搜索算法，"full" 或 "fast"
        regex: 是否按正则表达式匹配（False 时按字面量匹配）
        db: 数据库连接（SQLAlchemy Session），fast 模式预筛需要

    Returns:
        [{document_id, content, file_path}, ...]
    """
    import os
    from app.paths import get_markdown_path

    # fast 模式仅在未指定 doc_ids、且提供了 db 时启用
    fast_enabled = (
        algorithm == "fast"
        and doc_ids is None
        and db is not None
    )

    if fast_enabled:
        # 快速路径：先用元数据预筛候选文档
        candidate_ids = _metadata_candidate_ids(db, query)
        search_paths = [
            p
            for did in candidate_ids
            if (p := get_markdown_path(did)) and os.path.exists(p)
        ]
        if not search_paths:
            # 没有任何元数据命中，回退到原全量枚举，保证结果不遗漏
            search_paths = _list_all_markdown_paths()
            if not search_paths:
                return []
    else:
        # 原/"full" 路径：完全复现原有行为
        if doc_ids:
            search_paths = []
            for did in doc_ids:
                md = get_markdown_path(did)
                if md and os.path.exists(md):
                    search_paths.append(md)
            if not search_paths:
                return []
        else:
            search_paths = _list_all_markdown_paths()
            if not search_paths:
                return []

    # 构建 grep 命令（regex 控制是否使用 -F 字面匹配）
    cmd = _build_grep_cmd(query, context_lines, regex, search_paths)

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


def _list_all_markdown_paths() -> list[str]:
    """枚举规范 markdown 目录下的全部 *.md 文件（原 full 路径的核心逻辑）。"""
    import os
    from app.paths import get_markdown_path

    sample = get_markdown_path(0)
    if not sample:
        return []
    markdowns_dir = os.path.dirname(sample)
    if not os.path.isdir(markdowns_dir):
        return []
    return [
        os.path.join(markdowns_dir, name)
        for name in os.listdir(markdowns_dir)
        if name.endswith(".md")
    ]


def _build_grep_cmd(
    query: str,
    context_lines: int,
    regex: bool,
    search_paths: list[str],
) -> list[str]:
    """构造 grep 命令。

    regex=True 时使用 grep 默认 BRE 正则（原行为）；regex=False 时使用 -F
    进行字面量匹配。其余参数（递归、忽略大小写、上下文行数）保持不变。

    Args:
        query: 搜索关键词
        context_lines: 上下文行数
        regex: 是否按正则匹配（False 时加 -F 字面匹配）
        search_paths: 待搜索的文件路径列表

    Returns:
        grep 命令行参数列表
    """
    return [
        "grep",
        "-rn",                  # 递归，显示行号
        "-i",                   # 忽略大小写
        f"-C{context_lines}",   # 上下文行数
        *(not regex and ["-F"] or []),  # 非正则时加 -F 字面匹配
        query,
        *search_paths,
    ]


def _metadata_candidate_ids(db, query: str) -> list[int]:
    """根据元数据（标题/摘要/关键词/作者）预筛候选文档 ID。

    在 fast 模式下调用，用 LIKE 对常见元数据字段做子串匹配，缩小 grep 扫描范围。
    db 为 SQLAlchemy Session。

    Args:
        db: SQLAlchemy Session
        query: 搜索关键词（按字面子串匹配）

    Returns:
        命中元数据的文档 ID 列表
    """
    from app.models import Document
    from sqlalchemy import or_

    rows = (
        db.query(Document.id)
        .filter(
            or_(
                Document.title.ilike(f"%{query}%"),
                Document.abstract.ilike(f"%{query}%"),
                Document.abstract_en.ilike(f"%{query}%"),
                Document.keywords.ilike(f"%{query}%"),
                Document.keywords_en.ilike(f"%{query}%"),
                Document.authors.ilike(f"%{query}%"),
            )
        )
        .all()
    )
    return [r[0] for r in rows]


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

    支持两种路径格式：
    - 规范路径: .../markdowns/{id}.md（文件名即文档 ID）
    - 暂存路径: uploads/{id}/{id}.md
    """
    # 规范路径：文件名即文档 ID（{id}.md）
    match = re.search(r"/(\d+)\.md$", file_path)
    if match:
        return int(match.group(1))

    # 暂存路径: /{id}/{id}.md（目录名与文件名一致）
    match = re.search(r"/(\d+)/\1\.md$", file_path)
    if match:
        return int(match.group(1))

    # 备用：/数字目录/任意 .md 文件
    match = re.search(r"/(\d+)/[^/]+\.md$", file_path)
    if match:
        return int(match.group(1))

    return None
