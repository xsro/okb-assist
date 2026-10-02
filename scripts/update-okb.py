#!/usr/bin/env python3
"""
update-okb.py — 从 GitHub Actions 下载最新构建并替换 data/ 中的二进制文件

用法:
  ./scripts/update-okb.py [选项]

选项:
  -y, --yes       跳过确认，直接更新（非交互模式）
  -k, --keep      保留旧版本备份为 .bak
  -p, --proxy     设置代理，如 http://127.0.0.1:7899（仅下载时使用）
  --all-proxy     所有 gh 命令均使用代理（含查询 artifact 列表）
  -h, --help      显示帮助

环境变量:
  HTTPS_PROXY     同 --proxy
  YES             设为 1 跳过确认

依赖: gh (GitHub CLI, 已认证), unzip
"""

import argparse
import json
import os
import shutil
import stat
import subprocess
import sys
import tempfile
import time
from datetime import datetime, timezone, timedelta
from pathlib import Path
from urllib.request import Request, urlopen
from urllib.error import URLError

# ── 常量 ──
REPO = "xsro/okb-assist"
ARTIFACT_NAME = "okb-assist-linux-arm64"
SCRIPT_DIR = Path(__file__).resolve().parent
PROJECT_DIR = SCRIPT_DIR.parent
DATA_DIR = PROJECT_DIR / "data"
TARGET = DATA_DIR / ARTIFACT_NAME

RETRY_COUNT = 3
RETRY_DELAY = 10  # 秒


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


# ── 辅助函数 ──

def fmt_size(bytes_: int) -> str:
    """将字节数格式化为人类可读。"""
    if bytes_ >= 1048576:
        return f"{bytes_ / 1048576:.1f} MiB"
    elif bytes_ >= 1024:
        return f"{bytes_ / 1024:.1f} KiB"
    else:
        return f"{bytes_} B"


def run_gh(cmd_args: list[str], timeout: int = 120, use_proxy: bool | None = None) -> str:
    """执行 gh CLI 命令，返回 stdout。

    use_proxy: True=强制使用代理, False=不用, None=跟随 --all-proxy 标志
    """
    env = os.environ.copy()
    proxy = proxy_url()
    if proxy:
        if use_proxy is None:
            use_proxy = getattr(args, 'all_proxy', False)
        if use_proxy:
            env.setdefault("HTTPS_PROXY", proxy)
    try:
        result = subprocess.run(
            ["gh"] + cmd_args,
            capture_output=True, text=True, timeout=timeout,
            env=env,
        )
        if result.returncode != 0:
            err_msg = result.stderr.strip() or f"exit code {result.returncode}"
            raise RuntimeError(f"gh 命令失败: gh {' '.join(cmd_args)}\n  {err_msg}")
        return result.stdout.strip()
    except FileNotFoundError:
        raise RuntimeError("gh 未安装，请先安装 GitHub CLI: https://cli.github.com")
    except subprocess.TimeoutExpired:
        raise RuntimeError(f"gh 命令超时 (>{timeout}s): {' '.join(cmd_args)}")


def proxy_url() -> str | None:
    """返回代理 URL，命令行选项优先于环境变量。"""
    if hasattr(proxy_url, "_cached"):
        return proxy_url._cached
    val = None
    if args.proxy:
        val = args.proxy
    elif os.environ.get("HTTPS_PROXY"):
        val = os.environ["HTTPS_PROXY"]
    proxy_url._cached = val
    return val


def confirm(prompt: str, default: str = "Y") -> bool:
    """交互式确认。"""
    non_interactive = args.yes or os.environ.get("YES") == "1"
    if non_interactive:
        return True
    resp = input(f"{prompt} ").strip().lower() or default.lower()
    return resp.startswith("y")


