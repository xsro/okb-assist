# 网盘 WebDAV 同步（设计与 TODO）

> 目标：让用户可以用标准 WebDAV 客户端（macOS Finder、Windows 资源管理器、rclone、Cyberduck、davfs2…）
> **挂载并同步**自己的私有网盘，同时**只读**浏览可访问的公有文件夹。
>
> 状态：**方案评估完成，尚未实现**。已确定的两项决策见下方「已定决策」。

## 已定决策

| 决策项 | 结论 |
|---|---|
| 认证方式 | **新增 WebDAV 专用令牌作用域 `scope = dav`**（不复用登录密码，也不与 `api` / `mcp` 混用） |
| 首期范围 | **私有盘读写 + 公有文件夹只读**（公有盘写入留到后续阶段） |
| 实现路线 | 见「方案对比」，待选（推荐方案 B） |

## 现状与缺口（代码级）

### 可直接复用

| 能力 | 位置 |
|---|---|
| 作用域解析 + 角色阈值读写判定 | `backend-rs/src/services/drive.rs:374` `resolve_scope` |
| 路径安全（拒 `..`/反斜杠/控制字符、canonicalize 防符号链接逃逸） | `drive.rs:196` `validate_name`、`drive.rs:238` `resolve_path` |
| 目录列举、Content-Type 推断 | `drive.rs:471` `list_dir`、`content_type_for` |
| 配额与大小限制 | `drive.rs` `quota_bytes` / `check_size` / `check_quota` |
| 「非 `/assist/api/*` 路径自行鉴权」的范式 | `backend-rs/src/static_files.rs`（`token_middleware` 不覆盖这些路径） |
| 令牌哈希查找（sha256，近乎零成本） | `backend-rs/src/users.rs:587` `find_by_api_token` |

### 必须新写或不可复用

| 缺口 | 原因 |
|---|---|
| `drive.rs:513` `unique_child` 不能用于 WebDAV 写入 | 现有 rename/move/upload 遇同名会**自动加 `(1)`**；WebDAV 语义是 `Overwrite: T` 覆盖、否则 412/409。同步客户端看到名字被改会重复上传 |
| `drive.rs:452` `used_bytes` 每次全目录 walk | 同步场景几百个小 PUT 会把 O(n) 放大成 O(n²)，需要用量缓存/增量记账 |
| Basic 认证 + `401 WWW-Authenticate` 挑战 | 全新模块，且失败尝试不能计入账号锁定（否则桌面端令牌过期一次就锁号） |
| `main.rs:552` `spa_fallback` 前缀白名单 | 不加 `dav/`，`/assist/dav/xxx` 未命中会返回 `index.html` |
| 前端「WebDAV 连接」面板 + 文档 | 需要展示 URL、用户名、令牌类型与只读/可写范围 |

## 方案对比

### 方案 A：自研 WebDAV 子集 handler（axum 原生）

自己实现 `OPTIONS / PROPFIND / GET / HEAD / PUT / DELETE / MKCOL / MOVE / COPY / LOCK / UNLOCK`。

- **成本**：约 1200–1800 行 Rust，零新依赖
- **优点**：完全掌控 scope 映射、权限、配额、审计；不引入第三方 crate
- **缺点**：真正的难点不是 CRUD，而是 PROPFIND XML 变体、`If:` / lock-token 条件请求、
  Finder 的 `.DS_Store`/`.wrtest`、Explorer 的 `Translate: f` 与 `Accept-Ranges` 怪癖；
  Windows 资源管理器兼容性是最大黑洞，靠迭代调试，**风险与工时最高**

### 方案 B：集成 `dav-server` crate（推荐）

`dav-server = { version = "0.11", default-features = false, features = ["localfs"] }`，
路由 `/assist/dav/*` + 自实现 `GuardedFileSystem<Principal>` 做「私有盘 + 公有盘」合成根。

- **成本**：约 400–700 行胶水（FS 适配层 + 鉴权 + 错误映射 + 配置）
- **优点**：XML/propfind/lock/multistatus/条件请求全部现成，litmus 覆盖较好
- **缺点**：依赖个人维护的 fork（`messense/dav-server-rs`，衍生自 `webdav-handler`），
  API 在 0.6 之后变动较大（网上资料多为 0.5 时代写法）；错误映射、配额与审计钩子要写在 FS 层

