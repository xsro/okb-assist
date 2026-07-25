```
docker pull docker.1ms.run/qdrant/qdrant:latest
docker run -p 6333:6333 -v $(pwd)/qdrant_data:/qdrant/storage docker.1ms.run/qdrant/qdrant:latest
```

可以访问 `http://192.168.1.183:6333/dashboard` 管理向量数据库


```
cd data
qdrant
```