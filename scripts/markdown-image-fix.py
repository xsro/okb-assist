#!/usr/bin/env python3
"""
修复 Markdown 中内嵌的 data URL 图片。

读取 system.json（默认 backend-rs/system.json），按其中的 `cwd` / `markdown_path` /
`markdown_asset_path` 模板定位 Markdown 与图片资源包，然后：

  1. 提取 Markdown 里 `data:image/...;base64,...` 形式的图片；
  2. 图片内容若已存在于资源包 zip 中（MinerU 已提取过），直接复用其 zip 内路径；
     否则以内容哈希命名（`images/img_<sha256前16位>.<ext>`）合并写入 zip
     （保留 zip 中已有图片，只新增，不删除）；
  3. 把 Markdown 中的 data URL 替换为该图片在 zip 内的相对路径。

后端 `/assist/api/documents/{id}/image/{filename}` 按 basename 从 zip 取图，
Markdown 阅读时又会被重写成该接口，因此替换成 `page_1_image_9.jpg` /
`images/img_xxx.png` 都能正常显示。

脚本可重复执行（幂等）：已处理过的 Markdown 不含 data URL，会被直接跳过；
非 base64（如 `data:image/svg+xml,%3C...`）的 data URL 保留原样并给出警告。

用法:
    # 只预览（不写任何文件）
    python3 scripts/markdown-image-fix.py --dry-run

    # 修复全部文档
    python3 scripts/markdown-image-fix.py

    # 只处理后端 id 为 3607、3610 的文档
    python3 scripts/markdown-image-fix.py --ids 3607,3610

    # 指定 system.json / 保留原文件备份
    python3 scripts/markdown-image-fix.py --system backend-rs/system.json --backup-dir /tmp/okb-md-backup
"""

from __future__ import annotations

import argparse
import base64
import binascii
import glob
import hashlib
import json
import os
import re
import shutil
import sys
import zipfile
from dataclasses import dataclass, field
from pathlib import Path

# ── data URL 解析 ────────────────────────────────────────────────

# 只匹配 base64 形式的图片 data URL，允许 base64 内容跨行（空白只出现在
# 两个 base64 片段之间，避免把末尾换行一起吃掉）。
DATA_URI_RE = re.compile(
    r"data:image/(?P<mime>[a-z0-9][a-z0-9.+-]*)"
    r"(?P<params>(?:;[^,;]*)*?)"
    r";base64,(?P<payload>[A-Za-z0-9+/=]+(?:\s*[A-Za-z0-9+/=]+)*)",
    re.IGNORECASE,
)

# 任意 data:image 引用（用于发现没法处理的非 base64 data URL）
ANY_DATA_URI_RE = re.compile(r"data:image/", re.IGNORECASE)

MIME_EXT = {
    "jpeg": "jpg",
    "jpg": "jpg",
    "pjpeg": "jpg",
    "png": "png",
    "gif": "gif",
    "bmp": "bmp",
    "webp": "webp",
    "svg+xml": "svg",
    "svg": "svg",
    "tiff": "tiff",
    "tif": "tiff",
    "x-icon": "ico",
    "vnd.microsoft.icon": "ico",
    "avif": "avif",
    "heic": "heic",
    "jxl": "jxl",
}


def mime_to_ext(mime: str, data: bytes) -> str:
    """把 MIME 子类型映射成扩展名，未知时优先按文件头嗅探，最后退回 bin。"""
    mime = mime.lower().strip()
    if mime in MIME_EXT:
        return MIME_EXT[mime]

    if data.startswith(b"\x89PNG\r\n\x1a\n"):
        return "png"
    if data.startswith(b"\xff\xd8\xff"):
        return "jpg"
    if data.startswith(b"GIF87a") or data.startswith(b"GIF89a"):
        return "gif"
    if data[:2] == b"BM":
        return "bmp"
    if data[:4] == b"RIFF" and data[8:12] == b"WEBP":
        return "webp"
    if data[:4] == b"%PDF":
        return "pdf"
    if data.lstrip()[:5].lower() == b"<svg " or data.lstrip()[:4].lower() == b"<svg":
        return "svg"

    # 兜底：把 MIME 子类型清洗成合法扩展名
    clean = re.sub(r"[^a-z0-9]+", "", mime)
    return clean or "bin"


