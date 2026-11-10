#!/usr/bin/env python3
"""
okb-version-manager.py — 管理 OKB-Assist 多版本

下载、解压、管理各版本，支持 GitHub Actions Artifact 和 GitHub Release。

用法:
  {script} install latest-artifact|la [选项]   从 Actions Artifact 安装最新版
  {script} install latest-release|lr [选项]     从 GitHub Release 安装最新版
  {script} install --frontend latest-release|lr [选项]  从 GitHub Release 安装前端资源
  {script} bin [版本]                            列出/查询已安装版本
  {script} bin --latest                         仅输出最新版本二进制的绝对路径
  {script} self-update|su                       从 GitHub 更新自身脚本

选项:
  -p, --proxy PROXY          设置代理 (如 http://127.0.0.1:7899)
  --timeout SECONDS          下载超时秒数 (默认: 不限制)
  --repo REPO                设置 GitHub 仓库 (默认: xsro/okb-assist)
  --artifact-name NAME       强制指定 artifact/asset 名称 (默认: 自动探测)
  --frontend                 下载前端资源 (仅 latest-release/lr 有效)
  -y, --yes                  跳过确认
  -h, --help                 显示帮助

环境变量:
  HTTPS_PROXY    同 --proxy
  YES            设为 1 跳过确认

Token 来源 (la 需要，401 时自动尝试下一个)：
  1. 环境变量 GH_TOKEN
  2. 环境变量 GITHUB_TOKEN
  3. ~/.okb/github_token 文件
  4. gh auth token (gh CLI >= 2.11)
  5. ~/.config/gh/hosts.yml 中的 oauth_token (老版本 gh 兜底)

目录结构:
  ~/.okb/cache/               下载的压缩包缓存
  ~/.okb/versions/{git_hash}/  按提交 SHA(artifact) 解压的目录
  ~/.okb/releases/{tag_name}/  按 Release 标签名解压的目录 (后端/前端)
  ~/.okb/versions/latest/okb-assist  最新安装版本二进制的软链接
                             (install la / install lr 后自动更新)

产物命名 (与 .github/workflows/build-backends.yml 一致):
  okb-assist-linux-x64         Linux x86_64 (gnu，内嵌 mupdf)
  okb-assist-linux-arm64-musl  Linux ARM64 (静态 musl，PDF 引擎用外部 mutool)
  okb-assist-macos-arm64       macOS ARM64 (内嵌 mupdf)
  okb-assist-win-x64.exe       Windows x64 (内嵌 mupdf)
  名称会按平台自动探测，CI 改名时会在最近产物里回退匹配。
"""

# Python 3.9 的运行环境（如 Debian 11 / 树莓派）不支持在运行时求值 `dict | None`
# 这类注解，用 PEP 563 把注解变成字符串，脚本可在 3.9+ 运行。
from __future__ import annotations

import argparse
import json
import os
import platform
import shutil
import stat
import subprocess
import sys
import tempfile
import time
from datetime import datetime, timezone, timedelta
from pathlib import Path

# ── 常量 ──
REPO = "xsro/okb-assist"
OKB_DIR = Path.home() / ".okb"
CACHE_DIR = OKB_DIR / "cache"
VERSIONS_DIR = OKB_DIR / "versions"
RELEASES_DIR = OKB_DIR / "releases"
RETRY_COUNT = 3
RETRY_DELAY = 10  # 秒

# Cargo.toml 中定义的二进制名称
BINARY_NAME = "okb_assist"

# 最新版本的稳定软链接目录（versions/latest/，安装后自动指向最新二进制）
LATEST_DIR = VERSIONS_DIR / "latest"
LATEST_LINK_NAME = "okb-assist"


# ── 平台探测 ──

# 产物命名与 .github/workflows/build-backends.yml 保持一致：
#   okb-assist-linux-x64          Linux x86_64，gnu 动态链接，含内嵌 mupdf
#   okb-assist-linux-arm64-musl   Linux ARM64，静态 musl，**不含**内嵌 mupdf
#                                 （PDF 引擎走 system.json 的 mutool_path）
#   okb-assist-macos-arm64        macOS ARM64，含内嵌 mupdf
#   okb-assist-win-x64.exe        Windows x64，含内嵌 mupdf
# Windows 的系统段在工作流里写作 `win`（不是 `windows`）。
SYSTEM_ALIASES = {
    "linux": "linux",
    "darwin": "macos",
    "windows": "win",
}
ARCH_ALIASES = {
    "x86_64": "x64",
    "amd64": "x64",
    "aarch64": "arm64",
    "arm64": "arm64",
    "armv7l": "armv7",
    "armv7": "armv7",
}


def platform_parts() -> tuple[str, str]:
    """返回产物名里的（系统段, 架构段），如 ("linux", "arm64")。"""
    raw_sys = platform.system().lower()
    raw_machine = platform.machine().lower()
    return (SYSTEM_ALIASES.get(raw_sys, raw_sys),
            ARCH_ALIASES.get(raw_machine, raw_machine))


