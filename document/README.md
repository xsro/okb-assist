# OKB-Assist 文档

> 本地学术文献库管理系统 —— 上传 PDF → MinerU 解析 → 模型校对元数据 → 向量索引 → 检索 / 问答。

左侧是完整目录，也可以直接搜索（右上角或侧栏顶部）。所有页面就是本仓库 `document/` 下的普通 Markdown 文件，改完刷新即可看到效果。

## 快速开始

| 步骤 | 操作 |
| --- | --- |
| 1 | 启动后端：`cd backend-rs && cargo run`（默认 `0.0.0.0:5001`） |
| 2 | 浏览器打开 `http://<主机>:5001/document/` |
| 3 | 首次访问会跳转到 `/assist/login`，登录后自动回到这里 |

> 该页面由 `system.json` 的 `static_dirs` 挂载：URL 前缀 `/document` → 仓库 `document/` 目录，`try_index` 命中 `index.html`。

## 使用与功能

<div class="doc-grid">
  <a class="doc-card" href="#/user-management">
    <span class="doc-card__title">用户管理使用指南</span>
    <span class="doc-card__desc">登录、注册、个人令牌（API / MCP）、管理员审核与配额</span>
  </a>
  <a class="doc-card" href="#/drive">
    <span class="doc-card__title">用户网盘</span>
    <span class="doc-card__desc">私有目录 + 公有文件夹，角色阈值授权与配额设计</span>
  </a>
  <a class="doc-card" href="#/search">
    <span class="doc-card__title">高级搜索语法</span>
    <span class="doc-card__desc">多字段筛选、范围查询、逻辑组合，前端解析映射为 SQL</span>
  </a>
  <a class="doc-card" href="#/mcp">
    <span class="doc-card__title">MCP 连接指南</span>
    <span class="doc-card__desc">令牌类型、唯一传输端点（Streamable HTTP）、各客户端配置</span>
  </a>
  <a class="doc-card" href="#/mcp-tools">
    <span class="doc-card__title">MCP 工具列表</span>
    <span class="doc-card__desc">全部只读工具的入参与返回说明</span>
  </a>
  <a class="doc-card" href="#/skills/fix-info">
    <span class="doc-card__title">文献元数据修复</span>
    <span class="doc-card__desc">标题 / 作者 / 期刊 / DOI 解析错误时的排查与修复流程</span>
  </a>
</div>

## 部署与运维

<div class="doc-grid">
  <a class="doc-card" href="#/build">
    <span class="doc-card__title">编译与部署</span>
    <span class="doc-card__desc">前后端构建、系统依赖、产物位置</span>
  </a>
  <a class="doc-card" href="#/udisks2-automount">
    <span class="doc-card__title">U 盘自动挂载</span>
    <span class="doc-card__desc">udisks2 配置：插入即挂载，供文献导入使用</span>
  </a>
  <a class="doc-card" href="#/qdrant">
    <span class="doc-card__title">Qdrant 向量库</span>
    <span class="doc-card__desc">Docker 启动与 dashboard 入口</span>
  </a>
  <a class="doc-card" href="#/openwebui">
    <span class="doc-card__title">Open WebUI</span>
    <span class="doc-card__desc">本地运行的 Web UI 对接方式</span>
  </a>
  <a class="doc-card" href="#/datalab.to">
    <span class="doc-card__title">datalab.to</span>
    <span class="doc-card__desc">第三方文档提取服务的用量与价格</span>
  </a>
</div>

## MinerU 解析

<div class="doc-grid">
  <a class="doc-card" href="#/mineru4/README">
    <span class="doc-card__title">MinerU v4 私有部署</span>
    <span class="doc-card__desc">构建镜像、单卡 / 多卡启动 api-server</span>
  </a>
  <a class="doc-card" href="#/mineru4/mineru-api-server-report">
    <span class="doc-card__title">v1 REST API 调研报告</span>
    <span class="doc-card__desc">基于官方源码梳理的接口与行为</span>
  </a>
  <a class="doc-card" href="#/mineru4/mineru-cloud-api">
    <span class="doc-card__title">官方云 API</span>
    <span class="doc-card__desc">云端 V4 精准解析 / V1 轻量解析，Token 与限频</span>
  </a>
  <a class="doc-card" href="#/mineru4/parse-api-test">
    <span class="doc-card__title">解析接口测试报告</span>
    <span class="doc-card__desc">上传 → 解析 → 下载全流程实测与档位对比</span>
  </a>
</div>

## 设计与计划

<div class="doc-grid">
  <a class="doc-card" href="#/user-management-plan">
    <span class="doc-card__title">用户体系升级计划</span>
    <span class="doc-card__desc">从「静态 Token → 角色」到完整用户体系的决策与进度</span>
  </a>
  <a class="doc-card" href="#/search-filesize-plan">
    <span class="doc-card__title">文件大小筛选（计划）</span>
    <span class="doc-card__desc">高级搜索按 PDF / Markdown / ZIP 大小过滤</span>
  </a>
  <a class="doc-card" href="#/webdav-plan">
    <span class="doc-card__title">网盘 WebDAV 同步（计划）</span>
    <span class="doc-card__desc">方案对比、认证设计（scope=dav）、代价与分阶段 TODO</span>
  </a>
</div>

## 目录结构

```text
document/
├── index.html        # docsify 入口（本页），/document/ 默认首页
├── README.md         # 首页（本文件）
├── _sidebar.md       # 左侧目录
├── _404.md           # 未找到页面
├── assets/           # 站内自有样式与脚本（docsify 运行时走 CDN）
│   ├── css/custom.css
│   └── js/auth-guard.js
├── mineru4/          # MinerU 私有部署 / 云端 API / 测试报告
└── skills/           # 元数据修复等操作手册
```

## 撰写约定

- **新增文档后**，在 `_sidebar.md` 与首页对应分组里各补一条链接，否则只能靠搜索找到。
- 文件名用小写 + 短横线（如 `search-filesize-plan.md`），中文标题写在文件内的 `#` 标题里。
- 页内相对链接写成 `./xxx.md`（`relativePath: true` 已开启）；指向应用页面用 `/assist/...`（已在 `noCompileLinks` 中，走浏览器整页跳转）。
- 子目录（`mineru4/`、`skills/`）各自带一个 `_sidebar.md`，docsify 会就近取用；子目录侧栏里的文档链接写成相对该目录的路径（如 `parse-api-test.md`）。
- 本目录是仓库文档的单一来源，`AGENTS.md` 里的路径引用与之保持一致，**不要随意移动文件**。
