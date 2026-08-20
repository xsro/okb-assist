<template>
  <div class="toast-container">
    <div
      v-for="t in toasts"
      :key="t.id"
      class="toast"
      :class="t.type"
    >
      <button class="toast-close" @click.stop="remove(t.id)">×</button>
      <span class="toast-message">{{ t.message }}</span>
    </div>
  </div>
</template>

<script setup lang="ts">
import { useToastStore } from '@/stores/toast'

const { toasts, remove } = useToastStore()
</script>
