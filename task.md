# Markdown 阅读器重构方案

## 目标

将当前顶部工具栏分散的阅读器，重构为**沉浸式全屏阅读器**，所有控件收纳到右下角浮动区域。

---

## 设计原则

| 原则 | 说明 |
|------|------|
| **读优先** | 内容占满全部视口宽度，无顶部/侧边遮挡 |
| **渐进披露** | 高频功能（翻页、目录）一键可达，低频（设置）多一层展开 |
| **零干扰** | 默认不加载图片、不渲染复杂公式直到用户需要 |
| **响应式** | 桌面和移动端共用同一套右下角浮动方案 |

---

## 布局方案（桌面端）

```
┌──────────────────────────────────────────────┐
│                                              │
│               Markdown 内容                   │
│            （全宽、无顶部栏）                  │
│                                              │
│                                              │
│                              ┌──────────┐    │
│                              │  ☰ 目录  │    │  ← 右下角主按钮
│                              │  ⚙ 工具栏 │    │
│                              └──────────┘    │
│                               ← 1/5 →        │  ← 多页时出现
└──────────────────────────────────────────────┘
```

### 右下角区域结构

```
┌──────────────────────────────────────┐
│  ← 1/5 →      [☰] [⚙]              │
└──────────────────────────────────────┘
   ↑ 仅多页时显示     ↑ 始终显示
```

### ☰ 目录面板（右侧弹出抽屉）

```
┌──── 目录 ──────────────────────────────────┐
│                                            │
│  ● 1. Introduction          ← 当前章节高亮  │
│    1.1 Background                          │
│    1.2 Related Work                        │
│  ● 2. Method                               │
│    2.1 Overview                            │
│    2.2 Algorithm                           │
│  ● 3. Experiments                          │
│  ...                                       │
│                                            │
│                              [ 关闭 ]      │
└────────────────────────────────────────────┘
```

### ⚙ 工具栏面板（右下角 Popover）

```
┌──────────────────────┐
│  ← 返回              │
│  🔍 搜索 (Ctrl+F)    │
│                      │
│  数学渲染器:          │
│  ○ 不渲染  ○ KaTeX   │
│  ○ MathJax           │
│                      │
│  视图:               │
│  [ ] 显示源码         │
│  [ ] 加载全部图片     │
└──────────────────────┘
```

---

## 🏗️ 架构变更：分页逻辑前移

### 职责划分

```
┌──────────────────────────────┐
│         后端 (Rust)           │
│                              │
│  只管两件事：                  │
│  1. markdown?line_start=N     │
│     &line_count=M → 返回切片  │
│  2. /toc → 返回标题+行号      │
│                              │
│  ❌ 不再 split_into_pages     │
│  ❌ 不再算总页数               │
│  ❌ 不知道什么是"页"           │
└──────────────────────────────┘

┌──────────────────────────────┐
│         前端 (Vue)            │
│                              │
│  分页、跳转、滚动全部自己算：   │
│  总页数 = ceil(totalLines     │
│           / LINES_PER_PAGE)  │
│  目标页 = floor(targetLine    │
│           / LINES_PER_PAGE)  │
│  加载 → line_start = 页*行数  │
└──────────────────────────────┘
```

### 为什么后端不再分页？

| 之前（后端分页） | 之后（前端分页） |
|--|--|
| `split_into_pages()` 复杂，需维护字符→行映射 | 后端一行代码 `lines[start..end].join("\n")` |
| 后端要知道前端用什么 page_size | **解耦**，前端决定每页多少行 |
| 后端返回 `page`、`total_pages` | 后端只返回数据，前端算分页 |
| 跨页跳转需 `target_offset` 特殊逻辑 | **不需要**，前端自己算目标页，重新请求就行 |

---

## 🔧 后端 API 设计（简化版）

### 1. `GET /assist/api/documents/:id/markdown`

读取 Markdown 内容，支持行范围切片。

**参数**：

