import { createRouter, createWebHistory } from 'vue-router'

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
    meta: { title: '管理后台' }
  },
  {
    path: '/assist/monitor',
    name: 'monitor',
    component: () => import('@/views/MonitorView.vue'),
    meta: { title: '流水线监控' }
  },
  {
    path: '/assist/config',
    name: 'config',
    component: () => import('@/views/ConfigView.vue'),
    meta: { title: '服务配置' }
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
    meta: { title: 'Markdown 查看' }
  },
  {
    path: '/assist/markdown/:id/edit',
    name: 'markdownEdit',
    component: () => import('@/views/MarkdownEditView.vue'),
    meta: { title: 'Markdown 编辑' }
  },
  {
    path: '/assist/duplicates',
    name: 'duplicates',
    component: () => import('@/views/DuplicatesView.vue'),
    meta: { title: '去重' }
  },
  {
    path: '/assist/point',
    name: 'point',
    component: () => import('@/views/PointView.vue'),
    meta: { title: '向量库管理' }
  },
  {
    path: '/assist/mcp-setup',
    name: 'mcpSetup',
    component: () => import('@/views/McpSetupView.vue'),
    meta: { title: 'MCP 配置' }
  }
]

const router = createRouter({
  history: createWebHistory(),
  routes
})

// 路由守卫：设置页面标题
router.beforeEach((to, _from, next) => {
  const title = (to.meta?.title as string) || 'OKB-Assist'
  document.title = `${title} - OKB-Assist`
  next()
})

export default router