<template>
  <div class="upload-view">
    <h2>上传文献</h2>

    <!-- 拖拽上传区 -->
    <div
      class="upload-area"
      :class="{ dragover: isDragover }"
      @dragover.prevent="isDragover = true"
      @dragleave.prevent="isDragover = false"
      @drop.prevent="onDrop"
      @click="fileInput?.click()"
    >
      <svg class="upload-icon" width="48" height="48" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
        <path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4"></path>
        <polyline points="17 8 12 3 7 8"></polyline>
        <line x1="12" y1="3" x2="12" y2="15"></line>
      </svg>
      <p class="upload-hint">点击或拖拽 PDF 文件到此处上传</p>
      <p class="upload-subhint">支持一次选择多个文件，自动提取 PDF 元数据</p>
      <input
        ref="fileInput"
        type="file"
        accept=".pdf,application/pdf"
        multiple
        @change="onFileSelect"
      />
    </div>

    <!-- 操作栏 -->
    <div v-if="queue.length > 0" class="upload-toolbar">
      <div class="upload-summary">
        共 <strong>{{ queue.length }}</strong> 个文件
        <span v-if="completedCount > 0">，已完成 {{ completedCount }}</span>
        <span v-if="failedCount > 0" class="text-error">，失败 {{ failedCount }}</span>
      </div>
      <div class="upload-actions">
        <button class="btn btn-outline" @click="clearCompleted">清除已完成</button>
        <button class="btn" :disabled="!canUpload" @click="startUpload">
          {{ isUploading ? '上传中...' : '开始上传' }}
        </button>
      </div>
    </div>

    <!-- 文件队列 -->
    <div v-if="queue.length > 0" class="upload-queue section">
      <div
        v-for="item in queue"
        :key="item.id"
        class="upload-queue-item"
        :class="item.status"
      >
        <div class="file-info">
          <span class="file-name" :title="item.file.name">{{ item.file.name }}</span>
          <span class="file-size">{{ formatSize(item.file.size) }}</span>
        </div>
        <div class="file-progress">
          <div class="progress-bar">
            <div class="progress-fill" :style="{ width: item.progress + '%' }"></div>
          </div>
          <span class="file-status">{{ statusText(item) }}</span>
        </div>
        <div class="file-actions">
          <button
            v-if="item.status === 'error'"
            class="btn btn-sm btn-outline"
            @click="retryItem(item)"
          >
            重试
          </button>
          <button
            v-if="item.status === 'pending' || item.status === 'error'"
            class="btn btn-sm btn-danger"
            @click="removeItem(item)"
          >
            删除
          </button>
          <button
            v-else-if="item.status === 'done'"
            class="btn btn-sm btn-outline"
            @click="removeItem(item)"
          >
            移除
          </button>
        </div>
      </div>
    </div>
  </div>
</template>

<script setup lang="ts">
import { ref, computed } from 'vue'
import { uploadPdf } from '@/api/documents'
import { useToast } from '@/composables/useToast'
import { useRequireToken } from '@/composables/useRequireToken'

const { showSuccess, showError } = useToast()
const { requireToken } = useRequireToken()

interface QueueItem {
  id: string
  file: File
  status: 'pending' | 'uploading' | 'done' | 'error'
  progress: number
  error?: string
}

const isDragover = ref(false)
const fileInput = ref<HTMLInputElement>()
const queue = ref<QueueItem[]>([])
const isUploading = ref(false)

const canUpload = computed(() =>
  queue.value.some((item) => item.status === 'pending' || item.status === 'error')
)
const completedCount = computed(() => queue.value.filter((i) => i.status === 'done').length)
const failedCount = computed(() => queue.value.filter((i) => i.status === 'error').length)

function generateId() {
  return Math.random().toString(36).slice(2, 10)
}

function formatSize(bytes: number) {
  if (bytes === 0) return '0 B'
  const units = ['B', 'KB', 'MB', 'GB']
  const i = Math.floor(Math.log(bytes) / Math.log(1024))
  return `${(bytes / Math.pow(1024, i)).toFixed(1)} ${units[i]}`
}

function statusText(item: QueueItem) {
  switch (item.status) {
    case 'pending':
      return '等待上传'
    case 'uploading':
      return item.progress > 0 ? `上传中 ${item.progress}%` : '上传中...'
    case 'done':
      return '完成'
    case 'error':
      return item.error || '失败'
    default:
      return ''
  }
}

function addFiles(files: FileList | null) {
  if (!files) return
  const newItems: QueueItem[] = []
  for (const file of Array.from(files)) {
    if (!file.name.toLowerCase().endsWith('.pdf')) {
      showError(`${file.name} 不是 PDF 文件，已跳过`)
      continue
    }
    // 去重：同文件名且同大小视为同一文件
    const exists = queue.value.some(
      (i) => i.file.name === file.name && i.file.size === file.size
    )
    if (!exists) {
      newItems.push({ id: generateId(), file, status: 'pending', progress: 0 })
    }
  }
  queue.value.push(...newItems)
}

