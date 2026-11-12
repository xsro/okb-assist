#!/usr/bin/env bash
# okb-after-mount.sh — 插入外置盘后的收尾脚本（挂载 → 启动后端 → 同步文件事实 → 补哈希 → PDF 健康检查）
#
# 背景：文献库的 PDF / 图片资源包 / 网盘都放在外置盘（system.json 的 pdf_path、markdown_asset_path、
#      drive_root），而 Markdown 与数据库在系统盘。盘没挂载时后端仍能跑，但会把这些文件记成「不存在」，
#      因此插盘之后需要：挂载 → 让后端重新 stat 一遍文件大小 → 补 file_hash → 跑 PDF 健康检查。
#
# 用法：
#   scripts/okb-after-mount.sh                # 完整流程（已挂载/已运行的部分会自动跳过）
#   scripts/okb-after-mount.sh --skip-mount   # 盘已挂载，只做启动与维护
#   scripts/okb-after-mount.sh --only-mount   # 只挂载外置盘（校验目录可见后退出），其余全部跳过
#   scripts/okb-after-mount.sh --no-start     # 不启动后端，只做挂载与检查
#   scripts/okb-after-mount.sh --skip-health  # 跳过 PDF 健康检查（耗时较长）
#   scripts/okb-after-mount.sh -h             # 帮助
#
# 环境变量（都可选）：
#   OKB_SYSTEM_JSON  system.json 路径         默认 ~/.okb/system.json
#   OKB_BIN          后端可执行文件           默认 ~/.okb/versions/latest/okb-assist
#   OKB_BASE_URL     后端地址                 默认 http://127.0.0.1:5001
#   OKB_DEVICE       分区设备                 默认按卷标自动探测（blkid -L CCSICC）
#   OKB_MOUNT_POINT  挂载点                   默认从 pdf_path 推出 /media/<用户>/<卷标>
#   OKB_TOKEN        管理员会话/API 令牌      给了解析后的维护任务直接用它，不再要求密码
#   OKB_USERNAME     管理员用户名             默认 admin
#   OKB_PASSWORD     管理员密码               不给就在终端提示输入
#
# 说明：脚本不会删除任何数据；三个维护任务都是「只补缺失」（only_missing=1），重复执行安全。
set -Eeuo pipefail

# ────────────────────────── 参数 ──────────────────────────
SKIP_MOUNT=0
ONLY_MOUNT=0
NO_START=0
SKIP_HASH=0
SKIP_HEALTH=0

usage() {
  awk 'NR > 1 { if ($0 ~ /^set -Eeuo/) exit; print }' "$0" | sed -e 's/^# \{0,1\}//' -e '/^$/d'
  exit 0
}

while [ $# -gt 0 ]; do
  case "$1" in
    -h|--help) usage ;;
    --skip-mount) SKIP_MOUNT=1 ;;
    --only-mount) ONLY_MOUNT=1 ;;
    --no-start) NO_START=1 ;;
    --skip-hash) SKIP_HASH=1 ;;
    --skip-health) SKIP_HEALTH=1 ;;
    *) echo "未知参数: $1（用 -h 查看帮助）" >&2; exit 2 ;;
  esac
  shift
done

if [ "$ONLY_MOUNT" -eq 1 ] && [ "$SKIP_MOUNT" -eq 1 ]; then
  echo "--only-mount 与 --skip-mount 不能同时使用" >&2
  exit 2
fi

# ────────────────────────── 输出 ──────────────────────────
if [ -t 1 ]; then
  C_R=$'\e[31m'; C_G=$'\e[32m'; C_Y=$'\e[33m'; C_B=$'\e[36m'; C_0=$'\e[0m'
else
  C_R=''; C_G=''; C_Y=''; C_B=''; C_0=''
fi
step() { printf '\n%s==> %s%s\n' "$C_B" "$*" "$C_0"; }
ok()   { printf '  %s✓%s %s\n' "$C_G" "$C_0" "$*"; }
warn() { printf '  %s!%s %s\n' "$C_Y" "$C_0" "$*"; }
die()  { printf '  %s✗ %s%s\n' "$C_R" "$*" "$C_0" >&2; exit 1; }
info() { printf '    %s\n' "$*"; }