def detect_platform_artifact_names() -> list[str]:
    """当前平台可用的 artifact/asset 名称，按优先级从高到低。

    Linux 上 CI 对两种架构的构建方式不同，所以「优先项」也不同：
      - ARM64 只有静态 musl 构建（`-musl` 后缀），历史 Release 里的 glibc 版本只作回退；
      - x86_64 是 gnu 构建（含内嵌 mupdf），将来若出 musl 版本也只作回退。
    这样 CI 增删 `-musl` 后缀时脚本不必跟着改。
    """
    sys_name, arch_name = platform_parts()
    ext = ".exe" if sys_name == "win" else ""
    base = f"okb-assist-{sys_name}-{arch_name}"
    if sys_name == "linux":
        musl, plain = f"{base}-musl{ext}", f"{base}{ext}"
        return [musl, plain] if arch_name.startswith("arm") else [plain, musl]
    return [f"{base}{ext}"]


def detect_platform_artifact_name() -> str:
    """当前平台首选的 artifact/asset 名称（仅用于展示/兜底）。"""
    return detect_platform_artifact_names()[0]


def artifact_name_candidates() -> list[str]:
    """--artifact-name 优先；否则按平台探测的候选列表。"""
    forced = getattr(args, "artifact_name", None)
    return [forced] if forced else detect_platform_artifact_names()


def binary_name_candidates() -> list[str]:
    """解压目录里可能的二进制文件名（Cargo 名 + 各候选产物名）。"""
    return [BINARY_NAME, *artifact_name_candidates()]


def is_backend_artifact_name(name: str) -> bool:
    """后端产物以 okb-assist- 开头；排除客户端（okb-client-*）与前端 zip。"""
    n = name.lower()
    if not n.startswith("okb-assist-"):
        return False
    return not n.endswith(".zip")


def backend_artifact_score(name: str) -> int:
    """给 Release asset / Actions artifact 名打分，0 = 不是当前平台的后端产物。

    先精确命中平台候选名（越靠前的候选分越高），再退化为「系统段 + 架构段」
    模糊匹配，这样 CI 改了产物命名时仍能挑到合适的二进制。
    """
    n = name.lower()
    if not is_backend_artifact_name(n):
        return 0
    for idx, cand in enumerate(artifact_name_candidates()):
        if n == cand.lower():
            return 1000 - idx * 10
    sys_name, arch_name = platform_parts()
    score = 0
    if sys_name and sys_name in n:
        score += 20
    if arch_name and arch_name in n:
        score += 20
    if not score:
        raw_sys = platform.system().lower()
        if raw_sys and raw_sys in n:  # 兼容 darwin / windows 原始写法
            score += 10
    return score


def pick_backend_artifact(artifacts: list[dict]) -> dict | None:
    """从 asset/artifact 列表里挑出当前平台的后端产物（无匹配返回 None）。"""
    scored = [(backend_artifact_score(a.get("name", "")), a)
              for a in artifacts]
    scored = [item for item in scored if item[0] > 0]
    if not scored:
        return None
    scored.sort(key=lambda item: item[0], reverse=True)
    return scored[0][1]


# ── 色彩输出 ──

class Color:
    RED = "\033[0;31m"
    GREEN = "\033[0;32m"
    YELLOW = "\033[1;33m"
    CYAN = "\033[0;36m"
    NC = "\033[0m"


def info(msg):   print(f"{Color.CYAN}[INFO]{Color.NC}  {msg}")
def ok(msg):     print(f"{Color.GREEN}[OK]{Color.NC}    {msg}")
def warn(msg):   print(f"{Color.YELLOW}[WARN]{Color.NC}  {msg}")
def err(msg):    print(f"{Color.RED}[ERROR]{Color.NC} {msg}")


def fmt_size(bytes_: int) -> str:
    if bytes_ >= 1048576:
        return f"{bytes_ / 1048576:.1f} MiB"
    elif bytes_ >= 1024:
        return f"{bytes_ / 1024:.1f} KiB"
    else:
        return f"{bytes_} B"


# ── 初始化 ──

def ensure_dirs():
    OKB_DIR.mkdir(parents=True, exist_ok=True)
    CACHE_DIR.mkdir(parents=True, exist_ok=True)


# ── 依赖检查 ──

def check_basic_deps():
    missing = []
    for cmd in ("curl", "unzip"):
        if shutil.which(cmd) is None:
            missing.append(cmd)
    if missing:
        err(f"缺少必要依赖: {', '.join(missing)}")
        sys.exit(1)


# ── 认证工具 ──

# 最近一次真正通过认证的令牌（401 自动切换后回填，下载 artifact 时复用）
_TOKEN_STATE: dict[str, str | None] = {"label": None, "token": None}


def _gh_hosts_token() -> str | None:
    """从 gh CLI 的 hosts.yml 读取令牌。

    旧版 gh（< 2.11）没有 `gh auth token` 子命令，但 hosts.yml 里存了 oauth_token；
    这里用极简逐行解析，避免引入 PyYAML 依赖。
    """
    config_home = os.environ.get("XDG_CONFIG_HOME") or str(Path.home() / ".config")
    hosts = Path(config_home) / "gh" / "hosts.yml"
    if not hosts.exists():
        return None
    try:
        text = hosts.read_text(encoding="utf-8", errors="replace")
    except OSError:
        return None
    for line in text.splitlines():
        stripped = line.strip()
        if stripped.startswith("oauth_token:"):
            value = stripped.split(":", 1)[1].strip().strip("'\"")
            if value:
                return value
    return None