def check_dependencies():
    """检查必要的依赖。"""
    missing = []
    for cmd in ("gh", "unzip"):
        if shutil.which(cmd) is None:
            missing.append(cmd)
    if missing:
        err(f"缺少必要依赖: {', '.join(missing)}")
        if "gh" in missing:
            err("请安装 GitHub CLI: https://cli.github.com")
        sys.exit(1)

    # 验证 gh 已认证（注意：不设代理，因为这是本地凭据检查）
    try:
        result = subprocess.run(
            ["gh", "auth", "status"],
            capture_output=True, text=True, timeout=10,
        )
        if result.returncode != 0:
            raise RuntimeError(result.stderr.strip())
    except (FileNotFoundError, RuntimeError):
        err("gh 未认证，请先运行: gh auth login")
        sys.exit(1)


# ── 核心逻辑 ──

def get_latest_artifact() -> dict:
    """查询最新 artifact 信息。"""
    info(f"查找最新 Artifact: {ARTIFACT_NAME} (repo: {REPO})")

    api_args = [
        "api",
        f"repos/{REPO}/actions/artifacts?name={ARTIFACT_NAME}&per_page=1",
        "--jq", ".artifacts[0] // empty",
    ]
    raw = run_gh(api_args, timeout=30)

    if not raw:
        err(f"未找到名为 '{ARTIFACT_NAME}' 的 Artifact")
        sys.exit(1)

    data = json.loads(raw)

    created_str = data.get("created_at", "")
    try:
        created_dt = datetime.fromisoformat(created_str.replace("Z", "+00:00"))
        created_local = created_dt.astimezone(
            timezone(timedelta(hours=8))
        ).strftime("%Y-%m-%d %H:%M:%S")
    except (ValueError, AttributeError):
        created_local = f"{created_str} UTC"

    artifact = {
        "id": data["id"],
        "size": data["size_in_bytes"],
        "run_id": data.get("workflow_run", {}).get("id", "?"),
        "created_at": created_local,
        "name": data["name"],
    }
    return artifact


def get_gh_token() -> str:
    """通过 gh auth status --show-token 获取 GitHub API token。

    注意：不设任何代理环境变量，token 读取是本地操作。
    """
    try:
        result = subprocess.run(
            ["gh", "auth", "status", "--show-token"],
            capture_output=True, text=True, timeout=10, check=True,
        )
        # 输出在 stderr，格式 "✓ Token: xxx"
        for line in result.stderr.splitlines():
            if "Token:" in line:
                token = line.split("Token:")[-1].strip()
                if token:
                    return token
        raise RuntimeError("未找到 Token")
    except FileNotFoundError:
        raise RuntimeError("gh 未安装，请先安装 GitHub CLI: https://cli.github.com")
    except subprocess.CalledProcessError:
        raise RuntimeError("无法获取 GitHub token，请先运行: gh auth login")


def _do_curl_download(curl_cmd: list[str], timeout: int) -> None:
    """执行 curl 下载，统一异常处理。"""
    try:
        subprocess.run(curl_cmd, capture_output=True, text=True, timeout=timeout, check=True)
    except FileNotFoundError:
        raise RuntimeError("curl 未安装，请先安装 curl")
    except subprocess.TimeoutExpired:
        raise RuntimeError("下载超时")
    except subprocess.CalledProcessError as e:
        stderr = e.stderr.strip()
        raise RuntimeError(f"下载失败: {stderr[:200]}")