trap 'printf "\n%s脚本在第 %s 行异常退出（退出码 %s）%s\n" "$C_R" "$LINENO" "$?" "$C_0" >&2' ERR

need() { command -v "$1" >/dev/null 2>&1 || die "缺少命令: $1"; }

# ──────────────────── 读 system.json 推导路径 ────────────────────
SYSTEM_JSON="${OKB_SYSTEM_JSON:-$HOME/.okb/system.json}"
[ -f "$SYSTEM_JSON" ] || die "找不到 system.json: $SYSTEM_JSON"

need python3
need curl
need sqlite3

eval "$(python3 - "$SYSTEM_JSON" <<'PY'
import json, os, shlex, sys

p = sys.argv[1]
try:
    s = json.load(open(p, encoding='utf-8'))
except Exception as e:  # noqa: BLE001
    print(f"echo '解析 {p} 失败: {e}' >&2; exit 1")
    raise SystemExit(0)

system_dir = os.path.dirname(os.path.abspath(p))

def expand(v: str) -> str:
    v = v.replace('{system_dir}', system_dir)
    v = os.path.expanduser(os.path.expandvars(v))
    return v

def resolve(v: str, base: str) -> str:
    v = expand(v)
    return v if os.path.isabs(v) else os.path.normpath(os.path.join(base, v))

cwd = resolve(s.get('cwd') or system_dir, system_dir)
db_url = s.get('database_url') or 'sqlite:///data/okb_assist.db'
if db_url.startswith('sqlite://'):
    rest = db_url[len('sqlite://'):]
    rest = rest[1:] if rest.startswith('/') else rest
elif db_url.startswith('sqlite:'):
    rest = db_url[len('sqlite:'):]
else:
    rest = db_url
rest = rest.split('?', 1)[0]
db = rest if os.path.isabs(rest) else os.path.normpath(os.path.join(cwd, rest))

md_tpl = expand(s.get('markdown_path') or 'data/markdowns/{id}.md').replace('{id}', '1')
pdf_tpl = expand(s.get('pdf_path') or 'data/pdfs/{id}/{id}.pdf').replace('{id}', '1')
md_dir = os.path.dirname(os.path.abspath(md_tpl))
pdf_dir = os.path.dirname(os.path.dirname(os.path.abspath(pdf_tpl)))
drive = expand(s.get('drive_root') or 'data/drive')
if not os.path.isabs(drive):
    drive = os.path.normpath(os.path.join(cwd, drive))
cfg = expand(s.get('config_path') or 'config.json')
if not os.path.isabs(cfg):
    cfg = os.path.normpath(os.path.join(system_dir, cfg))
ui = expand(s.get('ui_path') or os.path.join(system_dir, 'frontend/dist'))
log_path = expand(s.get('log_path') or 'okb-log.jsonl')
if not os.path.isabs(log_path):
    log_path = os.path.normpath(os.path.join(cwd, log_path))

# 挂载点：pdf 目录形如 /media/<用户>/<卷标>/... 时取前三级
mp = ''
parts = pdf_dir.split(os.sep)
if len(parts) >= 4 and parts[1] == 'media' and not os.path.isdir('/media/' + parts[2]):
    mp = os.sep.join(parts[:4])
elif len(parts) >= 4 and parts[1] == 'media':
    mp = os.sep.join(parts[:4])

for k, v in dict(
    OKB_CWD=cwd, OKB_DB=db, OKB_MD_DIR=md_dir, OKB_PDF_DIR=pdf_dir, OKB_DRIVE=drive,
    OKB_CONFIG=cfg, OKB_UI=ui, OKB_LOG=log_path, OKB_MOUNT_HINT=mp,
).items():
    print(f'{k}={shlex.quote(v)}')
PY
)"