def get_github_token_candidates() -> list[tuple[str, str]]:
    """按优先级返回 [(来源说明, 令牌), ...]，相同令牌只保留首个来源。"""
    candidates: list[tuple[str, str]] = []

    def add(label: str, raw: str | None):
        token = (raw or "").strip()
        if token and all(token != existing for _, existing in candidates):
            candidates.append((label, token))

    add("环境变量 GH_TOKEN", os.environ.get("GH_TOKEN"))
    add("环境变量 GITHUB_TOKEN", os.environ.get("GITHUB_TOKEN"))

    token_file = OKB_DIR / "github_token"
    if token_file.exists():
        try:
            add(f"{token_file} 文件", token_file.read_text())
        except OSError:
            pass

    if shutil.which("gh"):
        try:
            result = subprocess.run(
                ["gh", "auth", "token"],
                capture_output=True, text=True, timeout=10,
            )
            if result.returncode == 0:
                add("gh auth token", result.stdout)
        except (FileNotFoundError, subprocess.TimeoutExpired):
            pass
        # 旧版 gh 没有 `auth token` 子命令，直接读配置文件兜底
        add("gh 配置 hosts.yml", _gh_hosts_token())

    return candidates


def get_github_token() -> str | None:
    """返回优先级最高的令牌（其余候选在请求失败时自动尝试）。"""
    candidates = get_github_token_candidates()
    return candidates[0][1] if candidates else None


# ── 代理工具 ──

def proxy_url() -> str | None:
    if hasattr(proxy_url, "_cached"):
        return proxy_url._cached
    val = None
    if getattr(args, 'proxy', None):
        val = args.proxy
    elif os.environ.get("HTTPS_PROXY"):
        val = os.environ["HTTPS_PROXY"]
    proxy_url._cached = val
    return val


def resolve_cache_file(cache_path: Path, download_url: str,
                       artifact_id: int | None = None) -> bool:
    """如果缓存存在则询问用户是否复用；返回 True=复用跳过下载。"""
    if not cache_path.exists():
        return False

    size_str = fmt_size(cache_path.stat().st_size)
    print(f"\n  缓存文件已存在: {cache_path}")
    print(f"  大小:           {size_str}")

    if getattr(args, 'yes', False):
        return True

    resp = input("  使用已有文件解压？[Y/n] (n=重新下载) ").strip().lower() or "y"
    return resp.startswith("y")


def confirm(prompt: str) -> bool:
    if getattr(args, 'yes', False) or os.environ.get("YES") == "1":
        return True
    resp = input(f"{prompt} ").strip().lower() or "y"
    return resp.startswith("y")


# ── 通用 API 请求 ──

_HTTP_STATUS_MARKER = "__OKB_HTTP_STATUS__"


def _token_candidates_for(token: str | None) -> list[tuple[str, str | None]]:
    """构造本次请求可用的令牌序列（调用方指定的排在最前）。"""
    candidates: list[tuple[str, str | None]] = []
    seen: set[str] = set()

    def add(label: str, raw: str | None):
        value = (raw or "").strip()
        if value.lower() in seen:
            return
        seen.add(value.lower())
        candidates.append((label, value or None))

    if token:
        add("当前令牌", token)
    for label, value in get_github_token_candidates():
        add(label, value)
    if not token:
        add("匿名请求", None)
    return candidates


def _curl_get_status(url: str, token: str | None = None,
                     timeout: int = 30) -> tuple[int, str]:
    """GET 请求（先代理后直连），返回 (HTTP 状态码, 响应体)；0 = 网络层失败。"""
    def build_cmd(use_proxy: bool) -> list[str]:
        cmd = ["curl", "-sSL", "--max-time", str(timeout),
               "-w", f"\n{_HTTP_STATUS_MARKER}%{{http_code}}"]
        p = proxy_url() if use_proxy else None
        if p:
            cmd += ["-x", p]
        if token:
            cmd += ["-H", f"Authorization: Bearer {token}"]
        cmd.append(url)
        return cmd

    use_proxies = [True, False] if proxy_url() else [False]
    last_error = ""
    for use_proxy in use_proxies:
        try:
            result = subprocess.run(
                build_cmd(use_proxy), capture_output=True, text=True,
                timeout=timeout + 10)
        except FileNotFoundError:
            raise RuntimeError("curl 未安装，请先安装 curl")
        except subprocess.TimeoutExpired:
            last_error = f"请求超时（{timeout}s）"
            continue

        out = result.stdout
        index = out.rfind(_HTTP_STATUS_MARKER)
        if index < 0:
            last_error = ((result.stderr or "").strip()
                          or f"curl 退出码 {result.returncode}")
            continue
        code = out[index + len(_HTTP_STATUS_MARKER):].strip()
        if code.isdigit() and code != "0":
            return int(code), out[:index]
        last_error = (result.stderr or "").strip() or "无法连接"
    return 0, last_error


