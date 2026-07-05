#!/usr/bin/env python3
"""
从 tasks.db 读取 bib 信息和 extra 信息，补充到 okb_assist.db 中状态为 markdown_done 的文献。
如果标题、文献类型和作者信息都有，将状态改为 meta_done。
"""

import json
import sqlite3
import re
from pathlib import Path

# 数据库路径
TASKS_DB = "/home/a422/repo/llm3/data/tasks.db"
OKB_DB = "/home/a422/repo/okb-assist/okb_assist.db"
UPLOADS_DIR = Path("/home/a422/repo/okb-assist/uploads")


def parse_authors(author_str: str) -> list[str]:
    """将 BibTeX 格式的作者字符串解析为列表。
    例如: "Kang, Honglong and Wang, Pengyu" -> ["Honglong Kang", "Pengyu Wang"]
    """
    if not author_str:
        return []

    # 清理 BibTeX 标记
    author_str = re.sub(r'[{}]', '', author_str)
    author_str = author_str.replace('\\', '')

    # 按 "and" 或 ";" 分割
    if ' and ' in author_str:
        authors = [a.strip() for a in author_str.split(' and ') if a.strip()]
    elif ';' in author_str:
        authors = [a.strip() for a in author_str.split(';') if a.strip()]
    else:
        authors = [author_str.strip()]

    result = []
    for author in authors:
        # 处理 "姓, 名" 格式
        if ',' in author:
            parts = author.split(',', 1)
            name = f"{parts[1].strip()} {parts[0].strip()}"
        else:
            name = author
        if name:
            result.append(name)

    return result


def fix_existing_authors(authors_str: str) -> list[str]:
    """修复已存在的错误格式的作者列表。
    例如: '["ZhongYi; Cui, Jing; Sun, FuChun Chu"]' -> ["ZhongYi Chu", "Jing Cui", "FuChun Sun"]
    """
    if not authors_str:
        return []

    try:
        authors = json.loads(authors_str)
        if not isinstance(authors, list):
            return []

        # 检查是否是错误格式（单个字符串包含分号）
        if len(authors) == 1 and ';' in str(authors[0]):
            # 重新解析
            return parse_authors(str(authors[0]))

        return authors
    except (json.JSONDecodeError, TypeError):
        return []


def clean_title(title: str) -> str:
    """清理标题中的特殊标记。"""
    if not title:
        return ""
    # 移除 {{ }} 标记
    title = re.sub(r'\{\{([^}]*)\}\}', r'\1', title)
    # 移除其他花括号
    title = title.replace('{', '').replace('}', '')
    # 清理多余空格
    title = re.sub(r'\s+', ' ', title).strip()
    return title


def parse_extra_info(extra_info_str: str) -> dict:
    """解析 extra_info JSON 字符串。"""
    if not extra_info_str:
        return {}
    try:
        return json.loads(extra_info_str)
    except json.JSONDecodeError:
        return {}


def has_yaml_frontmatter(content: str) -> bool:
    """检查 markdown 内容是否已有 YAML frontmatter。"""
    return content.startswith('---\n') or content.startswith('---\r\n')


def build_yaml_frontmatter(metadata: dict) -> str:
    """构建 YAML frontmatter。"""
    import yaml

    language = metadata.get("language", "en")

    frontmatter = {
        "language": language,
        "type": metadata.get("type", ""),
        "title": metadata.get("title", ""),
        "year": metadata.get("year"),
        "authors": metadata.get("authors", []),
        "abstract": metadata.get("abstract", ""),
        "doi": metadata.get("doi", ""),
        "source": metadata.get("source", ""),
        "journal": metadata.get("journal", ""),
        "keywords": metadata.get("keywords", []),
        "category": metadata.get("category", ""),
    }

    # 非英文文档添加英文字段
    if language != "en":
        if metadata.get("title_en"):
            frontmatter["title_en"] = metadata["title_en"]
        if metadata.get("authors_en"):
            frontmatter["authors_en"] = metadata["authors_en"]
        if metadata.get("abstract_en"):
            frontmatter["abstract_en"] = metadata["abstract_en"]
        if metadata.get("journal_en"):
            frontmatter["journal_en"] = metadata["journal_en"]
        if metadata.get("keywords_en"):
            frontmatter["keywords_en"] = metadata["keywords_en"]

    # 移除空值
    frontmatter = {
        k: v for k, v in frontmatter.items()
        if v is not None and v != "" and v != []
    }

    yaml_str = yaml.dump(frontmatter, allow_unicode=True, default_flow_style=False, sort_keys=False)
    return f"---\n{yaml_str}---\n\n"


