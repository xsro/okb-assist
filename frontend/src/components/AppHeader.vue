<template>
  <header class="app-header">
    <div class="header-inner">
      <router-link :to="{ name: 'home' }" class="logo">OKB-Assist</router-link>
      <AppNav />
      <div class="header-actions">
        <button v-if="!tokenStore.isAuthenticated" class="btn btn-sm" @click="tokenStore.promptForToken">
          设置 Token
        </button>
        <span v-else class="token-status">已连接</span>
      </div>
    </div>
  </header>
</template>

<script setup lang="ts">
import { useTokenStore } from '@/stores/token'
import AppNav from '@/components/AppNav.vue'

const tokenStore = useTokenStore()
</script>

<style scoped>
.header-actions {
  display: flex;
  align-items: center;
}
.token-status {
  font-size: 13px;
  color: var(--success);
  font-weight: 500;
}
</style>