# 权限管理 → 用户管理 升级计划

> 目标：把当前的「静态 Token → 角色」模式升级为完整的用户体系：**注册 / 密码登录 / Token 登录 / MCP 访问**，
> 用户数据存放在 `config.json`。已确认的关键决策见「0. 决策基线」。
>
> **后续变更（本计划完成后）**：`system.json` 的 `token` 与 `mcp_token` 两个全局凭据字段已**彻底删除**
> （`auth::ensure_bootstrap()` / `Settings::token()` / `Settings::mcp_token()` 均已移除）；
> 首个管理员只由前端初始化页创建，MCP 只接受用户个人令牌/会话令牌/受信任子网。
> 下文涉及这两个字段的描述均属历史记录。

---

## 0. 决策基线（已与需求方确认）

| 决策点 | 结论 |
|---|---|
| 会话凭证 | **服务端会话 token**：登录返回 opaque token（`sess_<32B base64url>`），服务端 `SessionStore` 维护 `token → Session`，支持过期、登出失效。前端仍走 `X-Token` 头，不改传输层。 |
| 注册策略 | **开放注册 + 管理员审核启用**：注册后 `status = pending`，管理员在用户管理页启用后才能登录；有 `allow_registration` 开关可关闭开放注册。 |
| 与旧模型兼容 | **彻底切换到 users，旧凭据全部作废（方案 B）**：删除 `config.json` 的 `permissions` 字典与 `system.json` 的 admin `token` 鉴权逻辑，只认 `users`。旧 `permissions` 条目的 token **一律失效**，只把「角色 + mcp」迁成无凭据的占位用户（`status=pending`），由管理员重设密码或重新签发 token（见 §5.2）。 |
| MCP 权限 | **用户级 `mcp` 布尔 + 可选个人 MCP token**：`users[].mcp = true` 的用户可用自己的会话 token 或个人 API token 访问 MCP；`system.json.mcp_token` 保留为全局通用入口。 |
| 用户数据落盘 | **直接存 `config.json`**（当前跟踪的是调试配置，密码哈希随仓库流转可接受）。`system.json` 仍只放系统级参数。 |
| 会话有效期 | **7 天**（`session_ttl_hours = 168`），滑动续期：每次有效请求刷新 `last_seen`，空闲超 7 天才失效。 |
| `trusted_subnets` | **默认关闭**：新增 `trusted_subnet_enabled = false`，即使已配置网段也不放行；显式开启后按 `trusted_subnet_role`（默认 `admin`）注入本地用户。 |

**非目标（本期不做）**：OAuth/LDAP/第三方登录、用户组、字段级文档权限、密码找回邮件（无邮件服务）。

---

## 1. 现状盘点

| 关注点 | 现有实现 | 升级后 |
|---|---|---|
| 角色 | `auth::Role`（admin / view-only / view-upload），`assert_role()` | 保留枚举不动，新增「角色来源 = 用户」 |
| 认证 | `main.rs::token_middleware` 读 `X-Token`/`?token=`，比对 `system.json token` + `config.json permissions` | 读同一头，先查会话、再查用户 API token |
| 管理入口 | `routers/admin.rs` 的 `/admin/permissions`（增删 token）+ `ConfigView.vue` 的 token 表格 | 新的 `/admin/users` + `UsersView.vue` |
| 归属 | `auth::OwnerKey::from_token`（sha256(token) 前 32 位，形如 `t:<hex>`） | `OwnerKey::from_user(id)`（形如 `u:<user_id>`），token 派生逻辑保留给 API token 场景 |
| MCP | `mcp_server.rs` 内联比对 `mcp_token` / permissions 的 `mcp:true` | 抽成 `auth::resolve_mcp_principal()`，支持用户 token |
| 免鉴权来源 | `system.json trusted_subnets`（命中即注入 admin）、`token == "change-me"` 全局跳过 | 子网白名单**默认关闭**（`trusted_subnet_enabled=false`），显式开启后才按 `trusted_subnet_role` 注入本地用户；`change-me` 跳过逻辑改为「未初始化引导态」 |