| 参数 | 类型 | 默认 | 说明 |
|------|------|------|------|
| `line_start` | `i64` | `0` | 起始行号（0-indexed） |
| `line_count` | `i64` | `-1` | 返回行数（-1 表示返回从 line_start 到末尾） |
| `full` | `bool` | `false` | 忽略其他参数，返回全部内容 |

**响应**：

```json
{
  "content": "从 line_start 开始的 line_count 行内容，按 \\n 拼接",
  "total_length": 150000,
  "total_lines": 15000,

  "line_start": 0,
  "line_count": 5000,
  "lines_returned": 5000
}
```

- `lines_returned`：实际返回的行数（文件尾可能不足 line_count）
- `line_start` `line_count`：回显请求参数，方便前端确认
- `total_length`：文件总字符数
- `total_lines`：文件总行数

**实现逻辑**（Rust）：

```rust
async fn get_markdown(...) -> Response {
    // 1. 读完整文件
    let content = std::fs::read_to_string(&md_path)?;
    let content = rewrite_image_paths(&content, id);

    // 2. 计算元数据
    let total_length = content.len();
    let total_lines = content.lines().count();

    // 3. 切片
    if params.full.unwrap_or(false) {
        return Json(json!({
            "content": content,
            "total_length": total_length,
            "total_lines": total_lines,
            "line_start": 0,
            "line_count": total_lines as i64,
            "lines_returned": total_lines as i64,
        }));
    }

    let line_start = params.line_start.unwrap_or(0).max(0) as usize;
    let line_count = params.line_count.unwrap_or(-1);

    let lines: Vec<&str> = content.lines().collect();
    let actual_start = line_start.min(lines.len());
    let actual_end = if line_count < 0 {
        lines.len()
    } else {
        (actual_start + line_count as usize).min(lines.len())
    };
    let slice = lines[actual_start..actual_end].join("\n");

    Json(json!({
        "content": slice,
        "total_length": total_length,
        "total_lines": total_lines,
        "line_start": actual_start,
        "line_count": (actual_end - actual_start) as i64,
        "lines_returned": (actual_end - actual_start) as i64,
    }))
}
```

### 2. `GET /assist/api/documents/:id/toc`

从完整 Markdown 文件中提取标题结构。

```json
{
  "toc": [
    { "level": 1, "title": "Introduction", "line": 0 },
    { "level": 2, "title": "Background", "line": 24 },
    { "level": 2, "title": "Related Work", "line": 58 },
    { "level": 1, "title": "Method", "line": 312 }
  ]
}
```

`line`：该标题在完整文件中的行号（0-indexed）。
实现：逐行扫描，匹配 `^(#{1,6})\s+(.+)$`。

### 3. 路由

```rust
.route("/assist/api/documents/:id/markdown", get(get_markdown).put(update_markdown))
.route("/assist/api/documents/:id/toc", get(get_toc))
```

> **注意**：旧版 `page` / `page_size` / `target_offset` 参数不再支持。前端全部改为 `line_start` / `line_count`。

---

## 🎯 目录跳转流程（纯前端计算）

```

                     用户点「Method」
                          │
                    tocItem.line = 312
                          │
                    ┌─────┴──────┐
                    │             │
               line >= curStart   │
               && line < curEnd   │
                    │             │
                   YES            NO
                    │             │
                    ▼             ▼
             在当前页内        跨页了
             直接滚动          targetPage = floor(312 / 5000) + 1 = 1
             到标题           加载第 1 页 → 渲染 → 滚动到标题
```

### 前端公式

```typescript
const LINES_PER_PAGE = 5000   // 每页行数常量

// 总页数
const totalPages = computed(() => Math.ceil(totalLines.value / LINES_PER_PAGE))

// 当前页起始行
const pageLineStart = computed(() => (currentPage.value - 1) * LINES_PER_PAGE)

// 加载某页
async function loadPage(page: number) {
  const res = await getMarkdown(id, {
    line_start: (page - 1) * LINES_PER_PAGE,
    line_count: LINES_PER_PAGE
  })
  applyContent(res)
}

// 目录跳转
async function jumpToToc(item: TocItem) {
  const targetPage = Math.floor(item.line / LINES_PER_PAGE) + 1
  if (targetPage === currentPage.value) {
    // 同页：DOM 搜索标题文本 → scrollIntoView
    scrollToHeading(item.title)
  } else {
    // 跨页：加载目标页 → 渲染 → 滚动
    await loadPage(targetPage)
    await nextTick()
    scrollToHeading(item.title)
  }
  tocOpen.value = false
}
```