**已做的可行性验证（本机 spike，`/tmp/davspike`）**

- `DavHandler::handle()` 直接接收 `http::Request<axum::body::Body>`：两者同为 http 1.0 / http-body 1.0，
  **无需协议转换层，也不需要第二个 hyper 版本**
- 实测通过：`OPTIONS` → `DAV: 1,2,3` + `MS-Author-Via: DAV`（Finder/Explorer 握手必需）；
  `PROPFIND Depth: 1` → `207 Multi-Status`（含 `getetag`/`getlastmodified`/`resourcetype`/`creationdate`）；
  `PUT` → `201` + ETag；`MKCOL` → `201`；`MOVE` / `COPY` 正常
- **LOCK 必须挂 locksystem**：不挂时 `LOCK` 返回 `405`，macOS Finder 会失效；
  挂 `memls::MemLs` 后 `LOCK` → `200` 带 `Lock-Token`，且无令牌的 `MOVE` 正确返回 `423 Locked`

### 方案 C：反向代理现成 WebDAV 服务

nginx（`dav` 模块）/ `rclone serve webdav` / sftpgo 直接对外提供 `drive_root`，应用只做认证收口。

- **成本**：几乎零 Rust 成本，最稳定成熟，客户端兼容性最好
- **缺点**：
  - 多用户 ACL 必须**在代理侧复制一份**：应用只存 Argon2 哈希，无法给 nginx 校验，
    只能新增 `auth_request` 回源端点 + per-user `alias`，配置复杂
  - 「公有文件夹按角色阈值」判定、配额、审计、日志全部绕过应用
  - 磁盘布局（`users/{user_id}`、`_public/{folder_id}`）直接暴露给客户端
- **适用**：只想最快拿到挂载能力、明确放弃与多用户权限体系联动

### 方案 D：不做 WebDAV（替代路线）

- **SFTP**：客户端覆盖好、无协议复杂度，但需要系统账户/ssh 服务，与「用户存 config.json」体系冲突
- **S3 兼容网关**：rclone 支持好，但 ACL/配额同样要在网关侧重建
- **自扩展同步协议**：扩 `client-rs/`（已有 Zotero 导入客户端）做专用同步 CLI，客户端覆盖面最差

## 推荐实现设计（方案 B + 已定决策）

### 1. 路由与挂载点

```
/assist/dav                 → 虚拟根（PROPFIND 列出「我的网盘」+ 可见公有文件夹）
/assist/dav/{任意子路径}     → 同上，映射到合成 FS
```

- 注册 `/assist/dav`、`/assist/dav/`、`/assist/dav/*rest` 三条（axum 通配符不匹配空剩余路径）
- **必须在 `.layer(middleware)` 之前注册**，才能拿到 `Extension<Settings/UserStore/SessionStore>`；
  与 `static_files` 同理（见 `AGENTS.md` 易错点 18/19）
- `token_middleware` 只覆盖 `/assist/api/*`，因此 WebDAV 处理器**自己做鉴权**
- `spa_fallback` 的前缀白名单（`main.rs:552`）追加 `dav/`

**命名空间约定（避免撞名）**

```
/                       私有盘根（可读写）
/公开/<公有文件夹名称>/   只读（首期）
```

公有文件夹用显示名做目录名；同名时追加 ` (<folder_id>)`。用户自己的目录若与 `公开` 同名，
**保留用户目录**、公有入口改为 `公开 (n)`。

> 备选（更简单但需要用户配两个挂载点）：`/assist/dav/private/` + `/assist/dav/public/{folder_id}/`。
> 首期如遇 composite FS 实现成本超预期，可降级到该形式。

### 2. 认证（`scope = dav`）

- Basic 头格式：`Authorization: Basic base64(<用户名>:<okb_ 令牌明文>)`
- 失败响应：`401` + `WWW-Authenticate: Basic realm="okb-drive", charset="UTF-8"`
- 用户名接受 `username`，也接受 `user_id`（两者都不泄露额外信息）
- 同时接受 `X-Token` 头与 `?token=`（rclone `--header`、部分客户端可配自定义头）
- 校验链复用 `users::find_by_api_token`（sha256 查表），**不做 Argon2**
- 失败尝试：按 IP 限流（复用 `auth::AttemptLimiter`），**不写 `failed_logins` / `locked_until`**

