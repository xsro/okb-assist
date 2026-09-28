# Mineru v4 (api v1) 服务器

follow `https://opendatalab.github.io/MinerU/quick_start/docker_deployment/`

## 构建docker image

build `https://github.com/opendatalab/MinerU/blob/master/docker/china/Dockerfile`

```
sudo docker build -t mineru4 -f Dockerfile .
```

## 以image 运行 container（单卡）

```
docker run --gpus all --shm-size 32g --ipc=host  -p 8000:8000 -d --name mineru-api  mineru4 mineru-kit api-server --host 0.0.0.0 --port 8000
```

## 以image 运行 container（多卡）

如果已经启动过，只是停止了，那么使用`sudo docker start mineru-api`，如果需要删除旧的那么运行 `docker rm mineru-api`
如果要删除image 就 `docker rmi mineru4`

```
docker run --gpus all  --shm-size 32g \
  -d \
  -p 8000:8000 -p 8002:8002 \
  --ipc=host \
  --name mineru-router \
  -it mineru4 \
  /bin/bash -c "CUDA_VISIBLE_DEVICES=0,1 mineru-kit router --host 0.0.0.0 --port 8002 --local-gpus 0,1  --preload-models"
```

## 登录到 container

```bash
sudo docker exec -it  mineru-api /bin/bash
mineru-kit --help #
```

ctrl+p 结合 ctrl+q 不杀死的情况下退出

## 查看日志

```
# 查看全部日志
sudo docker logs mineru-api

# 实时跟踪日志（最常用，Ctrl+C退出）
sudo docker logs -f mineru-api

# 实时跟踪 + 显示时间戳
sudo docker logs -f -t mineru-api

# 只看最后20行
sudo docker logs --tail=20 mineru-api

# 查看最近30分钟日志
sudo docker logs --since 30m mineru-api
```


