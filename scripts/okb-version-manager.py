#!/usr/bin/env python3
"""
okb-version-manager.py — 管理 OKB-Assist 多版本

下载、解压、管理各版本，支持 GitHub Actions Artifact 和 GitHub Release。

用法:
  {script} install latest-artifact|la [选项]   从 Actions Artifact 安装最新版
  {script} install latest-release|lr [选项]     从 GitHub Release 安装最新版
  {script} bin [版本]                            列出/查询已安装版本
  {script} self-update|su                       从 GitHub 更新自身脚本

选项:
  -p, --proxy PROXY          设置代理 (如 http://127.0.0.1:7899)
  --timeout SECONDS          下载超时秒数 (默认: 不限制)
  --repo REPO                设置 GitHub 仓库 (默认: xsro/okb-assist)
  --artifact-name NAME       强制指定 artifact/asset 名称 (默认: 自动探测)
  -y, --yes                  跳过确认
  -h, --help                 显示帮助

环境变量:
  HTTPS_PROXY    同 --proxy
  YES            设为 1 跳过确认

Token 来源 (la 需要):
  1. 环境变量 GH_TOKEN
  2. 环境变量 GITHUB_TOKEN
  3. ~/.okb/github_token 文件
  4. gh auth token (gh CLI 已安装时)

目录结构:
  ~/.okb/cache/               下载的压缩包缓存
  ~/.okb/cache/               下载的压缩包缓存
  ~/.okb/versions/{git_hash}/  按提交 SHA(artifact) 解压的目录
  ~/.okb/versions/{tag_name}/  按 Release 标签名解压的目录
"""

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
RETRY_COUNT = 3
RETRY_DELAY = 10  # 秒

# Cargo.toml 中定义的二进制名称
BINARY_NAME = "okb_assist"


# ── 平台探测 ──

def detect_platform_artifact_name() -> str:
    system_map = {
        "linux": "linux",
        "darwin": "macos",
        "windows": "windows",
    }
    arch_map = {
        "x86_64": "x86_64",
        "amd64": "x86_64",
        "aarch64": "arm64",
        "arm64": "arm64",
        "armv7l": "armv7",
    }
    raw_sys = platform.system().lower()
    raw_machine = platform.machine().lower()
    sys_name = system_map.get(raw_sys, raw_sys)
    arch_name = arch_map.get(raw_machine, raw_machine)
    if sys_name == "windows":
        return f"okb-assist-{sys_name}-{arch_name}.exe"
    return f"okb-assist-{sys_name}-{arch_name}"


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

def get_github_token() -> str | None:
    token = os.environ.get("GH_TOKEN") or os.environ.get("GITHUB_TOKEN")
    if token:
        return token

    token_file = OKB_DIR / "github_token"
    if token_file.exists():
        try:
            token = token_file.read_text().strip()
            if token:
                return token
        except OSError:
            pass

    if shutil.which("gh"):
        try:
            result = subprocess.run(
                ["gh", "auth", "token"],
                capture_output=True, text=True, timeout=10,
            )
            if result.returncode == 0:
                token = result.stdout.strip()
                if token:
                    return token
        except (FileNotFoundError, subprocess.TimeoutExpired):
            pass

    return None


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

def _curl_get_json(url: str, token: str | None = None) -> str:
    def build_cmd(use_proxy: bool) -> list[str]:
        cmd = ["curl", "-sSfL"]
        if use_proxy:
            p = proxy_url()
            if p:
                cmd += ["-x", p]
        if token:
            cmd += ["-H", f"Authorization: Bearer {token}"]
        cmd += [url]
        return cmd

    proxy = proxy_url()
    for attempt in range(1, RETRY_COUNT + 1):
        if attempt > 1:
            warn(f"重试第 {attempt} 次 ...")
            time.sleep(RETRY_DELAY)

        if proxy:
            try:
                result = subprocess.run(
                    build_cmd(True), capture_output=True, text=True,
                    timeout=30, check=True)
                return result.stdout
            except (subprocess.CalledProcessError, subprocess.TimeoutExpired) as e:
                warn(f"代理请求失败: {e}")
                proxy = None
                continue

        try:
            result = subprocess.run(
                build_cmd(False), capture_output=True, text=True,
                timeout=30, check=True)
            return result.stdout
        except (subprocess.CalledProcessError, subprocess.TimeoutExpired) as e:
            if attempt < RETRY_COUNT:
                warn(f"直连请求失败: {e}")
                continue
            raise RuntimeError(f"API 请求失败，已重试 {RETRY_COUNT} 次: {url}")

    raise RuntimeError(f"API 请求失败: {url}")


