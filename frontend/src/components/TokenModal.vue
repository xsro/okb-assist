<template>
  <div v-if="visible" class="modal-overlay" @click.self="close">
    <div class="modal">
      <h3>需要访问 Token</h3>
      <p>请输入 system.json 中配置的 token 以继续访问 API。</p>
      <input
        v-model="inputToken"
        type="password"
        placeholder="输入 Token"
        @keyup.enter="submit"
      />
      <div class="modal-actions">
        <button class="btn" @click="submit">确定</button>
        <button class="btn btn-outline" @click="close">取消</button>
      </div>
    </div>
  </div>
</template>

<script setup lang="ts">
import { computed, ref } from 'vue'
import { useTokenStore } from '@/stores/token'

const tokenStore = useTokenStore()

const visible = computed({
  get: () => tokenStore.showTokenModal,
  set: (v) => {
    if (!v) tokenStore.closeTokenModal()
  }
})

const inputToken = ref('')

function submit() {
  if (inputToken.value.trim()) {
    tokenStore.setToken(inputToken.value.trim())
    tokenStore.closeTokenModal()
    inputToken.value = ''
  }
}

function close() {
  tokenStore.closeTokenModal()
  inputToken.value = ''
}
</script>

<style scoped>
.modal-actions {
  display: flex;
  gap: 8px;
  justify-content: flex-end;
  margin-top: 20px;
}
.modal input {
  width: 100%;
  margin-top: 8px;
}
</style>