def decode_base64(payload: str) -> bytes:
    """解码 base64；标准字母表失败时再试 URL-safe 变体。"""
    clean = re.sub(r"\s+", "", payload)
    # 部分生成器会漏掉 padding
    padding = (-len(clean)) % 4
    if padding:
        clean += "=" * padding
    try:
        return base64.b64decode(clean, validate=True)
    except (binascii.Error, ValueError):
        return base64.urlsafe_b64decode(clean)


# ── 路径 / 模板 ──────────────────────────────────────────────────


def expand_vars(value: str, system_dir: Path) -> str:
    """展开 `{system_dir}` 与 `{env:VAR}`。"""
    value = value.replace("{system_dir}", str(system_dir))

    def repl_env(m: re.Match[str]) -> str:
        name = m.group(1)
        env_value = os.environ.get(name)
        if env_value is None:
            print(f"!! 环境变量 {name} 未设置，按空字符串处理", file=sys.stderr)
            return ""
        return env_value

    return re.sub(r"\{env:([A-Za-z_][A-Za-z0-9_]*)\}", repl_env, value)


def resolve_path(value: str, base: Path) -> Path:
    """展开变量；相对路径按 base 解析。"""
    p = Path(expand_vars(value, base))
    if not p.is_absolute():
        p = base / p
    return p


def build_id_regex(template: Path) -> re.Pattern[str]:
    """由含 `{id}` 的模板构造提取文档 id 的正则。"""
    prefix, suffix = str(template).split("{id}", 1)
    return re.compile("^" + re.escape(prefix) + r"(.+?)" + re.escape(suffix) + "$")


def iter_markdown_files(md_template: Path) -> list[tuple[str, Path]]:
    """按模板枚举 (doc_id, md_path)，仅返回能反解出 id 的常规文件。"""
    pattern = str(md_template).replace("{id}", "*")
    id_regex = build_id_regex(md_template)
    results: list[tuple[str, Path]] = []
    for raw in sorted(glob.glob(pattern, recursive=True)):
        path = Path(raw)
        if not path.is_file():
            continue
        m = id_regex.match(str(path))
        if not m:
            print(f"!! 无法从文件名解析文档 id，跳过: {path}", file=sys.stderr)
            continue
        results.append((m.group(1), path))
    return results


def substitute_id(template: Path, doc_id: str) -> Path:
    """把模板中的 `{id}` 替换成文档 id（其余变量已提前展开）。"""
    return Path(str(template).replace("{id}", doc_id))


# ── 统计 ────────────────────────────────────────────────────────


@dataclass
class Stats:
    md_scanned: int = 0
    md_changed: int = 0
    images_extracted: int = 0
    images_added: int = 0
    images_reused: int = 0
    zip_created: int = 0
    unreadable_md: int = 0
    bad_images: int = 0
    unsupported_data_uris: int = 0
    errors: list[str] = field(default_factory=list)

    def add_error(self, msg: str) -> None:
        self.errors.append(msg)
        print(f"!! {msg}", file=sys.stderr)


# ── 图片 / zip 操作 ─────────────────────────────────────────────


