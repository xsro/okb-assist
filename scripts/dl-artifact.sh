#!/usr/bin/env bash
# ============================================================================
# dl-artifact.sh — 从 GitHub Actions 下载最新构建制品
#
# 用法:
#   ./scripts/dl-artifact.sh [artifact-name] [output-dir]
#
# 默认:
#   artifact-name = okb-assist-linux-arm64
#   output-dir    = ./artifacts/
#
# 环境变量:
#   HTTPS_PROXY   — 如 http://127.0.0.1:7899，用于走代理下载
#   YES           — 设为 1 跳过确认，直接下载（非交互/自动化模式）
#
# 依赖: gh (GitHub CLI), 已认证, unzip
# ============================================================================

set -euo pipefail

REPO="xsro/okb-assist"
ARTIFACT_NAME="${1:-okb-assist-linux-arm64}"
OUTPUT_DIR="${2:-./artifacts}"

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

# --- 前置检查 ---
command -v gh &>/dev/null  || { err "gh 未安装"; exit 1; }
command -v unzip &>/dev/null || { err "unzip 未安装"; exit 1; }

# --- 获取最新 Artifact ---
info "查找最新 Artifact: ${YELLOW}${ARTIFACT_NAME}${NC} (repo: ${CYAN}${REPO}${NC})"

# 用 API 列出匹配 artifact，取最新一个
API_URL="repos/${REPO}/actions/artifacts?name=${ARTIFACT_NAME}&per_page=1"
JSON=$(gh api "$API_URL" --jq '.artifacts[0] // empty' 2>/dev/null || true)

if [ -z "$JSON" ] || [ "$JSON" = "null" ]; then
  err "未找到名为 '${ARTIFACT_NAME}' 的 Artifact"
  exit 1
fi

# 解析 JSON 字段（一次 python 调用搞定，避免嵌套对象序列化问题）
read_fields() {
  python3 -c "
import sys, json
d = json.load(sys.stdin)
print(d['id'])
print(d['size_in_bytes'])
print(d['workflow_run']['id'])
print(d['created_at'])
"
}

{
  read_fields_output=$(echo "$JSON" | read_fields)
  ARTIFACT_ID=$(echo "$read_fields_output" | sed -n '1p')
  ARTIFACT_SIZE=$(echo "$read_fields_output" | sed -n '2p')
  RUN_ID=$(echo "$read_fields_output" | sed -n '3p')
  CREATED_AT=$(echo "$read_fields_output" | sed -n '4p')
}

# 格式化大小
size_human() {
  local bytes=$1
  if [ "$bytes" -ge 1048576 ]; then
    echo "$(echo "scale=1; $bytes / 1048576" | bc) MiB"
  elif [ "$bytes" -ge 1024 ]; then
    echo "$(echo "scale=1; $bytes / 1024" | bc) KiB"
  else
    echo "${bytes} B"
  fi
}

# 格式化时间（UTC → 上海时区）
created_local=$(TZ=Asia/Shanghai date -d "${CREATED_AT}" '+%Y-%m-%d %H:%M:%S' 2>/dev/null || echo "${CREATED_AT} UTC")

echo ""
echo -e "  Artifact:   ${CYAN}${ARTIFACT_NAME}${NC}"
echo -e "  大小:       ${YELLOW}$(size_human ${ARTIFACT_SIZE})${NC}"
echo -e "  Run #${RUN_ID}"
echo -e "  构建时间:   ${created_local}"
echo ""

# --- 确认下载 ---
if [ "${YES:-}" != "1" ]; then
  read -rp "确认下载？[Y/n] " CONFIRM
  CONFIRM=${CONFIRM:-Y}
  if [[ ! "$CONFIRM" =~ ^[Yy]$ ]]; then
    info "已取消。"
    exit 0
  fi
else
  info "非交互模式，自动确认下载"
fi

# --- 下载 Artifact (zip) ---
mkdir -p "$OUTPUT_DIR"
TMP_ZIP=$(mktemp --suffix=.zip -p "$OUTPUT_DIR" 2>/dev/null || mktemp -u /tmp/okb-artifact-XXXXX.zip)

info "下载中 ... (可能较慢，请耐心等待)"
DOWNLOAD_URL="repos/${REPO}/actions/artifacts/${ARTIFACT_ID}/zip"

if [ -n "${HTTPS_PROXY:-}" ]; then
  info "使用代理: ${HTTPS_PROXY}"
fi

# 下载，最多重试 3 次
MAX_RETRIES=3
RETRY_DELAY=10
for attempt in $(seq 1 $MAX_RETRIES); do
  if [ "$attempt" -gt 1 ]; then
    warn "重试第 $attempt 次 ..."
    sleep "$RETRY_DELAY"
  fi

  if gh api "$DOWNLOAD_URL" > "$TMP_ZIP"; then
    # 检查是否下载了有效内容
    ZIP_SIZE=$(stat -c%s "$TMP_ZIP" 2>/dev/null || stat -f%z "$TMP_ZIP" 2>/dev/null || echo 0)
    if [ "$ZIP_SIZE" -gt 1000 ]; then
      break
    fi
    warn "下载内容异常（仅 ${ZIP_SIZE} 字节），重试 ..."
  fi

  # 最后一次还失败则退出
  if [ "$attempt" -eq "$MAX_RETRIES" ]; then
    err "下载失败，已重试 ${MAX_RETRIES} 次。"
    err "可尝试: export HTTPS_PROXY=http://127.0.0.1:7899"
    exit 1
  fi
done

# --- 解压 ---
info "解压到 ${OUTPUT_DIR}/"
unzip -o "$TMP_ZIP" -d "$OUTPUT_DIR"
rm -f "$TMP_ZIP"

echo ""

# 找到解压后的二进制文件
BINARY=$(find "$OUTPUT_DIR" -maxdepth 1 -type f -name "${ARTIFACT_NAME}" -print -quit 2>/dev/null || true)

if [ -n "$BINARY" ] && [ -f "$BINARY" ]; then
  chmod +x "$BINARY"
  REAL_SIZE=$(stat -c%s "$BINARY" 2>/dev/null || stat -f%z "$BINARY" 2>/dev/null)

  ok "下载完成！"
  echo -e "  路径:      ${GREEN}${BINARY}${NC}"
  echo -e "  大小:      ${YELLOW}$(size_human "${REAL_SIZE}")${NC}"
  echo ""
  file "$BINARY"
  echo ""
  info "使用方式:"
  echo "  ${BINARY} [--args]"
else
  ok "下载完成，文件位于: ${OUTPUT_DIR}/"
  ls -lh "$OUTPUT_DIR/"
fi