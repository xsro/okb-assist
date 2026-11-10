#!/usr/bin/env python3
"""把「盘上有 PDF、库里没有记录」的文献补进数据库（孤儿 PDF 恢复）。

用途：数据库被删/损坏后重建时，有些文献只有 PDF 没有 Markdown（当初上传后没跑解析），
这些行会彻底丢失。本脚本扫描 `pdfs/` 下所有 `{id}/{id}.pdf`，找出库里缺失的 id，
用 PDF 抽元数据、算 SHA256、做健康检查，然后**按原 id 新建记录**。

另外可选（--fix-images-zip）把目录里的 `images.zip` 改名为 `{id}.zip`：
`system.json` 的 `markdown_asset_path` 模板要求 `{id}.zip`，叫 `images.zip` 时
图片接口找不到资源包（Markdown 里的图片加载不出来）。改名记录写入报告，可回滚。

用法：
  uv run --no-project python scripts/recover-orphan-pdfs.py --dry-run
  uv run --no-project python scripts/recover-orphan-pdfs.py
  uv run --no-project python scripts/recover-orphan-pdfs.py --fix-images-zip

安全：只新增记录 / 只改名（不改内容、不删文件）；重复执行安全（已存在的 id 跳过）。
建议后端停止时运行。
"""
from __future__ import annotations

import argparse
import hashlib
import importlib.util
import json
import os
import re
import sqlite3
import sys
import time

HERE = os.path.dirname(os.path.abspath(__file__))


def load_common():
    """复用 recover-from-pdfs.py 里的配置解析 / mutool 封装 / 抽取 / 健康检查"""
    path = os.path.join(HERE, 'recover-from-pdfs.py')
    spec = importlib.util.spec_from_file_location('okb_pdf_common', path)
    mod = importlib.util.module_from_spec(spec)
    assert spec.loader
    spec.loader.exec_module(mod)
    return mod


def sha256_file(path: str, chunk: int = 1 << 20) -> str:
    h = hashlib.sha256()
    with open(path, 'rb') as f:
        while True:
            b = f.read(chunk)
            if not b:
                break
            h.update(b)
    return h.hexdigest()


BAD_FILENAME = re.compile(r'[\\/:*?"<>|\x00-\x1f]')


def safe_filename(title: str | None, doc_id: int) -> str:
    if not title:
        return f'{doc_id}.pdf'
    name = BAD_FILENAME.sub(' ', title)
    name = re.sub(r'\s+', ' ', name).strip(' .')
    if len(name) > 120:
        name = name[:120].rstrip()
    return f'{name or doc_id}.pdf'


