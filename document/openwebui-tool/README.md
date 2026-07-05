# OKB-Assist OpenWebUI 工具

这个工具允许 OpenWebUI 连接到 OKB-Assist 知识库，进行文献搜索和查询。

## 功能

1. **search_knowledge_base** - 在知识库中进行语义搜索
2. **list_documents** - 列出知识库中的文献资料
3. **get_document_detail** - 获取指定文献的详细信息
4. **get_document_links** - 获取指定文献的所有相关链接（详情页、PDF、Markdown）

## 配置

### 环境变量

在 OpenWebUI 中配置以下环境变量：

- `OKB_ASSIST_URL` - OKB-Assist 服务地址（默认：`http://localhost:8000`）
- `OKB_ASSIST_TOKEN` - 访问令牌（如果设置了 `upload_token`）

### 安装步骤

1. 登录 OpenWebUI 管理后台
2. 进入 "Tools" 或 "工具" 页面
3. 点击 "Import Tool" 或 "导入工具"
4. 上传 `okb_assist_tool.py` 文件
5. 配置环境变量

## 使用方法

### 搜索知识库

```
搜索关于"滑模控制"的文献
```

工具会调用 `search_knowledge_base` 函数，返回语义搜索结果。

### 列出文献

```
列出知识库中已索引的文献
```

工具会调用 `list_documents` 函数，返回文献列表。

### 查看文献详情

```
查看ID为710的文献详情
```

工具会调用 `get_document_detail` 函数，返回文献的完整元数据。

### 获取文献链接

```
获取ID为710的文献链接
```

工具会调用 `get_document_links` 函数，返回以下链接：
- 📄 详情页面 - 在浏览器中查看文献完整信息
- 📕 PDF 下载 - 下载或查看 PDF 文件
- 📝 Markdown 内容 - 获取 Markdown 格式内容

## API 端点

工具使用以下 API 端点：

- `GET /assist/api/documents/search?q={query}&limit={limit}` - 语义搜索
- `GET /assist/api/documents/?status_filter={status}&page_size={limit}` - 列出文献
- `GET /assist/api/documents/{doc_id}` - 获取文献详情
- `GET /assist/detail/{doc_id}` - 文献详情页面（浏览器访问）
- `GET /assist/api/documents/{doc_id}/pdf` - PDF 文件下载
- `GET /assist/api/documents/{doc_id}/markdown` - Markdown 内容

## 注意事项

1. 确保 OKB-Assist 服务正在运行
2. 如果设置了访问令牌，需要在环境变量中配置 `OKB_ASSIST_TOKEN`
3. 语义搜索需要文献已经被索引（状态为 `indexed`）
