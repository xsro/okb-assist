import json
from typing import Optional

import httpx
from app.config import get_settings

settings = get_settings()

EXTRACT_PROMPT = """Extract metadata from the following academic document content. Return strictly in JSON format without any additional text.

Important Rules:
1. First determine the primary language of the document (language field) using ISO 639-1 codes:
   - en = English
   - zh = Chinese
   - ja = Japanese
   - fr = French
   - ru = Russian
   - de = German
   - ko = Korean
   - es = Spanish
   - pt = Portuguese
   - ar = Arabic
   - Use the corresponding ISO 639-1 code for other languages

2. For non-English documents (language != "en"), provide both original and English versions:
   - title: Original title
   - title_en: English title
   - authors: Original author names (list)
   - authors_en: English author names (romanized or translated)
   - abstract: Original abstract
   - abstract_en: English abstract
   - journal: Original journal/conference name
   - journal_en: English journal/conference name
   - keywords: Original keywords (list)
   - keywords_en: English keywords (list)

3. For English documents (language="en"), leave *_en fields empty

Return the following JSON format:
{{"language": "language_code", "type": "book|article|conference|thesis", "title": "Title (original)", "title_en": "English title (required for non-English)", "year": publication_year (integer), "authors": ["Author1", "Author2"], "authors_en": ["Author1", "Author2"], "abstract": "Abstract (original)", "abstract_en": "English abstract (required for non-English)", "doi": "DOI", "source": "Source", "journal": "Journal/Conference name (original)", "journal_en": "English journal/conference (required for non-English)", "keywords": ["keyword1", "keyword2"], "keywords_en": ["keyword1", "keyword2"], "category": "Category"}}

Document Content:
{content}
"""


async def extract_metadata(markdown_content: str) -> dict:
    """
    Use Ollama to extract metadata from markdown content.
    Returns a dict with metadata fields including language detection.
    """
    # Truncate content if too long (keep first 4000 chars for faster processing)
    truncated = markdown_content[:4000]
    prompt = EXTRACT_PROMPT.format(content=truncated)

    try:
        # Use longer timeout (600s) for large documents
        async with httpx.AsyncClient(timeout=600.0) as client:
            response = await client.post(
                f"{settings.ollama_url}/api/generate",
                json={
                    "model": settings.ollama_model,
                    "prompt": prompt,
                    "stream": False,
                    "options": {
                        "temperature": 0.1,
                        "num_predict": 1024,  # Limit response length
                    },
                },
            )
            response.raise_for_status()
            result = response.json()
    except httpx.ConnectError as e:
        raise Exception(f"Failed to connect to Ollama service ({settings.ollama_url}): {e}")
    except httpx.TimeoutException as e:
        raise Exception(f"Ollama request timed out (600s): {e}")
    except httpx.HTTPStatusError as e:
        raise Exception(f"Ollama returned error status {e.response.status_code}: {e.response.text[:200]}")
    except Exception as e:
        raise Exception(f"Ollama request failed: {type(e).__name__}: {e}")

    # Extract the response text
    response_text = result.get("response", "")

    # Debug: print response length
    print(f"Ollama response length: {len(response_text)}")

    if not response_text:
        # Return empty metadata if no response
        return _get_empty_metadata()

    # Try to parse JSON from the response
    metadata = _parse_json_from_text(response_text)

    return metadata


def _get_empty_metadata() -> dict:
    """Return empty metadata dict."""
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


def _parse_json_from_text(text: str) -> dict:
    """Extract JSON from LLM response text."""
    import re

    # Clean the text - remove markdown code blocks if present
    text = text.strip()
    if text.startswith("```json"):
        text = text[7:]
    elif text.startswith("```"):
        text = text[3:]
    if text.endswith("```"):
        text = text[:-3]
    text = text.strip()

    # Try direct parse first
    try:
        return json.loads(text)
    except json.JSONDecodeError:
        pass

    # Try to find JSON block in the text using a more robust pattern
    # Match the outermost braces
    brace_start = text.find('{')
    if brace_start != -1:
        # Find matching closing brace
        depth = 0
        for i in range(brace_start, len(text)):
            if text[i] == '{':
                depth += 1
            elif text[i] == '}':
                depth -= 1
                if depth == 0:
                    json_str = text[brace_start:i + 1]
                    try:
                        return json.loads(json_str)
                    except json.JSONDecodeError:
                        # Try to fix common JSON issues
                        # Remove trailing commas before closing braces
                        fixed = re.sub(r',\s*}', '}', json_str)
                        fixed = re.sub(r',\s*]', ']', fixed)
                        try:
                            return json.loads(fixed)
                        except json.JSONDecodeError:
                            pass
                    break

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


async def get_embedding(text: str | list[str], vector_db_id: str = None) -> list[float] | list[list[float]]:
    """
    Get embedding vector(s) based on configuration.
    Supports both single text and batch (list of texts).

    - Single text -> returns list[float]
    - Batch (list) -> returns list[list[float]]

    Args:
        text: Text or list of texts to embed
        vector_db_id: Optional vector database ID to use specific embedding config
    """
    is_batch = isinstance(text, list)

    # 获取 embedding 配置
    if vector_db_id:
        from app.config_manager import get_vector_db_by_id
        vdb_config = get_vector_db_by_id(vector_db_id)
        if vdb_config and 'embedding' in vdb_config:
            emb_config = vdb_config['embedding']
            source = emb_config.get('source', 'ollama')
            model = emb_config.get('model', 'nomic-embed-text')
        else:
            source = settings.embedding_source
            model = settings.embedding_model
    else:
        source = settings.embedding_source
        model = settings.embedding_model

    if source == "builtin":
        return await _get_embedding_builtin(text, is_batch, model)
    else:
        return await _get_embedding_ollama(text, is_batch, model)


async def _get_embedding_ollama(text: str | list[str], is_batch: bool, model_name: str = None) -> list[float] | list[list[float]]:
    """Get embedding from Ollama API."""
    input_data = text if is_batch else [text]
    model = model_name or settings.embedding_model

    async with httpx.AsyncClient(timeout=120.0) as client:
        response = await client.post(
            f"{settings.ollama_url}/api/embed",
            json={
                "model": model,
                "input": input_data,
            },
        )
        response.raise_for_status()
        result = response.json()

    embeddings = result.get("embeddings", [])

    if is_batch:
        return embeddings
    return embeddings[0] if embeddings else []


async def _get_embedding_builtin(text: str | list[str], is_batch: bool, model_name: str = None) -> list[float] | list[list[float]]:
    """Get embedding using FastEmbed library (local embedding)."""
    import os

    # 设置 HuggingFace 镜像（如果需要）
    if not os.environ.get('HF_ENDPOINT'):
        os.environ['HF_ENDPOINT'] = 'https://hf-mirror.com'
    if not os.environ.get('HF_HUB_DISABLE_XET'):
        os.environ['HF_HUB_DISABLE_XET'] = '1'

    try:
        from fastembed import TextEmbedding
    except ImportError:
        raise ImportError("fastembed 库未安装，请运行: pip install fastembed")

    # 使用 fastembed 进行本地 embedding
    model = TextEmbedding(model_name=model_name or settings.embedding_model)

    if is_batch:
        # Batch embedding
        embeddings = list(model.embed(text))
        return [emb.tolist() for emb in embeddings]
    else:
        # Single embedding
        embeddings = list(model.embed([text]))
        return embeddings[0].tolist() if embeddings else []


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
