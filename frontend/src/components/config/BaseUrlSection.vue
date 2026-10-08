<template>
  <div class="section">
    <h3>部署地址</h3>
    <div class="form-group">
      <label>Base URL</label>
      <input v-model="modelValue.base_url" type="text" placeholder="https://xsro20.xyz" @input="emitUpdate" />
    </div>
    <p class="hint">部署的基础地址，用于拼接文档链接（pdf_need_auth_url / markdown_url / detail_url）。留空则使用相对路径。</p>
  </div>
</template>

<script setup lang="ts">
import type { ServiceConfig } from '@/types/config'

const props = defineProps<{
  modelValue: ServiceConfig
}>()

const emit = defineEmits<{
  (e: 'update:modelValue', val: ServiceConfig): void
}>()

function emitUpdate() {
  emit('update:modelValue', { ...props.modelValue })
}
</script>

<style scoped>
.hint {
  font-size: 12px;
  color: var(--text-secondary);
  margin: 8px 0 0;
}
</style>