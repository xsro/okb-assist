#!/usr/bin/env python3
"""从外置盘的 PDF 补齐 OKB-Assist 文献库的数据。

背景：数据库被删后由 Markdown 重建，但「文件事实」（pdf_size/zip_size/fs_synced_at）、
`file_hash`、`pdf_health` 这些只有 PDF 才能得出；部分文献的元数据（DOI/题名/作者/刊名/
摘要/关键词/年份）也缺失。本脚本直接读 PDF 补齐这些列。

做四件事（可单独挑，见 --steps）：
  stats    : stat 每篇的 PDF / Markdown / 资源包，写 pdf_size / md_size / zip_size / fs_synced_at
  hash     : 给缺 file_hash 且有 PDF 的文档算 SHA256（与后端 calculate_file_hash 一致）
  health   : PDF 健康检查（文件头 %PDF-、页数、加密、文件尾 %%EOF、解析器 stderr 诊断）
  meta     : 从 PDF 抽 DOI / 题名 / 作者 / 刊名 / 年份 / 摘要 / 关键词，只补空字段

安全约定：
  * 只补空（`--all` 才是覆盖），已有值一律不动；
  * 元数据只接受高置信度来源：引用块 / 显式字段标签（题目：作者：）/ 首页最大字号版面；
  * 版面抽出的作者还要能在同篇 Markdown 正文里找到至少一个名字，否则丢弃；
  * 所有改动写入 JSONL 报告，可逐条审计。

用法：
  uv run --no-project python scripts/recover-from-pdfs.py --dry-run
  uv run --no-project python scripts/recover-from-pdfs.py --steps stats,hash
  uv run --no-project python scripts/recover-from-pdfs.py --limit 20 --verbose
  uv run --no-project python scripts/recover-from-pdfs.py              # 四步全跑

注意：请在**后端停止**时运行（避免两边同时写库）。
"""
from __future__ import annotations

import argparse
import collections
import concurrent.futures as cf
import hashlib
import html
import json
import os
import re
import sqlite3
import subprocess
import sys
import time
import unicodedata
import xml.etree.ElementTree as ET  # noqa: F401  (保留：部分环境用 XML 解析兜底)

# ─────────────────────────── 通用工具 ───────────────────────────

CTRL_RE = re.compile(r'[\x00-\x08\x0b\x0c\x0e-\x1f\x7f]')
TAG_RE = re.compile(r'<[^>]{1,200}>')
CJK_RE = re.compile(r'[\u3400-\u9fff\uf900-\ufaff]')
MEANINGFUL_RE = re.compile(r'[\u3400-\u9fff\uf900-\ufaffA-Za-z0-9]')
DOI_RE = re.compile(r'10\.\s?\d{4,9}\s?/\s?[^\s"\'<>，。；、]+')
IMG_MD = re.compile(r'!\[[^\]]*\]\([^)]*\)')
LINK_MD = re.compile(r'\[([^\]]*)\]\([^)]*\)')
EMPH_MD = re.compile(r'\*\*|__|^\s*\*+\s*|\s*\*+\s*$')


def log(msg: str, quiet: bool = False) -> None:
    if not quiet:
        print(msg, flush=True)


def clean(s) -> str:
    if s is None:
        return ''
    s = str(s)
    s = IMG_MD.sub(' ', s)
    s = LINK_MD.sub(r'\1', s)
    s = TAG_RE.sub('', s)
    s = html.unescape(s)
    s = EMPH_MD.sub(' ', s)
    s = CTRL_RE.sub(' ', s)
    s = re.sub(r'[ \t\u00a0\u3000]+', ' ', s)
    return s.strip()


def despace(s) -> str:
    """中文逐字加空格时去掉字间空格（「舰 船 电 子」→「舰船电子」）"""
    s = clean(s)
    if len(CJK_RE.findall(s)) >= 2 and re.search(r'[\u4e00-\u9fff] [\u4e00-\u9fff]', s):
        s = re.sub(r'(?<=[\u4e00-\u9fff])\s+(?=[\u4e00-\u9fff])', '', s)
    return s


def norm_key(s) -> str:
    return re.sub(r'[\W_]+', '', unicodedata.normalize('NFKD', clean(s)).lower())


def is_garbage(text) -> bool:
    """字体缺 ToUnicode 时抽出成片乱码：实义字符占比 < 55% 即视为噪声"""
    t = re.sub(r'\s+', '', text or '')
    if not t:
        return True
    return len(MEANINGFUL_RE.findall(t)) / len(t) < 0.55


def tidy_doi(raw) -> str | None:
    s = clean(raw)
    if not s:
        return None
    s = s.split('](')[0].rstrip(']')
    s = re.sub(r'10\.\s+(?=\d)', '10.', s)
    s = re.sub(r'\s*/\s*', '/', s)
    s = re.sub(r'[\s\u3400-\u9fff].*$', '', s)
    s = s.rstrip('.,;:，。；-')
    while s.endswith(')') and s.count(')') > s.count('('):
        s = s[:-1]
    if not re.match(r'^10\.\d{4,9}/\S{3,}$', s):
        return None
    if not re.search(r'\d', s.split('/', 1)[1]):
        return None
    return s


# ─────────────────── mutool 封装（stext / 结构检查） ───────────────────

LINE_RE = re.compile(rb'<line\b([^>]*)>(.*?)</line>', re.S)
BBOX_RE = re.compile(rb'bbox="([^"]+)"')
SIZE_RE = re.compile(rb'<font\b[^>]*?size="([0-9.]+)"')
CHAR_RE = re.compile(rb'<char\b[^>]*?c="([^"]*)"')