# ── 通用下载 ──

def _do_curl_download(curl_cmd: list[str],
                      timeout: int | None = None) -> None:
    try:
        subprocess.run(curl_cmd, stdout=subprocess.DEVNULL,
                       timeout=timeout, check=True)
    except FileNotFoundError:
        raise RuntimeError("curl 未安装，请先安装 curl")
    except subprocess.TimeoutExpired:
        raise RuntimeError("下载超时")
    except subprocess.CalledProcessError:
        raise RuntimeError("下载失败，请检查网络或代理设置")


def _download_with_retry(download_url: str, output_path: Path,
                         extra_headers: list[str] | None = None,
                         timeout: int | None = None) -> Path:
    def build_cmd(use_proxy: bool) -> list[str]:
        cmd = ["curl", "-SfL"]
        if use_proxy:
            p = proxy_url()
            if p:
                cmd += ["-x", p]
        if extra_headers:
            for h in extra_headers:
                cmd += ["-H", h]
        cmd += ["-o", str(output_path), download_url]
        return cmd

    proxy = proxy_url()
    for attempt in range(1, RETRY_COUNT + 1):
        if attempt > 1:
            warn(f"重试第 {attempt} 次 ...")
            time.sleep(RETRY_DELAY)

        if proxy:
            info(f"使用代理: {proxy}")
            try:
                _do_curl_download(build_cmd(True), timeout=timeout)
                if output_path.stat().st_size > 1000:
                    break
            except RuntimeError as e:
                warn(f"代理下载失败: {e}")
                info("回退到直连下载 ...")
                proxy = None
                continue

        try:
            _do_curl_download(build_cmd(False), timeout=timeout)
        except RuntimeError as e:
            if attempt < RETRY_COUNT:
                warn(f"直连下载失败: {e}")
                continue
            raise RuntimeError(f"下载失败，已重试 {RETRY_COUNT} 次")

        if output_path.stat().st_size > 1000:
            break
        warn(f"下载内容异常（仅 {output_path.stat().st_size} 字节），重试 ...")

    ok("下载完成")
    return output_path


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
    artifact_name = (getattr(args, 'artifact_name', None)
                     or detect_platform_artifact_name())
    candidates = [BINARY_NAME, artifact_name]

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
    artifact_name = (getattr(args, 'artifact_name', None)
                     or detect_platform_artifact_name())
    repo = getattr(args, 'repo', REPO)
    info(f"查找最新 Artifact: {artifact_name} (repo: {repo})")

    url = (f"https://api.github.com/repos/{repo}/actions/artifacts"
           f"?name={artifact_name}&per_page=1")
    raw = _curl_get_json(url, token)
    data = json.loads(raw)
    artifacts = data.get("artifacts", [])
    if not artifacts:
        err(f"未找到名为 '{artifact_name}' 的 Artifact")
        sys.exit(1)

    data = artifacts[0]
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
    headers = [f"Authorization: Bearer {token}"]
    return _download_with_retry(download_url, output_path,
                                extra_headers=headers, timeout=timeout)


# ── Release 操作 ──