> 为什么不用后端返回 `page_total` 之类的字段？因为分页是前端常量，前端自己算 `totalPages = ceil(totalLines / 5000)` 就够了，后端不需要知道"页"这个概念。

---

## 📝 前端变更

### 类型（`types/document.ts`）

```typescript
export interface TocItem {
  level: number
  title: string
  line: number       // 0-indexed
}

export interface MarkdownResponse {
  content: string
  total_length: number
  total_lines: number
  line_start: number
  line_count: number
  lines_returned: number
}

// 分页请求参数
export interface MarkdownParams {
  line_start?: number
  line_count?: number
  full?: boolean
}
```

### API（`api/documents.ts`）

```typescript
export async function getMarkdown(id: number, params?: MarkdownParams): Promise<MarkdownResponse>
export async function getTOC(id: number): Promise<TocItem[]>
```

### MarkdownView.vue — 核心状态

```typescript
const LINES_PER_PAGE = 5000

const content = ref('')
const totalLength = ref(0)
const totalLines = ref(0)
const currentPage = ref(1)
const linesReturned = ref(0)   // 当前实际返回行数

// 计算属性
const totalPages = computed(() => Math.ceil(totalLines.value / LINES_PER_PAGE))
const showPagination = computed(() => totalPages.value > 1)
const pageLineStart = computed(() => (currentPage.value - 1) * LINES_PER_PAGE)
const pageLineEnd = computed(() => pageLineStart.value + linesReturned.value)

// 目录相关
const tocOpen = ref(false)
const toolbarOpen = ref(false)
const toc = ref<TocItem[]>([])
```

### load() 和 loadTOC() 并行

```typescript
onMounted(async () => {
  const id = parseInt(route.params.id as string)
  if (!requireToken()) return

  loading.value = true
  try {
    const [res, tocRes] = await Promise.all([
      getMarkdown(id, { line_start: 0, line_count: LINES_PER_PAGE }),
      getTOC(id)
    ])
    applyContent(res)
    toc.value = tocRes
  } catch (err) {
    error.value = '加载失败'
    showError('加载失败')
  } finally {
    loading.value = false
  }
})
```

### MarkdownViewer 组件 — 图片占位符

**新增 Prop**：

```typescript
const props = defineProps<{
  // ... 现有 props
  placeholderImages?: boolean  // 默认 true
}>()
```

**图片渲染**：当 `placeholderImages=true` 时，MarkdownViewer 的图片渲染器将 `<img>` 替换为可点击占位符。用户点击占位符 → 加载该图。工具栏「加载全部图片」→ 传 `placeholderImages=false` 重新渲染。

---

## 🎨 MarkdownView.vue 布局

### 右下角浮动区域

```html
<div class="reader-floatbar">
  <!-- 分页控件（仅多页时显示） -->
  <div v-if="showPagination" class="float-pagination">
    <button @click="prevPage" title="上一页">‹</button>
    <button class="page-btn" @click="showPageJump = !showPageJump">
      {{ currentPage }}/{{ totalPages }}
    </button>
    <button @click="nextPage" title="下一页">›</button>
  </div>

  <!-- 主按钮 -->
  <div class="float-actions">
    <button @click="tocOpen = !tocOpen" title="目录">☰</button>
    <button @click="toolbarOpen = !toolbarOpen" title="工具栏">⚙</button>
  </div>
</div>
```

### 目录抽屉