OKB_CWD="${OKB_CWD%/}"
BASE_URL="${OKB_BASE_URL:-http://127.0.0.1:5001}"
BASE_URL="${BASE_URL%/}"
USERNAME="${OKB_USERNAME:-admin}"
MOUNT_POINT="${OKB_MOUNT_POINT:-${OKB_MOUNT_HINT:-}}"
LOG_FILE="${OKB_LOG:-$HOME/.okb/okbdata/okb-log.jsonl}"
PID_FILE="$(dirname "$OKB_DB")/backend.pid"

dbq() { sqlite3 -noheader "$OKB_DB" "$1" 2>/dev/null | tr -d '[:space:]'; }
num() { local v; v="$(dbq "$1")"; printf '%s' "${v:-0}"; }

# ────────────────────────── 0. 概览 ──────────────────────────
step "配置"
info "system.json : $SYSTEM_JSON"
info "工作目录    : $OKB_CWD"
info "数据库      : $OKB_DB"
info "Markdown    : $OKB_MD_DIR"
info "PDF         : $OKB_PDF_DIR"
info "网盘        : $OKB_DRIVE"
info "后端        : $BASE_URL"

if [ "$ONLY_MOUNT" -eq 1 ]; then
  info "模式        : --only-mount（只挂载，不碰数据库与后端）"
else
  [ -f "$OKB_DB" ] || die "数据库不存在: $OKB_DB"
  integrity="$(sqlite3 "$OKB_DB" 'PRAGMA integrity_check;' 2>&1 | head -1)"
  [ "$integrity" = "ok" ] || die "数据库完整性检查失败: $integrity（先别启动后端，检查文件）"
  DOCS="$(num 'select count(*) from documents')"
  ok "数据库完整性 ok，共 $DOCS 篇文献"
fi

# ────────────────────────── 1. 挂载外置盘 ──────────────────────────
if [ "$SKIP_MOUNT" -eq 1 ]; then
  step "跳过挂载（--skip-mount）"
else
  step "挂载外置盘"
  if [ -n "$MOUNT_POINT" ] && mountpoint -q "$MOUNT_POINT" 2>/dev/null; then
    ok "已挂载: $MOUNT_POINT（$(findmnt -no SOURCE,FSTYPE -T "$MOUNT_POINT" 2>/dev/null)）"
  else
    [ -n "$MOUNT_POINT" ] || MOUNT_POINT="/media/$(id -un)/CCSICC"
    DEV="${OKB_DEVICE:-}"
    if [ -z "$DEV" ] && command -v blkid >/dev/null 2>&1; then
      DEV="$(blkid -L "$(basename "$MOUNT_POINT")" 2>/dev/null || true)"
    fi
    if [ -z "$DEV" ]; then
      for cand in /dev/sda1 /dev/sdb1 /dev/sdc1; do
        [ -b "$cand" ] && { DEV="$cand"; break; }
      done
    fi
    [ -n "$DEV" ] || die "找不到分区设备（盘没插好？）：$MOUNT_POINT"
    info "设备 $DEV → 挂载点 $MOUNT_POINT"
    if command -v udisksctl >/dev/null 2>&1 && udisksctl mount -b "$DEV" 2>/dev/null; then
      ok "udisksctl 挂载成功"
    else
      mkdir -p "$MOUNT_POINT" 2>/dev/null || true
      if sudo -n true 2>/dev/null; then
        sudo mount -t exfat -o "uid=$(id -u),gid=$(id -g),fmask=0022,dmask=0022,iocharset=utf8" "$DEV" "$MOUNT_POINT" \
          || die "挂载失败: $DEV → $MOUNT_POINT"
      else
        info "需要 sudo 权限挂载，请输入密码："
        sudo mount -t exfat -o "uid=$(id -u),gid=$(id -g),fmask=0022,dmask=0022,iocharset=utf8" "$DEV" "$MOUNT_POINT" \
          || die "挂载失败: $DEV → $MOUNT_POINT"
      fi
      ok "挂载成功"
    fi
  fi
fi

