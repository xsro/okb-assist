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
const navItems = [
  { path: '/assist', label: '文献列表' },
  { path: '/assist/upload', label: '上传' },
  { path: '/assist/tools', label: '工具' },
  { path: '/assist/monitor', label: '监控' },
  { path: '/assist/admin', label: '管理' },
  { path: '/assist/config', label: '配置' }
]
</script>