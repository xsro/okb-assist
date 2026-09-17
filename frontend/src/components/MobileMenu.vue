<template>
  <div class="mobile-menu-overlay" @click="closeMenu">
    <div class="mobile-menu-panel" @click.stop>
      <div class="mobile-menu-header">
        <span class="logo">OKB-Assist</span>
        <button class="mobile-menu-close" @click="closeMenu" aria-label="关闭菜单">×</button>
      </div>
      <nav class="mobile-menu-items">
        <router-link
          v-for="item in navItems"
          :key="item.path"
          :to="item.path"
          class="mobile-menu-item"
          active-class="router-link-active"
          @click="closeMenu"
        >
          {{ item.label }}
        </router-link>
      </nav>
      <div class="mobile-menu-footer">
        <button v-if="!tokenStore.isAuthenticated" class="btn" @click="promptToken">
          设置 Token
        </button>
        <span v-else class="token-status">已连接</span>
      </div>
    </div>
  </div>
</template>

<script setup lang="ts">
import { inject } from 'vue'
import { useTokenStore } from '@/stores/token'

const tokenStore = useTokenStore()

const closeMenu = inject('closeMobileMenu') as (() => void) | undefined

const navItems = [
  { path: '/assist', label: '文献列表' },
  { path: '/assist/upload', label: '上传' },
  { path: '/assist/tools', label: '工具' },
  { path: '/assist/monitor', label: '监控' },
  { path: '/assist/admin', label: '管理' },
  { path: '/assist/config', label: '配置' },
  { path: '/assist/duplicates', label: '去重' },
  { path: '/assist/point', label: '向量库' },
  { path: '/assist/mcp-setup', label: 'MCP 配置' }
]

function promptToken() {
  closeMenu?.()
  tokenStore.promptForToken()
}
</script>

<style scoped>
.mobile-menu-overlay {
  position: fixed;
  inset: 0;
  background: rgba(0, 0, 0, 0.4);
  z-index: 90;
  animation: fadeOverlay 0.2s ease;
}

@keyframes fadeOverlay {
  from { opacity: 0; }
  to { opacity: 1; }
}

.mobile-menu-panel {
  position: fixed;
  top: 0;
  right: 0;
  bottom: 0;
  width: 280px;
  max-width: 80vw;
  background: #fff;
  z-index: 91;
  display: flex;
  flex-direction: column;
  animation: slideInPanel 0.25s ease;
  box-shadow: -4px 0 24px rgba(0, 0, 0, 0.15);
}

@keyframes slideInPanel {
  from { transform: translateX(100%); }
  to { transform: translateX(0); }
}

.mobile-menu-header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 16px 20px;
  border-bottom: 1px solid var(--border);
  background: #fafafa;
}

.mobile-menu-header .logo {
  font-size: 18px;
  font-weight: 700;
  color: var(--text);
}

.mobile-menu-close {
  background: transparent;
  border: none;
  font-size: 28px;
  color: var(--text-secondary);
  cursor: pointer;
  padding: 4px 8px;
  min-width: 44px;
  min-height: 44px;
  border-radius: 6px;
  line-height: 1;
}

.mobile-menu-close:hover {
  background: var(--bg);
  color: var(--text);
}

.mobile-menu-items {
  flex: 1;
  overflow-y: auto;
  -webkit-overflow-scrolling: touch;
  padding: 8px 0;
}

.mobile-menu-item {
  display: flex;
  align-items: center;
  padding: 14px 20px;
  font-size: 16px;
  color: var(--text);
  border-bottom: 1px solid #f0f0f0;
  transition: background 0.15s;
  min-height: 48px;
}

.mobile-menu-item:hover {
  background: var(--bg);
}

.mobile-menu-item.router-link-active {
  background: #e3f2fd;
  color: var(--primary);
  font-weight: 600;
}

.mobile-menu-footer {
  padding: 16px 20px;
  border-top: 1px solid var(--border);
  background: #fafafa;
}

.mobile-menu-footer .btn {
  width: 100%;
}

.token-status {
  display: block;
  text-align: center;
  font-size: 14px;
  color: var(--success);
  font-weight: 500;
}
</style>