涉及文件（后端）：`main.rs`、`auth.rs`、`config.rs`、`config_manager.rs`、`routers/{admin,config,chat,documents,pipeline,mod}.rs`、`mcp_server.rs`、`services/chat_store.rs`。
（前端）：`api/client.ts`、`stores/token.ts`、`router/index.ts`、`components/TokenModal.vue`、`AppHeader.vue`、`views/{ConfigView,ChatView,McpSetupView}.vue`。

---

## 2. 数据模型（`config.json`）

### 2.1 新增顶层段 `users`

```json
{
  "users": {
    "version": 1,
    "allow_registration": true,
    "require_approval": true,
    "registration_role": "view-only",
    "session_ttl_hours": 168,
    "items": [
      {
        "id": "u_8f3c1a7d4b2e4c5f",
        "username": "alice",
        "display_name": "Alice",
        "role": "admin",
        "status": "active",
        "mcp": true,
        "password_hash": "$argon2id$v=19$m=19456,t=2,p=1$....",
        "api_tokens": [
          {
            "id": "at_1",
            "label": "zotero",
            "token_sha256": "60b8be6583ac41abb3af63e01151bd86...",
            "mcp": true,
            "created_at": "2026-01-01T00:00:00Z",
            "last_used_at": null
          }
        ],
        "created_at": "2026-01-01T00:00:00Z",
        "updated_at": "2026-01-01T00:00:00Z",
        "last_login_at": null,
        "failed_logins": 0,
        "locked_until": null
      }
    ]
  }
}
```

要点：

- **只存哈希**：`password_hash` 用 Argon2id PHC 字符串；`token_sha256` 为 `format!("okb_<32B base64url>")` 的 sha256 hex（前缀便于识别与日志脱敏）。明文 token 只在校验响应里出现一次。
- **`id` 稳定不可变**，作为 `OwnerKey` 与会话归属的主键；`username` 唯一（大小写不敏感比较），登录凭证。
- `status`：`pending`（待审核）/ `active` / `disabled`（禁用，会话与 token 一并失效）。
- `role` 沿用三值枚举，`registration_role` 只允许 `view-only` / `view-upload`（不允许自助注册成 admin）。
- `users` 段**必须**加入 `config_manager::default_config()`，否则 `save_config()` 的 `service_keys` 过滤会把它丢掉。

### 2.2 删除 / 降级的旧数据

- `config.json.permissions`：删除。启动迁移时按 §5 处理；迁移完成后从磁盘移除该键（保留一个版本周期可只清空为 `{}`）。
- `system.json.token`：降级为 **一次性引导凭证**（见 §5.1），鉴权不再使用。
- `system.json.mcp_token`：保留（全局 MCP 入口），语义不变。
- `system.json.trusted_subnets`：保留但**默认关闭**，新增 `trusted_subnet_enabled`（默认 `false`，为 `true` 才生效）与 `trusted_subnet_role`（默认 `admin`），避免「局域网即管理员」被默认踩到。
- `system.json` 默认值同步调整为 `trusted_subnets: []` + `trusted_subnet_enabled: false` + `trusted_subnet_role: "admin"`（`config_manager::default_system()`）。

---

## 3. 后端改造

### 3.1 新模块