```html
<Transition name="drawer-slide">
  <div v-if="tocOpen" class="toc-overlay" @click.self="tocOpen = false">
    <div class="toc-drawer">
      <div class="toc-header">
        <h3>目录</h3>
        <button @click="tocOpen = false">✕</button>
      </div>
      <div class="toc-items">
        <div v-for="item in toc" :key="item.line"
          class="toc-item"
          :class="{
            'toc-l1': item.level === 1,
            'toc-l2': item.level === 2,
            'toc-l3': item.level >= 3,
            'toc-active': tocActiveLine === item.line
          }"
          @click="jumpToToc(item)">
          {{ item.title }}
        </div>
      </div>
    </div>
  </div>
</Transition>
```

### 工具栏 Popover

```html
<Teleport to="body">
  <div v-if="toolbarOpen" class="toolbar-overlay" @click.self="toolbarOpen = false">
    <div class="toolbar-popover">
      <button @click="goBack">← 返回</button>
      <button @click="triggerSearch">🔍 搜索 (Ctrl+F)</button>
      <hr />
      <label>数学渲染器:</label>
      <select v-model="mathMode" @change="reloadViewer">
        <option value="none">不渲染</option>
        <option value="katex">KaTeX</option>
        <option value="mathjax">MathJax</option>
      </select>
      <hr />
      <label><input v-model="showSource" type="checkbox" /> 显示源码</label>
      <label><input v-model="loadAllImages" type="checkbox" @change="toggleImages" /> 加载全部图片</label>
    </div>
  </div>
</Teleport>
```

---

## 📋 文件变更清单

| 文件 | 改动 |
|------|------|
| `backend-rs/src/routers/documents.rs` | 移除 `split_into_pages()`；改 `MarkdownQuery` 为 `line_start`/`line_count`；改 `get_markdown()` 为行切片逻辑；新增 `GET /:id/toc` 路由 + `get_toc()` |
| `frontend/src/types/document.ts` | 新增 `TocItem`、`MarkdownResponse`、`MarkdownParams` |
| `frontend/src/api/documents.ts` | 新增 `getTOC()`；改 `getMarkdown()` 参数为 `MarkdownParams` |
| `frontend/src/components/MarkdownViewer.vue` | 新增 `placeholderImages` prop + 图片点击加载 |
| `frontend/src/views/MarkdownView.vue` | **重写**：全屏布局 + 右下角浮动 + 目录抽屉 + 工具栏 Popover + 前端分页逻辑 |
| `frontend/src/views/DetailView.vue` | 适配新的 `MarkdownResponse` 类型（去掉 `page`/`total_pages`，用 `total_lines` 和 `LINES_PER_PAGE` 自算） |

---

## ✅ 验收标准

**后端**：
- [ ] 移除 `split_into_pages()`，`get_markdown` 仅做行切片
- [ ] `GET /:id/markdown?line_start=0&line_count=5000` 正确返回切片
- [ ] `GET /:id/markdown?full=true` 返回全部内容
- [ ] 响应中包含 `total_lines`、`total_length`
- [ ] `GET /:id/toc` 返回标题 + 行号
- [ ] `cargo build` 编译通过，无 warning

**前端**：
- [ ] 阅读器无顶部工具栏，内容全宽显示
- [ ] 右下角有「☰ 目录」「⚙ 工具栏」两个按钮
- [ ] 目录加载与正文并行（`Promise.all`）
- [ ] 目录抽屉显示标题层级树
- [ ] 点击目录项：同页直接滚动，跨页加载目标页再滚动
- [ ] 图片默认显示为可点击占位符，点击后加载该图
- [ ] 工具栏可切换 KaTeX/MathJax/不渲染
- [ ] 工具栏可切换源码模式
- [ ] 搜索按钮触发浏览器原生 Ctrl+F
- [ ] 分页（多页时）右下角显示 `‹ 当前页/总页数 ›`
- [ ] 分页支持上一页/下一页/点击页码弹出跳转输入
- [ ] 每页默认 5000 行
- [ ] 移动端触摸友好（≥44px 触摸目标）
- [ ] `vite build` 编译通过