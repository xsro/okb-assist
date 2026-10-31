<template>
  <header class="app-header">
    <div class="header-inner">
      <router-link :to="{ name: 'home' }" class="logo">OKB-Assist</router-link>
      <AppNav class="desktop-nav" />
      <div class="header-actions">
        <button class="mobile-menu-btn" @click="toggleMenu" aria-label="菜单">
          <span class="hamburger" :class="{ open: menuOpen }">
            <span></span>
            <span></span>
            <span></span>
          </span>
        </button>
        <router-link v-if="tokenStore.isAuthenticated" to="/assist/account" class="account-link desktop-token">
          账户
        </router-link>
        <div v-if="tokenStore.isAuthenticated" class="token-status-wrap desktop-token" @click="tokenStore.promptForToken">
          <div class="token-status">
            <span class="token-status-text">{{ tokenStore.displayName || '已登录' }}</span>
            <span class="token-status-role">{{ tokenStore.roleLabel }}</span>
          </div>
          <span class="token-arrow">▾</span>
        </div>
        <button v-else class="btn btn-sm desktop-token" @click="goLogin">登录</button>
      </div>
    </div>
  </header>
</template>

<script setup lang="ts">
import { ref, inject, onMounted, onUnmounted } from 'vue'
import { useRouter } from 'vue-router'
import { useTokenStore } from '@/stores/token'
import AppNav from '@/components/AppNav.vue'

const tokenStore = useTokenStore()
const router = useRouter()
const menuOpen = ref(false)

const toggleMenu = inject('toggleMobileMenu') as (() => void) | undefined

function goLogin() {
  router.push({ path: '/assist/login' })
}
const closeMenu = inject('closeMobileMenu') as (() => void) | undefined

function handleMenuClick() {
  if (toggleMenu) {
    toggleMenu()
    menuOpen.value = !menuOpen.value
  }
}

function closeMenuHandler() {
  if (closeMenu) {
    closeMenu()
    menuOpen.value = false
  }
}

onMounted(() => {
  window.addEventListener('close-mobile-menu', closeMenuHandler)
})

onUnmounted(() => {
  window.removeEventListener('close-mobile-menu', closeMenuHandler)
})
</script>

<style scoped>
.header-actions {
  display: flex;
  align-items: center;
  gap: 8px;
}
.token-status {
  display: flex;
  flex-direction: column;
  align-items: flex-start;
  line-height: 1.3;
  cursor: pointer;
}

.token-status-text {
  font-size: 13px;
  color: var(--success);
  font-weight: 500;
}

.token-status-role {
  font-size: 11px;
  color: var(--text-secondary);
  font-weight: 400;
}

.token-status-wrap {
  position: relative;
  display: inline-flex;
  align-items: center;
  gap: 4px;
  cursor: pointer;
  padding: 4px 8px;
  border-radius: 6px;
  user-select: none;
}

.token-status-wrap:hover {
  background: var(--bg-secondary);
}

.token-arrow {
  font-size: 10px;
  color: var(--text-secondary);
}

/* 桌面端显示导航 */
.desktop-nav {
  display: flex;
}

/* 桌面端显示 Token 按钮 */
.desktop-token {
  display: inline-flex;
}

/* 移动端菜单按钮 */
.mobile-menu-btn {
  display: none;
  background: transparent;
  border: none;
  cursor: pointer;
  padding: 8px;
  min-width: 44px;
  min-height: 44px;
  border-radius: 6px;
  align-items: center;
  justify-content: center;
}

.mobile-menu-btn:hover {
  background: var(--bg);
}

.hamburger {
  display: flex;
  flex-direction: column;
  gap: 5px;
  width: 24px;
  height: 24px;
  justify-content: center;
  align-items: center;
}

.hamburger span {
  display: block;
  width: 22px;
  height: 2px;
  background: var(--text);
  border-radius: 1px;
  transition: all 0.2s ease;
}

.hamburger.open span:nth-child(1) {
  transform: translateY(7px) rotate(45deg);
}

.hamburger.open span:nth-child(2) {
  opacity: 0;
}

.hamburger.open span:nth-child(3) {
  transform: translateY(-7px) rotate(-45deg);
}

@media (max-width: 768px) {
  .desktop-nav {
    display: none !important;
  }

  .desktop-token {
    display: none !important;
  }

  .mobile-menu-btn {
    display: flex !important;
  }
}
</style>