| 文件 | 职责 |
|---|---|
| `backend-rs/src/users.rs` | `UserStore`：以 `ConfigManager` 为唯一持久化载体，提供 `list/get_by_username/get_by_id/create/update/delete/set_status/touch_login/verify_password/hash_password`；负责 username 校验（`^[A-Za-z0-9_.-]{3,32}$`）、密码强度（≥8、≤128）、唯一性、**「至少一个 active admin」不变量**。所有写入走「读盘 → 改 users 段 → 原子写」。 |
| `backend-rs/src/session.rs` | `SessionStore`：`RwLock<HashMap<String, Session>>`，`Session { user_id, username, role, mcp, issued_at, expires_at, last_seen, ip, ua }`；`issue/validate/touch/revoke/revoke_user/revoke_all/purge_expired`；每次 `validate` 顺带惰性清理过期项。**进程内存储，重启即失效**（文档中明确）。 |
| `backend-rs/src/auth.rs`（扩写） | 新增 `Principal { user_id, username, role, mcp, via: CredentialKind }` 与 `CredentialKind { Session, ApiToken, LegacyAdmin, LocalSubnet }`；`resolve_principal(token, settings, sessions, users) -> Option<Principal>`；`OwnerKey::from_user(id)`；`is_mcp_allowed()`。保留 `Role` / `assert_role()`（全仓 40+ 处调用不动）。 |

依赖新增：`argon2 = "0.5"`（含 `password-hash`）；`base64`/`rand`/`sha2`/`hex` 已有。若目标设备（OrangePi）不便联网拉新依赖，备选：`bcrypt`，或纯 Rust 自实现 PBKDF2-HMAC-SHA256（不推荐，但零新依赖）。

### 3.2 中间件（`main.rs::token_middleware`）

新的判定顺序（仅 `/assist/api/*`）：

1. 白名单跳过：`/assist/api/auth/{check,status,login,register,logout,config}`、文档图片 URL。
2. 从 `X-Token` / `?token=` 取凭证 → `resolve_principal()`：
   - `sess_` 前缀 → 会话表；
   - `okb_` 前缀 / 其它 → 遍历用户 `api_tokens` 的 sha256（先查内存索引 `HashMap<token_sha256, user_id>`，索引随写操作重建）；
   - 可选：`system.json.token` 仅在「未初始化」时作为 `LegacyAdmin` 接受（见 §5.1）。
3. 命中的 `Principal` 注入 extensions：
   ```rust
   req.extensions_mut().insert(principal.role.clone());
   req.extensions_mut().insert(principal);              // 新
   req.extensions_mut().insert(OwnerKey::from_user(&principal.user_id));
   ```
4. 受信任子网：仅当 `trusted_subnet_enabled = true` 时注入「本地用户」（`user_id = "local"`，角色取 `trusted_subnet_role`，`mcp = true`）。
5. 未命中 → 401（沿用现有文案，避免用户名枚举）。

`auth_check_handler` 改为基于 `Principal` 生成同样的 `permissions` 结构（前端零改造），并额外返回 `user: {id, username, display_name, role, mcp}` 和 `needs_relogin`（旧 style token 时）。

**处理器签名不变**：继续用 `Extension(auth::Role)`，需要用户身份的（会话归属、`/auth/me`、审计日志）才额外取 `Extension<Principal>`。

### 3.3 新路由：`backend-rs/src/routers/auth.rs`（公开）

| 方法 | 路径 | 说明 |
|---|---|---|
| GET | `/assist/api/auth/status` | `{initialized, user_count, allow_registration, require_approval, session_ttl_hours, version}`，供登录页决定展示「初始化管理员 / 注册 / 登录」。无凭证可访问。 |
| POST | `/assist/api/auth/register` | `{username, password, display_name?}` → 创建 `pending` 用户（`require_approval=false` 时直接 `active` 并返回会话）。**仅在 `allow_registration` 为 true 或未初始化时可用**；按 IP 内存限流（如 5 次/10 分钟）。 |
| POST | `/assist/api/auth/login` | `{username, password}` → `{token, expires_at, user, permissions}`；错误统一 `{"detail":"用户名或密码错误"}`；失败计数 +5 次后指数退避（`locked_until`）。即使用户名不存在也执行一次 dummy Argon2 verify，防时序枚举。 |
| POST | `/assist/api/auth/logout` | 撤销当前会话。 |
| GET | `/assist/api/auth/me` | 当前用户（含 `mcp`、`api_tokens[]` 的**元数据**，不含 token）。 |
| POST | `/assist/api/auth/password` | `{old_password, new_password}` 改自己密码，改后撤销该用户其它会话。 |
| GET | `/assist/api/auth/check` | **保留兼容**：返回 `{role, valid, permissions, user}`；无 token 时 401（现行为）。 |
| POST | `/assist/api/auth/bootstrap` | 仅当 `user_count == 0` 时创建首个 admin（`{username, password}`），成功后返回会话。 |

