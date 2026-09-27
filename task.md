帮我写一个python辅助脚本，放在scripts中。

- 从` --base-url http://192.168.1.122:5001 --token 60b8be6583ac41abb3af63e01151bd86` 中的文献服务器中获取状态为uploading或者failed的文献，
打印文献id和mineru_id。
- 然后尝试从http://192.168.1.185:8002/ 上的mineru中获取到结果，并下载，如果下载data/mineru_result文件夹中。
- 如果data/mineru_result 中已有结果，则无需下载。
- 然后询问用户是否需要将结果上传到文献服务器中。