def read_zip_entries(zip_path: Path, stats: Stats) -> dict[str, tuple[bytes, int]]:
    """读取已有 zip 的全部文件（保留压缩方式），损坏的条目跳过。"""
    entries: dict[str, tuple[bytes, int]] = {}
    if not zip_path.exists():
        return entries
    try:
        with zipfile.ZipFile(zip_path) as zf:
            for info in zf.infolist():
                if info.is_dir():
                    continue
                try:
                    entries[info.filename] = (zf.read(info), info.compress_type)
                except (zipfile.BadZipFile, OSError, RuntimeError) as e:
                    stats.add_error(f"读取 {zip_path} 中的 {info.filename} 失败: {e}")
    except (zipfile.BadZipFile, OSError) as e:
        stats.add_error(f"无法读取图片包 {zip_path}（将重建）: {e}")
        return {}
    return entries


def write_zip_entries(zip_path: Path, entries: dict[str, tuple[bytes, int]]) -> None:
    """原子写入 zip（先写同目录临时文件，再 rename）。"""
    zip_path.parent.mkdir(parents=True, exist_ok=True)
    tmp_path = zip_path.with_name(f".{zip_path.name}.tmp-{os.getpid()}")
    try:
        with zipfile.ZipFile(tmp_path, "w", zipfile.ZIP_DEFLATED) as zf:
            for arcname, (data, compress_type) in entries.items():
                zf.writestr(arcname, data, compress_type=compress_type)
        os.replace(tmp_path, zip_path)
    finally:
        if tmp_path.exists():
            tmp_path.unlink()


def write_bytes_atomic(path: Path, data: bytes) -> None:
    """原子写文件，避免中途失败留下半截内容。"""
    path.parent.mkdir(parents=True, exist_ok=True)
    tmp_path = path.with_name(f".{path.name}.tmp-{os.getpid()}")
    try:
        tmp_path.write_bytes(data)
        os.replace(tmp_path, path)
    finally:
        if tmp_path.exists():
            tmp_path.unlink()


def build_hash_index(entries: dict[str, tuple[bytes, int]]) -> dict[str, str]:
    """内容哈希 → zip 内路径，用于复用已有图片（避免同一张图重复入库）。"""
    index: dict[str, str] = {}
    for arcname, (data, _compress) in entries.items():
        index.setdefault(hashlib.sha256(data).hexdigest(), arcname)
    return index


def rewrite_markdown(
    text: str,
    existing: dict[str, tuple[bytes, int]],
    new_entries: dict[str, tuple[bytes, int]],
    stats: Stats,
) -> str:
    """把文本里的 base64 图片 data URL 替换为资源包内的图片路径。

    若图片内容已存在于 zip 中（MinerU 已提取过），直接复用其原路径；
    否则以内容哈希命名（`images/img_<sha256前16位>.<ext>`）新增。
    """
    hash_index = build_hash_index(existing)

    def repl(m: re.Match[str]) -> str:
        payload = m.group("payload")
        try:
            data = decode_base64(payload)
        except (binascii.Error, ValueError) as e:
            stats.bad_images += 1
            stats.add_error(f"图片 base64 解码失败，保留原样: {e}")
            return m.group(0)
        if not data:
            stats.bad_images += 1
            stats.add_error("图片内容为空，保留原样")
            return m.group(0)

        digest = hashlib.sha256(data).hexdigest()
        stats.images_extracted += 1

        arcname = hash_index.get(digest)
        if arcname is not None:
            stats.images_reused += 1
            return arcname

        ext = mime_to_ext(m.group("mime"), data)
        arcname = f"images/img_{digest[:16]}.{ext}"
        new_entries[arcname] = (data, zipfile.ZIP_DEFLATED)
        hash_index[digest] = arcname
        stats.images_added += 1
        return arcname

    return DATA_URI_RE.sub(repl, text)


def backup_file(src: Path, backup_dir: Path) -> None:
    """把原文件复制到备份目录（同名，保留一份；重复运行覆盖）。"""
    backup_dir.mkdir(parents=True, exist_ok=True)
    shutil.copy2(src, backup_dir / src.name)


# ── 主流程 ──────────────────────────────────────────────────────


