<template>
  <nav class="app-nav">
    <router-link
      v-for="item in navItems"
      :key="item.path"
      :to="item.path"
      active-class="router-link-active"
    >
      {{ item.label }}
    </router-link>
  </nav>
</template>

<script setup lang="ts">
import { computed } from 'vue'
import { useTokenStore } from '@/stores/token'

const tokenStore = useTokenStore()

const adminOnlyPaths = ['/assist/admin', '/assist/config', '/assist/logs']

const allNavItems = [
  { path: '/assist', label: '文献列表' },
  { path: '/assist/upload', label: '上传' },
  { path: '/assist/tools', label: '工具' },
  { path: '/assist/admin', label: '管理' },
  { path: '/assist/config', label: '配置' },
  { path: '/assist/logs', label: '日志' },
]

const navItems = computed(() => {
  if (tokenStore.role === 'admin') {
    return allNavItems
  }
  return allNavItems.filter(item => !adminOnlyPaths.includes(item.path))
})
</script>