def _api_error_detail(body: str) -> str:
    """从 GitHub 的错误响应里取出 message 字段。"""
    text = (body or "").strip()
    try:
        data = json.loads(text)
    except (json.JSONDecodeError, ValueError):
        data = None
    if isinstance(data, dict) and data.get("message"):
        return str(data["message"])
    return text[:200]


def _curl_get_json(url: str, token: str | None = None) -> str:
    """请求 GitHub API，返回 JSON 文本。

    - 401/403（令牌无效、过期或无权限）自动换下一个候选令牌，不再重试同一个坏令牌；
    - 网络层错误在代理与直连之间回退，最多重试 RETRY_COUNT 次；
    - 其它状态码直接抛出可读错误（不再只报“已重试 N 次”）。
    """
    candidates = _token_candidates_for(token)
    rejected: list[str] = []
    last_network_error = ""

    for label, value in candidates:
        for attempt in range(1, RETRY_COUNT + 1):
            if attempt > 1:
                warn(f"重试第 {attempt} 次 ...")
                time.sleep(RETRY_DELAY)

            status, body = _curl_get_status(url, value)
            if status == 0:
                last_network_error = body or "网络不可用"
                warn(f"请求失败（{label}）: {last_network_error}")
                continue
            if status == 200:
                if value:
                    _TOKEN_STATE["label"] = label
                    _TOKEN_STATE["token"] = value
                return body
            if status in (401, 403):
                detail = _api_error_detail(body)
                rejected.append(f"{label}: HTTP {status} {detail}")
                warn(f"令牌不可用（{label}）: HTTP {status} {detail}")
                break
            raise RuntimeError(
                f"GitHub API 返回 HTTP {status}: {_api_error_detail(body)}"
                f" ({url})"
            )

    if rejected:
        raise RuntimeError(
            "GitHub 令牌无效、已过期或无权限，已尝试所有来源：\n    "
            + "\n    ".join(rejected)
            + f"\n  请更新 {OKB_DIR / 'github_token'}，"
            "或执行 `gh auth login` 后重试。"
        )
    raise RuntimeError(
        f"API 请求失败（网络不可用，已重试 {RETRY_COUNT} 次）: "
        f"{last_network_error or '未知错误'} ({url})"
    )


# ── 通用下载 ──

class _DownloadAuthError(RuntimeError):
    """下载返回 401/403：令牌无效或权限不足，可换下一个候选令牌重试。"""


def _do_curl_download(curl_cmd: list[str],
                      timeout: int | None = None) -> int:
    """执行一次下载，返回 HTTP 状态码（0 = 没拿到响应）。"""
    try:
        result = subprocess.run(
            curl_cmd + ["-w", "%{http_code}"],
            stdout=subprocess.PIPE, stderr=subprocess.PIPE,
            text=True, timeout=timeout)
    except FileNotFoundError:
        raise RuntimeError("curl 未安装，请先安装 curl")
    except subprocess.TimeoutExpired:
        raise RuntimeError("下载超时")
    code = result.stdout.strip()
    return int(code) if code.isdigit() else 0


def _download_with_retry(download_url: str, output_path: Path,
                         extra_headers: list[str] | None = None,
                         timeout: int | None = None,
                         token_candidates: list[tuple[str, str | None]]
                         | None = None) -> Path:
    """下载文件；401/403 时自动换下一个候选令牌重试。"""
    candidates = token_candidates or [(None, None)]
    last_error = "未知错误"

    for label, token in candidates:
        try:
            return _download_with_token(download_url, output_path,
                                        extra_headers, timeout,
                                        token, label)
        except _DownloadAuthError as e:
            last_error = str(e)
            warn(last_error)
            continue
    raise RuntimeError(f"下载失败: {last_error}")


def _download_with_token(download_url: str, output_path: Path,
                         extra_headers: list[str] | None,
                         timeout: int | None,
                         token: str | None,
                         label: str) -> Path:
    def build_cmd(use_proxy: bool) -> list[str]:
        cmd = ["curl", "-SfL"]
        p = proxy_url() if use_proxy else None
        if p:
            cmd += ["-x", p]
        if token:
            cmd += ["-H", f"Authorization: Bearer {token}"]
        if extra_headers:
            for h in extra_headers:
                cmd += ["-H", h]
        cmd += ["-o", str(output_path), download_url]
        return cmd

    use_proxies = [True, False] if proxy_url() else [False]
    last_error = "未知错误"
    for use_proxy in use_proxies:
        p = proxy_url() if use_proxy else None
        if p:
            info(f"使用代理: {p}")
        for attempt in range(1, RETRY_COUNT + 1):
            if attempt > 1:
                warn(f"重试第 {attempt} 次 ...")
                time.sleep(RETRY_DELAY)

            try:
                status = _do_curl_download(build_cmd(use_proxy),
                                           timeout=timeout)
            except RuntimeError as e:
                last_error = f"{e}（{'代理' if use_proxy else '直连'}）"
                warn(last_error)
                continue

            size = output_path.stat().st_size if output_path.exists() else 0
            if status in (200, 206) and size > 1000:
                ok("下载完成")
                return output_path
            if status in (401, 403):
                raise _DownloadAuthError(
                    f"下载被拒绝（{label or '匿名'}）: HTTP {status}"
                )
            last_error = f"HTTP {status or '无响应'}，收到 {size} 字节"
            warn(f"下载失败: {last_error}")

        if use_proxy:
            info("回退到直连下载 ...")

    raise RuntimeError(f"下载失败，已重试 {RETRY_COUNT} 次: {last_error}")