def parse_pages(xml_bytes: bytes):
    """容错解析 `mutool -F stext`（PDF 里常有非法 UTF-8 字体名，不能用严格 XML 解析）"""
    pages = []
    for chunk in re.split(rb'<page\b', xml_bytes)[1:]:
        lines = []
        for m in LINE_RE.finditer(chunk):
            bbox = BBOX_RE.search(m.group(1))
            if not bbox:
                continue
            try:
                x0, y0, x1, y1 = (float(v) for v in bbox.group(1).split()[:4])
            except ValueError:
                continue
            sizes = []
            for v in SIZE_RE.findall(m.group(2)):
                try:
                    sizes.append(float(v))
                except ValueError:
                    pass
            text = html.unescape(b''.join(CHAR_RE.findall(m.group(2))).decode('utf-8', 'replace'))
            lines.append({'text': text, 'size': max(sizes) if sizes else 0.0,
                          'x': x0, 'y': y0, 'x2': x1, 'y2': y1})
        lines.sort(key=lambda l: (round(l['y'], 1), l['x']))
        pages.append(lines)
    return pages


class Ctx:
    """运行期上下文：路径模板、mutool 路径、超时"""

    def __init__(self, mutool: str, pdf_tpl: str, md_tpl: str, zip_tpl: str, timeout: int):
        self.mutool = mutool
        self.pdf_tpl = pdf_tpl
        self.md_tpl = md_tpl
        self.zip_tpl = zip_tpl
        self.timeout = timeout

    def pdf(self, doc_id: int) -> str:
        return self.pdf_tpl.replace('{id}', str(doc_id))

    def md(self, doc_id: int) -> str:
        return self.md_tpl.replace('{id}', str(doc_id))

    def zip(self, doc_id: int) -> str:
        return self.zip_tpl.replace('{id}', str(doc_id))


def run(cmd, timeout: int):
    try:
        r = subprocess.run(cmd, capture_output=True, timeout=timeout)
        return r.returncode, r.stdout, r.stderr.decode('utf-8', 'replace')
    except subprocess.TimeoutExpired:
        return -1, b'', 'timeout'
    except OSError as e:  # noqa: PERF203
        return -2, b'', str(e)


def stext(ctx: Ctx, path: str, pages: str = '1-2'):
    rc, out, err = run([ctx.mutool, 'draw', '-q', '-F', 'stext', '-o', '-', path, pages], ctx.timeout)
    return out, err


# ─────────────────────────── PDF 元数据抽取 ───────────────────────────

COVER_FIELDS = re.compile(
    r'^(题目|论文题目|标题|作者|作者姓名|指导教师|导师|学号|专业|学院|学校|密级|分类号|单位代码|'
    r'中图分类号|UDC|授予单位|学位|研究方向|提交日期|答辩日期|培养单位)\s*[:：]', re.I)
COVER_JUNK = re.compile(
    r'^(学位论文|(博士|硕士)(专业)?学位论文|摘要|abstract|关键词|keywords?|引言|前言|目录|'
    r'参考文献|references|致谢|附录|作者简介|收稿日期|funding|基金项目)\b', re.I)
SECTION_NO = re.compile(r'^(?:[IVXLC]+\.|[A-Z]\.|\d+(\.\d+)*\.?)\s+\S')
NAME_BAD = re.compile(
    r'(university|college|institute|department|laborator|academy|school of|center for|'
    r'大学|学院|研究所|研究院|实验室|公司|@|http|www\.|member|fellow|学生会员|'
    r'\d{4,}|邮编|e-mail|email)', re.I)
TITLE_BAD = re.compile(
    r'(issn|isbn|vol\.\s*\d|\bno\.\s*\d|pp\.\s*\d|©|copyright|all rights reserved|'
    r'authorized licensed|download(ed)? (from|by)|full terms|terms & conditions|'
    r'^(?:type|published|received|accepted|revised)\b|^correspondence$|'
    r'^(?:research|review|original)\s+(?:article|paper)s?$|^contents?$|'
    r'\b(editorial|book review|erratum|corrigendum)\b)', re.I)
NAME_ROW = re.compile(r'^[A-Z][\w.\-\']+(?:\s+[A-Z][\w.\-\']+)*\s*(?:,|\band\b)\s*[A-Z]')
JOURNAL_HEADER_HINT = re.compile(
    r'(journal homepage|contents lists available|sciencedirect|springer\s*link|'
    r'wiley online library|taylor & francis|ieeexplore|www\.|https?://|'
    r'vol\.\s*\d|第\s*\d+\s*卷|ISSN)', re.I)
JOURNAL_NAME = re.compile(
    r'\b(journal|transactions|letters|proceedings|reviews?|acta|annals|bulletin|archives|'
    r'advances|frontiers|magazine)\b|学报|杂志|期刊|通报', re.I)
CITE_LABEL = re.compile(r'(?:引用格式|引用本文|引用信息|引用规范|Cite this article as|To cite this article)'
                        r'[:：]?\s*(.{0,400})', re.I)
CITE_PAT = re.compile(
    r'^(?P<authors>.+?)[.．]\s*(?P<title>.+?)\s*\[(?P<kind>[JjCcMmDdRrSs])(?:/OL)?\]\s*[.．]?\s*'
    r'(?P<journal>[^,，.．]*?)\s*[,，]\s*(?P<year>(?:19|20)\d\d)\s*'
    r'(?:[,，]\s*(?P<volume>\d+)\s*(?:[(（](?P<issue>[^)）]*)[)）])?\s*'
    r'[:：]\s*(?P<pages>[0-9\-–—~～]+))?')


