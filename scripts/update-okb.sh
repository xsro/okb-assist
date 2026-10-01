#!/usr/bin/env bash
# ============================================================================
# update-okb.sh — 下载最新构建并原地替换 data/ 中的二进制文件
#
# 用法:
#   ./scripts/update-okb.sh [options]
#
# 选项:
#   -y, --yes     跳过确认，直接更新（非交互模式）
#   -k, --keep    保留旧版本备份为 data/okb-assist-linux-arm64.bak
#   -p, --proxy   设置代理，如 http://127.0.0.1:7899
#   -h, --help    显示帮助
#
# 环境变量:
#   HTTPS_PROXY   — 同 --proxy
#   YES           — 同 --yes
#
# 流程:
#   1. 调用 scripts/dl-artifact.sh 下载最新构建到临时目录
#   2. 停止当前运行中的 okb-assist 进程（可选）
#   3. 替换 data/okb-assist-linux-arm64 二进制文件
#   4. 保持可执行权限
# ============================================================================

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
DATA_DIR="$PROJECT_DIR/data"
DL_SCRIPT="$SCRIPT_DIR/dl-artifact.sh"
ARTIFACT_NAME="okb-assist-linux-arm64"
TARGET="$DATA_DIR/$ARTIFACT_NAME"

KEEP_BACKUP=false
NON_INTERACTIVE=false
PROXY=""

# --- 色彩输出 ---
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
CYAN='\033[0;36m'
NC='\033[0m'

info()  { echo -e "${CYAN}[INFO]${NC}  $*"; }
ok()    { echo -e "${GREEN}[OK]${NC}    $*"; }
warn()  { echo -e "${YELLOW}[WARN]${NC}  $*"; }
err()   { echo -e "${RED}[ERROR]${NC} $*"; }

# --- 解析参数 ---
while [[ $# -gt 0 ]]; do
  case "$1" in
    -y|--yes) NON_INTERACTIVE=true ;;
    -k|--keep) KEEP_BACKUP=true ;;
    -p|--proxy) PROXY="$2"; shift ;;
    --proxy=*) PROXY="${1#*=}" ;;
    -h|--help)
      sed -n '3,20p' "$0" | sed 's/^# \?//'
      exit 0
      ;;
    *) err "未知参数: $1"; exit 1 ;;
  esac
  shift
done

# --- 前置检查 ---
if [ ! -f "$DL_SCRIPT" ]; then
  err "找不到 dl-artifact.sh: $DL_SCRIPT"
  exit 1
fi

if [ ! -d "$DATA_DIR" ]; then
  err "data/ 目录不存在: $DATA_DIR"
  exit 1
fi

# --- 确认信息 ---
CURRENT_SIZE="?"
CURRENT_TIME="?"
if [ -f "$TARGET" ]; then
  CURRENT_SIZE=$(stat -c%s "$TARGET" 2>/dev/null || stat -f%z "$TARGET" 2>/dev/null)
  CURRENT_SIZE=$(echo "scale=1; $CURRENT_SIZE / 1048576" | bc)
  CURRENT_TIME=$(stat -c%y "$TARGET" 2>/dev/null | cut -d. -f1 || stat -f%Sm "$TARGET" 2>/dev/null)
fi

echo "============================================"
echo "  OKB-Assist 更新脚本"
echo "============================================"
echo ""
echo "  当前版本:"
echo "    路径: $TARGET"
echo "    大小: ${CURRENT_SIZE} MiB"
echo "    时间: ${CURRENT_TIME}"
echo ""
echo "  更新源:  GitHub Actions (xsro/okb-assist)"
echo "  Artifact: $ARTIFACT_NAME"
echo ""

if [ "$NON_INTERACTIVE" = false ] && [ "${YES:-}" != "1" ]; then
  read -rp "确认更新？[Y/n] " CONFIRM
  CONFIRM=${CONFIRM:-Y}
  if [[ ! "$CONFIRM" =~ ^[Yy]$ ]]; then
    info "已取消。"
    exit 0
  fi
fi

# --- 1. 下载新版本 ---
TMP_DIR=$(mktemp -d)
# 确保退出时清理
cleanup() { rm -rf "$TMP_DIR"; }
trap cleanup EXIT