def download_artifact(artifact_id: int, output_dir: Path) -> Path:
    """下载 artifact zip 到 output_dir，返回 zip 路径。

    用 curl 替代 gh api 下载，以获得更好的代理兼容性。
    若设了代理但下载失败，自动回退到直连。
    """
    info("下载中 ... (可能较慢，请耐心等待)")

    download_url = f"https://api.github.com/repos/{REPO}/actions/artifacts/{artifact_id}/zip"
    zip_path = output_dir / f"{ARTIFACT_NAME}.zip"

    # 获取 token（不走代理，纯本地操作）
    token = get_gh_token()

    def build_cmd(use_proxy: bool) -> list[str]:
        cmd = ["curl", "-sSfL"]
        if use_proxy:
            p = proxy_url()
            if p:
                cmd += ["-x", p]
        cmd += [
            "-H", f"Authorization: Bearer {token}",
            "-o", str(zip_path),
            download_url,
        ]
        return cmd

    proxy = proxy_url()
    for attempt in range(1, RETRY_COUNT + 1):
        if attempt > 1:
            warn(f"重试第 {attempt} 次 ...")
            time.sleep(RETRY_DELAY)

        # 第一轮尝试：用代理下载
        if proxy:
            info(f"使用代理: {proxy}")
            try:
                _do_curl_download(build_cmd(True), timeout=30)
                if zip_path.stat().st_size > 1000:
                    break
            except RuntimeError as e:
                warn(f"代理下载失败: {e}")
                info("回退到直连下载 ...")
                proxy = None  # 后续重试走直连
                # 这轮直接用直连重试（不递增 attempt）
                continue

        # 直连下载
        try:
            _do_curl_download(build_cmd(False), timeout=120)
        except RuntimeError as e:
            if attempt < RETRY_COUNT:
                warn(f"直连下载失败: {e}")
                continue
            raise

        # 检查文件大小
        if zip_path.stat().st_size > 1000:
            break
        warn(f"下载内容异常（仅 {zip_path.stat().st_size} 字节），重试 ...")

    ok("下载完成")
    return zip_path


def extract_artifact(zip_path: Path, output_dir: Path) -> Path:
    """解压并找到二进制文件。"""
    info(f"解压到 {output_dir}/")
    shutil.unpack_archive(str(zip_path), str(output_dir), format="zip")

    # 查找二进制
    for f in output_dir.iterdir():
        if f.is_file() and f.name == ARTIFACT_NAME:
            f.chmod(f.stat().st_mode | stat.S_IEXEC | stat.S_IXGRP | stat.S_IXOTH)
            return f

    raise RuntimeError(f"解压后未找到 {ARTIFACT_NAME}")


