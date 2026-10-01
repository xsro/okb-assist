import { createRouter, createWebHistory } from 'vue-router'
import { useTokenStore } from '@/stores/token'

const routes = [
  {
    path: '/',
    redirect: '/assist'
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
    meta: { title: '文档详情' }
  },
  {
    path: '/assist/upload',
    name: 'upload',
    component: () => import('@/views/UploadView.vue'),
    meta: { title: '上传文献' }
  },
  {
    path: '/assist/tools',
    name: 'tools',
    component: () => import('@/views/ToolsView.vue'),
    meta: { title: '工具面板' }
  },
  {
    path: '/assist/admin',
    name: 'admin',
    component: () => import('@/views/AdminView.vue'),
    meta: { title: '管理后台', requiresAdmin: true }
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
    meta: { title: 'Markdown 编辑', requiresAdmin: true }
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
    path: '/assist/mcp-setup',
    name: 'mcpSetup',
    component: () => import('@/views/McpSetupView.vue'),
    meta: { title: 'MCP 配置', requiresAdmin: true }
  },
  {
    path: '/assist/logs',
    name: 'logs',
    component: () => import('@/views/LogViewer.vue'),
    meta: { title: '日志' }
  }
]

const router = createRouter({
  history: createWebHistory(),
  routes
})

// 路由守卫：权限检查 + 设置页面标题
router.beforeEach((to, _from, next) => {
  const title = (to.meta?.title as string) || 'OKB-Assist'
  document.title = `${title} - OKB-Assist`

  const store = useTokenStore()
  const role = store.role
  const hasToken = store.isAuthenticated

  // role 为 null 但已有 token（正在验证中），放行让 api 鉴权
  if (!role && hasToken) {
    next()
    return
  }

  // requiresAdmin: 仅 admin 可访问
  if (to.meta.requiresAdmin && role !== 'admin') {
    if (!hasToken) {
      store.promptForToken()
    }
    next('/assist')
    return
  }

  // 上传页面：admin 或 view-upload 可访问
  if (to.name === 'upload' && role !== 'admin' && role !== 'view-upload') {
    if (!hasToken) {
      store.promptForToken()
    }
    next('/assist')
    return
  }

  next()
})

export default router