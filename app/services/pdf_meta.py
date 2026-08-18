"""轻量级 PDF 元数据（含 DOI）提取。

仅在上传/注册时从 PDF 中读取标准信息字典（/Title、/Author、
/Subject、/Keywords、/CreationDate），并尽量从元数据或首页正文中
解析 DOI。所有解析都被 try/except 包裹，失败时返回空字典，确保
上传流程永不中断。

优先使用 pypdf；若不可用则尝试 PyPDF2；两者皆无则无法解析。
"""
import io
import re

# 可选依赖：优先 pypdf，回退 PyPDF2
try:
    from pypdf import PdfReader  # type: ignore
except ImportError:  # pragma: no cover
    try:
        from PyPDF2 import PdfReader  # type: ignore
    except ImportError:
        PdfReader = None  # type: ignore

# DOI 正则：10.XXXX/...（大小写不敏感）
_DOI_RE = re.compile(r"10\.\d{4,9}/[-._;()/:A-Za-z0-9]+", re.IGNORECASE)


def normalize_doi(raw: str | None) -> str | None:
    """规范化 DOI 字符串。

    去除首尾空白；剥离前置 ``doi:`` / ``https://doi.org/`` /
    ``http://doi.org/`` / ``http://dx.doi.org/`` 等前缀；剥离尾部的
    ``.``、``,``、``;``、``)`` 等常见标点。为空时返回 None。
    """
    if not raw:
        return None
    s = str(raw).strip()
    # 剥离已知前缀（只剥离一次）
    for prefix in ("https://doi.org/", "http://doi.org/",
                   "https://dx.doi.org/", "http://dx.doi.org/", "doi:"):
        if s.lower().startswith(prefix):
            s = s[len(prefix):].strip()
            break
    # 剥离尾部标点
    s = s.rstrip(".,;)")
    if not s:
        return None
    return s


def _split_authors(raw: str | None) -> list[str]:
    """将 /Author 字段按常见分隔符拆分为作者列表。"""
    if not raw:
        return []
    # 常见分隔符：分号、逗号、" and "
    parts = re.split(r";|,|\s+and\s+", raw)
    out = [p.strip() for p in parts if p and p.strip()]
    return out


def _split_keywords(raw: str | None) -> list[str] | None:
    """将 /Keywords 字段拆分为关键词列表。"""
    if not raw:
        return None
    parts = re.split(r";|,", raw)
    out = [p.strip() for p in parts if p and p.strip()]
    return out or None


def _parse_year_from_metadata(*strings: str | None) -> int | None:
    """从元数据字符串中解析 4 位年份（19xx/20xx）。"""
    for s in strings:
        if not s:
            continue
        # 优先匹配 PDF 日期格式 D:YYYYMMDDHHmmSS
        m = re.search(r"D:(\d{4})(\d{2})(\d{2})", s)
        if m:
            year = int(m.group(1))
            if 1900 <= year <= 2100:
                return year
        # 退而求其次：任意 19xx/20xx 年份
        m = re.search(r"(19|20)\d{2}", s)
        if m:
            return int(m.group(0))
    return None


def _collect_metadata_strings(meta) -> list[str]:
    """收集信息字典中的全部字符串，供 DOI 扫描使用。"""
    values = []
    for key in ("/Title", "/Author", "/Subject", "/Keywords"):
        v = getattr(meta, key.lower().lstrip("/"), None)
        if v:
            values.append(str(v))
    return values


def extract_pdf_metadata(content: bytes) -> dict:
    """从 PDF 字节内容中提取元数据。

    返回字典：``{title, authors, year, doi, keywords, abstract}``。
    任何解析异常都被吞掉并返回空字典 ``{}``，保证上传流程不中断。
    """
    if PdfReader is None:
        # 无可用 PDF 库，直接跳过
        return {}

    try:
        # 外层统一捕获：任意异常均视为解析失败，返回空字典 {}
        reader = PdfReader(io.BytesIO(content))

        meta = reader.metadata

        result: dict = {
            "title": None,
            "authors": None,
            "year": None,
            "doi": None,
            "keywords": None,
            "abstract": None,
        }

        if meta is None:
            return result

        title = str(meta.title) if meta.title else None
        author = str(meta.author) if meta.author else None
        subject = str(meta.subject) if meta.subject else None
        keywords_raw = str(meta.keywords) if meta.keywords else None
        creation = str(meta.creation_date) if meta.creation_date else None

        # 标题
        if title and title.strip():
            result["title"] = title.strip()

        # 作者（列表）
        authors = _split_authors(author)
        if authors:
            result["authors"] = authors

        # 关键词（列表）
        keywords = _split_keywords(keywords_raw)
        if keywords:
            result["keywords"] = keywords

        # 年份
        year = _parse_year_from_metadata(
            creation or "", title or "", author or "", subject or "", keywords_raw or ""
        )
        if year:
            result["year"] = year

        # 摘要：仅当 Subject 明显长于标题时，认为它是摘要而非标题
        if subject and len(subject) > 80 and subject != title:
            result["abstract"] = subject.strip()

        # DOI：先从元数据字符串中扫描
        doi = None
        for s in _collect_metadata_strings(meta):
            m = _DOI_RE.search(s)
            if m:
                doi = normalize_doi(m.group(0))
                if doi:
                    break
        # 若元数据中没有，则从首页正文（有界）中扫描
        if not doi:
            first_page = reader.pages[0]
            text = first_page.extract_text() or ""
            # 限制扫描范围，避免大页耗时过长
            for m in _DOI_RE.finditer(text[:20000]):
                candidate = normalize_doi(m.group(0))
                if candidate:
                    doi = candidate
                    break
        result["doi"] = doi

        # 清理：移除所有为 None 的键，便于调用方判断
        return {k: v for k, v in result.items() if v is not None}
    except Exception:
        # 任意解析异常（含 PdfReader 构造/读取失败）均返回空字典
        return {}