def main():
    # 连接数据库
    tasks_conn = sqlite3.connect(TASKS_DB)
    tasks_conn.row_factory = sqlite3.Row

    okb_conn = sqlite3.connect(OKB_DB)
    okb_conn.row_factory = sqlite3.Row

    # 从 tasks.db 读取所有有 extra_info 的任务，按 pdf_path 建立索引
    print("正在读取 tasks.db 中的数据...")
    tasks_cursor = tasks_conn.execute("""
        SELECT pdf_path, extra_info, bib, filename
        FROM tasks
        WHERE extra_info != '' OR bib != ''
    """)

    # 按 pdf_path 建立映射
    tasks_by_path = {}
    for row in tasks_cursor:
        pdf_path = row['pdf_path']
        if pdf_path:
            tasks_by_path[pdf_path] = {
                'extra_info': row['extra_info'],
                'bib': row['bib'],
                'filename': row['filename'],
            }

    print(f"找到 {len(tasks_by_path)} 个有 bib/extra_info 的任务")

    # 从 okb_assist.db 读取状态为 markdown_done 或 meta_done 的文档
    print("正在读取 okb_assist.db 中的文档...")
    docs_cursor = okb_conn.execute("""
        SELECT id, filename, file_path, title, authors, doc_type, status, markdown_path
        FROM documents
        WHERE status IN ('markdown_done', 'meta_done')
    """)

    docs = list(docs_cursor)
    print(f"找到 {len(docs)} 个需要处理的文档")

    # 统计
    matched = 0
    updated = 0
    status_changed = 0
    skipped_no_match = 0
    skipped_no_extra = 0
    errors = 0

    for doc in docs:
        doc_id = doc['id']
        file_path = doc['file_path']
        markdown_path = doc['markdown_path']

        # 尝试匹配
        task = tasks_by_path.get(file_path)
        if not task:
            skipped_no_match += 1
            continue

        matched += 1

        # 解析 extra_info
        extra_info = parse_extra_info(task['extra_info'])
        if not extra_info:
            skipped_no_extra += 1
            continue

        try:
            # 提取字段
            title = clean_title(extra_info.get('title', ''))
            authors = parse_authors(extra_info.get('author', ''))
            year = extra_info.get('year')
            journal = extra_info.get('journal', '')
            doi = extra_info.get('doi', '')
            bib_type = extra_info.get('bib_type', '')

            # 如果 extra_info 没有作者信息，尝试从现有数据修复
            if not authors and doc['authors']:
                authors = fix_existing_authors(doc['authors'])

            # 转换年份
            if year:
                try:
                    year = int(year)
                except (ValueError, TypeError):
                    year = None

            # 构建更新数据
            update_fields = {}
            if title:
                update_fields['title'] = title
            if authors:
                update_fields['authors'] = json.dumps(authors, ensure_ascii=False)
            if year:
                update_fields['year'] = year
            if journal:
                update_fields['journal'] = journal
            if doi:
                update_fields['doi'] = doi
            if bib_type:
                update_fields['doc_type'] = bib_type

            # 更新数据库
            if update_fields:
                set_clause = ', '.join(f"{k} = ?" for k in update_fields.keys())
                values = list(update_fields.values())
                values.append(doc_id)

                okb_conn.execute(f"""
                    UPDATE documents SET {set_clause} WHERE id = ?
                """, values)
                updated += 1

            # 更新 YAML frontmatter 到 markdown 文件
            if markdown_path:
                md_full_path = UPLOADS_DIR.parent / markdown_path
                if md_full_path.exists():
                    content = md_full_path.read_text(encoding='utf-8')

                    # 移除已有的 YAML frontmatter
                    if has_yaml_frontmatter(content):
                        # 找到第二个 --- 的位置
                        second_sep = content.find('---', 4)
                        if second_sep != -1:
                            content = content[second_sep + 3:].lstrip('\n')

                    # 构建 metadata
                    metadata = {
                        "language": extra_info.get('language', 'en'),
                        "type": bib_type,
                        "title": title,
                        "year": year,
                        "authors": authors,
                        "journal": journal,
                        "doi": doi,
                    }

                    frontmatter = build_yaml_frontmatter(metadata)
                    new_content = frontmatter + content

                    md_full_path.write_text(new_content, encoding='utf-8')

            # 检查是否满足状态变更条件（标题、类型、作者都有）
            has_title = bool(title)
            has_type = bool(bib_type)
            has_authors = bool(authors)

            if has_title and has_type and has_authors:
                if doc['status'] != 'meta_done':
                    okb_conn.execute("""
                        UPDATE documents SET status = 'meta_done' WHERE id = ?
                    """, (doc_id,))
                    status_changed += 1

        except Exception as e:
            errors += 1
            print(f"处理文档 {doc_id} 时出错: {e}")

    # 提交更改
    okb_conn.commit()

    # 关闭连接
    tasks_conn.close()
    okb_conn.close()

    # 打印统计
    print("\n=== 处理完成 ===")
    print(f"总文档数: {len(docs)}")
    print(f"匹配成功: {matched}")
    print(f"已更新: {updated}")
    print(f"状态已改为 meta_done: {status_changed}")
    print(f"无匹配跳过: {skipped_no_match}")
    print(f"无 extra_info 跳过: {skipped_no_extra}")
    print(f"错误: {errors}")


if __name__ == "__main__":
    main()
