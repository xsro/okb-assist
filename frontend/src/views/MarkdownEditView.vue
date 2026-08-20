<template>
  <div class="markdown-edit-view">
    <div class="edit-header">
      <h2>编辑 Markdown</h2>
      <div class="edit-actions">
        <button class="btn btn-outline" @click="preview = !preview">
          {{ preview ? '编辑' : '预览' }}
        </button>
        <button class="btn" @click="save" :disabled="saving">保存</button>
      </div>
    </div>

    <div v-if="preview" class="preview-container">
      <MarkdownViewer :content="content" />
    </div>
    <textarea
      v-else
      v-model="content"
      class="edit-textarea"
      placeholder="Markdown 内容..."
    ></textarea>
  </div>
</template>

<script setup lang="ts">
import { ref, onMounted } from 'vue'
import { useRoute } from 'vue-router'
import { getMarkdown, saveMarkdown } from '@/api/documents'
import { useToast } from '@/composables/useToast'
import { useRequireToken } from '@/composables/useRequireToken'
import MarkdownViewer from '@/components/MarkdownViewer.vue'

const route = useRoute()
const { showSuccess, showError } = useToast()
const { requireToken } = useRequireToken()

const content = ref('')
const preview = ref(false)
const saving = ref(false)

async function load() {
  const id = parseInt(route.params.id as string)
  if (!requireToken()) return
  try {
    const res = await getMarkdown(id)
    content.value = res.content
  } catch {
    showError('加载失败')
  }
}

async function save() {
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

onMounted(load)
</script>

<style scoped>
.edit-header {
  display: flex;
  justify-content: space-between;
  align-items: center;
  margin-bottom: 16px;
}
.edit-actions {
  display: flex;
  gap: 8px;
}
.edit-textarea {
  width: 100%;
  height: calc(100vh - 120px);
  padding: 16px;
  border: 1px solid var(--border);
  border-radius: 8px;
  font-family: 'SF Mono', 'Consolas', monospace;
  font-size: 14px;
  line-height: 1.6;
  resize: none;
  background: #1e1e1e;
  color: #d4d4d4;
}
.preview-container {
  border: 1px solid var(--border);
  border-radius: 8px;
  padding: 24px;
  height: calc(100vh - 120px);
  overflow-y: auto;
  background: #fff;
}
</style>