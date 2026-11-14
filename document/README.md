# OKB-Assist 开发文档

> 本地学术文献库管理系统 —— 上传 PDF → MinerU 解析 → 模型校对元数据 → 向量索引 → 检索 / 问答。

**这里只放开发 / 部署 / 设计类文档。** 面向使用者的操作说明（登录与令牌、网盘、搜索语法、MCP）
已经迁到应用内的「文档」页：`/assist/docs`（内容文件在仓库 `frontend/public/docs/`）。

## 快速开始

| 步骤 | 操作 |
| --- | --- |
| 1 | 启动后端：`cd backend-rs && cargo run`（默认 `0.0.0.0:5001`） |
| 2 | 浏览器打开 `http://<主机>:5001/document/`（开发文档）/ `http://<主机>:5001/assist/docs`（用户文档） |
| 3 | 首次访问会跳转到 `/assist/login`，登录后自动回到这里 |

> 本页由 `system.json` 的 `static_dirs` 挂载：URL 前缀 `/document` → 仓库 `document/` 目录，`try_index` 命中 `index.html`。

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
  <a class="doc-card" href="#/skills/fix-info">
    <span class="doc-card__title">文献元数据修复</span>
    <span class="doc-card__desc">标题 / 作者 / 期刊 / DOI 解析错误时的排查与修复流程</span>
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
  <a class="doc-card" href="#/webdav-plan">
    <span class="doc-card__title">网盘 WebDAV 同步（计划）</span>
    <span class="doc-card__desc">方案对比、认证设计（scope=dav）、代价与分阶段 TODO</span>
  </a>
  <a class="doc-card" href="#/db-status-removal-plan">
    <span class="doc-card__title">文献状态去库化（计划）</span>
    <span class="doc-card__desc">删除 documents.status，状态改由文件事实 + 运行时注册表推导</span>
  </a>
</div>

## 两套文档的分工

| | 开发文档（本目录） | 用户文档（`frontend/public/docs/`） |
| --- | --- | --- |
| 位置 | `document/`（仓库根） | `frontend/public/docs/` |
| 访问方式 | `/document/`（docsify 站点，`static_dirs` 挂载） | 应用内 `/assist/docs`（需登录），原文可直链 `/assist/docs/<file>.md` |
| 内容 | 编译部署、MinerU 接入、运维手册、设计与计划 | 登录与令牌、网盘、搜索语法、MCP 连接与工具列表 |
| 索引/顺序 | `_sidebar.md` + 本页卡片 | `frontend/src/docs/userDocs.ts` 的 `DOC_GROUPS` |

```text
document/                     # 开发文档（docsify）
├── index.html                # docsify 入口，/document/ 默认首页
├── README.md                 # 首页（本文件）
├── _sidebar.md               # 左侧目录
├── _404.md                   # 未找到页面
├── assets/                   # 站内自有样式与脚本（docsify 运行时走 CDN）
├── mineru4/                  # MinerU 私有部署 / 云端 API / 测试报告
└── skills/                   # 元数据修复等操作手册

frontend/public/docs/         # 用户文档（应用内 /assist/docs）
├── user-management.md        # 用户与令牌
├── drive.md                  # 网盘
├── search.md                 # 高级搜索语法
├── mcp.md                    # MCP 连接指南
└── mcp-tools.md              # MCP 工具列表
```

## 撰写约定

- **判断归属**：讲「用户怎么操作」→ `frontend/public/docs/`；讲「怎么部署 / 为什么这样设计 / 运维排查」→ `document/`。
- **开发文档新增后**，在 `_sidebar.md` 与首页对应分组里各补一条链接，否则只能靠搜索找到。
- **用户文档新增后**，在 `frontend/src/docs/userDocs.ts` 的 `DOC_GROUPS` 里加一条
  `{ slug, file, title, desc }`，并把 Markdown 放进 `frontend/public/docs/`（`pnpm run build` 会一起复制到 `dist/docs/`）。
  站内互链写成 `./other.md`（应用会自动转成 `/assist/docs/other` 路由），不要写绝对文件路径。
- 文件名用小写 + 短横线（如 `db-status-removal-plan.md`），中文标题写在文件内的 `#` 标题里。
- 页内相对链接写成 `./xxx.md`（`relativePath: true` 已开启）；指向应用页面用 `/assist/...`（已在 `noCompileLinks` 中，走浏览器整页跳转）。
- 子目录（`mineru4/`、`skills/`）各自带一个 `_sidebar.md`，docsify 会就近取用；子目录侧栏里的文档链接写成相对该目录的路径（如 `parse-api-test.md`）。
- 两处目录都是仓库文档的单一来源，`AGENTS.md` 里的路径引用与之保持一致，**不要随意移动文件**。