# ────────────────── 1.5 只挂载模式：校验后直接退出 ──────────────────
if [ "$ONLY_MOUNT" -eq 1 ]; then
  step "挂载结果"
  if [ -n "$MOUNT_POINT" ]; then
    mountpoint -q "$MOUNT_POINT" 2>/dev/null \
      || die "挂载点未生效: $MOUNT_POINT"
    ok "已挂载: $MOUNT_POINT（$(findmnt -no SOURCE,FSTYPE -T "$MOUNT_POINT" 2>/dev/null)）"
  else
    warn "没能从 pdf_path 推出挂载点；用 OKB_MOUNT_POINT=/media/<用户>/<卷标> 重跑可校验"
  fi
  [ -d "$OKB_PDF_DIR" ] && ok "PDF 目录可见: $OKB_PDF_DIR" || warn "PDF 目录不可见: $OKB_PDF_DIR"
  [ -d "$OKB_DRIVE" ] && ok "网盘目录可见: $OKB_DRIVE" || warn "网盘目录不可见: $OKB_DRIVE"
  info "后端启动 / 同步文件事实 / 补哈希 / PDF 健康检查均已跳过"
  info "需要时再跑：scripts/okb-after-mount.sh --skip-mount"
  exit 0
fi

# ────────────────────────── 2. 文件与目录检查 ──────────────────────────
step "检查数据目录"
MD_COUNT=0
if [ -d "$OKB_MD_DIR" ]; then
  MD_COUNT="$(find "$OKB_MD_DIR" -maxdepth 1 -name '*.md' | wc -l | tr -d ' ')"
  ok "Markdown 目录: $MD_COUNT 个 .md"
else
  warn "Markdown 目录不存在: $OKB_MD_DIR"
fi

if [ -d "$OKB_PDF_DIR" ]; then
  PDF_COUNT="$(find "$OKB_PDF_DIR" -maxdepth 2 -name '*.pdf' 2>/dev/null | wc -l | tr -d ' ')"
  ok "PDF 目录: $PDF_COUNT 个 .pdf"
  [ -w "$OKB_PDF_DIR" ] || warn "PDF 目录不可写（挂载参数少了 uid/gid？）"
else
  warn "PDF 目录不存在: $OKB_PDF_DIR（未挂载或路径不对，哈希与健康检查会跳过）"
  PDF_COUNT=0
fi

if [ -d "$OKB_DRIVE" ]; then
  ok "网盘目录存在: $OKB_DRIVE"
else
  warn "网盘目录不存在: $OKB_DRIVE（网盘页面会报错）"
fi

TOKEN_DIR="$OKB_CWD/data/tokens"
if [ -d "$TOKEN_DIR" ] && [ -n "$(ls -A "$TOKEN_DIR" 2>/dev/null)" ]; then
  ok "密钥目录: $TOKEN_DIR"
else
  warn "密钥目录不存在或为空: $TOKEN_DIR（config.json 里 '!cat ./data/tokens/xxx' 会解析为空，LLM/MinerU 不可用）"
fi

[ -f "$OKB_CONFIG" ] || warn "config.json 不存在: $OKB_CONFIG（后端会用默认配置，用户/向量库设置丢失）"
[ -d "$OKB_UI" ] || warn "前端产物不存在: $OKB_UI"

# ────────────────────────── 3. 后端 ──────────────────────────
backend_up() { curl -fsS -m 3 "$BASE_URL/assist/api/auth/status" >/dev/null 2>&1; }

step "后端状态"
if backend_up; then
  ok "已在运行: $BASE_URL"
else
  if [ "$NO_START" -eq 1 ]; then
    die "后端未运行，且指定了 --no-start"
  fi
  BIN="${OKB_BIN:-$HOME/.okb/versions/latest/okb-assist}"
  if [ ! -x "$BIN" ]; then
    for cand in "$HOME/.okb/versions/latest/okb-assist" \
                "$(dirname "$0")/../backend-rs/target/release/okb_assist" \
                "$(dirname "$0")/../backend-rs/target/debug/okb_assist"; do
      [ -x "$cand" ] && { BIN="$cand"; break; }
    done
  fi
  [ -x "$BIN" ] || die "找不到后端可执行文件（试过 OKB_BIN、~/.okb/versions/latest/okb-assist、backend-rs/target/*）；可用 scripts/okb-version-manager.py 安装"
  info "启动: $BIN --system-path $SYSTEM_JSON"
  nohup "$BIN" --system-path "$SYSTEM_JSON" >>"$LOG_FILE" 2>&1 &
  echo $! >"$PID_FILE"
  ok "已拉起，PID $(cat "$PID_FILE")，日志 $LOG_FILE"