### 3.4 新路由：`backend-rs/src/routers/users.rs`（admin）

| 方法 | 路径 | 说明 |
|---|---|---|
| GET | `/assist/api/admin/users` | 列表（**不含** `password_hash`/`token_sha256`）；支持 `?status=pending` 过滤；返回待审核计数。 |
| POST | `/assist/api/admin/users` | 直接创建用户（指定 role/status/mcp/password）。 |
| PUT | `/assist/api/admin/users/:id` | 改 `display_name/role/status/mcp`；禁止把最后一个 active admin 降级/禁用。 |
| POST | `/assist/api/admin/users/:id/approve` | `pending → active`（也可用 PUT status）。 |
| POST | `/assist/api/admin/users/:id/password` | 管理员重置密码；可选 `revoke_sessions`。 |
| DELETE | `/assist/api/admin/users/:id` | 删除用户；级联撤销会话与 token；最后一个 admin 不可删。 |
| GET | `/assist/api/admin/users/:id/tokens` | 列出 API token 元数据（label/mcp/时间）。 |
| POST | `/assist/api/admin/users/:id/tokens` | `{label, mcp}` → **返回一次性明文 `okb_...`**，落盘只存 sha256。 |
| DELETE | `/assist/api/admin/users/:id/tokens/:tokenId` | 撤销。 |

同时删除 `admin.rs` 的 `/admin/permissions*` 三个端点与 `PermissionTokenBody`。

### 3.5 `config.json` 读写安全（必须做）

- `ConfigManager::save_config()` 增加**排除键**：`users`（以及过渡期的 `permissions`）不允许被 `/assist/api/config` 的 PUT 覆盖 —— 否则前端表单回传旧快照就可能把管理员改掉或注入 `password_hash`。
- 新增 `ConfigManager::update_users_section<F>(f)`：内部加写锁（新增 `write_lock: Arc<Mutex<()>>`），**从磁盘重读 → 只改 `users` → 临时文件 + rename 原子写**，避免与 LLM/向量库配置保存互相覆盖，也避免写一半崩溃丢账号。
- `write_json_file` 统一改为 `tmp + rename`（`config.json` 现在持有密码哈希，不能再有半写风险）。
- `GET /assist/api/config` 与 `get_config()` 返回值中剔除/脱敏 `users` 段（前端配置页看不到、也不回显哈希）。若配置页需要用户数，只返回 `users_summary: {count, pending}`。
- 保持 `load_service_config()` 的 `migrate_legacy_service_config()` 风格：新增 `migrate_legacy_users(raw)`（幂等），在 deep_merge 之前作用于磁盘原始内容。

### 3.6 MCP（`mcp_server.rs`）

- 认证段替换为：
  ```rust
  let principal = auth::resolve_mcp_principal(token, &settings, &sessions, &users);
  // 全局 mcp_token（system.json）→ 伪 principal(local, admin)
  // 会话/API token → 要求 user.status == active && user.mcp
  // trusted_subnet_enabled && 命中子网 → local principal
  ```
- 工具集保持只读 9 个，不做角色区分（现状即如此）；日志中记录 `user=<username>` 便于审计。
- `has_mcp_access()` 删除，逻辑并入 `Principal`。

### 3.7 会话归属迁移（`chat_store.rs` / `chat.rs`）

