<template>
  <div id="app">
    <AppHeader />
    <Toast />
    <main class="main-content">
      <router-view />
    </main>
    <TokenModal />
    <footer class="app-footer">
      <span>OKB-Assist</span>
    </footer>
  </div>
</template>

<script setup lang="ts">
import { onMounted } from 'vue'
import { useTokenStore } from '@/stores/token'
import AppHeader from '@/components/AppHeader.vue'
import Toast from '@/components/Toast.vue'
import TokenModal from '@/components/TokenModal.vue'

const tokenStore = useTokenStore()

onMounted(() => {
  window.addEventListener('auth:required', () => {
    tokenStore.promptForToken()
  })
})
</script>

<style scoped>
.main-content {
  min-height: calc(100vh - 56px);
  padding: 20px 24px;
  max-width: 1400px;
  margin: 0 auto;
}
.app-footer {
  text-align: center;
  padding: 16px;
  color: #999;
  font-size: 12px;
  border-top: 1px solid #eee;
  background: #fafafa;
}
</style>