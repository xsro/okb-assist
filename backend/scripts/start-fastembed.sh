#!/bin/bash
# 启动 FastEmbed 独立 embedding 服务
# 用法: bash scripts/start-fastembed.sh [--model MODEL_NAME] [--port PORT]

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"

cd "$PROJECT_DIR"

# 默认模型（可通过 --model 参数覆盖）
DEFAULT_MODEL="sentence-transformers/all-MiniLM-L6-v2"

# 从 config.json 读取第一个 fastembed 类型的 embedding 模型
if [ -f config.json ] && command -v python3 &>/dev/null; then
    MODEL_FROM_CONFIG=$(python3 -c "
import json
with open('config.json') as f:
    cfg = json.load(f)
for vdb in cfg.get('vector_dbs', []):
    emb = vdb.get('embedding', {})
    if emb.get('source') == 'fastembed' and emb.get('model'):
        print(emb['model'])
        break
" 2>/dev/null)
    if [ -n "$MODEL_FROM_CONFIG" ]; then
        DEFAULT_MODEL="$MODEL_FROM_CONFIG"
    fi
fi

echo "FastEmbed Server"
echo "  Default model: $DEFAULT_MODEL"
echo "  Port: 8003"
echo ""

exec uv run python scripts/fastembed_server.py --model "$DEFAULT_MODEL" "$@"
