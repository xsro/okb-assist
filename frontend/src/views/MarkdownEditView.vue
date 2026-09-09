<template>
  <div class="markdown-edit-view">
    <div class="edit-header">
      <h2>编辑 Markdown</h2>
      <div class="edit-actions">
        <button class="btn btn-outline" @click="goBack">返回</button>
        <button class="btn" @click="save" :disabled="saving">保存</button>
      </div>
    </div>

    <AceEditor
      v-model="content"
      mode="markdown"
      theme="monokai"
      :showToolbar="true"
      :fontSize="15"
    />

    <ConfirmDialog ref="confirmDlg" />
  </div>
</template>

<script setup lang="ts">
import { ref, onMounted } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import { getMarkdown, saveMarkdown } from '@/api/documents'
import { useToast } from '@/composables/useToast'
import { useRequireToken } from '@/composables/useRequireToken'
import AceEditor from '@/components/AceEditor.vue'
import ConfirmDialog from '@/components/ConfirmDialog.vue'

const route = useRoute()
const router = useRouter()
const { showSuccess, showError } = useToast()
const { requireToken } = useRequireToken()

const content = ref('')
const saving = ref(false)
const confirmDlg = ref<InstanceType<typeof ConfirmDialog> | null>(null)

async function load() {
  const id = parseInt(route.params.id as string)
  if (!requireToken()) return
  try {
    const res = await getMarkdown(id, { full: true })
    content.value = res.content
  } catch {
    showError('加载失败')
  }
}

async function save() {
  const confirmed = await confirmDlg.value?.show('保存确认', '确定要保存当前修改吗？')
  if (!confirmed) return

  const id = parseInt(route.params.id as string)
  saving.value = true
  try {
    await saveMarkdown(id, content.value)
    showSuccess('已保存')
  } catch {
    showError('保存失败')
  } finally {
    saving.value = false
  }
}

function goBack() {
  router.back()
}

onMounted(load)
</script>

<style scoped>
.markdown-edit-view {
  display: flex;
  flex-direction: column;
  height: calc(100vh - 60px);
  padding: 16px;
  gap: 16px;
}

.edit-header {
  display: flex;
  justify-content: space-between;
  align-items: center;
  flex-shrink: 0;
}

.edit-actions {
  display: flex;
  gap: 8px;
}

.btn {
  padding: 8px 16px;
  border: 1px solid var(--border);
  border-radius: 6px;
  background: #fff;
  cursor: pointer;
  font-size: 14px;
  transition: all 0.2s;
}

.btn:hover:not(:disabled) {
  background: var(--primary-color, #4a90d9);
  color: #fff;
  border-color: var(--primary-color, #4a90d9);
}

.btn:disabled {
  opacity: 0.6;
  cursor: not-allowed;
}

.btn-outline {
  background: transparent;
}
</style>