def main() -> None:
    ap = argparse.ArgumentParser(description='把孤儿 PDF 补成数据库记录 / 修正 images.zip 命名')
    ap.add_argument('--system-json', default=os.path.expanduser('~/.okb/system.json'))
    ap.add_argument('--db', default=None)
    ap.add_argument('--mutool', default=None)
    ap.add_argument('--timeout', type=int, default=180)
    ap.add_argument('--dry-run', action='store_true')
    ap.add_argument('--fix-images-zip', action='store_true',
                    help='把目录里的 images.zip 改名成 {id}.zip')
    ap.add_argument('--limit', type=int, default=0)
    args = ap.parse_args()

    common = load_common()
    cfg = common.load_config(args.system_json, args)
    ctx = common.Ctx(cfg['mutool'], cfg['pdf'], cfg['md'], cfg['zip'], args.timeout)
    pdf_root = os.path.dirname(os.path.dirname(cfg['pdf']))
    print(f'数据库 : {cfg["db"]}\nPDF 根 : {pdf_root}\nmutool : {cfg["mutool"]}')
    print('模式   : ' + ('只统计不改动' if args.dry_run else '写库'))

    conn = common.open_db(cfg['db'])
    docs = {r[0] for r in conn.execute('select id from documents')}

    # ── 1. 找出孤儿 PDF ──
    orphans = []
    for name in sorted(os.listdir(pdf_root)):
        if not name.isdigit():
            continue
        doc_id = int(name)
        if doc_id in docs:
            continue
        path = os.path.join(pdf_root, name, f'{name}.pdf')
        if os.path.isfile(path):
            orphans.append((doc_id, path))
    if args.limit:
        orphans = orphans[: args.limit]
    print(f'\n孤儿 PDF: {len(orphans)} 个')
    if orphans:
        print('  id:', ', '.join(str(i) for i, _ in orphans))
    if not orphans:
        print('  没有需要补的记录')

    report = []
    for doc_id, path in orphans:
        meta = common.extract_meta(ctx, path)
        try:
            pdf_size = os.path.getsize(path)
            file_hash = None if args.dry_run else sha256_file(path)
        except OSError as e:
            print(f'  {doc_id}: 读取失败 {e}')
            continue
        if args.dry_run:
            status, msg = 'ok', '(dry-run 未检查)'
        else:
            status, msg = common.check_health(ctx, path)
        title = meta.get('title')
        authors = meta.get('authors') or []
        row = {
            'id': doc_id,
            'filename': safe_filename(title, doc_id),
            'file_hash': file_hash,
            'title': title,
            'authors': json.dumps(authors, ensure_ascii=False) if authors else None,
            'year': meta.get('year'),
            'doi': meta.get('doi'),
            'source': 'pdf',
            'journal': meta.get('journal'),
            'keywords': (json.dumps(meta['keywords'], ensure_ascii=False)
                         if meta.get('keywords') and 2 <= len(meta['keywords']) <= 20 else None),
            'abstract': meta.get('abstract') if (meta.get('abstract') and
                                                 common.CJK_RE.search(meta['abstract'])) else None,
            'abstract_en': meta.get('abstract') if (meta.get('abstract') and
                                                    not common.CJK_RE.search(meta['abstract'])) else None,
            'category': None,
            'doc_type': None,
            'language': meta.get('language'),
            'mineru_task_id': None,
            'fs_synced_at': time.strftime('%Y-%m-%dT%H:%M:%SZ', time.gmtime()),
            'qdrant_collection': None,
            'vector_db_id': None,
            'pdf_size': pdf_size,
            'md_size': None,
            'zip_size': os.path.getsize(ctx.zip(doc_id)) if os.path.isfile(ctx.zip(doc_id)) else None,
            'pdf_health': status,
            'pdf_health_message': msg,
            'created_at': time.strftime('%Y-%m-%d %H:%M:%S',
                                        time.localtime(os.path.getmtime(path))),
            'updated_at': time.strftime('%Y-%m-%d %H:%M:%S',
                                        time.localtime(os.path.getmtime(path))),
        }
        # 英文文献的 *_en 镜像（与库内既有习惯一致）
        if row['language'] == 'en':
            row['title_en'] = row['title']
            row['authors_en'] = row['authors']
            row['journal_en'] = row['journal']
            row['keywords_en'] = row['keywords']
        print(f"  {doc_id}: {str(title)[:60]} | {row['year']} | {row['doi']} | "
              f"{len(authors)} 作者 | {status}")
        if args.dry_run:
            report.append({'id': doc_id, 'step': 'add-orphan', 'dry_run': True, **{
                k: v for k, v in row.items() if k in ('filename', 'title', 'doi', 'year')}})
            continue
        cols = list(row)
        conn.execute(f"INSERT INTO documents ({','.join(cols)}) VALUES ({','.join('?' * len(cols))})",
                     [row[c] for c in cols])
        conn.commit()
        report.append({'id': doc_id, 'step': 'add-orphan', **{k: v for k, v in row.items()
                                                              if k in ('filename', 'title', 'doi', 'year', 'pdf_size', 'pdf_health')}})

    # ── 2. images.zip → {id}.zip ──
    if args.fix_images_zip:
        print('\n修正图片包命名（images.zip → {id}.zip）')
        renamed = 0
        for name in sorted(os.listdir(pdf_root)):
            if not name.isdigit():
                continue
            doc_id = int(name)
            src = os.path.join(pdf_root, name, 'images.zip')
            dst = os.path.join(pdf_root, name, f'{name}.zip')
            if not os.path.isfile(src) or os.path.exists(dst):
                continue
            if args.dry_run:
                print(f'  {doc_id}: 将改名 images.zip → {name}.zip')
                renamed += 1
                continue
            os.rename(src, dst)
            size = os.path.getsize(dst)
            conn.execute('UPDATE documents SET zip_size=?, updated_at=? WHERE id=?',
                         (size, time.strftime('%Y-%m-%dT%H:%M:%SZ', time.gmtime()), doc_id))
            renamed += 1
            report.append({'id': doc_id, 'step': 'rename-zip', 'from': 'images.zip',
                           'to': f'{name}.zip', 'size': size})
            if renamed % 20 == 0:
                conn.commit()
                print(f'  已改名 {renamed} 个')
        if not args.dry_run:
            conn.commit()
        print(f'  处理 {renamed} 个')

    # ── 3. 报告 ──
    if report and not args.dry_run:
        out = os.path.join(os.path.dirname(cfg['db']), 'pdf-recovery-report.jsonl')
        with open(out, 'a', encoding='utf-8') as f:
            for item in report:
                f.write(json.dumps(item, ensure_ascii=False) + '\n')
        print(f'\n报告已追加到 {out}（{len(report)} 条）')
    if not args.dry_run:
        total = conn.execute('select count(*) from documents').fetchone()[0]
        print(f'当前文档总数: {total}')


if __name__ == '__main__':
    main()