fi

step "等待后端就绪"
for i in $(seq 1 60); do
  backend_up && break
  sleep 1
  [ "$i" -eq 60 ] && die "60 秒内后端未就绪，看日志: $LOG_FILE"
done
ok "HTTP 就绪"

# ───────────────────── 4. 登录（维护任务需要 admin） ─────────────────────
TOKEN="${OKB_TOKEN:-}"
if [ -z "$TOKEN" ]; then
  step "登录管理员账号（$USERNAME）"
  PASS="${OKB_PASSWORD:-}"
  if [ -z "$PASS" ] && [ -t 0 ]; then
    printf '  密码（不显示）: '
    read -rs PASS
    printf '\n'
  fi
  if [ -n "$PASS" ]; then
    resp="$(curl -sS -m 20 -X POST "$BASE_URL/assist/api/auth/login" \
      -H 'Content-Type: application/json' \
      --data "$(python3 -c 'import json,sys;print(json.dumps({"username":sys.argv[1],"password":sys.argv[2]}))' "$USERNAME" "$PASS")" 2>/dev/null || true)"
    TOKEN="$(printf '%s' "$resp" | python3 -c 'import json,sys
try:
    print(json.load(sys.stdin).get("token") or "")
except Exception:
    print("")' 2>/dev/null)"
    unset PASS
  fi
  if [ -n "$TOKEN" ]; then
    ok "登录成功"
  else
    warn "没有拿到凭证（没输入密码 / 密码错误）。跳过需要 admin 的维护步骤。"
    info "也可以：export OKB_TOKEN=<会话或 API 令牌> 后重跑，或在网页「账户 / 管理」页手动执行同样操作。"
  fi
else
  ok "使用 OKB_TOKEN 环境变量"
fi

api_post() {  # $1=路径  $2=说明
  local out
  out="$(curl -sS -m 30 -X POST "$BASE_URL$1" -H "X-Token: $TOKEN" -H 'Content-Type: application/json' --data '{}' 2>&1 || true)"
  case "$out" in
    *detail*权限*|*Unauthorized*|*401*|*mcp_only_token*) warn "$2 被拒绝: $out"; return 1 ;;
    '') warn "$2 无响应"; return 1 ;;
  esac
  info "$2 → $(printf '%s' "$out" | head -c 200)"
}

# ───────────────── 5. 同步文件事实（pdf/md/zip 大小） ─────────────────
step "同步文件事实（pdf_size / md_size / zip_size）"
null_before="$(num 'select count(*) from documents where fs_synced_at is null')"
pdf_before="$(num 'select count(*) from documents where pdf_size is not null')"
info "同步前：pdf_size 有值 $pdf_before 篇，未同步 $null_before 篇"
if [ -n "$TOKEN" ]; then
  if api_post '/assist/api/admin/sync-file-sizes?only_missing=1' 'POST sync-file-sizes'; then
    # 后端逐条 stat，用「pdf_size 有值的篇数连续 3 次不变」判定跑完
    prev=-1; stable=0
    for _ in $(seq 1 300); do
      have="$(num 'select count(*) from documents where pdf_size is not null')"
      printf '\r    pdf_size 有值 %s 篇' "$have"
      if [ "$have" = "$prev" ]; then stable=$((stable + 1)); else stable=0; fi
      [ "$stable" -ge 3 ] && break
      prev="$have"
      sleep 3
    done
    printf '\n'
  fi
else
  warn "跳过（没有凭证）"