# ── 解压 ──

def extract_zip(zip_path: Path, output_dir: Path):
    info(f"解压到 {output_dir}/")
    shutil.unpack_archive(str(zip_path), str(output_dir), format="zip")


def extract_archive(archive_path: Path, output_dir: Path):
    output_dir.mkdir(parents=True, exist_ok=True)
    suffix = "".join(archive_path.suffixes).lower()
    info(f"解压到 {output_dir}/")
    if suffix.endswith(".zip"):
        shutil.unpack_archive(str(archive_path), str(output_dir), format="zip")
    else:
        shutil.unpack_archive(str(archive_path), str(output_dir))
    ok("解压完成")


# ── 查找二进制 ──

def find_binary_in_dir(directory: Path) -> Path | None:
    # 解压出来的文件名就是产物名（如 okb-assist-linux-arm64-musl），
    # 所以必须把整条候选链都试一遍，不能只试首选名。
    candidates = binary_name_candidates()

    for name in candidates:
        target = directory / name
        if target.exists() and target.is_file():
            target.chmod(target.stat().st_mode | stat.S_IEXEC
                         | stat.S_IXGRP | stat.S_IXOTH)
            return target

    for root, dirs, files in os.walk(str(directory)):
        for f in files:
            if f in candidates:
                path = Path(root) / f
                path.chmod(path.stat().st_mode | stat.S_IEXEC
                           | stat.S_IXGRP | stat.S_IXOTH)
                return path

    return None


# ── Artifact 操作 ──

def get_latest_artifact_info(token: str) -> dict:
    repo = getattr(args, 'repo', REPO)
    names = artifact_name_candidates()
    info(f"查找最新 Artifact: {' / '.join(names)} (repo: {repo})")

    data = None
    for name in names:
        url = (f"https://api.github.com/repos/{repo}/actions/artifacts"
               f"?name={name}&per_page=1")
        raw = _curl_get_json(url, token)
        artifacts = json.loads(raw).get("artifacts", [])
        if artifacts:
            data = artifacts[0]
            break

    if data is None:
        # 名称全部未命中（CI 又改了产物命名）时，退回「最近的 artifact 列表里挑最像的」
        info("按名称未命中，改为在最近的 Artifact 列表里匹配 ...")
        url = (f"https://api.github.com/repos/{repo}/actions/artifacts"
               f"?per_page=100")
        raw = _curl_get_json(url, token)
        artifacts = [a for a in json.loads(raw).get("artifacts", [])
                     if not a.get("expired")]
        data = pick_backend_artifact(artifacts)
        if data is None:
            err("未找到匹配当前平台的 Artifact")
            err(f"已尝试名称: {', '.join(names)}")
            if artifacts:
                print("  最近的 Artifacts:")
                for a in artifacts:
                    print(f"    - {a['name']}  "
                          f"({fmt_size(a.get('size_in_bytes', 0))})")
            sys.exit(1)
        warn(f"已回退匹配到: {data['name']}")
    workflow = data.get("workflow_run", {})
    run_id = workflow.get("id")
    head_sha = workflow.get("head_sha")

    if not head_sha and run_id:
        try:
            url = f"https://api.github.com/repos/{repo}/actions/runs/{run_id}"
            run_raw = _curl_get_json(url, token)
            run_data = json.loads(run_raw)
            head_sha = run_data.get("head_sha")
        except (RuntimeError, json.JSONDecodeError):
            pass

    created_str = data.get("created_at", "")
    try:
        created_dt = datetime.fromisoformat(created_str.replace("Z", "+00:00"))
        created_local = created_dt.astimezone(
            timezone(timedelta(hours=8))
        ).strftime("%Y-%m-%d %H:%M:%S")
    except (ValueError, AttributeError):
        created_local = f"{created_str} UTC"

    return {
        "id": data["id"],
        "size": data["size_in_bytes"],
        "name": data["name"],
        "run_id": run_id,
        "head_sha": head_sha or f"run-{run_id}",
        "created_at": created_local,
    }


def download_artifact(artifact_id: int, output_path: Path,
                      token: str,
                      timeout: int | None = None) -> Path:
    repo = getattr(args, 'repo', REPO)
    download_url = (
        f"https://api.github.com/repos/{repo}/actions/artifacts"
        f"/{artifact_id}/zip"
    )
    # 优先用 API 请求阶段真正认证成功的令牌（_TOKEN_STATE），并带上所有候选
    resolved = _TOKEN_STATE.get("token") or token
    return _download_with_retry(
        download_url, output_path, timeout=timeout,
        token_candidates=_token_candidates_for(resolved))


# ── Release 操作 ──