- 新请求的 `owner_key` = `u:<user_id>`，同一用户换浏览器仍能看到自己的会话（比现在按 token 派生更符合直觉）。
- 旧的 `t:<sha256>` 会话文件无法再关联到人 → 规则：**admin 可见全部**（现有规则已覆盖），非 admin 不可见。文档中标注为可接受的破坏性变更；如需保留，可在迁移期一次性把「旧 admin token」对应的 `t:` 重写为 `u:<首个 admin id>`（仅此一例，见 §5.2）。
- `chat.rs::assert_role` 调用不变。

---

## 4. 前端改造

| 文件 | 变更 |
|---|---|
| `stores/token.ts`（原地改写成认证 store，保留 `useTokenStore` 名以免大改；另导出 `useAuthStore` 别名） | state：`sessionToken / user / role / permissions / initialized`；actions：`login(username,password)`、`register()`、`loginWithToken(token)`（“Token 登录”入口）、`logout()`、`restore()`（启动时用本地 `okb_session` 调 `/auth/check` 刷新角色）、`refreshMe()`。`localStorage` 键沿用 `okb_token` 以复用旧登录态，新增 `okb_user`。保留 `canAdmin/canEdit/...` getters 以免改动全部页面。 |
| `views/LoginView.vue`（新建，`/assist/login`） | 三个 tab：**账号登录 / 注册 / Token 登录**；未初始化（`/auth/status.initialized=false`）时显示「创建初始管理员」；`pending` 用户登录返回友好提示「账号待管理员审核」。 |
| `router/index.ts` | 新增 `/assist/login`（`meta.public = true`）与 `/assist/users`（`requiresAdmin`）；守卫改为：未认证且非 public → `next('/assist/login?redirect=...')`；`role` 未就绪时先 `restore()` 再判（避免刷新后误跳）。 |
| `components/TokenModal.vue` | 改为「登录状态面板」：显示用户名/角色、切换账号、登出、粘贴 Token 登录（保留原有历史记录功能）。 |
| `AppHeader.vue` / `MobileMenu.vue` | 右上角显示 `display_name || username` + 角色徽标 + 登出；admin 增加「用户管理」入口。 |
| `views/UsersView.vue`（新建，`/assist/users`） | 用户表格（用户名/姓名/角色/状态/MCP/最后登录/操作）；待审核区（批准 / 拒绝=删除）；角色与 `mcp` 下拉即时保存；重置密码对话框；**API/MCP token 管理**（创建后一次性展示 + 复制，列表只显示 label 与时间）；删除二次确认。 |
| `views/ConfigView.vue` | 移除「权限 Token」区块（原 `permTokens` 那套），替换为一行「用户与权限管理」跳转链接 + 用户数/待审核数摘要。 |
| `views/McpSetupView.vue` | 增加「生成个人 MCP token」引导（复用 users token 端点），并说明 `Authorization: Bearer <token>`。 |
| `api/auth.ts`、`api/users.ts`（新建） | 对应 §3.3 / §3.4 的调用封装 + TS 类型（放进 `types/auth.ts`）。 |
| `api/client.ts` | 401 拦截：清本地 token 并派发 `auth:required`（登录/注册/status/check 等端点不触发跳转）；请求头仍是 `X-Token`。 |
| `ChatView.vue` | 会话归属改为用户维度，无需前端改动；仅「保存会话」提示语可显示归属用户名。 |

---

## 5. 迁移与首次启动

### 5.1 引导（bootstrap）

启动时 `migrate_legacy_users()`：

1. 若 `users.items` 非空 → 什么都不做。
2. 若为空且 `system.json.token` 不是空/`change-me` → 创建 `admin` 用户：
   `username = "admin"`、`role = "admin"`、`status = "active"`、`mcp = true`、`password_hash = None`、`must_set_password = true`，
   并**仅在此刻**把 `system.json.token` 作为该用户的一个 `label = "bootstrap"` 的 api_token（sha256 落盘）。
   该 token 的生命周期：**一旦该用户通过 `POST /auth/password` 设置密码成功，立即自动吊销**（日志提示），之后 `system.json.token` 彻底失效。
   → 旧 admin token 只在「初始化过渡期」可用，登录后前端 banner 提示「请立即设置密码」。
