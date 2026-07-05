### 启动 open webui

本项目的数据很多，使用docker其实有些不太方便，为了更好的性能，我直接逻辑运行。

```bash
export OLLAMA_BASE_URL=http://127.0.0.1:11434
export HF_ENDPOINT=https://hf-mirror.com
export UV_INDEX_URL=https://pypi.tuna.tsinghua.edu.cn/simple/
export ENABLE_KB_EXEC=True
export ENV=dev
export DATA_DIR=~/.open-webui 
uvx --python 3.11 open-webui@latest serve
# 更加推荐
uv tool install --python 3.11 open-webui@latest
open-webui serve
```