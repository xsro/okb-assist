follow `https://opendatalab.github.io/MinerU/quick_start/docker_deployment/`


build `https://github.com/opendatalab/MinerU/blob/master/docker/china/Dockerfile`

```
sudo docker build -t mineru4 -f Dockerfile .
```

```
docker run --gpus all --shm-size 32g --ipc=host  -p 8000:8000 -d --name mineru-api  mineru4 mineru-kit api-server --host 0.0.0.0 --port 8000
```


如果已经启动过，只是停止了，那么使用`sudo docker start mineru-api`，如果需要删除旧的那么运行 `docker rm mineru-api`
如果要删除image 就 `docker rmi mineru4`

```bash
# 在上面的命令启动的终端里面输入 https://opendatalab.github.io/MinerU/usage/quick_usage/#quick-usage-via-command-line
docker run --gpus all \
  --shm-size 32g \
  -d \
  -p 30000:30000 -p 7860:7860 -p 8000:8000 -p 8002:8002 \
  --ipc=host \
  --name mineru-bash \
  -it mineru4 \
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