### 3. 令牌作用域扩展（`backend-rs/src/users.rs` + `auth.rs`）

| 位置 | 改动 |
|---|---|
| `users.rs` `TokenScope` | 新增 `Dav`，`as_str` = `"dav"`，`from_str` 兼容旧值 |
| `users.rs` `TokenScope::allows_api/allows_mcp` | `Dav` 两项均为 `false`；新增 `allows_dav()` |
| `auth.rs` `Principal` | 新增 `dav: bool`（= 用户 active && 令牌 scope == dav 或 会话） |
| `auth.rs` `resolve_principal` | 会话：`dav = true`（密码登录者可在账户页看到自己的 WebDAV 入口）；令牌：`dav = scope == Dav` |
| `routers/auth.rs` + `routers/users.rs` | 签发/列出令牌的 `scope` 枚举加 `dav` |
| `token_middleware` | `scope = dav` 的令牌调 `/assist/api/*` → `401 code: dav_only_token` |
| `mcp_server.rs` | 不变（`dav` 令牌 `mcp = false`） |
| 前端 `AccountView.vue` / `UsersView.vue` | 作用域选择加「WebDAV」，并给出连接说明 |

> 只读令牌（`dav_readonly`）首期不做，字段设计上预留。

### 4. FS 适配层（`backend-rs/src/services/webdav_fs.rs`，新增）

实现 `dav_server::GuardedFileSystem<Principal>`，credentials 即当次请求的 `Principal`：

- 每次请求按 credentials 解析作用域（复用 `DriveService::resolve_scope`），不缓存跨用户结果
- 合成根：`read_dir("/")` 返回私有盘内容 + `公开/` 虚拟目录；`公开/` 下的条目来自 `config.drive.public_folders`
- `open(Write)` / `create_dir` / `remove_file` / `remove_dir` / `rename`：`can_write == false` → `FsError::Forbidden`
- **PUT 语义**：写 `<目标>.part` → `rename` 覆盖目标（避免同步客户端读到半截文件）
- 配额：`open(Write)` 时校验（私有盘），单文件上限复用 `check_size`；超限映射到 `InsufficientStorage (507)`
- 全部路径经 `resolve_path` 校验，符号链接一律隐藏（`hide_symlinks(true)`）
- 错误映射表：`DriveError` → `FsError`（NotFound / Forbidden / Conflict / InsufficientStorage / BadRequest）
- 审计日志：写操作记 `audit = "drive_webdav"`，字段与现有 `drive_upload` 对齐

### 5. 配置

`config.json` 的 `drive` 段（受保护段，写操作走 `/assist/api/admin/drive/settings`）新增：

```json
{ "drive": { "webdav_enabled": false } }
```

默认关闭；开启后账户页与网盘页显示连接信息。`system.json` 不需要新字段（沿用 `drive_root`）。

### 6. 运维前置（必须写进文档）

- **TLS 必需**：Windows 资源管理器（WebClient）默认拒绝明文 HTTP 上的 Basic 认证
  （需改注册表 `BasicAuthLevel=2` 才能用）。推荐做法：反向代理终止 TLS，或后续给 axum 加 rustls
- 请求体上限：WebDAV 路由单独设 `DefaultBodyLimit`，且按 `max_upload_mb` 边读边校验
- 日志降噪：同步客户端会产生大量 4xx 探测（`.DS_Store`、`._*`、锁探测），按前缀降噪
- 备份/软删除：WebDAV 客户端删除是**直接 DELETE**，没有回收站 → 误删不可恢复

## TODO

### P0 —— 只读可用（私有盘读写 + 公有盘只读）

- [ ] `users.rs`：`TokenScope::Dav` + `allows_dav()`；`AuthToken` 序列化兼容
- [ ] `auth.rs`：`Principal.dav` 字段与 `resolve_principal` 分支；`dav_only_token` 拒绝逻辑
- [ ] `routers/auth.rs` / `routers/users.rs`：令牌签发与列表中接受 `scope = dav`
- [ ] 依赖：`dav-server = "0.11"`（`default-features = false, features = ["localfs"]`）
- [ ] `services/webdav_basic.rs`：Basic 头解析 + 401 挑战 + IP 限流（不计入账号锁定）
- [ ] `services/webdav_fs.rs`：`GuardedFileSystem<Principal>`（私有盘 + `公开/` 只读视图）
- [ ] `routers/webdav.rs`：`/assist/dav`、`/assist/dav/`、`/assist/dav/*rest` 三条路由、
      `DefaultBodyLimit`、`DavConfig`（strip_prefix、MemLs、hide_symlinks、autoindex=false）
