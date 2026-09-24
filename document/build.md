# 编译说明

## 前置条件

- Rust 工具链（`rustup` / `cargo`），建议 nightly 或 2021 edition 稳定版
- 系统依赖：`libfontconfig1-dev`（必须）、`pkg-config`
- 包管理器：`cargo`（无需额外配置 registry）

## 安装系统依赖

```bash
sudo apt-get update
sudo apt-get install -y libfontconfig1-dev pkg-config
```

## 编译后端（Rust）

```bash
cd backend-rs
cargo build --release
```

### 常见问题

**`fontconfig.pc not found`**

安装 `libfontconfig1-dev` 即可。

**编译超时**

`reqwest`、`mupdf-sys` 等依赖编译较慢，建议给 `cargo build` 设置较长的超时时间（如 10 分钟以上）。 ARM 设备上首次编译可能需要 15-30 分钟。

**编译产物**

产物路径：`backend-rs/target/release/okb_assist`

## 编译前端（Vue 3 + TypeScript）

```bash
cd frontend
pnpm install
pnpm run build
```

构建输出到 `frontend/dist/`，由后端直接 serving。

> 注意：`pnpm run type-check` 当前不可用（`vue-tsc` 与 TypeScript 版本不兼容），构建通过即视为类型检查通过。

## 运行

```bash
cd backend-rs
cargo run -- --host 0.0.0.0 --port 5001 --log-level debug
```

支持的命令行参数：`--host`、`--port`（默认 5001）、`--log-level`（trace/debug/info/warn/error，默认 info）。

## 仅代码检查（不编译）

在嵌入式设备（如 Orange Pi）上，如果磁盘空间或内存不足，可以仅做编译检查：

```bash
cd backend-rs
cargo check
```