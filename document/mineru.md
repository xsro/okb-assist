wget https://gcore.jsdelivr.net/gh/opendatalab/MinerU@master/docker/china/Dockerfile
docker build -t mineru:latest -f Dockerfile .


### 启动 mineru


启动mineru 服务

```bash
# 启动mineru 服务 https://opendatalab.github.io/MinerU/quick_start/docker_deployment/#docker-description
docker run --gpus all \
  --shm-size 32g \
  -d \
  -p 30000:30000 -p 7860:7860 -p 8000:8000 -p 8002:8002 \
  --ipc=host \
  --name mineru \
  -it mineru:latest2 \
  /bin/bash -c "CUDA_VISIBLE_DEVICES=1,2 mineru-router --host 0.0.0.0 --port 8002 --local-gpus auto"
```

如果已经启动过，只是停止了，那么使用`sudo docker start mineru`，如果需要删除旧的那么运行 `docker rm mineru`

```bash
# 在上面的命令启动的终端里面输入 https://opendatalab.github.io/MinerU/usage/quick_usage/#quick-usage-via-command-line
docker run --gpus all \
  --shm-size 32g \
  -d \
  -p 30000:30000 -p 7860:7860 -p 8000:8000 -p 8002:8002 \
  --ipc=host \
  --name mineru-bash \
  -it mineru:latest2 \
  /bin/bash
sudo docker ps
sudo docker exec -it  mineru-bash /bin/bash
CUDA_VISIBLE_DEVICES=2 mineru-api --help # 单GPU
```

ctrl+p 结合 ctrl+q 不杀死的情况下退出

```
mineru-gradio --server-name 0.0.0.0 --api-url http://127.0.0.1:8002 --allow-public-http-client
mineru-gradio --server-name 0.0.0.0 --enable-api true --allow-public-http-client
```
