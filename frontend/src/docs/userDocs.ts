/**
 * 用户文档（使用文档）清单与加载器。
 *
 * **内容文件在 `frontend/public/docs/*.md`**（跟着 `pnpm run build` 复制到 `dist/docs/`，
 * 可以直接用 `/assist/docs/mcp.md` 打开原始 Markdown）；本文件只维护「目录顺序 / 标题 / 简介」
 * 这类导航元数据，并负责在构建时把 Markdown 原文打进前端包（dev 与生产都不需要额外 HTTP 请求）。
 *
 * 新增一篇用户文档的步骤：
 * 1. 在 `frontend/public/docs/` 下新建 `xxx.md`；
 * 2. 在下面 `DOC_GROUPS` 的某个分组里加一条 `{ slug: 'xxx', file: 'xxx.md', title, desc }`；
 * 3. 开发文档（部署 / 设计 / 计划）仍然放仓库根目录的 `document/`（docsify 站点），不要混进来。
 */

export interface UserDoc {
  /** 路由片段：`/assist/docs/<slug>`（同时用于页面内的 .md 互链解析） */
  slug: string
  /** `public/docs/` 下的文件名 */
  file: string
  /** 侧栏与页面标题 */
  title: string
  /** 侧栏一句话简介 */
  desc: string
}

export interface UserDocGroup {
  title: string
  items: UserDoc[]
}

export const DOC_GROUPS: UserDocGroup[] = [
  {
    title: '开始使用',
    items: [
      {
        slug: 'user-management',
        file: 'user-management.md',
        title: '用户与令牌',
        desc: '登录、注册、个人令牌（API / MCP）、管理员审核',
      },
      {
        slug: 'drive',
        file: 'drive.md',
        title: '用户网盘',
        desc: '私有目录 + 公有文件夹、配额与只读规则',
      },
    ],
  },
  {
    title: '检索',
    items: [
      {
        slug: 'search',
        file: 'search.md',
        title: '高级搜索语法',
        desc: 'WoS 风格检索式：字段标签、布尔运算、通配符与范围',
      },
    ],
  },
  {
    title: 'MCP 与 AI 助手',
    items: [
      {
        slug: 'mcp',
        file: 'mcp.md',
        title: 'MCP 连接指南',
        desc: '令牌类型、传输端点、各客户端配置方法',
      },
      {
        slug: 'mcp-tools',
        file: 'mcp-tools.md',
        title: 'MCP 工具列表',
        desc: '全部工具的入参与返回说明',
      },
    ],
  },
]

/** 扁平清单（顺序即上一篇 / 下一篇的顺序） */
export const DOC_INDEX: UserDoc[] = DOC_GROUPS.flatMap((g) => g.items)

export function findDoc(slug: string): UserDoc | undefined {
  return DOC_INDEX.find((d) => d.slug === slug)
}

/** 原始 Markdown 的直链（`/assist/docs/<file>`，后端按 `text/markdown` 返回） */
export function rawDocUrl(file: string): string {
  return `${import.meta.env.BASE_URL}docs/${file}`
}

// 构建期把 `public/docs/*.md` 以原文形式登记为动态 import（惰性加载，每篇一个 chunk）。
// 注意：`public/` 目录不能通过普通 import 使用，必须用 glob + `?raw`。
const RAW_MODULES = import.meta.glob('../../public/docs/*.md', {
  query: '?raw',
  import: 'default',
}) as Record<string, () => Promise<string>>

/** 读取某篇文档的 Markdown 原文 */
export async function loadDocContent(file: string): Promise<string> {
  const key = Object.keys(RAW_MODULES).find((k) => k.endsWith(`/${file}`))
  if (!key) {
    throw new Error(`文档文件不存在：public/docs/${file}`)
  }
  return RAW_MODULES[key]()
}
