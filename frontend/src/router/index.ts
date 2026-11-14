import { createRouter, createWebHistory } from 'vue-router'
import { useTokenStore } from '@/stores/token'

const routes = [
  {
    path: '/',
    redirect: '/assist'
  },
  {
    path: '/assist/login',
    name: 'login',
    component: () => import('@/views/LoginView.vue'),
    meta: { title: '登录', public: true }
  },
  {
    path: '/assist/account',
    name: 'account',
    component: () => import('@/views/AccountView.vue'),
    meta: { title: '账户' }
  },
  {
    // 用户管理已并入「配置」页的 tab；保留旧路径做重定向（书签/文档链接兼容）
    path: '/assist/users',
    redirect: (to) => ({ path: '/assist/config', query: { ...to.query, tab: 'users' } })
  },
  {
    // MCP 配置引导已并入「工具」页的 MCP 配置 tab；保留旧路径做重定向（书签/文档链接兼容）
    path: '/assist/mcp-setup',
    redirect: { path: '/assist/tools', query: { tab: 'mcp' } }
  },
  {
    path: '/assist',
    name: 'home',
    component: () => import('@/views/HomeView.vue'),
    meta: { title: '文献列表' }
  },
  {
    path: '/assist/detail/:id',
    name: 'detail',
    component: () => import('@/views/DetailView.vue'),
    // 访客可读：未登录时也放行，页面自带登录提示；
    // 后端仅在 `system.json` 的 `public_detail_enabled = true` 时提供公开信息。
    meta: { title: '文档详情', allowGuest: true }
  },
  {
    path: '/assist/upload',
    name: 'upload',
    component: () => import('@/views/UploadView.vue'),
    meta: { title: '上传文献' }
  },
  {
    path: '/assist/drive',
    name: 'drive',
    component: () => import('@/views/DriveView.vue'),
    meta: { title: '网盘' }
  },
  {
    path: '/assist/drive/admin',
    name: 'driveAdmin',
    component: () => import('@/views/DriveAdminView.vue'),
    meta: { title: '网盘管理', requiresAdmin: true }
  },
  {
    path: '/assist/docs/:slug?',
    name: 'docs',
    component: () => import('@/views/DocsView.vue'),
    // 使用文档阅读页（`frontend/public/docs/` 下的 Markdown），需登录
    meta: { title: '使用文档' }
  },
  {
    path: '/assist/tools',
    name: 'tools',
    component: () => import('@/views/ToolsView.vue'),
    meta: { title: '工具面板' }
  },
  {
    // 问答已并入「工具」页的问答 tab；保留旧路径做重定向（书签/文档链接兼容）
    path: '/assist/chat',
    redirect: { path: '/assist/tools', query: { tab: 'chat' } }
  },
  {
    path: '/assist/admin',
    name: 'admin',
    component: () => import('@/views/AdminView.vue'),
    meta: { title: '监控面板', requiresAdmin: true }
  },
  {
    path: '/assist/config',
    name: 'config',
    component: () => import('@/views/ConfigView.vue'),
    meta: { title: '服务配置', requiresAdmin: true }
  },
  {
    path: '/assist/doc/:id',
    name: 'docManage',
    component: () => import('@/views/DocManageView.vue'),
    meta: { title: '文档管理' }
  },
  {
    path: '/assist/markdown/:id',
    name: 'markdown',
    component: () => import('@/views/MarkdownView.vue'),
    meta: { title: 'Markdown 查看', requiresViewMarkdown: true }
  },
  {
    path: '/assist/markdown/:id/edit',
    name: 'markdownEdit',
    component: () => import('@/views/MarkdownEditView.vue'),
    meta: { title: 'Markdown 编辑', requiresAdmin: true, bare: true }
  },
  {
    path: '/assist/duplicates',
    name: 'duplicates',
    component: () => import('@/views/DuplicatesView.vue'),
    meta: { title: '去重', requiresAdmin: true }
  },
  {
    path: '/assist/point',
    name: 'point',
    component: () => import('@/views/PointView.vue'),
    meta: { title: '向量库管理', requiresAdmin: true }
  },
  {
    path: '/assist/logs',
    name: 'logs',
    component: () => import('@/views/LogViewer.vue'),
    meta: { title: '日志', requiresAdmin: true }
  }
]

const router = createRouter({
  history: createWebHistory(),
  routes
})

// 路由守卫：登录态检查 + 权限检查 + 设置页面标题
router.beforeEach(async (to, _from, next) => {
  const title = (to.meta?.title as string) || 'OKB-Assist'
  document.title = `${title} - OKB-Assist`

  const store = useTokenStore()

  // 公开路由（登录页）
  if (to.meta?.public === true) {
    next()
    return
  }

  // 刷新页面：本地区有 token 但还没有角色 → 先恢复会话
  if (!store.role && store.token) {
    await store.restore()
  }

  // 未认证 → 访客可读的页面（如文献详情）放行，其余去登录页
  if (!store.role) {
    if (to.meta?.allowGuest === true) {
      next()
      return
    }
    next({ path: '/assist/login', query: { redirect: to.fullPath } })
    return
  }

  // requiresAdmin: 仅 admin 可访问
  if (to.meta.requiresAdmin && store.role !== 'admin') {
    next('/assist')
    return
  }

  // 上传页面：admin 或 view-upload 可访问
  if (to.name === 'upload' && store.role !== 'admin' && store.role !== 'view-upload') {
    next('/assist')
    return
  }

  next()
})

export default router