- [ ] `main.rs`：注册路由（在 `.layer(middleware)` 之前）+ `spa_fallback` 前缀白名单加 `dav/`
- [ ] `drive.rs`：`DriveConfig.webdav_enabled` + 默认值 + 管理端开关（`/assist/api/admin/drive/settings`）
- [ ] 单元测试：作用域→虚拟路径映射、`公开/` 只读拒绝、Basic 解析、`..` 穿越拒绝

### P1 —— 写入语义与配额

- [ ] PUT：`.part` 原子写 + 覆盖语义（**不复用 `unique_child`**）
- [ ] MKCOL / DELETE / MOVE / COPY：`Overwrite` 头语义（覆盖 / 412）、非空目录 DELETE
- [ ] `used_bytes` 缓存或增量记账（内存 + mtime 失效），避免每次 PUT 全量 walk
- [ ] 配额超限 → `507 InsufficientStorage`；单文件超限 → `413`
- [ ] `Depth: infinity` → `403`，Depth 限 0/1；大目录 PROPFIND 分页/上限
- [ ] `ETag` 策略统一（`size-mtime`），弱校验在文档中说明

### P2 —— 体验与安全加固

- [ ] 软删除：删除先落 `{drive_root}/_trash/{user_id}/`，UI 与 WebDAV 共用
- [ ] 只读 WebDAV 令牌（`dav_readonly`）
- [ ] 公有文件夹写入（`write_role` 阈值）+ 公有配额/磁盘水位保护
- [ ] 前端：账户页 / 网盘页「WebDAV 连接」卡片（URL、用户名、令牌类型、只读说明、「一键复制」）
- [ ] 文档：`document/drive.md` 增加「WebDAV 同步」章节（Finder / Explorer / rclone / Cyberduck / davfs2 配置）
- [ ] 日志降噪与访问审计面板

## 验收矩阵

| 项目 | 手段 |
|---|---|
| 协议正确性 | `litmus` 测试套（basic / copymove / props / locks） |
| macOS Finder | 挂载 → 拖入 / 拖出 / 重命名 / 删除，观察是否反复弹认证 |
| Windows 资源管理器 | HTTPS + Basic，`PROPFIND` 根、拖拽、Office 直存 |
| rclone | `rclone mount` 只读浏览 + `rclone sync` 双向（含 `--dry-run` 观察删除行为） |
| Cyberduck / davfs2 | 挂载、Range 下载大文件 |
| 权限 | 只读角色对 `公开/` 写 → 403；`view-only` 用户私有盘可写；无 `dav` scope 令牌 → 401 |
| 配额 | 超额 → 507；单文件超限 → 413 |
| 安全 | `..`/绝对路径/符号链接穿越全部拒绝；`_public` 下不可见其他 folder |

## 工作量粗估

| 阶段 | 内容 | 估算 |
|---|---|---|
| 可行性 spike | 已完成（axum 0.7 + dav-server 0.11 编译并实测） | ✅ |
| P0 | 令牌作用域 + 鉴权 + FS 适配 + 路由 + 只读公有盘 | 2–3 天 |
| P1 | 写路径语义、配额、锁、条件请求 | 2–3 天 |
| P2 | 软删除、只读令牌、前端 + 文档、公有盘写入 | 2–3 天 |
| 兼容性调试缓冲 | Finder / Explorer / rclone / litmus | +30%~50% |

## 风险清单

1. **认证面扩大**：Basic 是明文口令，必须 TLS；令牌会落在客户端配置里，需要可撤销 + 只在账户页展示
2. **语义分叉**：WebDAV 写路径与现有 REST API（`unique_child` 自动改名）行为不同，两套语义要各自维护
3. **无回收站**：客户端删除即永久删除（P2 软删除前风险最高）
4. **依赖风险**：`dav-server` 为个人维护 fork，锁版本并保留自研降级路径
5. **性能**：配额全量 walk、大目录 PROPFIND、日志放大在高频同步下同时出现
6. **单实例假设**：LOCK 表与 session 一样存在进程内存，重启丢锁、不支持多实例部署
