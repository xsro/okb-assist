<template>
  <div class="upload-view">
    <h2>上传文献</h2>

    <!-- 拖拽上传 -->
    <div
      class="upload-area"
      :class="{ dragover: isDragover }"
      @dragover.prevent="isDragover = true"
      @dragleave.prevent="isDragover = false"
      @drop.prevent="onDrop"
      @click="$refs.fileInput.click()"
    >
      <svg class="upload-icon" width="48" height="48" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
        <path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4"></path>
        <polyline points="17 8 12 3 7 8"></polyline>
        <line x1="12" y1="3" x2="12" y2="15"></line>
      </svg>
      <p>点击或拖拽 PDF 文件到此处上传</p>
      <input
        ref="fileInput"
        type="file"
        accept=".pdf"
        multiple
        @change="onFileSelect"
        style="display: none"
      />
    </div>

    <!-- 按路径登记 -->
    <div class="section">
      <h3>按路径登记</h3>
      <div class="form-group">
        <label>文件路径</label>
        <input v-model="filePath" type="text" placeholder="/path/to/document.pdf" />
      </div>
      <div class="form-group">
        <label>标题（可选）</label>
        <input v-model="title" type="text" placeholder="文档标题" />
      </div>
      <button class="btn" @click="register">登记</button>
    </div>

    <!-- 上传队列 -->
    <div v-if="uploadQueue.length > 0" class="section">
      <h3>上传队列</h3>
      <div v-for="item in uploadQueue" :key="item.name" class="upload-queue-item">
        <span class="file-name">{{ item.name }}</span>
        <span class="file-status" :class="item.status">
          {{ item.status === 'uploading' ? '上传中...' : item.status === 'done' ? '完成' : '失败' }}
        </span>
      </div>
    </div>
  </div>
</template>

<script setup lang="ts">
import { ref } from 'vue'
import { uploadPdf, registerPath } from '@/api/documents'
import { useToast } from '@/composables/useToast'
import { useRequireToken } from '@/composables/useRequireToken'

const { showSuccess, showError } = useToast()
const { requireToken } = useRequireToken()

const isDragover = ref(false)
const fileInput = ref<HTMLInputElement>()
const filePath = ref('')
const title = ref('')
const uploadQueue = ref<{ name: string; status: 'uploading' | 'done' | 'error' }[]>([])

function onDrop(e: DragEvent) {
  isDragover.value = false
  const files = Array.from(e.dataTransfer?.files || [])
  handleFiles(files)
}

function onFileSelect(e: Event) {
  const input = e.target as HTMLInputElement
  const files = Array.from(input.files || [])
  handleFiles(files)
  input.value = '' // 清空以便重复选择同一文件
}

async function handleFiles(files: File[]) {
  if (!requireToken()) return

  for (const file of files) {
      if (file.type !== 'application/pdf') {
        showError(`${file.name} 不是 PDF 文件`)
        continue
      }

      const item = { name: file.name, status: 'uploading' as const }
      uploadQueue.value.push(item)

      try {
        await uploadPdf(file)
        item.status = 'done'
        showSuccess(`${file.name} 上传成功`)
      } catch {
        item.status = 'error'
        showError(`${file.name} 上传失败`)
      }
    }
  }

async function register() {
  if (!filePath.value.trim()) {
    showError('请输入文件路径')
    return
  }
  if (!requireToken()) return
  try {
    await registerPath(filePath.value.trim())
    showSuccess('登记成功')
    filePath.value = ''
    title.value = ''
  } catch {
    showError('登记失败')
  }
}
</script>

<style scoped>
.upload-area {
  border: 2px dashed var(--border);
  border-radius: 12px;
  padding: 40px;
  text-align: center;
  cursor: pointer;
  transition: all 0.15s;
  margin-bottom: 24px;
}

.upload-area:hover,
.upload-area.dragover {
  border-color: var(--primary);
  background: #e3f2fd;
}

.upload-icon {
  display: block;
  margin: 0 auto 12px;
  color: var(--text-secondary);
}

.upload-area p {
  color: var(--text-secondary);
  font-size: 14px;
}

.upload-queue-item {
  display: flex;
  justify-content: space-between;
  align-items: center;
  padding: 8px 0;
  border-bottom: 1px solid var(--border);
}

.file-name {
  font-size: 14px;
}

.file-status {
  font-size: 13px;
  padding: 2px 8px;
  border-radius: 10px;
}

.file-status.uploading { background: #e3f2fd; color: #1565c0; }
.file-status.done { background: #e8f5e9; color: #2e7d32; }
.file-status.error { background: #ffebee; color: #c62828; }
</style>