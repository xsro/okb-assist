import json
from typing import Optional

import httpx
from app.config import get_settings

settings = get_settings()

EXTRACT_PROMPT = """请从以下学术文献内容中提取元数据，严格按照 JSON 格式返回，不要包含任何其他文字。

重要规则：
1. 首先判断文献的主要语言（language字段），使用 ISO 639-1 语言代码：
   - en = English
   - zh = 中文
   - ja = 日本語
   - fr = Français
   - ru = Русский
   - de = Deutsch
   - ko = 한국어
   - es = Español
   - pt = Português
   - ar = العربية
   - 其他语言请使用对应的 ISO 639-1 代码

2. 如果是非英文文献（language != "en"），需要同时提供原文和英文版本的元数据：
   - title: 原文标题
   - title_en: 英文标题
   - authors: 原文作者名列表
   - authors_en: 英文作者名列表（拼音或翻译）
   - abstract: 原文摘要
   - abstract_en: 英文摘要
   - journal: 原文期刊/会议名
   - journal_en: 英文期刊/会议名
   - keywords: 原文关键词列表
   - keywords_en: 英文关键词列表

3. 如果是英文文献（language="en"），*_en字段留空即可

请返回以下JSON格式：
{
  "language": "语言代码",
  "type": "book 或 article 或 conference 或 thesis",
  "title": "标题（原文）",
  "title_en": "英文标题（非英文文献必填）",
  "year": 发表年份(整数),
  "authors": ["作者1", "作者2"],
  "authors_en": ["Author1", "Author2"],
  "abstract": "摘要（原文）",
  "abstract_en": "英文摘要（非英文文献必填）",
  "doi": "DOI号",
  "source": "来源",
  "journal": "期刊名或会议名（原文）",
  "journal_en": "英文期刊/会议名（非英文文献必填）",
  "keywords": ["关键词1", "关键词2"],
  "keywords_en": ["keyword1", "keyword2"],
  "category": "分类"
}

文献内容：
{content}
"""


async def extract_metadata(markdown_content: str) -> dict:
    """
    Use Ollama to extract metadata from markdown content.
    Returns a dict with metadata fields including language detection.
    """
    # Truncate content if too long (keep first 8000 chars for context)
    truncated = markdown_content[:8000]
    prompt = EXTRACT_PROMPT.format(content=truncated)

    async with httpx.AsyncClient(timeout=120.0) as client:
        response = await client.post(
            f"{settings.ollama_url}/api/generate",
            json={
                "model": settings.ollama_model,
                "prompt": prompt,
                "stream": False,
                "options": {
                    "temperature": 0.1,
                },
            },
        )
        response.raise_for_status()
        result = response.json()

    # Extract the response text
    response_text = result.get("response", "")

    # Try to parse JSON from the response
    metadata = _parse_json_from_text(response_text)

    return metadata


def _parse_json_from_text(text: str) -> dict:
    """Extract JSON from LLM response text."""
    # Try direct parse first
    try:
        return json.loads(text)
    except json.JSONDecodeError:
        pass

    # Try to find JSON block in the text
    import re
    json_match = re.search(r'\{[^{}]*(?:\{[^{}]*\}[^{}]*)*\}', text, re.DOTALL)
    if json_match:
        try:
            return json.loads(json_match.group())
        except json.JSONDecodeError:
            pass

    # Return empty dict if parsing fails
    return {
        "language": "",
        "type": "",
        "title": "",
        "title_en": "",
        "year": None,
        "authors": [],
        "authors_en": [],
        "abstract": "",
        "abstract_en": "",
        "doi": "",
        "source": "",
        "journal": "",
        "journal_en": "",
        "keywords": [],
        "keywords_en": [],
        "category": "",
    }


async def get_embedding(text: str) -> list[float]:
    """
    Get embedding vector from Ollama.
    """
    async with httpx.AsyncClient(timeout=60.0) as client:
        response = await client.post(
            f"{settings.ollama_url}/api/embed",
            json={
                "model": settings.ollama_embed_model,
                "input": text,
            },
        )
        response.raise_for_status()
        result = response.json()

    embeddings = result.get("embeddings", [])
    if embeddings:
        return embeddings[0]
    return []


def add_yaml_frontmatter(markdown_content: str, metadata: dict) -> str:
    """
    Add YAML frontmatter with metadata to markdown content.
    Includes bilingual fields for non-English documents.
    """
    import yaml

    language = metadata.get("language", "en")

    # Build frontmatter
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

    # Add English fields for non-English documents
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

    # Remove None values and empty strings/lists
    frontmatter = {
        k: v for k, v in frontmatter.items()
        if v is not None and v != "" and v != []
    }

    yaml_str = yaml.dump(frontmatter, allow_unicode=True, default_flow_style=False, sort_keys=False)

    return f"---\n{yaml_str}---\n\n{markdown_content}"