def split_authors(raw: str) -> list[str]:
    s = clean(re.sub(r'<[^>]{1,80}>', ' ', raw or ''))
    s = re.sub(r'\b(?:Member|Fellow|Senior\s+Member|Student\s+Member|Life\s+Fellow),?\s*IEEE\b', '', s, flags=re.I)
    s = re.sub(r'\d+(?:\s*[,，]\s*\d+)*', ' ', s)
    s = re.sub(r'[*∗†‡§¶]+', ' ', s)
    if not re.search(r'[A-Za-z]', s):
        parts = re.split(r'[;；,，、]|\s+', s)
    else:
        parts = re.split(r'[;；,，、]| and (?=[A-Z\u4e00-\u9fff])|\s{2,}', s)
    out = []
    for p in parts:
        p = clean(p).strip(' .')
        p = re.sub(r'\s+[a-z](?:\s*,\s*[a-z])*\s*$', '', p).strip(' .,')
        if not p or len(p) > 60 or (len(p) <= 2 and re.fullmatch(r'[a-z]', p)):
            continue
        if NAME_BAD.search(p) or not re.search(r'[\u4e00-\u9fffA-Za-z]', p):
            continue
        if len(p.split()) > 6:
            continue
        out.append(p)
    seen, res = set(), []
    for a in out:
        k = a.lower()
        if k not in seen:
            seen.add(k)
            res.append(a)
    return res


def name_ok(n: str) -> bool:
    n = clean(n)
    if not n or len(n) > 40 or re.search(r'\d', n):
        return False
    if CJK_RE.search(n):
        return 2 <= len(CJK_RE.findall(n)) <= 5
    parts = n.split()
    return 1 <= len(parts) <= 4 and len(n) >= 3


def parse_citation(block: str) -> dict:
    s = despace(clean(block))
    s = re.sub(r'^(?:\[[JjCcMmDdRr]\]|\[EB/OL\])\s*', '', s)
    m = CITE_PAT.match(s)
    if not m:
        m2 = re.match(r'^(?P<authors>.+?)[.．]\s*(?P<title>.+?)[.．]\s*(?P<journal>[^,，.．]*?)\s*[,，]\s*'
                      r'(?P<year>(?:19|20)\d\d)', s)
        if not m2:
            return {}
        d = m2.groupdict()
        return {'authors_raw': d['authors'], 'title': d['title'], 'journal': d['journal'],
                'year': d['year']}
    d = m.groupdict()
    return {'authors_raw': d['authors'], 'title': d['title'], 'journal': d.get('journal'),
            'year': d.get('year')}


def journal_header_rows(lines) -> set[int]:
    bad: set[int] = set()
    for idx, ln in enumerate(lines[:40]):
        t = despace(ln['text'])
        if not t:
            continue
        if JOURNAL_HEADER_HINT.search(t):
            bad.add(idx)
            for jdx in range(max(0, idx - 2), min(len(lines), idx + 3)):
                if jdx == idx:
                    continue
                other = despace(lines[jdx]['text'])
                if other and abs(lines[jdx]['y'] - ln['y']) <= 45 and (
                        JOURNAL_NAME.search(other) or re.match(r'^[A-Za-z .&]{4,60}\d?$', other)):
                    bad.add(jdx)
        elif idx < 12 and ln['y'] < lines[0]['y'] + 140 and JOURNAL_NAME.search(t) \
                and re.match(r'^[A-Za-z .&\-]{4,70}\d?$', t):
            bad.add(idx)
    return bad


def _rows_near(lines, y, tol=3.0):
    return sorted([l for l in lines if abs(l['y'] - y) <= tol], key=lambda l: l['x'])


def authors_below(lines, title_size: float, start: float):
    if not title_size or not start:
        return None
    rows, used = [], set()
    for ln in lines:
        if ln['y'] <= start + 1 or ln['y'] > start + 90:
            continue
        if ln['size'] >= title_size * 0.88 or ln['size'] < title_size * 0.5:
            continue
        key = round(ln['y'], 0)
        if key in used:
            continue
        group = _rows_near(lines, ln['y'])
        used.add(key)
        merged = ' '.join(despace(g['text']) for g in group
                          if not re.fullmatch(r'[\d\s,，.−–\-]+', despace(g['text'])))
        merged = clean(merged)
        if not merged or is_garbage(merged):
            continue
        if re.match(r'^(abstract|摘要|keywords?|关键词|index terms)\b', merged, re.I):
            break
        if len(merged) < 4 or re.match(r'^[\d\s,.\-–]+$', merged):
            continue
        names = split_authors(re.sub(r'^(?:and|&)\s+', '', merged, flags=re.I))
        names = [n for n in names if not NAME_BAD.search(n)]
        if names:
            if rows and abs(ln['size'] - rows[0][2]) > rows[0][2] * 0.05:
                break
            rows.append((names, ln['y'], ln['size']))
        if len(rows) >= 3:
            break
    if not rows:
        return None
    out, last_y = [], None
    for names, y, _size in rows:
        if last_y is not None and y - last_y > 22:
            break
        out.extend(names)
        last_y = y
    seen, res = set(), []
    for n in out:
        k = n.lower().replace(' ', '')
        if k not in seen:
            seen.add(k)
            res.append(n)
    return res or None