def get_latest_release_info() -> dict:
    repo = getattr(args, 'repo', REPO)
    info(f"查找最新 Release (repo: {repo})")

    url = f"https://api.github.com/repos/{repo}/releases/latest"
    raw = _curl_get_json(url)
    data = json.loads(raw)

    artifact_name = (getattr(args, 'artifact_name', None)
                     or detect_platform_artifact_name())
    system = platform.system().lower()
    machine = platform.machine().lower()

    def score_asset(name: str) -> int:
        n = name.lower()
        score = 0
        if artifact_name.lower() in n:
            score += 100
        if system in n:
            score += 10
        if machine[:4] in n:
            score += 10
        return score

    scored = [(score_asset(a["name"]), a) for a in data["assets"]]
    scored.sort(key=lambda x: x[0], reverse=True)
    best_score, best_asset = scored[0]

    if best_score == 0 or best_asset is None:
        err("未在 Release 中找到匹配当前平台的 asset")
        print(f"  尝试匹配: {artifact_name}")
        print(f"  平台: {system} / {machine}")
        print(f"  可用的 assets:")
        for a in data["assets"]:
            print(f"    - {a['name']}  ({fmt_size(a['size'])})")
        sys.exit(1)

    return {"tag": data["tag_name"], "asset": best_asset}


def download_release_asset(download_url: str, output_path: Path,
                           timeout: int | None = None) -> Path:
    return _download_with_retry(download_url, output_path, timeout=timeout)


# ── 版本管理 ──

def get_installed_versions() -> list[dict]:
    versions = []
    if not VERSIONS_DIR.exists():
        return versions
    for item in VERSIONS_DIR.iterdir():
        if item.is_dir():
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

def cmd_bin(version: str | None):
    versions = get_installed_versions()
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
            err("请使用 GH_TOKEN/GITHUB_TOKEN 环境变量或 gh CLI 认证后重试")
            sys.exit(1)

    extract_zip(cache_path, version_dir)
    ok(f"已安装到: {version_dir}/")

    binary = find_binary_in_dir(version_dir)
    if binary:
        ok(f"二进制文件: {binary}")
    else:
        warn(f"未在 {version_dir}/ 中找到二进制文件 (预期名称: {BINARY_NAME})")


# ── 命令: install latest-release ──

def cmd_install_release():
    release = get_latest_release_info()
    tag = release["tag"]
    asset = release["asset"]
    version_dir = VERSIONS_DIR / tag
    cache_path = CACHE_DIR / asset["name"]
    dl_timeout = getattr(args, 'timeout', None)

    if version_dir.exists():
        existing_bin = find_binary_in_dir(version_dir)
        if existing_bin:
            ok(f"版本 {tag} 已安装")
            print(f"  路径: {existing_bin}")
            return
        warn(f"目录 {tag} 已存在但缺少二进制，重新安装 ...")

    print(f"\n  Release:    {Color.CYAN}{tag}{Color.NC}")
    print(f"  Asset:      {asset['name']}  ({fmt_size(asset['size'])})")
    print()

    if not confirm("确认下载？[Y/n]"):
        info("已取消。")
        return

    ensure_dirs()

    if resolve_cache_file(cache_path, asset["download_url"]):
        info("使用已有缓存文件解压 ...")
    else:
        if cache_path.exists():
            cache_path.unlink()
        info("下载中 ... (可能较慢，请耐心等待)")
        download_release_asset(asset["download_url"], cache_path,
                               timeout=dl_timeout)

    extract_archive(cache_path, version_dir)
    ok(f"已安装到: {version_dir}/")

    binary = find_binary_in_dir(version_dir)
    if binary:
        ok(f"二进制文件: {binary}")
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
            f"  {OKB_DIR}/versions/{{tag_name}}/  按 Release 标签解压的目录\n\n"
            "Token 来源 (la 需要):\n"
            "  1. 环境变量 GH_TOKEN\n"
            "  2. 环境变量 GITHUB_TOKEN\n"
            f"  3. {OKB_DIR / 'github_token'} 文件\n"
            "  4. gh auth token (gh CLI 已安装时)"
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

    bin_parser = subparsers.add_parser(
        "bin", help="查看已安装版本的路径")
    bin_parser.add_argument(
        "version", nargs="?",
        help="版本标识 (留空则列出所有已安装版本)")

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
            cmd_install_artifact()
        elif args.source in ("latest-release", "lr"):
            cmd_install_release()
    elif args.command == "bin":
        cmd_bin(args.version)
    elif args.command in ("self-update", "su"):
        cmd_self_update()


if __name__ == "__main__":
    main()