3. 若为空且 `token` 是 `change-me`/空 → `initialized = false`，仅允许 `/auth/status` 与 `/auth/bootstrap`，前端引导创建首个管理员；此状态下除这两个路径外**全部 API 返回 401**（避免裸奔）。

引导完成后（`users.items` 中存在带 `password_hash` 的 active admin）→ `system.json.token` 不再参与任何鉴权（实现上直接忽略，不保留 legacy 分支）。

### 5.2 旧 `permissions` 迁移（已选定：方案 B，全部失效）

- 每个 `permission` 条目 → 一个**无凭据占位用户**：`username = "legacy-<sha1(token) 前 4 位>"`、`password_hash = None`、`status = "pending"`、`role = access`、`mcp = 原值`、`api_tokens = []`。
  - **原 token 不迁移、不入库、不落盘**（只在日志中出现 sha256 前 8 位用于对账）。旧脚本/客户端**立即失效**（401）。
  - 迁移后从磁盘**删除** `permissions` 键；日志输出迁移清单（用户名 / 角色 / mcp），并 `warn!` 提示「旧 token 已全部失效，请分配新密码或签发新 token」。
- 管理员后续两种处置：① 在用户管理页为该 `legacy-*` 用户重置密码 → 变可登录的正常账号；② 直接删除这些占位用户（推荐，若明确不再需要）。
- 升级说明（`README.md` / 发布说明）必须**显著提示这是破坏性变更**，并给出「管理员如何签发新 token」的两步操作。
- 同时清理 `system.json.token` 的鉴权作用（§5.1 完成后即失效；文件里保留字段仅作引导用，值建议改为 `change-me`）。

### 5.3 会话与状态

- 会话在内存 → **重启需重新登录**（前端 `restore()` 收到 401 自动跳登录页）。TTL = **7 天滑动**（`session_ttl_hours = 168`）：每次有效请求刷新 `last_seen`，空闲超过 7 天才失效。若需重启不丢登录，P7 可选加 `data/sessions.json`（原子写，启动加载）。
- 批量任务、pipeline 状态不受影响。

---

## 6. 安全清单

- [ ] Argon2id（`m=19456, t=2, p=1`）；`password_hash` 永不出现在任何 API 响应与日志中。
- [ ] API token 只存 sha256，明文仅创建响应返回一次；日志/响应统一脱敏为 `okb_****`。
- [ ] 登录失败限流（用户名 + IP 双维度，内存滑动窗口），`locked_until` 指数退避；错误文案统一，杜绝用户名枚举。
- [ ] 注册限流 + `allow_registration` 开关 + 用户名大小写不敏感唯一。
- [ ] 权限提升防护：`registration_role` 不允许 admin；`PUT /admin/users/:id` 不允许把最后一位 active admin 降级/禁用/删除。
- [ ] `PUT /assist/api/config` 无法覆盖 `users` / `permissions`；`users` 段不出现在 `/api/config` 响应里。
- [ ] 会话：登录新建 token（不复用请求里带来的旧 token）；滑动续期 7 天（`session_ttl_hours=168`，每次有效请求刷新 `last_seen`，空闲超期才失效）；登出、改密、禁用、删除用户 → 撤销对应会话与 token。**实现口径**：改密撤销该用户**全部会话** + `bootstrap` token；用户自签 API token 保留（可在用户管理页手动撤销，避免误断 Zotero 等客户端）。
- [ ] `trusted_subnets` **默认关闭**（`trusted_subnet_enabled=false`），开启需显式配置角色，日志标注 `via=LocalSubnet`。
- [ ] 审计日志（`info!`）：登录成功/失败、注册、审批、角色变更、token 签发/撤销、删除用户；新增 `audit` 字段便于 JSON 日志检索。
- [ ] **`config.json` 入 git 已确认为可接受**（调试配置 + 调试账号）：仍在 `AGENTS.md` / `README.md` 注明「内含密码哈希，生产部署前请改用独立配置或修改默认账号」，并在 `.gitignore` 注释中提醒不要提交真实用户数据。

