<template>
  <div v-if="visible" class="modal-overlay" @click.self="cancel">
    <div class="modal">
      <h3>{{ title }}</h3>
      <p>{{ message }}</p>
      <div class="modal-actions">
        <button class="btn btn-danger" @click="confirm">确定</button>
        <button class="btn btn-outline" @click="cancel">取消</button>
      </div>
    </div>
  </div>
</template>

<script setup lang="ts">
import { ref } from 'vue'

const visible = ref(false)
const title = ref('')
const message = ref('')
let resolveFn: ((v: boolean) => void) | null = null

function show(t: string, m: string): Promise<boolean> {
  title.value = t
  message.value = m
  visible.value = true
  return new Promise((resolve) => {
    resolveFn = resolve
  })
}

function confirm() {
  visible.value = false
  resolveFn?.(true)
}

function cancel() {
  visible.value = false
  resolveFn?.(false)
}

// 暴露全局调用
(window as any).showConfirm = show
</script>