def process_document(
    doc_id: str,
    md_path: Path,
    asset_template: Path,
    args: argparse.Namespace,
    stats: Stats,
) -> bool:
    """处理单个文档；返回该 Markdown 是否含有 data URL 图片。"""
    stats.md_scanned += 1
    label = f"[{doc_id}]"

    try:
        raw = md_path.read_bytes()
    except OSError as e:
        stats.unreadable_md += 1
        stats.add_error(f"{label} 读取 Markdown 失败 {md_path}: {e}")
        return False

    try:
        text = raw.decode("utf-8")
    except UnicodeDecodeError as e:
        stats.unreadable_md += 1
        stats.add_error(f"{label} Markdown 不是合法 UTF-8，跳过 {md_path}: {e}")
        return False

    data_uri_count = len(ANY_DATA_URI_RE.findall(text))
    if data_uri_count == 0:
        if args.verbose:
            print(f"{label} 无 data URL 图片，跳过")
        return False

    zip_path = substitute_id(asset_template, doc_id)
    zip_created = not zip_path.exists()
    existing = read_zip_entries(zip_path, stats)

    # 先只收集图片，真正写 zip 要等 Markdown 全部替换成功
    new_entries: dict[str, tuple[bytes, int]] = {}
    before_added = stats.images_added
    before_reused = stats.images_reused
    new_text = rewrite_markdown(text, existing, new_entries, stats)

    if not new_entries and stats.images_reused == before_reused:
        stats.unsupported_data_uris += data_uri_count
        stats.add_error(
            f"{label} 发现 {data_uri_count} 个 data URL，但没有可提取的 base64 图片"
            f"（非 base64 或解码失败），未修改 {md_path}"
        )
        return True

    md_changed = new_text != text

    if args.verbose or args.dry_run:
        print(
            f"{label} {md_path.name}: data URL {data_uri_count} 个 → "
            f"新增图片 {stats.images_added - before_added}，"
            f"复用 {stats.images_reused - before_reused}，zip={zip_path}"
        )

    if args.dry_run:
        if md_changed:
            stats.md_changed += 1
        return True

    if args.backup_dir:
        backup_file(md_path, args.backup_dir)
        if zip_path.exists():
            backup_file(zip_path, args.backup_dir)

    for arcname, entry in new_entries.items():
        existing[arcname] = entry

    if new_entries or zip_created:
        try:
            write_zip_entries(zip_path, existing)
        except OSError as e:
            stats.add_error(f"{label} 写入图片包失败 {zip_path}: {e}")
            return True
        if zip_created:
            stats.zip_created += 1

    if md_changed:
        try:
            write_bytes_atomic(md_path, new_text.encode("utf-8"))
        except OSError as e:
            stats.add_error(f"{label} 写入 Markdown 失败 {md_path}: {e}")
            return True
        stats.md_changed += 1

    return True


def load_system_json(system_path: Path) -> dict:
    try:
        with open(system_path, encoding="utf-8") as f:
            data = json.load(f)
    except OSError as e:
        raise SystemExit(f"无法读取 {system_path}: {e}")
    except json.JSONDecodeError as e:
        raise SystemExit(f"{system_path} 不是合法 JSON: {e}")
    if not isinstance(data, dict):
        raise SystemExit(f"{system_path} 顶层必须是对象")
    return data