def get_latest_release_info() -> dict:
    repo = getattr(args, 'repo', REPO)
    info(f"查找最新 Release (repo: {repo})")

    url = f"https://api.github.com/repos/{repo}/releases/latest"
    raw = _curl_get_json(url)
    data = json.loads(raw)

    return {"tag": data["tag_name"], "assets": data["assets"]}


def find_best_backend_asset(assets: list[dict]) -> dict | None:
    """在 Release assets 中找到匹配当前平台的后端二进制。"""
    return pick_backend_artifact(assets)


def find_frontend_asset(assets: list[dict]) -> dict | None:
    """在 Release assets 中找到前端资源。"""
    forced_name = getattr(args, 'artifact_name', None)

    if forced_name:
        for a in assets:
            if a["name"] == forced_name:
                return a
        return None

    frontend_keywords = ["frontend", "front-end", "front_end", "web"]
    for keyword in frontend_keywords:
        for a in assets:
            if keyword in a["name"].lower():
                return a

    return None


def download_release_asset(download_url: str, output_path: Path,
                           timeout: int | None = None) -> Path:
    return _download_with_retry(download_url, output_path, timeout=timeout)


# ── 最新版本软链接 ──

def link_latest_binary(binary: Path | None) -> Path | None:
    """把刚安装的二进制软链接到 versions/latest/okb-assist。

    链接使用相对路径（相对于 versions/latest/），便于整个 ~/.okb 目录搬移。
    失败只告警，不影响安装结果。
    """
    if not binary:
        return None

    try:
        target = Path(binary).resolve()
    except OSError:
        target = Path(binary)

    link_path = LATEST_DIR / LATEST_LINK_NAME
    try:
        LATEST_DIR.mkdir(parents=True, exist_ok=True)
        # 先清理旧链接（broken symlink 的 exists() 为 False，故先判断 is_symlink）
        if link_path.is_symlink() or link_path.exists():
            link_path.unlink()
        rel_target = os.path.relpath(target, LATEST_DIR)
        link_path.symlink_to(rel_target)
    except OSError as e:
        warn(f"创建最新版本链接失败: {e}")
        return None

    ok(f"最新版本链接: {link_path} -> {rel_target}")
    return link_path


# ── 版本管理 ──

def get_installed_versions() -> list[dict]:
    versions = []
    for base_dir in (VERSIONS_DIR, RELEASES_DIR):
        if not base_dir.exists():
            continue
        for item in base_dir.iterdir():
            # versions/latest 是软链接目录，不作为独立版本列出
            if item.is_dir() and item.resolve() != LATEST_DIR.resolve():
                binary = find_binary_in_dir(item)
                versions.append({
                    "version": item.name,
                    "path": str(item),
                    "binary": str(binary) if binary else None,
                    "mtime": item.stat().st_mtime,
                })
    versions.sort(key=lambda v: v["mtime"], reverse=True)
    return versions


# ── 命令: bin ──

def cmd_bin(version: str | None, latest: bool = False):
    versions = get_installed_versions()

    if latest:
        # 只输出最新（按 mtime）且含二进制的版本的绝对路径，便于 `$(...)` 直接取用
        for v in versions:
            if v["binary"]:
                print(Path(v["binary"]).resolve())
                return
        if not versions:
            print("没有已安装的版本。", file=sys.stderr)
            print("使用 `install latest-artifact|la` 或 "
                  "`install latest-release|lr` 下载。", file=sys.stderr)
        else:
            print(f"未找到可执行文件: {versions[0]['version']} "
                  f"({versions[0]['path']})", file=sys.stderr)
        sys.exit(1)

    if not versions:
        info("没有已安装的版本。")
        info("使用 `install latest-artifact|la` 或 `install latest-release|lr` 下载。")
        return

    if version:
        for v in versions:
            if v["version"] == version:
                display = v["binary"] or v["path"]
                print(display)
                return
        for v in versions:
            if v["version"].startswith(version) or version in v["version"]:
                display = v["binary"] or v["path"]
                print(display)
                return
        err(f"未找到版本: {version}")
        print(f"  可用版本: {', '.join(v['version'] for v in versions)}")
        sys.exit(1)
    else:
        print(f"\n  {Color.CYAN}已安装的版本:{Color.NC}")
        print(f"  {'版本':<25} {'路径'}")
        print(f"  {'-'*23} {'-'*50}")
        for v in versions:
            display = v["binary"] or v["path"]
            print(f"  {v['version']:<25} {display}")
        print()


# ── 命令: install latest-artifact ──