def title_ok_layout(title: str, size: float, first_lines) -> bool:
    t = clean(title)
    if not t or len(t) < (10 if CJK_RE.search(t) else 15) or len(t) > 300:
        return False
    if SECTION_NO.match(t) or TITLE_BAD.search(t) or NAME_ROW.match(t):
        return False
    if t.isupper() and len(t) < 25:
        return False
    body = sorted(l['size'] for l in first_lines
                  if len(despace(l['text'])) >= 20 and not is_garbage(despace(l['text'])))
    if body:
        med = body[len(body) // 2]
        if med > 0 and size < med * 1.18:
            return False
    return True


def extract_meta(ctx: Ctx, path: str) -> dict:
    """从 PDF 前两页抽元数据；返回 {field: value, '_src': {...}}"""
    out: dict = {'_src': {}}
    stdout, err = stext(ctx, path)
    if err.strip():
        out['_diag'] = err.strip().splitlines()[0][:200]
    pages = parse_pages(stdout)
    out['_pages'] = len(pages)
    if not pages:
        return out
    first = pages[0]
    all_text = '\n'.join('\n'.join(l['text'] for l in p) for p in pages)
    cut = all_text.split('参考文献')[0].split('REFERENCES')[0]

    # 1) 显式字段标签（期刊「网络首发」封面）
    labeled: dict[str, str] = {}
    texts1 = [despace(l['text']) for l in first[:80]]
    for idx, t in enumerate(texts1):
        m = re.match(r'^(题目|论文题目|标题)\s*[:：]\s*(.*)$', t)
        if m:
            val = m.group(2).strip()
            if len(val) < 4:
                for nxt in texts1[idx + 1: idx + 3]:
                    if nxt:
                        val = nxt
                        break
            if len(val) >= 4:
                labeled.setdefault('title', val)
        m = re.match(r'^(?:作者|作者姓名)\s*[:：]\s*(.*)$', t)
        if m:
            val = m.group(1).strip()
            if len(val) < 2:
                for nxt in texts1[idx + 1: idx + 3]:
                    if nxt:
                        val = nxt
                        break
            if len(val) >= 2:
                labeled.setdefault('authors_raw', val)
    if labeled.get('title'):
        out['title'] = labeled['title']
        out['_src']['title'] = 'label'
    if labeled.get('authors_raw'):
        got = split_authors(labeled['authors_raw'])
        if got:
            out['authors'] = got
            out['_src']['authors'] = 'label'

    # 2) 引用块（最可靠）
    m = CITE_LABEL.search(cut)
    if m:
        cite = parse_citation(m.group(1))
        if cite.get('title'):
            out['title'] = clean(cite['title'])
            out['_src']['title'] = 'cite'
        if cite.get('authors_raw'):
            got = split_authors(cite['authors_raw'])
            if got:
                out['authors'] = got
                out['_src']['authors'] = 'cite'
        if cite.get('journal'):
            out['journal'] = clean(cite['journal'])
            out['_src']['journal'] = 'cite'
        if cite.get('year'):
            out['year'] = int(cite['year'])
            out['_src']['year'] = 'cite'

    # 3) 首页版面：最大字号行 = 题名
    header_rows = journal_header_rows(first)
    if not out.get('title'):
        cands = []
        for idx, ln in enumerate(first[:80]):
            t = despace(ln['text'])
            if len(t) < 6 or len(t) > 300 or is_garbage(t) or t.isdigit():
                continue
            if COVER_JUNK.match(t) or COVER_FIELDS.match(t) or idx in header_rows:
                continue
            if ln['size'] <= 0:
                continue
            cands.append((t, ln))
        if cands:
            top = max(c[1]['size'] for c in cands)
            big = sorted([c for c in cands if c[1]['size'] >= top * 0.92],
                         key=lambda c: (round(c[1]['y'], 1), c[1]['x']))
            title_lines = [big[0]]
            for t, ln in big[1:]:
                prev = title_lines[-1][1]
                if abs(ln['size'] - prev['size']) <= top * 0.06 and -6 <= ln['y'] - prev['y'] <= 45:
                    title_lines.append((t, ln))
                else:
                    break
            title = ' '.join(t for t, _ in title_lines)
            if title_ok_layout(title, top, first):
                out['title'] = title
                out['_src']['title'] = 'layout'
                out['_title_size'] = top
                y2 = title_lines[-1][1]['y2']
                got = [n for n in (authors_below(first, top, y2) or []) if name_ok(n)]
                if len(got) >= 2 and not (CJK_RE.search(title) and not any(CJK_RE.search(n) for n in got)):
                    out['authors'] = got
                    out['_src']['authors'] = 'layout'

    # 4) DOI（首页优先，其次正文前 20k 字符）
    for ln in first[:80]:
        t = ln['text']
        m = DOI_RE.search(t)
        if m and '@' not in t:
            d = tidy_doi(m.group(0))
            if d and not d.endswith(('.pdf', '.jpg')):
                out['doi'] = d
                out['_src']['doi'] = 'page1'
                break
    if not out.get('doi'):
        m = DOI_RE.search(cut[:20000])
        if m:
            d = tidy_doi(m.group(0))
            if d:
                out['doi'] = d
                out['_src']['doi'] = 'head'

    # 5) 年份（只留高精度来源）
    if not out.get('year'):
        head = cut[:12000]
        for pat in (r'((?:19|20)\d\d)\s*年\s*第\s*\d+\s*期',
                    r'第\s*\d+\s*卷\s*第\s*\d+\s*期\s*((?:19|20)\d\d)'):
            m = re.search(pat, head)
            if m:
                y = int(m.group(1))
                if 1950 <= y <= 2100:
                    out['year'] = y
                    out['_src']['year'] = 'header'
                    break

    # 6) 摘要 / 关键词（首页或第 2 页的标签行，避免双栏拼垃圾）
    texts = [despace(l['text']) for p in pages[:2] for l in p]
    for idx, t in enumerate(texts):
        m = re.match(r'^(?:摘\s*要|abstract)\s*[:：—\-–]?\s*(.*)$', t, re.I)
        if m:
            body = [m.group(1)]
            for nxt in texts[idx + 1: idx + 12]:
                if re.match(r'^(?:关键词|关键字|key\s*words?|index\s*terms)\s*[:：]?', nxt, re.I):
                    break
                if COVER_JUNK.match(nxt) and not re.match(r'^(?:摘要|abstract)', nxt, re.I):
                    break
                body.append(nxt)
            abstract = ' '.join(x for x in body if x).strip(' :：-—')
            if len(abstract) >= 80:
                out['abstract'] = abstract[:4000]
                out['_src']['abstract'] = 'label'
            break
    for t in texts:
        m = re.match(r'^(?:关键词|关键字|key\s*words?|index\s*terms)\s*[:：]?\s*(.+)$', t, re.I)
        if m:
            kw = [clean(x).strip('.:：;；,，—–-') for x in re.split(r'[;；,，、]', m.group(1))]
            kw = [x for x in kw if len(x) >= 2 and len(x) <= 40 and re.search(r'[^\W\d_]', x)]
            if kw:
                out['keywords'] = kw
                out['_src']['keywords'] = 'label'
            break

    if out.get('title'):
        out['language'] = 'zh' if CJK_RE.search(out['title']) else 'en'
    return out


# ─────────────────────────── PDF 健康检查 ───────────────────────────

def _read(path: str, offset: int, size: int) -> bytes:
    with open(path, 'rb') as f:
        if offset:
            f.seek(offset)
        return f.read(size)


def _summarize_diag(lines: list[str]) -> str:
    joined = ' '.join(lines).lower()
    kinds = []
    if 'xref' in joined or 'repair' in joined:
        kinds.append('交叉引用表(xref)损坏')
    if any(k in joined for k in ('premature', 'truncat', 'end of data', 'eof')):
        kinds.append('数据被截断')
    if 'syntax' in joined or 'invalid' in joined:
        kinds.append('对象语法错误')
    if any(k in joined for k in ('cannot find', 'out of range', 'missing object')):
        kinds.append('对象引用无效')
    if not kinds:
        kinds.append('解析器报告异常')
    return '、'.join(kinds) + f'（{lines[0][:120]}）'


def check_health(ctx: Ctx, path: str) -> tuple[str, str]:
    """→ (status, message)，规则对齐 backend-rs/src/services/pdf_health.rs"""
    if not os.path.exists(path):
        return 'missing', 'PDF 文件缺失或无法访问'
    try:
        size = os.path.getsize(path)
    except OSError as e:
        return 'missing', f'PDF 文件缺失或无法访问: {e}'
    if not os.path.isfile(path):
        return 'missing', 'PDF 路径不是普通文件'
    if size == 0:
        return 'error', 'PDF 文件大小为 0（空文件）'
    try:
        if b'%PDF-' not in _read(path, 0, 1024):
            return 'error', '文件头缺少 %PDF- 标记，不是有效的 PDF'
    except OSError as e:
        return 'error', f'读取文件头失败: {e}'
    try:
        tail_ok = b'%%EOF' in _read(path, max(0, size - 2048), 2048)
    except OSError:
        tail_ok = False

    diags: list[str] = []

    def note(err: str) -> None:
        for ln in err.splitlines():
            ln = ln.strip()
            if not ln or ln.startswith('warning: ICC support is not available'):
                continue
            if ln not in diags:
                diags.append(ln)

    # 页数：优先 trailer/Root/Pages/Count，退回 mutool pages
    rc, out, err = run([ctx.mutool, 'show', path, 'trailer/Root/Pages/Count'], ctx.timeout)
    note(err)
    pages = None
    text = out.decode('utf-8', 'replace').strip()
    if rc == 0 and re.fullmatch(r'\d+', text):
        pages = int(text)
    if pages is None:
        rc2, out2, err2 = run([ctx.mutool, 'pages', path], ctx.timeout)
        note(err2)
        if rc2 == 0:
            nums = re.findall(rb'pagenum="(\d+)"', out2)
            if nums:
                pages = max(int(n) for n in nums)
    # 加密
    rc3, out3, err3 = run([ctx.mutool, 'show', path, 'trailer/Encrypt'], ctx.timeout)
    note(err3)
    encrypted = rc3 == 0 and out3.strip() not in (b'', b'null', b'0') and b'/Type' in out3

    if pages is None:
        extra = f'；解析器诊断 {_summarize_diag(diags)}' if diags else ''
        return 'error', f'PDF 结构异常：读不出页数{extra}'
    if encrypted and not pages:
        return 'warning', 'PDF 已加密，无法读取结构（可能需要密码）'
    if pages <= 0:
        return 'error', 'PDF 结构异常：页数为 0'
    if encrypted:
        msg = f'PDF 已加密（{pages} 页），部分解析功能可能受限'
        if diags:
            msg += f'；另：解析器诊断 {_summarize_diag(diags)}'
        return 'warning', msg
    if not tail_ok:
        msg = f'文件尾缺少 %%EOF 标记（可能被截断），共 {pages} 页'
        if diags:
            msg += f'；另：解析器诊断 {_summarize_diag(diags)}'
        return 'warning', msg
    if diags:
        return 'warning', (f'PDF 结构已损坏（{_summarize_diag(diags)}），解析器自动修复后能读出 '
                           f'{pages} 页；严格的阅读器/工具可能直接报错')
    return 'ok', f'正常（{pages} 页，引擎：mutool）'


# ─────────────────────────── 配置 / 数据库 ───────────────────────────

def load_config(system_json: str, args) -> dict:
    s = json.load(open(system_json, encoding='utf-8'))
    system_dir = os.path.dirname(os.path.abspath(system_json))

    def expand(v: str) -> str:
        v = v.replace('{system_dir}', system_dir)
        return os.path.expanduser(os.path.expandvars(v))

    def resolve(v: str, base: str) -> str:
        v = expand(v)
        return v if os.path.isabs(v) else os.path.normpath(os.path.join(base, v))

    cwd = resolve(s.get('cwd') or system_dir, system_dir)
    url = s.get('database_url') or 'sqlite:///data/okb_assist.db'
    if url.startswith('sqlite://'):
        rest = url[len('sqlite://'):]
        rest = rest[1:] if rest.startswith('/') else rest
    elif url.startswith('sqlite:'):
        rest = url[len('sqlite:'):]
    else:
        rest = url
    db = rest.split('?', 1)[0]
    db = db if os.path.isabs(db) else os.path.normpath(os.path.join(cwd, db))

    def tpl(key: str, default: str) -> str:
        v = expand(s.get(key) or default)
        return v if os.path.isabs(v) else os.path.normpath(os.path.join(cwd, v))

    return {
        'db': args.db or db,
        'cwd': cwd,
        'pdf': tpl('pdf_path', 'data/pdfs/{id}/{id}.pdf'),
        'md': tpl('markdown_path', 'data/markdowns/{id}.md'),
        'zip': tpl('markdown_asset_path', 'data/pdfs/{id}/{id}.zip'),
        'mutool': args.mutool or expand(s.get('mutool_path') or 'mutool'),
        'log': tpl('log_path', 'okb-log.jsonl'),
    }


def open_db(path: str) -> sqlite3.Connection:
    if not os.path.exists(path):
        sys.exit(f'数据库不存在: {path}')
    conn = sqlite3.connect(path, timeout=30)
    conn.execute('PRAGMA busy_timeout=30000')
    conn.execute('PRAGMA journal_mode=WAL')
    return conn


COLS = ['id', 'title', 'doi', 'year', 'authors', 'journal', 'abstract', 'i18n',
        'keywords', 'language', 'file_hash', 'pdf_size', 'md_size', 'zip_size',
        'fs_synced_at', 'pdf_health', 'pdf_health_message']


def i18n_merge(raw, lang: str, **fields) -> str:
    """把字段补进 `i18n` JSON（只补空）。`raw` 可以是数据库里的 JSON 字符串或 None。"""
    data: dict = {}
    if raw:
        try:
            data = json.loads(raw) or {}
        except (TypeError, ValueError):
            data = {}
    entry = data.setdefault(lang, {})
    for key, value in fields.items():
        if value and not entry.get(key):
            entry[key] = value
    return json.dumps(data, ensure_ascii=False)


def i18n_has(raw, lang: str, key: str) -> bool:
    if not raw:
        return False
    try:
        data = json.loads(raw) or {}
    except (TypeError, ValueError):
        return False
    return bool((data.get(lang) or {}).get(key))


def load_docs(conn, ids=None) -> list[dict]:
    q = f"SELECT {','.join(COLS)} FROM documents"
    params = ()
    if ids:
        q += f" WHERE id IN ({','.join('?' * len(ids))})"
        params = tuple(ids)
    q += ' ORDER BY id'
    cur = conn.execute(q, params)
    return [dict(zip(COLS, r)) for r in cur]


def now_iso() -> str:
    return time.strftime('%Y-%m-%dT%H:%M:%SZ', time.gmtime())


# ─────────────────────────── 四个步骤 ───────────────────────────

def step_stats(conn, docs, ctx: Ctx, dry: bool) -> list[dict]:
    changed, missing = [], 0
    stmt = ("UPDATE documents SET pdf_size=?, md_size=?, zip_size=?, fs_synced_at=?, updated_at=? "
            "WHERE id=?")
    rows, stamp = [], now_iso()
    for d in docs:
        sizes = []
        for path in (ctx.pdf(d['id']), ctx.md(d['id']), ctx.zip(d['id'])):
            try:
                sizes.append(os.path.getsize(path) if os.path.isfile(path) else None)
            except OSError:
                sizes.append(None)
        pdf_size, md_size, zip_size = sizes
        if pdf_size is None:
            missing += 1
        old = (d['pdf_size'], d['md_size'], d['zip_size'])
        if old != (pdf_size, md_size, zip_size) or not d['fs_synced_at']:
            changed.append({'id': d['id'], 'step': 'stats',
                            'pdf_size': [old[0], pdf_size], 'md_size': [old[1], md_size],
                            'zip_size': [old[2], zip_size]})
            rows.append((pdf_size, md_size, zip_size, stamp, stamp, d['id']))
    if rows and not dry:
        conn.executemany(stmt, rows)
        conn.commit()
    log(f'  stats : 更新 {len(rows)} 条（其中 PDF 缺失 {missing} 条）')
    return changed


def _sha256(path: str, chunk: int = 1 << 20) -> str:
    h = hashlib.sha256()
    with open(path, 'rb') as f:
        while True:
            b = f.read(chunk)
            if not b:
                break
            h.update(b)
    return h.hexdigest()


def step_hash(conn, docs, ctx: Ctx, dry: bool, workers: int) -> list[dict]:
    todo = [d for d in docs if not d['file_hash'] and os.path.isfile(ctx.pdf(d['id']))]
    log(f'  hash  : 待计算 {len(todo)} 条')
    if not todo or dry:
        return []
    done, changed, t0 = 0, [], time.time()

    def work(d):
        try:
            return d['id'], _sha256(ctx.pdf(d['id']))
        except OSError:
            return d['id'], None

    with cf.ThreadPoolExecutor(max_workers=workers) as ex:
        for doc_id, h in ex.map(work, todo):
            done += 1
            if h:
                conn.execute('UPDATE documents SET file_hash=?, updated_at=? WHERE id=?',
                             (h, now_iso(), doc_id))
                changed.append({'id': doc_id, 'step': 'hash', 'file_hash': h})
            if done % 25 == 0 or done == len(todo):
                conn.commit()
                rate = done / max(1e-6, time.time() - t0)
                print(f'\r    进度 {done}/{len(todo)}（{rate:.1f} 个/秒）', end='', flush=True)
    print()
    log(f'  hash  : 写入 {len(changed)} 条')
    return changed


def step_health(conn, docs, ctx: Ctx, dry: bool, workers: int) -> list[dict]:
    todo = [d for d in docs if not d['pdf_health']]
    log(f'  health: 待检查 {len(todo)} 条')
    if not todo or dry:
        return []
    changed, done, t0 = [], 0, time.time()

    def work(d):
        return d['id'], check_health(ctx, ctx.pdf(d['id']))

    with cf.ThreadPoolExecutor(max_workers=workers) as ex:
        for doc_id, (status, msg) in ex.map(work, todo):
            done += 1
            conn.execute('UPDATE documents SET pdf_health=?, pdf_health_message=?, updated_at=? '
                         'WHERE id=?', (status, msg, now_iso(), doc_id))
            changed.append({'id': doc_id, 'step': 'health', 'status': status, 'message': msg})
            if done % 25 == 0 or done == len(todo):
                conn.commit()
                rate = done / max(1e-6, time.time() - t0)
                print(f'\r    进度 {done}/{len(todo)}（{rate:.1f} 个/秒）', end='', flush=True)
    print()
    stat = collections.Counter(c['status'] for c in changed)
    log(f'  health: {dict(stat)}')
    return changed


def _md_contains_name(ctx: Ctx, doc_id: int, names: list[str]) -> bool:
    path = ctx.md(doc_id)
    try:
        with open(path, encoding='utf-8', errors='replace') as f:
            text = f.read(400_000)
    except OSError:
        return False
    low = text.lower()
    for n in names:
        token = n.split()[-1].lower() if ' ' in n else n.lower()
        if token and token in low:
            return True
    return False


META_FIELDS = ('title', 'doi', 'year', 'authors', 'journal', 'abstract', 'keywords')


def step_meta(conn, docs, ctx: Ctx, dry: bool, workers: int, overwrite: bool,
              verbose: bool) -> list[dict]:
    todo = []
    for d in docs:
        need = any(d[f] in (None, '') for f in META_FIELDS)
        if need and os.path.isfile(ctx.pdf(d['id'])):
            todo.append(d)
    log(f'  meta  : 待抽取 {len(todo)} 条（只补空字段）')
    if not todo or dry:
        return []
    changed, done, t0 = [], 0, time.time()
    stat = collections.Counter()
    by_id = {d['id']: d for d in todo}

    def work(d):
        try:
            return d['id'], extract_meta(ctx, ctx.pdf(d['id']))
        except Exception as e:  # noqa: BLE001
            return d['id'], {'_error': str(e)}

    with cf.ThreadPoolExecutor(max_workers=workers) as ex:
        for doc_id, r in ex.map(work, todo):
            done += 1
            d = by_id[doc_id]
            src = r.get('_src', {})
            upd: dict[str, object] = {}
            # DOI
            if r.get('doi') and (overwrite or not d['doi']):
                upd['doi'] = r['doi']
                stat['doi'] += 1
            # 题名
            if r.get('title') and (overwrite or not d['title']):
                if r.get('_src', {}).get('title') in ('cite', 'label', 'layout'):
                    upd['title'] = r['title']
                    stat['title'] += 1
            # 作者：版面抽取需在正文里找到至少一个名字
            au = r.get('authors') or []
            if au and (overwrite or not d['authors']):
                ok = True
                if src.get('authors') == 'layout':
                    ok = _md_contains_name(ctx, doc_id, au)
                if ok:
                    upd['authors'] = '; '.join(str(x).strip() for x in au if str(x).strip())
                    stat['authors'] += 1
                else:
                    stat['authors_skipped'] += 1
            # 刊名 / 年份：只认引用块等高置信度来源
            if r.get('journal') and src.get('journal') == 'cite' and (overwrite or not d['journal']):
                upd['journal'] = r['journal']
                stat['journal'] += 1
            if r.get('year') and src.get('year') in ('cite', 'header') and (overwrite or not d['year']):
                upd['year'] = int(r['year'])
                stat['year'] += 1
            # 摘要 / 关键词：基础列存 PDF 主要语言；非主语言的英文文本进 i18n.en
            lang = (d['language'] or r.get('language') or '').lower()
            if r.get('abstract') and src.get('abstract') == 'label':
                has_cjk = bool(CJK_RE.search(r['abstract']))
                if has_cjk or lang in ('', 'en'):
                    if overwrite or not d['abstract']:
                        upd['abstract'] = r['abstract']
                        stat['abstract'] += 1
                elif overwrite or not i18n_has(d['i18n'], 'en', 'abstract'):
                    upd['i18n'] = i18n_merge(upd.get('i18n') or d['i18n'], 'en',
                                             abstract=r['abstract'])
                    stat['i18n.abstract'] += 1
            if r.get('keywords') and src.get('keywords') == 'label' and 2 <= len(r['keywords']) <= 20:
                has_cjk = bool(CJK_RE.search(''.join(r['keywords'])))
                keywords_text = '; '.join(str(x).strip() for x in r['keywords'] if str(x).strip())
                if has_cjk or lang in ('', 'en'):
                    if overwrite or not d['keywords']:
                        upd['keywords'] = keywords_text
                        stat['keywords'] += 1
                elif overwrite or not i18n_has(d['i18n'], 'en', 'keywords'):
                    upd['i18n'] = i18n_merge(upd.get('i18n') or d['i18n'], 'en',
                                             keywords=keywords_text)
                    stat['i18n.keywords'] += 1
            if r.get('language') and not d['language']:
                upd['language'] = r['language']
            if upd:
                sets = ', '.join(f'{k}=?' for k in upd)
                conn.execute(f'UPDATE documents SET {sets}, updated_at=? WHERE id=?',
                             (*upd.values(), now_iso(), doc_id))
                changed.append({'id': doc_id, 'step': 'meta',
                                **{k: v for k, v in upd.items()}})
                if verbose:
                    log(f'    {doc_id}: ' + ', '.join(f'{k}={str(v)[:40]}' for k, v in upd.items()))
            if done % 25 == 0 or done == len(todo):
                conn.commit()
                rate = done / max(1e-6, time.time() - t0)
                print(f'\r    进度 {done}/{len(todo)}（{rate:.1f} 个/秒）', end='', flush=True)
    print()
    log(f'  meta  : {dict(stat)}')
    return changed


# ─────────────────────────── main ───────────────────────────

def main() -> None:
    ap = argparse.ArgumentParser(description='从 PDF 补齐 OkB-Assist 文献库数据')
    ap.add_argument('--system-json', default=os.path.expanduser('~/.okb/system.json'))
    ap.add_argument('--db', default=None, help='覆盖数据库路径')
    ap.add_argument('--mutool', default=None, help='mutool 可执行文件（默认取 system.json）')
    ap.add_argument('--steps', default='stats,hash,health,meta',
                    help='逗号分隔：stats,hash,health,meta')
    ap.add_argument('--all', action='store_true', help='覆盖已有值（默认只补空）')
    ap.add_argument('--dry-run', action='store_true', help='只统计不改库')
    ap.add_argument('--limit', type=int, default=0, help='只处理前 N 条（调试用）')
    ap.add_argument('--ids', default='', help='只处理这些 id，逗号分隔')
    ap.add_argument('--workers', type=int, default=4)
    ap.add_argument('--timeout', type=int, default=45, help='单次 mutool 超时秒数')
    ap.add_argument('--verbose', action='store_true')
    args = ap.parse_args()

    steps = [s.strip() for s in args.steps.split(',') if s.strip()]
    bad = [s for s in steps if s not in ('stats', 'hash', 'health', 'meta')]
    if bad:
        sys.exit(f'未知步骤: {bad}')

    cfg = load_config(args.system_json, args)
    ctx = Ctx(cfg['mutool'], cfg['pdf'], cfg['md'], cfg['zip'], args.timeout)
    log('配置：')
    for k, v in cfg.items():
        log(f'  {k:8s} {v}')
    if not os.path.exists(cfg['mutool']) and not any(
            os.access(os.path.join(p, cfg['mutool']), os.X_OK)
            for p in os.environ.get('PATH', '').split(':')):
        sys.exit(f'mutool 不可用: {cfg["mutool"]}')

    conn = open_db(cfg['db'])
    ids = [int(x) for x in args.ids.split(',') if x.strip().isdigit()] or None
    docs = load_docs(conn, ids)
    if args.limit:
        docs = docs[: args.limit]
    log(f'\n文档 {len(docs)} 条，步骤 {steps}，{"只统计不改库" if args.dry_run else "写库"}')

    before = collections.Counter()
    for d in docs:
        for f in ('file_hash', 'pdf_size', 'md_size', 'zip_size', 'fs_synced_at', 'pdf_health',
                  'doi', 'title', 'authors', 'journal', 'abstract', 'keywords'):
            if d[f] not in (None, ''):
                before[f] += 1

    report = []
    t0 = time.time()
    timings = {}
    for name, fn in (('stats', lambda: step_stats(conn, docs, ctx, args.dry_run)),
                     ('hash', lambda: step_hash(conn, docs, ctx, args.dry_run, args.workers)),
                     ('health', lambda: step_health(conn, docs, ctx, args.dry_run, args.workers)),
                     ('meta', lambda: step_meta(conn, docs, ctx, args.dry_run, args.workers,
                                                args.all, args.verbose))):
        if name not in steps:
            continue
        t_step = time.time()
        report += fn()
        timings[name] = round(time.time() - t_step, 1)
    if timings:
        log(f'\n各步骤耗时: {timings}')

    docs_after = load_docs(conn, [d['id'] for d in docs])
    after = collections.Counter()
    for d in docs_after:
        for f in ('file_hash', 'pdf_size', 'md_size', 'zip_size', 'fs_synced_at', 'pdf_health',
                  'doi', 'title', 'authors', 'journal', 'abstract', 'keywords'):
            if d[f] not in (None, ''):
                after[f] += 1

    log('\n字段覆盖（前 → 后）：')
    for f in ('pdf_size', 'md_size', 'zip_size', 'fs_synced_at', 'file_hash', 'pdf_health',
              'title', 'doi', 'year', 'authors', 'journal', 'abstract', 'keywords'):
        log(f'  {f:14s} {before.get(f, 0):5d} → {after.get(f, 0):5d}')

    if report and not args.dry_run:
        out = os.path.join(os.path.dirname(cfg['db']), 'pdf-recovery-report.jsonl')
        with open(out, 'w', encoding='utf-8') as f:
            for item in report:
                f.write(json.dumps(item, ensure_ascii=False) + '\n')
        log(f'\n改动明细已写入 {out}（{len(report)} 条）')

    log(f'耗时 {time.time() - t0:.1f}s')
    if args.dry_run:
        log('（--dry-run：没有写库）')


if __name__ == '__main__':
    main()