info "下载最新构建 ..."
# 继承父进程已有的 HTTPS_PROXY，命令行 --proxy 优先级更高
DL_ENV=()
if [ -n "$PROXY" ]; then
  DL_ENV+=("HTTPS_PROXY=$PROXY")
elif [ -n "${HTTPS_PROXY:-}" ]; then
  DL_ENV+=("HTTPS_PROXY=$HTTPS_PROXY")
fi
if [ "$NON_INTERACTIVE" = true ] || [ "${YES:-}" = "1" ]; then
  DL_ENV+=("YES=1")
fi

# 用 env 传环境变量执行
info "调用 dl-artifact.sh ..."
if [ ${#DL_ENV[@]} -gt 0 ]; then
  env "${DL_ENV[@]}" bash "$DL_SCRIPT" "$ARTIFACT_NAME" "$TMP_DIR"
else
  bash "$DL_SCRIPT" "$ARTIFACT_NAME" "$TMP_DIR"
fi

NEW_BINARY=$(find "$TMP_DIR" -type f -name "$ARTIFACT_NAME" -print -quit 2>/dev/null || true)

if [ -z "$NEW_BINARY" ] || [ ! -f "$NEW_BINARY" ]; then
  err "下载后未找到二进制文件，退出。"
  exit 1
fi

NEW_SIZE=$(stat -c%s "$NEW_BINARY" 2>/dev/null || stat -f%z "$NEW_BINARY" 2>/dev/null)

# --- 2. (可选) 停止正在运行的 okb-assist ---
RUNNING_PIDS=$(pgrep -f "$ARTIFACT_NAME" 2>/dev/null || true)
if [ -n "$RUNNING_PIDS" ]; then
  echo ""
  warn "检测到正在运行的 ${ARTIFACT_NAME} 进程 (PID: $(echo "$RUNNING_PIDS" | tr '\n' ' '))"
  if [ "$NON_INTERACTIVE" = false ] && [ "${YES:-}" != "1" ]; then
    read -rp "停止进程并继续？[Y/n] " STOP_CONFIRM
    STOP_CONFIRM=${STOP_CONFIRM:-Y}
  else
    STOP_CONFIRM="Y"
  fi
  if [[ "$STOP_CONFIRM" =~ ^[Yy]$ ]]; then
    info "停止进程 ..."
    kill "$RUNNING_PIDS" 2>/dev/null || true
    sleep 1
    # 如果没停掉，强制杀
    RUNNING_PIDS=$(pgrep -f "$ARTIFACT_NAME" 2>/dev/null || true)
    if [ -n "$RUNNING_PIDS" ]; then
      warn "进程未响应，强制终止 ..."
      kill -9 "$RUNNING_PIDS" 2>/dev/null || true
      sleep 1
    fi
    ok "进程已停止。"
  else
    warn "跳过进程停止，更新后请手动重启。"
  fi
fi

# --- 3. 替换二进制 ---
echo ""
info "替换二进制文件 ..."

# 备份旧文件
if [ -f "$TARGET" ]; then
  if [ "$KEEP_BACKUP" = true ]; then
    BACKUP="${TARGET}.bak"
    cp "$TARGET" "$BACKUP"
    ok "旧版本已备份到: $BACKUP"
  else
    # 默认删掉旧的 .bak 然后创建新备份
    rm -f "${TARGET}.bak"
    cp "$TARGET" "${TARGET}.bak"
    ok "旧版本已备份到: ${TARGET}.bak"
  fi
fi

cp "$NEW_BINARY" "$TARGET"
chmod +x "$TARGET"

NEW_SIZE_HUMAN=$(echo "scale=1; $NEW_SIZE / 1048576" | bc)

echo ""
ok "更新完成！"
echo ""
echo "  目标路径:  $TARGET"
echo "  新大小:    ${NEW_SIZE_HUMAN} MiB"
echo ""

# --- 验证 ---
info "验证二进制文件 ..."
file "$TARGET"

echo ""
info "启动方式:"
echo "  cd $PROJECT_DIR && ./data/okb-assist-linux-arm64 [--args]"