def cmd_install_artifact():
    token = get_github_token()
    if not token:
        err("无法获取 GitHub token")
        err("请设置环境变量 GH_TOKEN 或 GITHUB_TOKEN")
        err(f"或创建文件 {OKB_DIR / 'github_token'} 并写入 token")
        err("或安装并认证 gh CLI: gh auth login")
        sys.exit(1)
    for label, _value in get_github_token_candidates():
        info(f"令牌来源候选: {label}")

    artifact = get_latest_artifact_info(token)
    head_sha = artifact["head_sha"]
    short_sha = head_sha[:12] if len(head_sha) > 12 else head_sha
    version_dir = VERSIONS_DIR / short_sha
    cache_path = CACHE_DIR / f"{short_sha}.zip"
    repo = getattr(args, 'repo', REPO)
    download_url = (
        f"https://api.github.com/repos/{repo}/actions/artifacts"
        f"/{artifact['id']}/zip"
    )
    dl_timeout = getattr(args, 'timeout', None)

    if version_dir.exists():
        existing_bin = find_binary_in_dir(version_dir)
        if existing_bin:
            ok(f"版本 {short_sha} 已安装")
            print(f"  路径: {existing_bin}")
            link_latest_binary(existing_bin)
            return
        warn(f"目录 {short_sha} 已存在但缺少二进制，重新安装 ...")

    print(f"\n  Artifact:   {Color.CYAN}{artifact['name']}{Color.NC}")
    print(f"  提交:       {short_sha}")
    print(f"  大小:       {Color.YELLOW}{fmt_size(artifact['size'])}{Color.NC}")
    print(f"  构建时间:   {artifact['created_at']}")
    print()

    if not confirm("确认下载？[Y/n]"):
        info("已取消。")
        return

    ensure_dirs()

    if resolve_cache_file(cache_path, download_url, artifact["id"]):
        info("使用已有缓存文件解压 ...")
    else:
        if cache_path.exists():
            cache_path.unlink()
        info("下载中 ... (可能较慢，请耐心等待)")
        try:
            download_artifact(artifact["id"], cache_path, token,
                              timeout=dl_timeout)
        except RuntimeError as e:
            err(str(e))
            err(f"下载 URL: {download_url}")
            err(f"目标路径: {cache_path}")
            err("请用有效的 GH_TOKEN/GITHUB_TOKEN 环境变量，更新 "
                f"{OKB_DIR / 'github_token'}，或执行 gh auth login 后重试")
            sys.exit(1)

    extract_zip(cache_path, version_dir)
    ok(f"已安装到: {version_dir}/")

    binary = find_binary_in_dir(version_dir)
    if binary:
        ok(f"二进制文件: {binary}")
        link_latest_binary(binary)
    else:
        warn(f"未在 {version_dir}/ 中找到二进制文件 (预期名称: {BINARY_NAME})")


# ── 命令: install latest-release ──

def cmd_install_release():
    is_frontend = getattr(args, 'frontend', False)
    release = get_latest_release_info()
    tag = release["tag"]
    assets = release["assets"]

    if is_frontend:
        asset = find_frontend_asset(assets)
        if not asset:
            err("未在 Release 中找到前端资源")
            print(f"  可用的 assets:")
            for a in assets:
                print(f"    - {a['name']}  ({fmt_size(a['size'])})")
            sys.exit(1)
        info(f"找到前端资源: {asset['name']}")
    else:
        asset = find_best_backend_asset(assets)
        if not asset:
            err("未在 Release 中找到匹配当前平台的后端")
            print(f"  平台: {platform.system().lower()} / "
                  f"{platform.machine().lower()}")
            print(f"  期望名称: {', '.join(artifact_name_candidates())}")
            print(f"  可用的 assets:")
            for a in assets:
                print(f"    - {a['name']}  ({fmt_size(a['size'])})")
            sys.exit(1)
        info(f"找到后端资源: {asset['name']}")

    version_dir = RELEASES_DIR / tag
    cache_path = CACHE_DIR / asset["name"]
    dl_timeout = getattr(args, 'timeout', None)

    if version_dir.exists():
        existing_bin = find_binary_in_dir(version_dir)
        if existing_bin:
            ok(f"版本 {tag} 已安装")
            print(f"  路径: {existing_bin}")
            link_latest_binary(existing_bin)
            return
        warn(f"目录 {tag} 已存在但缺少二进制，重新安装 ...")

    print(f"\n  Release:    {Color.CYAN}{tag}{Color.NC}")
    print(f"  Asset:      {asset['name']}  ({fmt_size(asset['size'])})")
    print()

    if not confirm("确认下载？[Y/n]"):
        info("已取消。")
        return

    ensure_dirs()

    download_url = asset["browser_download_url"]
    if resolve_cache_file(cache_path, download_url):
        info("使用已有缓存文件解压 ...")
    else:
        if cache_path.exists():
            cache_path.unlink()
        info("下载中 ... (可能较慢，请耐心等待)")
        download_release_asset(download_url, cache_path,
                               timeout=dl_timeout)

    extract_archive(cache_path, version_dir)
    ok(f"已安装到: {version_dir}/")

    if is_frontend:
        info("前端资源已安装。")
    else:
        binary = find_binary_in_dir(version_dir)
        if binary:
            ok(f"二进制文件: {binary}")
            link_latest_binary(binary)
        else:
            warn(f"未在 {version_dir}/ 中找到二进制文件 (预期名称: {BINARY_NAME})")


# ── 命令: self-update ──

