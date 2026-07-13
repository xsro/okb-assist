#!/bin/bash


export OLLAMA_BASE_URL=http://192.168.1.185:11434
export HF_ENDPOINT=https://hf-mirror.com
export UV_INDEX_URL=https://pypi.tuna.tsinghua.edu.cn/simple/
export ENABLE_KB_EXEC=True
# export ENV=dev
export DATA_DIR=~/.open-webui 
cd ~
uvx --python 3.11 open-webui@latest serve