---

## 7. 分阶段实施

**进度**：✅ **P1–P6 全部完成**（P1 基础层 / P2 认证链 / P3 迁移 / P4 用户管理 API + CLI 兜底 / P5 前端 / P6 文档）。
验证：`cargo check` 通过、`cargo test` 121 个单元测试全绿、`pnpm run build` 通过、真实进程 curl 端到端（引导/登录/注册/审核/令牌/MCP/禁用/删除/重置）+ Playwright 浏览器走查（初始化→登录→用户管理→账户）无 console 错误。⬜ P7 为可选增强（会话持久化等）。

| 阶段 | 内容 | 交付可直接验证的结果 | 预估 |
|---|---|---|---|
| **P1 基础层** ✅ | `users.rs`（含 Argon2）+ `session.rs` + `default_config().users` + `ConfigManager` 原子写与 `update_users_section` + 单元测试（`cargo test`：哈希校验、唯一性、最后 admin 不变量、会话过期） | `cargo check` 通过；`config.json` 出现 `users` 段 | 1 天 |
| **P2 认证链** ✅ | `auth::Principal` + `resolve_principal` + `token_middleware` 改造 + `routers/auth.rs`（status/login/register/logout/me/password/check/bootstrap） | curl 走通：bootstrap → login → 带 token 调 `/documents` → logout 后 401 | 1.5 天 |
| **P3 迁移** ✅ | `migrate_legacy_users`（§5.1/5.2，方案 B）+ 删除 `admin.rs` 的 permissions 端点 + `mcp_server.rs` 认证替换 + `OwnerKey::from_user` + chat 会话归属 | 引导 admin 可用 bootstrap token 登录；旧 `permissions` token 全部 401；`legacy-*` 用户以 pending 出现；`/assist/mcp` 用新签发 token 可通 | 1 天 |
| **P4 用户管理 API** ✅ | `routers/users.rs` 全量端点 + 不变量校验 + 审计日志 + `--reset-password` CLI 兜底 | curl 覆盖 §3.4 全部端点，含“删最后一个 admin 返回 400” | 1 天 |
| **P5 前端** ✅ | `stores/token.ts`（认证 store）、`LoginView.vue`、`AccountView.vue`、`UsersView.vue`、路由守卫、TokenModal/AppHeader/AppNav/MobileMenu/ConfigView/McpSetupView 改造、`api/{auth,users}.ts`、`types/auth.ts` | `pnpm run build` 通过；浏览器完成注册→审核→登录→改角色→MCP token 闭环 | 2 天 |
| **P6 文档与验收** ✅ | 更新 `AGENTS.md`（权限→用户、system.json 字段表、路由表、易错点）、`README.md`、`document/mcp.md`；新增 `document/user-management.md`（面向使用者）；执行 §8 验收清单 | 文档与代码一致 | 0.5 天 |
| **P7（可选）** | 会话持久化 `data/sessions.json`、密码策略可配置、登录历史页、`users_path` 外置文件 | — | 按需 |

> 嵌入式设备（OrangePi）上只跑 `cargo check`，不编译后端（见 `AGENTS.md`）。新增依赖 `argon2` 需联网拉 crate。

---

## 8. 验收清单