def main() -> int:
    parser = argparse.ArgumentParser(
        description="把 Markdown 中的 base64 data URL 图片提取到资源包 zip，并改写图片路径"
    )
    parser.add_argument(
        "system_json",
        nargs="?",
        default="backend-rs/system.json",
        help="system.json 路径（默认: %(default)s）",
    )
    parser.add_argument(
        "--system",
        dest="system_json_opt",
        help="同 system_json（方便把 --system 放在前面）",
    )
    parser.add_argument(
        "--ids",
        default="",
        help="只处理指定文档 id，逗号分隔（默认: 全部）",
    )
    parser.add_argument(
        "--limit", "-n", type=int, default=0,
        help="最多处理多少个含 data URL 的文档（0 = 不限制，试跑时很有用）",
    )
    parser.add_argument(
        "--dry-run", "-N", action="store_true",
        help="只报告将要做的修改，不写任何文件",
    )
    parser.add_argument(
        "--backup-dir",
        help="修改前把原 Markdown / zip 备份到该目录（同名覆盖）",
    )
    parser.add_argument(
        "--verbose", "-v", action="store_true",
        help="打印每个文档的处理细节",
    )
    args = parser.parse_args()
    if args.backup_dir:
        args.backup_dir = Path(args.backup_dir).expanduser().resolve()

    system_path = Path(args.system_json_opt or args.system_json).expanduser()
    if not system_path.is_file():
        print(f"system.json 不存在: {system_path}", file=sys.stderr)
        return 2
    system_path = system_path.resolve()
    system_dir = system_path.parent

    system = load_system_json(system_path)

    # cwd：支持 {system_dir}；相对路径按 system.json 所在目录解析
    cwd_raw = str(system.get("cwd") or "{system_dir}")
    cwd = resolve_path(cwd_raw, system_dir)
    if not cwd.is_dir():
        print(f"cwd 目录不存在: {cwd}", file=sys.stderr)
        return 2

    md_template_raw = system.get("markdown_path")
    asset_template_raw = system.get("markdown_asset_path")
    if not md_template_raw or not asset_template_raw:
        print("system.json 缺少 markdown_path / markdown_asset_path", file=sys.stderr)
        return 2

    md_template = resolve_path(str(md_template_raw), cwd)
    asset_template = resolve_path(str(asset_template_raw), cwd)

    if "{id}" not in str(md_template):
        print(f"markdown_path 模板缺少 {{id}}: {md_template}", file=sys.stderr)
        return 2
    if "{id}" not in str(asset_template):
        print(f"markdown_asset_path 模板缺少 {{id}}: {asset_template}", file=sys.stderr)
        return 2

    wanted_ids = {s.strip() for s in args.ids.split(",") if s.strip()}
    documents = [
        (doc_id, md_path)
        for doc_id, md_path in iter_markdown_files(md_template)
        if not wanted_ids or doc_id in wanted_ids
    ]

    if not documents:
        print(f"未找到匹配的 Markdown 文件（模板: {md_template}）", file=sys.stderr)
        return 1

    print(f"system.json : {system_path}")
    print(f"cwd         : {cwd}")
    print(f"Markdown    : {md_template}")
    print(f"图片包      : {asset_template}")
    print(f"待扫描文件  : {len(documents)}")
    if args.dry_run:
        print("模式        : dry-run（不写文件）")

    stats = Stats()
    processed_with_data = 0

    for doc_id, md_path in documents:
        # limit 只统计真正含 data URL 的文档，避免被大量普通文件占满
        if process_document(doc_id, md_path, asset_template, args, stats):
            processed_with_data += 1
            if args.limit and processed_with_data >= args.limit:
                if args.verbose:
                    print(f"已达到 --limit {args.limit}，停止")
                break

    print("\n完成！")
    print(f"  扫描 Markdown : {stats.md_scanned}")
    print(f"  含 data URL   : {processed_with_data}")
    print(f"  修改 Markdown : {stats.md_changed}")
    print(f"  提取图片      : {stats.images_extracted}")
    print(f"    - 新增      : {stats.images_added}")
    print(f"    - 已存在    : {stats.images_reused}")
    print(f"  新建图片包    : {stats.zip_created}")
    print(f"  解码失败图片  : {stats.bad_images}")
    print(f"  无法读取/解码 : {stats.unreadable_md}")
    if stats.unsupported_data_uris:
        print(f"  不支持的 data URL: {stats.unsupported_data_uris}")
    if stats.errors:
        print(f"  错误/警告     : {len(stats.errors)}（见上）")

    return 0


if __name__ == "__main__":
    sys.exit(main())