def stop_running_process():
    """停止正在运行的 okb-assist 进程。"""
    import signal

    try:
        result = subprocess.run(
            ["pgrep", "-f", ARTIFACT_NAME],
            capture_output=True, text=True, timeout=10,
        )
    except FileNotFoundError:
        # macOS 没有 pgrep
        try:
            result = subprocess.run(
                ["ps", "aux"],
                capture_output=True, text=True, timeout=10,
            )
            pids = []
            for line in result.stdout.splitlines():
                if ARTIFACT_NAME in line and "grep" not in line:
                    parts = line.split()
                    if parts:
                        pids.append(parts[1])
            if not pids:
                return
        except subprocess.TimeoutExpired:
            return

    pids_str = result.stdout.strip()
    if not pids_str:
        return

    pids = [int(p) for p in pids_str.splitlines()]
    warn(f"检测到正在运行的 {ARTIFACT_NAME} 进程 (PID: {', '.join(map(str, pids))})")

    if not confirm("停止进程并继续？[Y/n]"):
        warn("跳过进程停止，更新后请手动重启。")
        return

    info("停止进程 ...")
    for pid in pids:
        try:
            os.kill(pid, signal.SIGTERM)
        except ProcessLookupError:
            pass

    time.sleep(1)

    # 检查是否已停止
    still_running = []
    for pid in pids:
        try:
            os.kill(pid, 0)
            still_running.append(pid)
        except ProcessLookupError:
            pass

    if still_running:
        warn(f"进程未响应，强制终止 (PID: {', '.join(map(str, still_running))}) ...")
        for pid in still_running:
            try:
                os.kill(pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
        time.sleep(1)

    ok("进程已停止。")


def replace_binary(new_binary: Path, keep_backup: bool):
    """替换 data/ 目录下的二进制文件。"""
    info("替换二进制文件 ...")

    DATA_DIR.mkdir(parents=True, exist_ok=True)

    if TARGET.exists():
        if keep_backup:
            backup = TARGET.with_suffix(".bak")
            shutil.copy2(TARGET, backup)
            ok(f"旧版本已备份到: {backup}")
        else:
            backup = TARGET.with_suffix(".bak")
            if backup.exists():
                backup.unlink()
            shutil.copy2(TARGET, backup)
            ok(f"旧版本已备份到: {backup}")

    shutil.copy2(new_binary, TARGET)
    TARGET.chmod(TARGET.stat().st_mode | stat.S_IEXEC | stat.S_IXGRP | stat.S_IXOTH)

    new_size = TARGET.stat().st_size
    ok("更新完成！")
    print(f"\n  目标路径:  {TARGET}")
    print(f"  新大小:    {fmt_size(new_size)}\n")

    # 验证
    info("验证二进制文件 ...")
    subprocess.run(["file", str(TARGET)], check=False)


# ── 主流程 ──

def main():
    global args

    parser = argparse.ArgumentParser(
        description="从 GitHub Actions 下载最新构建并替换 data/ 中的二进制文件",
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog=(
            "环境变量:\n"
            "  HTTPS_PROXY     同 --proxy\n"
            "  YES             设为 1 跳过确认\n\n"
            "依赖: gh (GitHub CLI, 已认证), unzip"
        ),
    )
    parser.add_argument("-y", "--yes", action="store_true", help="跳过确认，直接更新")
    parser.add_argument("-k", "--keep", action="store_true", help="保留旧版本备份为 .bak")
    parser.add_argument("-p", "--proxy", help="设置代理，如 http://127.0.0.1:7899（仅下载时使用）")
    parser.add_argument("--all-proxy", action="store_true", default=False,
                        help="所有 gh 命令均使用代理（含查询 artifact 列表）")
    args = parser.parse_args()

    # ── 前置检查 ──
    check_dependencies()

    if not DATA_DIR.exists():
        err(f"data/ 目录不存在: {DATA_DIR}")
        sys.exit(1)

    # ── 显示当前版本信息 ──
    current_size = "?"
    current_time = "?"
    if TARGET.exists():
        st = TARGET.stat()
        current_size = fmt_size(st.st_size)
        mtime = datetime.fromtimestamp(st.st_mtime)
        current_time = mtime.strftime("%Y-%m-%d %H:%M:%S")

    print("=" * 44)
    print("  OKB-Assist 更新脚本")
    print("=" * 44)
    print(f"\n  当前版本:")
    print(f"    路径: {TARGET}")
    print(f"    大小: {current_size}")
    print(f"    时间: {current_time}")
    print(f"\n  更新源:  GitHub Actions ({REPO})")
    print(f"  Artifact: {ARTIFACT_NAME}\n")

    if not confirm("确认更新？[Y/n]"):
        info("已取消。")
        sys.exit(0)

    # ── 1. 查询最新 artifact ──
    artifact = get_latest_artifact()
    print(f"\n  Artifact:   {Color.CYAN}{artifact['name']}{Color.NC}")
    print(f"  大小:       {Color.YELLOW}{fmt_size(artifact['size'])}{Color.NC}")
    print(f"  Run #{artifact['run_id']}")
    print(f"  构建时间:   {artifact['created_at']}\n")

    if not confirm("确认下载？[Y/n]"):
        info("已取消。")
        sys.exit(0)

    # ── 2. 下载 ──
    with tempfile.TemporaryDirectory(prefix="okb-update-") as tmp_dir:
        tmp_path = Path(tmp_dir)

        zip_path = download_artifact(artifact["id"], tmp_path)

        # ── 3. 解压 ──
        new_binary = extract_artifact(zip_path, tmp_path)

        # ── 4. (可选) 停止正在运行的进程 ──
        stop_running_process()

        # ── 5. 替换二进制 ──
        replace_binary(new_binary, args.keep)

    # ── 完成提示 ──
    info("启动方式:")
    print(f"  cd {PROJECT_DIR} && ./data/{ARTIFACT_NAME} [--args]")


if __name__ == "__main__":
    main()