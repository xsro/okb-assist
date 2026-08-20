"""Crossref 元数据获取服务（异步）。

将 scripts/enrich_empty_title_via_crossref.py 中的 Crossref 逻辑重写为
应用内可用的异步模块：

- 通过 ``https://api.crossref.org/works`` 按 DOI 或题名查询；
- 解析出与 ``DocumentUpdate`` 形状一致的字段（含 *_en 双语副本）；
- 原始返回 JSON 写入 ``get_crossref_path(doc_id)``，便于追溯。

配置从 system.json 读取：``crossref_url``、``crossref_mailto``，
缺失时使用内置默认值。
"""
import html
import json
import re

import httpx

from app.config_manager import get_system_config


def _get_base_url() -> str:
    """返回 Crossref 接口基址（可配置）。"""
    return get_system_config().get("crossref_url", "https://api.crossref.org/works")


def _get_mailto() -> str:
    """返回 polite-pool 联系邮箱（可配置）。"""
    return get_system_config().get("crossref_mailto", "okb-assist@example.com")


def _get_user_agent() -> str:
    """构造符合 polite-pool 约定的 User-Agent。"""
    return f"okb-assist/1.0 (mailto:{_get_mailto()})"


def strip_markup(text: str | None) -> str:
    """去除 HTML/MathML 标记并解码常见实体。

    保留 <tex-math>/<mml:math> 内的文本（避免破坏数学内容），
    其余标签直接删除，最后解码常见 HTML 实体。None 返回空串。
    """
    if not text:
        return ""
    # 保留数学标记内的文本
    t = re.sub(r"<tex-math[^>]*>(.*?)</tex-math>", r" \1 ", text, flags=re.S | re.I)
    t = re.sub(r"<mml:math[^>]*>(.*?)</mml:math>", r" \1 ", t, flags=re.S | re.I)
    # 删除其余标签
    t = re.sub(r"<[^>]+>", "", t)
    # 解码常见实体
    t = t.replace("&amp;", "&").replace("&lt;", "<").replace("&gt;", ">")
    t = t.replace("&quot;", '"').replace("&#39;", "'").replace("&nbsp;", " ")
    return html.unescape(t).strip()


def normalize_doi(raw) -> str | None:
    """规范化 DOI：剥离前缀/后缀标点，空值返回 None。"""
    if not raw:
        return None
    s = str(raw).strip()
    for prefix in ("https://doi.org/", "http://doi.org/",
                   "https://dx.doi.org/", "http://dx.doi.org/", "doi:"):
        if s.lower().startswith(prefix):
            s = s[len(prefix):].strip()
            break
    s = s.rstrip(".,;)")
    if not s:
        return None
    return s


def crossref_authors(item: dict) -> list[str]:
    """从 Crossref author 列表构造展示名列表。

    优先 ``given family``，缺失任一时退化为 family 或 given，
    再退化为 name。两者皆缺则跳过。
    """
    out: list[str] = []
    for a in item.get("author", []) or []:
        name = (a.get("name") or "").strip()
        if name:
            out.append(name)
            continue
        given = (a.get("given") or "").strip()
        family = (a.get("family") or "").strip()
        if given and family:
            out.append(f"{given} {family}")
        elif family:
            out.append(family)
        elif given:
            out.append(given)
    return out


def doc_type_map(crossref_type: str) -> str:
    """将 Crossref 的 type 映射到本项目的 doc_type 词表。

    未命中已知值时回退为 ``other``。
    """
    mapping = {
        "journal-article": "journal",
        "book": "book",
        "book-chapter": "book",
        "proceedings-article": "conference",
        "conference-paper": "conference",
        "dataset": "dataset",
        "report": "report",
        "thesis": "thesis",
        "posted-content": "preprint",
        "reference-entry": "reference",
        "peer-review": "other",
    }
    return mapping.get((crossref_type or "").strip().lower(), "other")


