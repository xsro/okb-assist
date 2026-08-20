"""轻量级 PDF 元数据（含 DOI）提取。

优先使用经典信息字典（/Title、/Author、/Subject、/Keywords、
/CreationDate），并尽量从以下来源补全标题与 DOI：
1. 信息字典（Info dictionary）；
2. XMP 元数据（xmp_metadata）；
3. 首页/前两页正文推断（针对缺失 Info /Title 的 PDF）。

同时兼容空密码加密的 PDF：若 ``reader.is_encrypted`` 为真，则尝试
用空密码 ``reader.decrypt("")`` 解密。

所有解析都被 try/except 包裹，失败时返回空字典，确保上传/修复流程
永不中断。

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

# 正文推断标题时，需要跳过的“非标题”行前缀（小写比较）
_BAD_LINE_PREFIXES = (
    "abstract", "introduction", "keywords", "contents", "references",
    "doi", "http", "https", "www.", "©", "vol", "pp", "page", "figure",
    "table", "appendix", "arxiv",
)

# 用于判断“标题是否看起来像文件名”的 CJK 字符范围
_CJK_RE = re.compile(
    r"[\u3000-\u9fff\uff00-\uffef\u3040-\u30ff\uac00-\ud7af]"
)


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


def _norm_filename(s: str) -> str:
    """将文件名/标题归一化，用于“标题是否等于文件名”的比较。

    规则：转小写、去除首尾空白；若以 ``.pdf`` 结尾则去掉扩展名；
    再去掉 ``. _ -`` 与空白字符（保留 CJK），便于比较
    ``Real Title Here`` 与 ``real-title-here.pdf`` 是否等价。
    """
    s = s.strip().lower()
    if s.endswith(".pdf"):
        s = s[:-4]
    for ch in "._- ":
        s = s.replace(ch, "")
    return s


def _looks_like_filename(title: str, filename: str | None = None) -> bool:
    """判断给定标题是否“看起来像文件名”（应被当作无效标题跳过）。

    判定为 True 的情况：
    1) 以小写 ``.pdf`` 结尾；
    2) 与给定 ``filename`` 归一化后相等（含去掉扩展名、``._-`` 与空白）；
    3) 无空白、无 CJK、且完全由 ``[a-z0-9._-]`` 组成的 slug（如 ``paper``、
       ``some-document-v2`` 这类典型文件名/标识串）。

    保持简单、安全：CJK 标题与含空格的正常标题都会被判定为 False。
    """
    if not title:
        return False
    t = str(title).strip()
    low = t.lower()

    # 1) 以 .pdf 结尾（最常见情况）
    if low.endswith(".pdf"):
        return True

    # 2) 与文件名归一化后等价，且标题本身“不含空格”（即退化的“文件名 stem”，
    #    例如导入时退化为 ``some-document-v2`` 这类 slug）。
    #    若标题含有空格（正常论文标题，如 “Output Feedback Regulation of Heat
    #    Equations”）却恰好与文件名相等，应视为真实标题予以保留，避免把正确
    #    标题误判为文件名而丢弃（否则含空格的真实标题会被错误置空）。
    if filename:
        if " " not in t and _norm_filename(t) == _norm_filename(filename):
            return True

    # 3) 单 token 的 ASCII slug（无空白、无 CJK、仅含 [a-z0-9._-]）
    if (
        " " not in t
        and not _CJK_RE.search(t)
        and re.fullmatch(r"[a-z0-9._-]+", low) is not None
    ):
        return True

    return False


def _extract_xmp_title(reader) -> str | None:
    """从 pypdf 的 XMP 元数据中尝试提取标题。

    pypdf 的 ``XmpInformation`` 可能暴露 ``titles`` / ``dc_title`` 属性，
    其取值既可能是“字符串列表”，也可能是“(语言, 字符串) 元组列表”。
    这里做防御式处理，返回第一个非空、去空白后的字符串；否则返回 None。
    """
    xmp = getattr(reader, "xmp_metadata", None)
    if xmp is None:
        return None

    candidates: list[str] = []

    def _gather(value):
        if not value:
            return
        if isinstance(value, str):
            candidates.append(value)
        elif isinstance(value, (list, tuple)):
            for item in value:
                if isinstance(item, tuple) and len(item) >= 2:
                    # (lang, title) 形式
                    candidates.append(str(item[1]))
                elif isinstance(item, str):
                    candidates.append(item)

    _gather(getattr(xmp, "titles", None))
    _gather(getattr(xmp, "dc_title", None))

    for c in candidates:
        c = (c or "").strip()
        if c:
            return c
    return None


def _looks_like_citation(line: str) -> bool:
    """判断某行是否“看起来像期刊引用条 / 页眉横幅”（应被当作伪标题跳过）。

    命中以下任一情形即返回 True：

    1) 形如 “期刊名 卷号 (年份) 文章编号”，例如
       ``Automatica 193 (2026) 113206``；
    2) 包含（大小写不敏感）下列“强引用标记”之一：``doi:`` /
       ``https://doi`` / ``http://doi`` / ``issn`` / ``©`` / ``(c)`` /
       ``received`` / ``accepted`` / ``available online`` / ``sciencedirect`` /
       ``article number`` / ``elsevier`` / ``springer`` / ``wiley``；
    3) 卷期 / 页码类“弱标记”后接数字，例如 ``vol. 64`` / ``volume 12`` /
       ``issue 3`` / ``no. 8`` / ``pp. 123``。

    注意：``vol.`` / ``volume`` / ``issue`` / ``no.`` / ``pp.`` 等弱标记
    必须后接数字才判定为引用条，避免把正文里的 ``Initiative``、
    ``Issues for Congress`` 等普通词误判成期刊引用条（否则会把正确的真实
    标题当作误题记录而错误覆盖）。

    用于在正文推断标题时排除“期刊引用条”，避免把“卷期年号 + 文章编号”
    误当作论文标题（期刊论文首页首行常是此类横幅，而非真实论文标题）。
    """
    if not line:
        return False
    s = line.strip()

    # 情形 1：期刊名 卷号 (年份) 文章编号
    if re.search(r"^\s*[\w\s.,&/\'\-]+?\s+\d+\s*\(\d{4}\)\s*[\d-]+\s*$", s):
        return True

    low = s.lower()

    # 情形 2：强引用标记（出现即判定为引用条 / 页眉）
    strong_markers = (
        "doi:", "https://doi", "http://doi", "issn", "©", "(c)",
        "received", "accepted", "available online", "sciencedirect",
        "article number", "elsevier", "springer", "wiley",
    )
    if any(marker in low for marker in strong_markers):
        return True

    # 情形 3：卷期 / 页码类弱标记，需后接数字才是“卷期年号”引用条
    soft_patterns = (
        r"vol\.\s*\d",      # vol. 64
        r"volume\s+\d",     # volume 12
        r"issue\s+\d",      # issue 3
        r"no\.\s*\d",       # no. 8
        r"pp\.\s*\d",       # pp. 123
    )
    if any(re.search(p, low) for p in soft_patterns):
        return True

    return False


def _infer_title_from_text(reader) -> str | None:
    """从 PDF 前两页正文中推断真实论文标题。

    采集前两页（``reader.pages[:2]``）的非空行，先构建候选标题集，再按
    启发式挑选真实标题：

    - 候选需满足：长度 8~250；不以已知章节 / 页眉前缀开头；不是编号小节；
      不是纯数字 / 标点；不是“期刊引用条”（``_looks_like_citation``）；
      且不是“单个全大写单词”（多数情况下只是期刊名横幅，而非论文标题）；
    - 优选：第一个“含 >=3 个词、且至少含一个小写字母”的候选（正常标题
      大小写混合，而非全大写横幅）；
    - 退路：若无候选满足优选条件，则返回第一个候选；仍无候选则返回 None。

    整体包在 try/except 中，遇损坏 PDF 时返回 None，保证调用方流程不中断。
    """
    try:
        pages = getattr(reader, "pages", None)
        if not pages:
            return None

        # 采集前两页的非空、去空白行
        lines: list[str] = []
        for page in pages[:2]:
            try:
                page_text = page.extract_text() or ""
            except Exception:
                page_text = ""
            for line in page_text.splitlines():
                line = line.strip()
                if line:
                    lines.append(line)

        # 构建候选标题行（逐条过滤伪标题）
        candidates: list[str] = []
        for line in lines:
            low = line.lower()
            # 长度窗口：太短或太长都不像标题
            if not (8 <= len(line) <= 250):
                continue
            # 跳过已知章节 / 页眉前缀（大小写不敏感）
            if any(low.startswith(p) for p in _BAD_LINE_PREFIXES):
                continue
            # 跳过“数字.”或“数字)”开头的编号小节
            if re.match(r"^\d+[\.\)]", low):
                continue
            # 跳过纯数字 / 标点行
            if re.fullmatch(r"[\d\s\W]+", line):
                continue
            # 跳过期刊引用条（卷期年号 + 文章编号等）
            if _looks_like_citation(line):
                continue
            # 跳过“单个全大写单词”（通常是期刊名横幅，而非论文标题）
            tokens = line.split()
            if len(tokens) == 1 and line.isupper():
                continue
            candidates.append(line)

        if not candidates:
            return None

        # 优选：>=3 个词且至少含一个小写字母（正常大小写混合的标题）
        for cand in candidates:
            words = cand.split()
            has_lower = any(c.islower() for c in cand)
            if len(words) >= 3 and has_lower:
                return cand

        # 退路：返回第一个候选
        return candidates[0]
    except Exception:
        # 损坏 PDF / 任意解析异常：返回 None
        return None


def extract_pdf_metadata(content: bytes, filename: str | None = None) -> dict:
    """从 PDF 字节内容中提取元数据。

    返回字典：``{title, authors, year, doi, keywords, abstract}``。
    任意解析异常都被吞掉并返回空字典 ``{}``，保证调用方流程不中断。

    :param content: PDF 文件二进制内容。
    :param filename: 可选的原始文件名（用于判断“标题是否只是文件名”，
                     帮助在 Info 字典缺失 /Title 时从 XMP / 正文推断真实标题）。
                     保持向后兼容：不传也能正常工作。
    """
    if PdfReader is None:
        # 无可用 PDF 库，直接跳过
        return {}

    try:
        # 外层统一捕获：任意异常均视为解析失败，返回空字典 {}
        reader = PdfReader(io.BytesIO(content))

        # 解密：兼容空密码加密的 PDF（让无密码加密文档也能读取）
        if getattr(reader, "is_encrypted", False):
            try:
                reader.decrypt("")
            except Exception:
                # 解密失败（例如有密码）：忽略，后续解析可能为空字典
                pass

        meta = reader.metadata

        result: dict = {
            "title": None,
            "authors": None,
            "year": None,
            "doi": None,
            "keywords": None,
            "abstract": None,
        }

        # 读取信息字典字符串（meta 可能为 None）
        title = None
        author = None
        subject = None
        keywords_raw = None
        creation = None
        if meta is not None:
            title = str(meta.title) if meta.title else None
            author = str(meta.author) if meta.author else None
            subject = str(meta.subject) if meta.subject else None
            keywords_raw = str(meta.keywords) if meta.keywords else None
            creation = str(meta.creation_date) if meta.creation_date else None

        # -------- 标题解析：Info 字典 → XMP → 正文推断 --------
        # 最终仅在“标题非空、不像文件名、且不是期刊引用条”时写入 result["title"]
        resolved_title = str(title).strip() if title else None

        # 是否需要放弃当前 resolved_title，继续尝试 XMP / 正文推断：
        #   1) 缺失（None / 空串）；
        #   2) 像文件名（退化成文件名 stem）；
        #   3) 像期刊引用条（卷期年号横幅，例如 “IEEE TRANSACTIONS ... VOL. 71,
        #      NO. 8, AUGUST 2026” 或 “Automatica 193 (2026) 113206”）。
        # 注意：部分 PDF 的 Info /Title 本身就是期刊引用条而非真实标题，若不
        # 排除会导致“信任错误的 Info 标题、跳过正文推断”，从而留下误题记录。
        def _need_better_title(t: str | None) -> bool:
            return (
                t is None
                or _looks_like_filename(t, filename)
                or _looks_like_citation(t)
            )

        # 若 Info 标题缺失 / 像文件名 / 像期刊引用条，尝试 XMP 标题
        if _need_better_title(resolved_title):
            xmp_title = _extract_xmp_title(reader)
            if xmp_title and not _looks_like_filename(xmp_title, filename):
                resolved_title = xmp_title

        # 仍缺失 / 像文件名 / 像期刊引用条，尝试从正文推断
        if _need_better_title(resolved_title):
            inferred = _infer_title_from_text(reader)
            if inferred and not _looks_like_filename(inferred, filename):
                resolved_title = inferred

        if (
            resolved_title
            and not _looks_like_filename(resolved_title, filename)
            and not _looks_like_citation(resolved_title)
        ):
            result["title"] = resolved_title

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
        if subject and len(subject) > 80 and subject != (title or ""):
            result["abstract"] = subject.strip()

        # -------- DOI：先扫信息字典 4 个字符串，再扫前三页正文 --------
        doi = None
        for s in _collect_metadata_strings(meta) if meta is not None else []:
            m = _DOI_RE.search(s)
            if m:
                doi = normalize_doi(m.group(0))
                if doi:
                    break
        # 若元数据中仍没有，则从前三页正文中扫描（有界范围）
        if not doi:
            pages = getattr(reader, "pages", None) or []
            for page in pages[:3]:
                try:
                    text = page.extract_text() or ""
                except Exception:
                    text = ""
                # 限制扫描范围，避免大页耗时过长
                for m in _DOI_RE.finditer(text[:20000]):
                    candidate = normalize_doi(m.group(0))
                    if candidate:
                        doi = candidate
                        break
                if doi:
                    break
        result["doi"] = doi

        # 清理：移除所有为 None 的键，便于调用方判断
        return {k: v for k, v in result.items() if v is not None}
    except Exception:
        # 任意解析异常（含 PdfReader 构造/读取失败）均返回空字典
        return {}