def cmd_self_update():
    """从 GitHub 下载最新版本脚本并替换自身。"""
    repo = getattr(args, 'repo', REPO)
    script_name = Path(__file__).name
    raw_url = (f"https://raw.githubusercontent.com/{repo}/main/scripts/"
               f"{script_name}")

    info(f"检查更新: {raw_url}")

    tmp_dir = Path(tempfile.mkdtemp(prefix="okb-self-update-"))
    tmp_path = tmp_dir / script_name

    ensure_dirs()
    _download_with_retry(raw_url, tmp_path, timeout=60)

    # 验证 Python 语法
    try:
        import py_compile
        py_compile.compile(str(tmp_path), doraise=True)
    except py_compile.PyCompileError as e:
        err(f"下载的脚本语法错误: {e}")
        shutil.rmtree(tmp_dir, ignore_errors=True)
        sys.exit(1)

    self_path = Path(__file__).resolve()
    local_content = self_path.read_bytes() if self_path.exists() else b""
    new_content = tmp_path.read_bytes()

    if local_content == new_content:
        ok("已是最新版本，无需更新。")
        shutil.rmtree(tmp_dir, ignore_errors=True)
        return

    print(f"\n  当前: {self_path}")
    print(f"  来源: {raw_url}")
    print(f"  新旧差异: {fmt_size(abs(len(new_content) - len(local_content)))}")
    print()

    if not confirm("确认更新自身脚本？[Y/n]"):
        info("已取消。")
        shutil.rmtree(tmp_dir, ignore_errors=True)
        return

    try:
        self_path.write_bytes(new_content)
        self_path.chmod(self_path.stat().st_mode | stat.S_IEXEC
                        | stat.S_IXGRP | stat.S_IXOTH)
    except OSError as e:
        err(f"写入失败: {e}")
        err(f"请手动复制 {tmp_path} 到 {self_path}")
        sys.exit(1)

    shutil.rmtree(tmp_dir, ignore_errors=True)

    new_size = fmt_size(new_content)
    ok("更新完成！")
    print(f"\n  路径: {self_path}")
    print(f"  大小: {new_size}")
    print(f"\n  提示: 部分改动可能需要重新运行本命令才能生效。\n")


# ── 入口 ──

def main():
    global args

    parser = argparse.ArgumentParser(
        description="管理 OKB-Assist 多版本",
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog=(
            "目录结构:\n"
            f"  {OKB_DIR}/cache/              下载的压缩包缓存\n"
            f"  {OKB_DIR}/versions/{{git_hash}}/  按提交 SHA 解压的目录\n"
            f"  {OKB_DIR}/releases/{{tag_name}}/  按 Release 标签解压的目录\n"
            f"  {OKB_DIR}/versions/latest/okb-assist  最新二进制的软链接\n"
            "                             (install la / install lr 后自动更新)\n\n"
            "Token 来源 (la 需要):\n"
            "  1. 环境变量 GH_TOKEN\n"
            "  2. 环境变量 GITHUB_TOKEN\n"
            f"  3. {OKB_DIR / 'github_token'} 文件\n"
            "  4. gh auth token (gh CLI >= 2.11)\n"
            "  5. gh 配置 ~/.config/gh/hosts.yml 中的 oauth_token (老版本 gh 兜底)\n"
            "  令牌无效(401)时自动尝试下一个来源"
        ),
    )
    parser.add_argument("-p", "--proxy",
                        help="设置代理 (如 http://127.0.0.1:7899)")
    parser.add_argument("--timeout", type=int, default=None,
                        help="下载超时秒数 (默认: 不限制)")
    parser.add_argument("--repo", default=REPO,
                        help=f"GitHub 仓库 (默认: {REPO})")
    parser.add_argument("--artifact-name",
                        help="artifact/asset 名称 (默认: 自动探测)")
    parser.add_argument("-y", "--yes", action="store_true",
                        help="跳过确认")

    subparsers = parser.add_subparsers(dest="command", help="可用命令")

    install_parser = subparsers.add_parser(
        "install", aliases=["i"],
        help="安装一个版本 (latest-artifact|la / latest-release|lr)")
    install_parser.add_argument(
        "source",
        choices=["latest-artifact", "la", "latest-release", "lr"],
        help="安装来源",
    )
    install_parser.add_argument(
        "--frontend", action="store_true",
        help="下载前端资源 (仅 latest-release/lr 有效)",
    )

    bin_parser = subparsers.add_parser(
        "bin", help="查看已安装版本的路径")
    bin_parser.add_argument(
        "version", nargs="?",
        help="版本标识 (留空则列出所有已安装版本)")
    bin_parser.add_argument(
        "--latest", action="store_true",
        help="仅输出最新版本二进制的绝对路径 (忽略 version 参数)")

    subparsers.add_parser(
        "self-update", aliases=["su"],
        help="从 GitHub 更新自身脚本")

    args = parser.parse_args()

    if args.command is None:
        parser.print_help()
        return

    if args.command in ("install", "i"):
        check_basic_deps()
        if args.source in ("latest-artifact", "la"):
            if getattr(args, 'frontend', False):
                warn("--frontend 仅对 latest-release (lr) 有效，已忽略。")
            cmd_install_artifact()
        elif args.source in ("latest-release", "lr"):
            cmd_install_release()
    elif args.command == "bin":
        cmd_bin(args.version, getattr(args, "latest", False))
    elif args.command in ("self-update", "su"):
        cmd_self_update()


if __name__ == "__main__":
    main()