function onDrop(e: DragEvent) {
  isDragover.value = false
  addFiles(e.dataTransfer?.files || null)
}

function onFileSelect(e: Event) {
  const input = e.target as HTMLInputElement
  addFiles(input.files)
  input.value = ''
}

function removeItem(item: QueueItem) {
  if (item.status === 'uploading') return
  queue.value = queue.value.filter((i) => i.id !== item.id)
}

function clearCompleted() {
  queue.value = queue.value.filter((i) => i.status !== 'done')
}

async function uploadItem(item: QueueItem) {
  item.status = 'uploading'
  item.progress = 0
  item.error = undefined

  try {
    // 模拟进度：文件较小则直接完成，大文件显示渐进进度
    const progressTimer = window.setInterval(() => {
      if (item.progress < 90) {
        item.progress += Math.floor(Math.random() * 10) + 5
        if (item.progress > 90) item.progress = 90
      }
    }, 200)

    await uploadPdf(item.file)

    window.clearInterval(progressTimer)
    item.progress = 100
    item.status = 'done'
  } catch (e: any) {
    item.status = 'error'
    item.progress = 0
    item.error = e?.response?.data?.detail || '上传失败'
  }
}

async function retryItem(item: QueueItem) {
  if (!requireToken()) return
  await uploadItem(item)
}

async function startUpload() {
  if (!requireToken()) return
  const pending = queue.value.filter(
    (item) => item.status === 'pending' || item.status === 'error'
  )
  if (pending.length === 0) return

  isUploading.value = true
  const concurrency = 3
  let index = 0

  async function worker() {
    while (index < pending.length) {
      const item = pending[index++]
      await uploadItem(item)
    }
  }

  const workers = Array.from({ length: Math.min(concurrency, pending.length) }, worker)
  await Promise.all(workers)

  isUploading.value = false
  const failed = queue.value.filter((i) => i.status === 'error')
  if (failed.length === 0) {
    showSuccess('全部上传完成')
  } else {
    showError(`${failed.length} 个文件上传失败`)
  }
}
</script>

<style scoped>
.upload-area {
  border: 2px dashed var(--border);
  border-radius: 12px;
  padding: 48px 40px;
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

.upload-hint {
  color: var(--text-primary);
  font-size: 16px;
  margin: 0 0 6px;
}

.upload-subhint {
  color: var(--text-secondary);
  font-size: 13px;
  margin: 0;
}

.upload-area input[type='file'] {
  display: none;
}

.upload-toolbar {
  display: flex;
  justify-content: space-between;
  align-items: center;
  margin-bottom: 16px;
  flex-wrap: wrap;
  gap: 12px;
}

.upload-summary {
  font-size: 14px;
  color: var(--text-secondary);
}

.upload-summary strong {
  color: var(--text-primary);
}

.text-error {
  color: #c62828;
}

.upload-actions {
  display: flex;
  gap: 8px;
}

.upload-queue {
  display: flex;
  flex-direction: column;
  gap: 12px;
}

.upload-queue-item {
  display: grid;
  grid-template-columns: 1fr 180px auto;
  align-items: center;
  gap: 16px;
  padding: 12px 16px;
  border: 1px solid var(--border);
  border-radius: 8px;
  background: #fff;
}

.upload-queue-item.uploading {
  border-color: var(--primary);
  background: #f5faff;
}

.upload-queue-item.error {
  border-color: #ef9a9a;
  background: #ffebee;
}

.upload-queue-item.done {
  border-color: #a5d6a7;
  background: #f1f8e9;
}

.file-info {
  min-width: 0;
  display: flex;
  flex-direction: column;
  gap: 4px;
}

.file-name {
  font-size: 14px;
  color: var(--text-primary);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.file-size {
  font-size: 12px;
  color: var(--text-secondary);
}

.file-progress {
  display: flex;
  flex-direction: column;
  gap: 6px;
}

.progress-bar {
  height: 6px;
  background: #e0e0e0;
  border-radius: 3px;
  overflow: hidden;
}

.progress-fill {
  height: 100%;
  background: var(--primary);
  transition: width 0.2s;
}

.file-status {
  font-size: 12px;
  color: var(--text-secondary);
}

.file-actions {
  display: flex;
  gap: 8px;
}

@media (max-width: 640px) {
  .upload-queue-item {
    grid-template-columns: 1fr;
    gap: 10px;
  }

  .upload-toolbar {
    flex-direction: column;
    align-items: flex-start;
  }
}
</style>