def parse_crossref_item(message: dict) -> dict:
    """将 Crossref work 项解析为 DocumentUpdate 形状的字段字典。"""
    # 标题
    raw_title = message.get("title")
    if isinstance(raw_title, list):
        title = strip_markup(raw_title[0] if raw_title else "")
    else:
        title = strip_markup(raw_title or "")

    authors = crossref_authors(message)

    # DOI
    doi = normalize_doi(message.get("DOI"))

    # 年份
    year = None
    try:
        date_parts = message.get("issued", {}).get("date-parts", [[None]])
        first_part = date_parts[0] if date_parts else [None]
        y = first_part[0] if first_part else None
        if isinstance(y, int):
            year = y
    except Exception:
        year = None

    # 来源/出版方
    source = message.get("publisher") or "Crossref"

    # 期刊/会议名
    raw_ct = message.get("container-title")
    if isinstance(raw_ct, list) and raw_ct:
        journal = raw_ct[0] or ""
    elif isinstance(raw_ct, str):
        journal = raw_ct
    else:
        journal = ""
    journal = html.unescape(journal).strip() if journal else ""

    # 关键词
    keywords = message.get("subject") if isinstance(message.get("subject"), list) else None

    # 摘要
    abstract = strip_markup(message.get("abstract"))

    # 文献类型
    doc_type = doc_type_map(message.get("type", ""))

    # 语言
    language = message.get("language")

    # 组装基础字段
    parsed = {
        "title": title,
        "authors": authors,
        "doi": doi,
        "year": year,
        "source": source,
        "journal": journal,
        "keywords": keywords,
        "abstract": abstract,
        "doc_type": doc_type,
        "language": language,
    }

    # 双语 *_en 副本：基础字段存在则保持一致
    parsed["title_en"] = title if title else None
    parsed["authors_en"] = authors if authors else None
    parsed["journal_en"] = journal if journal else None
    parsed["keywords_en"] = keywords if keywords else None
    parsed["abstract_en"] = abstract if abstract else None

    return parsed


async def lookup_by_doi(doi: str) -> dict | None:
    """按 DOI 查询 Crossref。

    成功返回 ``{"raw": <完整响应信封>, "parsed": <parsed dict>}``；
    404 或无结果返回 None；网络/解析异常则抛出封装后的 Exception，
    交由调用方报告。
    """
    base = _get_base_url()
    mailto = _get_mailto()
    try:
        async with httpx.AsyncClient(timeout=30.0) as client:
            r = await client.get(
                f"{base}/{normalize_doi(doi)}",
                params={"mailto": mailto},
                headers={"User-Agent": _get_user_agent()},
            )
        if r.status_code == 404:
            return None
        r.raise_for_status()
        data = r.json()
        msg = data["message"]
        return {"raw": data, "parsed": parse_crossref_item(msg)}
    except httpx.ConnectError as e:
        raise Exception(f"无法连接 Crossref 服务 ({base}): {e}")
    except httpx.TimeoutException as e:
        raise Exception(f"Crossref 请求超时: {e}")
    except httpx.HTTPStatusError as e:
        raise Exception(f"Crossref 返回错误状态 {e.response.status_code}: {e.response.text[:200]}")
    except Exception as e:
        raise Exception(f"Crossref 请求失败: {type(e).__name__}: {e}")


async def lookup_by_title(title: str) -> dict | None:
    """按题名模糊查询 Crossref，取第一条结果。

    返回形状同 ``lookup_by_doi``；无结果返回 None。
    """
    base = _get_base_url()
    mailto = _get_mailto()
    try:
        async with httpx.AsyncClient(timeout=30.0) as client:
            r = await client.get(
                base,
                params={"query.bibliographic": title, "rows": 1, "mailto": mailto},
                headers={"User-Agent": _get_user_agent()},
            )
        r.raise_for_status()
        data = r.json()
        items = data["message"]["items"]
        if not items:
            return None
        item = items[0]
        return {"raw": data, "parsed": parse_crossref_item(item)}
    except httpx.ConnectError as e:
        raise Exception(f"无法连接 Crossref 服务 ({base}): {e}")
    except httpx.TimeoutException as e:
        raise Exception(f"Crossref 请求超时: {e}")
    except httpx.HTTPStatusError as e:
        raise Exception(f"Crossref 返回错误状态 {e.response.status_code}: {e.response.text[:200]}")
    except Exception as e:
        raise Exception(f"Crossref 请求失败: {type(e).__name__}: {e}")