fi
null_after="$(num 'select count(*) from documents where fs_synced_at is null')"
pdf_after="$(num 'select count(*) from documents where pdf_size is not null')"
zip_after="$(num 'select count(*) from documents where zip_size is not null')"
ok "pdf_size 有值 $pdf_after 篇（盘上没 PDF 的会是 0/NULL），zip_size 有值 $zip_after 篇，未同步 $null_after 篇"
if [ "$pdf_after" -eq 0 ] && [ -n "$TOKEN" ]; then
  warn "同步跑完了 pdf_size 还是全空：核对 $OKB_PDF_DIR 下是否真有 {id}/{id}.pdf（当前 $PDF_COUNT 个）"
fi

# ───────────────────────── 6. 补 file_hash ─────────────────────────
if [ "$SKIP_HASH" -eq 0 ]; then
  step "补算缺失的 file_hash"
  miss="$(num 'select count(*) from documents where file_hash is null')"
  info "缺失 $miss 篇"
  if [ "$miss" -eq 0 ]; then
    ok "无需处理"
  elif [ -z "$TOKEN" ]; then
    warn "跳过（没有凭证）。可在网页「管理 → 维护操作 → 补充缺失哈希」手动触发"
  else
    if api_post '/assist/api/admin/recalculate-hashes?only_missing=1' 'POST recalculate-hashes'; then
      for _ in $(seq 1 720); do
        left="$(num 'select count(*) from documents where file_hash is null')"
        printf '\r    剩余缺失 %s 篇' "$left"
        [ "$left" = "0" ] && break
        sleep 5
      done
      printf '\n'
      ok "file_hash 有值 $(num 'select count(*) from documents where file_hash is not null') 篇"
    fi
  fi
else
  step "跳过 file_hash（--skip-hash）"
fi

# ─────────────────────── 7. PDF 健康检查 ───────────────────────
if [ "$SKIP_HEALTH" -eq 0 ]; then
  step "PDF 健康检查（逐个读文件头尾 + mutool 诊断，4000 篇约需十几分钟）"
  miss="$(num 'select count(*) from documents where pdf_health is null')"
  info "未检查 $miss 篇"
  if [ "$miss" -eq 0 ]; then
    ok "无需处理"
  elif [ -z "$TOKEN" ]; then
    warn "跳过（没有凭证）。可在网页「管理 → 维护操作 → PDF 健康检查」手动触发"
  else
    if api_post '/assist/api/admin/check-pdf-health?only_missing=1' 'POST check-pdf-health'; then
      for _ in $(seq 1 1440); do
        left="$(num 'select count(*) from documents where pdf_health is null')"
        printf '\r    剩余未检查 %s 篇' "$left"
        [ "$left" = "0" ] && break
        sleep 5
      done
      printf '\n'
      info "结果: $(sqlite3 -noheader "$OKB_DB" "select coalesce(pdf_health,'未检查')||'='||count(*) from documents group by 1 order by 2 desc" | paste -sd ' ' -)"
    fi
  fi
else
  step "跳过 PDF 健康检查（--skip-health）"
fi

# ───────────────────────── 8. 收尾报告 ─────────────────────────
step "结果"
sqlite3 -noheader "$OKB_DB" "
select '  文档总数      : '||count(*) from documents;
select '  有 PDF        : '||count(*) from documents where pdf_size>0;
select '  有 Markdown   : '||count(*) from documents where md_size>0;
select '  有资源包      : '||count(*) from documents where zip_size>0;
select '  file_hash     : '||count(*) from documents where file_hash is not null;
select '  PDF 健康 ok   : '||count(*) from documents where pdf_health='ok';
select '  PDF 健康 警告 : '||count(*) from documents where pdf_health='warning';
select '  PDF 健康 错误 : '||count(*) from documents where pdf_health='error';
" | sed 's/^ *//'

cat <<EOF

  下一步（按需）：
  1. 语义搜索：向量库（Qdrant）里若没有旧数据，索引状态是假的（库里标着 indexed）。
     请在网页「向量库管理」重置后重建索引。
  2. 网盘：$OKB_DRIVE 现在应可用；若报错看后端日志 $LOG_FILE。
  3. 密钥：把 mineru / datalab / 模型 key 放回 $TOKEN_DIR，否则解析与元数据抽取会失败。
  4. 浏览器打开： $BASE_URL/assist
EOF