**功能**
- [ ] 未初始化时仅能访问 `/auth/status` 与 `/auth/bootstrap`，创建首个 admin 后其余 API 可用。
- [ ] 注册 → `pending` → 管理员批准 → 登录成功；未批准登录返回明确提示；关闭 `allow_registration` 后注册返回 403。
- [ ] 密码登录返回会话 token，`X-Token` 调用 `/documents`、`/chat`、`/api/config`（admin）均按角色生效。
- [ ] Token 登录（用户 api_token / 个人 MCP token / `system.json.mcp_token`）均可用；登出后同一 token 立即 401。
- [ ] 引导期 `system.json.token`（bootstrap api_token）可用；首次设置密码后自动失效。
- [ ] 改密码 / 禁用 / 删除用户后，该用户所有会话与 token 失效。
- [ ] 最后一个 active admin 不可降级、禁用、删除（返回 400）。
- [ ] `/assist/mcp` 用 `users[].mcp=true` 的个人 token 可调工具；`mcp=false` 的用户返回 401；`system.json.mcp_token` 仍可用。
- [ ] Chat 会话历史按用户隔离：同角色不同用户互不可见，admin 全见。
- [ ] `GET /assist/api/config` 不含 `users` 段；`PUT /assist/api/config` 不影响 `users`。

**回归**
- [ ] 会话滑动续期：7 天内有请求则不过期；模拟超过 168h 无请求 → 401。
- [ ] 旧 `permissions` 里的 token 全部 401（方案 B）；迁移后 `legacy-*` 占位用户以 `pending` 出现，日志可见迁移清单。
- [ ] `trusted_subnets` 默认不生效（未配 `trusted_subnet_enabled` 时即使填了网段也不放行）；显式开启后按 `trusted_subnet_role` 生效。
- [ ] 前端 `pnpm run build` 通过；路由守卫刷新不闪跳；401 自动跳登录页。
- [ ] `cargo check` 通过（设备端）。

---

## 9. 风险与缓解

| 风险 | 影响 | 缓解 |
|---|---|---|
| `config.json` 入 git，含密码哈希 | 离线爆破 | 已确认可接受（调试配置）；仍用强密码 + Argon2id（默认参数），文档注明生产需改密/外置；P7 可选 `users_path` 外置 |
| 旧凭据一次性全失效（方案 B） | 依赖旧 token 的脚本/客户端断链 | 升级说明显著提示，并给“管理员签发新 token”两步指引；可回滚（旧 `permissions` 备份保留一个版本） |
| 会话在内存，重启失效 | 用户需重登 | 明确文档；P7 可持久化 |
| `users` 段被配置页写回覆盖 | 账号丢失/权限提升 | `save_config` 排除 `users`；`update_users_section` 原子写 + 写锁 |
| 旧 chat 会话归属 `t:` 断裂 | 非 admin 看不到历史会话 | admin 兜底可见；迁移期可选重写首个 admin 的 `t:` |
| 忘记管理员密码 | 无法登录 | 保留 CLI 兜底：`okb_assist --reset-admin-password`（P4 增加，读 stdin 或新密码参数），直接改 `config.json` |
| 注册接口被刷 | 垃圾账号 | 限流 + 审核 + `allow_registration` 开关 + 单 IP 待审核上限 |

---

## 10. 已确认决策（已归档，不再为待确认项）

1. **落盘位置**：用户数据直接存 `config.json`（当前跟踪的是调试配置，密码哈希落盘**可接受**）。P7 的 `users_path` 外置仅作为可选增强。
2. **旧 `permissions` 迁移**：选 **B —— 全部失效重发**。旧 token 一个不迁移，只留 `legacy-*` 无凭据占位用户（`status=pending`），由管理员重设密码或删除。
3. **`trusted_subnets`**：**默认关闭**（新增 `trusted_subnet_enabled=false`）；已配网段的部署也需显式开启才生效（升级说明中标注）。
4. **会话 TTL**：**7 天**（`session_ttl_hours=168`）滑动续期。

### 实施前仍需内部决定的小项

- 首个管理员用户名固定 `admin` 还是引导页自填（§5.1 已默认 `admin` + 强制改密，可覆盖）。
- `legacy-*` 占位用户是保留还是迁移完成即自动删除（倾向保留待管理员确认）。
- 是否同时提供 `--reset-admin-password` CLI 兜底（§9 建议提供，约 0.2 天工作量）。
