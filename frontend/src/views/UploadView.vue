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
        <label class="force-checkbox">
          <input type="checkbox" v-model="forceUpload" />
          强制上传（跳过重复检查）
        </label>
      </div>
    </div>

    <!-- 总体进度条（上传中时显示） -->
    <div v-if="isUploading && totalBytes > 0" class="overall-progress">
      <div class="overall-progress-bar">
        <div
          class="overall-progress-fill"
          :style="{ width: overallPercent + '%' }"
        ></div>
      </div>
      <div class="overall-progress-info">
        <span class="overall-text">{{ formatSize(uploadedBytes) }} / {{ formatSize(totalBytes) }}</span>
        <span class="overall-text">{{ overallPercent }}%</span>
      </div>
      <div v-if="uploadSpeed > 0" class="overall-speed">
        {{ formatSpeed(uploadSpeed) }}
        <span v-if="eta > 0">· 剩余约 {{ formatDuration(eta) }}</span>
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
        <div class="item-main">
          <div class="file-info">
            <span class="file-name" :title="item.file.name">{{ item.file.name }}</span>
            <span class="file-size">{{ formatSize(item.file.size) }}</span>
          </div>
          <div class="file-actions">
            <button
              v-if="item.errorKind === 'duplicate-doi' && item.existingHasPdf"
              class="btn btn-sm btn-primary"
              @click="overwriteItem(item)"
            >
              覆盖已有 PDF
            </button>
            <button
              v-if="item.errorKind === 'duplicate-doi' && item.existingHasPdf"
              class="btn btn-sm btn-outline"
              @click="keepExisting(item)"
            >
              保留原 PDF
            </button>
            <button
              v-if="item.existingId && (item.status === 'error' || item.status === 'done')"
              class="btn btn-sm btn-primary"
              @click="viewExisting(item)"
            >
              查看已有文献
            </button>
            <button
              v-if="item.status === 'error' && (item.errorKind === 'duplicate' || item.errorKind === 'duplicate-doi')"
              class="btn btn-sm btn-outline"
              @click="forceItem(item)"
            >
              强制上传（新建一条）
            </button>
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
        <div class="file-progress">
          <div class="progress-bar" :class="{ indeterminate: isBusyIndeterminate(item) }">
            <div
              class="progress-fill"
              :style="{ width: isBusyIndeterminate(item) ? '100%' : item.progress + '%' }"
            ></div>
          </div>
          <span
            class="file-status"
            :class="{ 'status-error': item.status === 'error' }"
            :title="item.errorDetail || ''"
          >
            {{ statusText(item) }}
          </span>
          <span v-if="item.note" class="file-hint file-note">{{ item.note }}</span>
          <span v-if="item.errorHint" class="file-hint">{{ item.errorHint }}</span>
        </div>
      </div>
    </div>

    <!-- BibTeX 批量新建条目 -->
    <BibtexImportPanel />
  </div>
</template>

<script setup lang="ts">
import { ref, computed } from 'vue'
import BibtexImportPanel from '@/components/BibtexImportPanel.vue'
import { uploadPdf } from '@/api/documents'
import type { UploadPhase } from '@/api/documents'
import { describeUploadError } from '@/utils/uploadError'
import type { UploadFailureKind } from '@/utils/uploadError'
import { useToast } from '@/composables/useToast'
import { useRequireToken } from '@/composables/useRequireToken'

const { showSuccess, showError, showInfo } = useToast()
const { requireToken } = useRequireToken()

interface QueueItem {
  id: string
  file: File
  status: 'pending' | 'uploading' | 'done' | 'error'
  /** 0–100 真实上传进度（服务端处理阶段停在 100） */
  progress: number
  /** 当前已上传字节（用于总体进度汇总） */
  loaded: number
  /** 上传子阶段：算校验值 / 传字节 / 等服务端 */
  phase?: UploadPhase
  /** 可读的失败原因 */
  error?: string
  /** 失败后的处置建议 */
  errorHint?: string
  /** 原始 detail（悬停查看） */
  errorDetail?: string
  errorKind?: UploadFailureKind
  existingId?: number
  /** 该文件单独走强制上传（内容重复时的手动确认） */
  force?: boolean
  /** 该文件单独走「覆盖已有 PDF」：DOI 相同且已有条目已有 PDF，用户点了「覆盖已有 PDF」 */
  overwrite?: boolean
  /** DOI 重复时，已有条目里是否已经有 PDF（true → 由用户决定覆盖还是保留） */
  existingHasPdf?: boolean
  /** 成功后的附加说明：补入已有条目 / 覆盖了已有 PDF / 未改动 */
  note?: string
}

const isDragover = ref(false)
const fileInput = ref<HTMLInputElement>()
const queue = ref<QueueItem[]>([])
const isUploading = ref(false)
const forceUpload = ref(false)

// 总体进度跟踪
const uploadSpeed = ref(0) // 字节/秒
const lastProgressTime = ref(0)
const lastProgressBytes = ref(0)

const canUpload = computed(() =>
  queue.value.some((item) => item.status === 'pending' || item.status === 'error')
)
const completedCount = computed(() => queue.value.filter((i) => i.status === 'done').length)
const failedCount = computed(() => queue.value.filter((i) => i.status === 'error').length)

/** 待上传的总字节数（pending + error + uploading 状态的文件） */
const totalBytes = computed(() => {
  let sum = 0
  for (const item of queue.value) {
    if (item.status !== 'done') {
      sum += item.file.size
    }
  }
  return sum
})

/** 已上传的总字节（uploading 的 loaded + done 的完整大小） */
const uploadedBytes = computed(() => {
  let sum = 0
  for (const item of queue.value) {
    if (item.status === 'uploading' || item.status === 'done') {
      sum += item.loaded
    }
  }
  return sum
})

/** 总体上传百分比（0–100） */
const overallPercent = computed(() => {
  if (totalBytes.value === 0) return 0
  return Math.min(100, Math.round((uploadedBytes.value / totalBytes.value) * 100))
})

/** 预计剩余时间（秒），0 表示无法估算 */
const eta = computed(() => {
  if (uploadSpeed.value <= 0 || uploadedBytes.value >= totalBytes.value) return 0
  const remaining = totalBytes.value - uploadedBytes.value
  return Math.round(remaining / uploadSpeed.value)
})

function generateId() {
  return Math.random().toString(36).slice(2, 10)
}

function formatSize(bytes: number) {
  if (bytes === 0) return '0 B'
  const units = ['B', 'KB', 'MB', 'GB']
  const i = Math.floor(Math.log(bytes) / Math.log(1024))
  return `${(bytes / Math.pow(1024, i)).toFixed(1)} ${units[i]}`
}

/** 格式化上传速度（字节/秒 → 可读单位） */
function formatSpeed(bytesPerSec: number): string {
  if (bytesPerSec <= 0) return ''
  return `${formatSize(bytesPerSec)}/s`
}

/** 格式化剩余时间（秒 → 分:秒） */
function formatDuration(seconds: number): string {
  if (seconds <= 0) return ''
  const m = Math.floor(seconds / 60)
  const s = Math.floor(seconds % 60)
  if (m > 0) return `${m}分${s}秒`
  return `${s}秒`
}

function statusText(item: QueueItem) {
  switch (item.status) {
    case 'pending':
      return '等待上传'
    case 'uploading':
      if (item.phase === 'hashing') return '计算校验值（查重准备）…'
      if (item.phase === 'processing') return '已发送完毕，服务端查重 / 提取元数据 / 落盘中…'
      if (item.progress > 0) return `上传中 ${item.progress}%`
      return '正在发送…'
    case 'done':
      return '完成'
    case 'error':
      return item.error || '上传失败'
    default:
      return ''
  }
}

/** 算校验值 / 等服务端响应时没有百分比可报，进度条走「不确定」滑动动画 */
function isBusyIndeterminate(item: QueueItem) {
  return (
    item.status === 'uploading' &&
    (item.phase === 'hashing' || item.phase === 'processing')
  )
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

function viewExisting(item: QueueItem) {
  if (item.existingId) {
    window.location.href = `/assist/detail/${item.existingId}`
  }
}

function clearCompleted() {
  queue.value = queue.value.filter((i) => i.status !== 'done')
}

async function uploadItem(item: QueueItem) {
  item.status = 'uploading'
  item.progress = 0
  item.loaded = 0
  item.phase = 'hashing'
  item.error = undefined
  item.errorHint = undefined
  item.errorDetail = undefined
  item.errorKind = undefined
  item.existingId = undefined
  item.existingHasPdf = undefined
  item.note = undefined

  try {
    const res = await uploadPdf(item.file, item.force ?? forceUpload.value, (p) => {
      item.phase = p.phase
      item.progress = p.percent
      item.loaded = p.loaded

      // 更新上传速度（每 500ms 采样一次）
      const now = Date.now()
      const elapsed = now - lastProgressTime.value
      if (elapsed >= 500) {
        const bytesDelta = p.loaded - lastProgressBytes.value
        if (bytesDelta > 0 && elapsed > 0) {
          uploadSpeed.value = Math.round((bytesDelta / elapsed) * 1000)
        }
        lastProgressTime.value = now
        lastProgressBytes.value = p.loaded
      }
    }, {
      // 用户确认覆盖时，服务端用本次文件替换已有条目的 PDF；否则命中重复就返回 409 由界面询问
      onDuplicate: item.overwrite ? 'replace' : undefined,
    })
    item.phase = undefined
    item.progress = 100
    item.loaded = item.file.size
    item.status = 'done'

    // 服务端去重结果：attached（DOI 相同 + 已有条目缺 PDF，已自动补入）/ replaced（覆盖）/ skipped
    const targetId = res?.existing_id ?? res?.id
    const idText = targetId ? ` #${targetId}` : ''
    if (res?.status === 'attached') {
      item.existingId = targetId
      item.note = `已补入已有条目${idText}（DOI 相同，该条目原先缺 PDF）`
      showSuccess(`PDF 已补入已有条目${idText}`)
    } else if (res?.status === 'replaced') {
      item.existingId = targetId
      item.note = `已覆盖已有条目${idText} 的 PDF`
      showSuccess(`已覆盖${idText} 的 PDF`)
    } else if (res?.status === 'skipped') {
      item.existingId = targetId
      item.note = `未改动：库里已有相同条目${idText}`
      showInfo(`库里已有相同条目${idText}，未做改动`)
    }
  } catch (e) {
    // 失败不把进度清零：停在哪一格就显示哪一格，避免「传了 80% 突然回到 0」的错觉
    const failure = describeUploadError(e)
    item.phase = undefined
    item.status = 'error'
    item.errorKind = failure.kind
    item.error = failure.message
    item.errorHint = failure.hint
    item.errorDetail = failure.detail
    item.existingId = failure.existingId
  }
}

/** 内容重复但用户仍要建新记录：该文件单独走 force 上传 */
async function forceItem(item: QueueItem) {
  if (!requireToken()) return
  item.force = true
  await uploadItem(item)
}

/** DOI 相同、已有条目已有 PDF：用户确认用本次文件覆盖它 */
async function overwriteItem(item: QueueItem) {
  if (!requireToken()) return
  item.overwrite = true
  await uploadItem(item)
}

/** DOI 相同、已有条目已有 PDF：用户选择保留原 PDF，本次上传作废 */
function keepExisting(item: QueueItem) {
  showInfo(
    `已保留原有 PDF，未覆盖${item.existingId ? ` #${item.existingId}` : ''}`
  )
  removeItem(item)
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
  uploadSpeed.value = 0
  lastProgressTime.value = Date.now()
  lastProgressBytes.value = 0

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
  uploadSpeed.value = 0

  const failed = queue.value.filter((i) => i.status === 'error')
  if (failed.length === 0) {
    showSuccess('全部上传完成')
  } else if (failed.length === 1) {
    showError(`上传失败：${failed[0].error}`)
  } else {
    showError(`${failed.length} 个文件上传失败，首个问题：${failed[0].error}`)
  }
}
</script>

<style scoped>
.upload-view :deep(.bibtex-panel) {
  margin-top: 24px;
}

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
  align-items: center;
}

.force-checkbox {
  display: flex;
  align-items: center;
  gap: 6px;
  font-size: 13px;
  color: var(--text-secondary);
  cursor: pointer;
}

.force-checkbox input {
  width: auto;
}

/* 总体进度条 */
.overall-progress {
  margin-bottom: 20px;
  padding: 12px 16px;
  background: #f8fafc;
  border-radius: 8px;
  border: 1px solid var(--border);
}

.overall-progress-bar {
  height: 8px;
  background: #e0e0e0;
  border-radius: 4px;
  overflow: hidden;
  margin-bottom: 8px;
}

.overall-progress-fill {
  height: 100%;
  background: var(--primary);
  transition: width 0.3s ease;
  border-radius: 4px;
}

.overall-progress-info {
  display: flex;
  justify-content: space-between;
  font-size: 13px;
  color: var(--text-secondary);
  margin-bottom: 4px;
}

.overall-progress-info span:first-child {
  font-variant-numeric: tabular-nums;
}

.overall-progress-info span:last-child {
  font-weight: 600;
  color: var(--text-primary);
  font-variant-numeric: tabular-nums;
}

.overall-speed {
  font-size: 12px;
  color: var(--text-secondary);
}

.upload-queue {
  display: flex;
  flex-direction: column;
  gap: 12px;
}

.upload-queue-item {
  display: flex;
  flex-direction: column;
  gap: 8px;
  padding: 12px 16px;
  border: 1px solid var(--border);
  border-radius: 8px;
  background: #fff;
}

.item-main {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 16px;
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

/* 算校验值 / 等服务端响应：没有百分比可报，走滑动动画 */
.progress-bar.indeterminate .progress-fill {
  width: 100% !important;
  background-image: linear-gradient(
    90deg,
    var(--primary) 0%,
    #bbdefb 45%,
    var(--primary) 90%
  );
  background-size: 220% 100%;
  animation: progress-shimmer 1.1s linear infinite;
}

@keyframes progress-shimmer {
  from {
    background-position: 120% 0;
  }
  to {
    background-position: -20% 0;
  }
}

.upload-queue-item.error .progress-bar {
  background: #ffcdd2;
}

.upload-queue-item.error .progress-fill {
  background: #ef5350;
}

.file-status {
  font-size: 12px;
  color: var(--text-secondary);
  word-break: break-word;
  line-height: 1.5;
}

.file-status.status-error {
  color: #c62828;
  font-weight: 500;
}

.file-hint.file-note {
  color: var(--success, #2e7d32);
}

.file-hint {
  font-size: 12px;
  color: var(--text-secondary);
  line-height: 1.5;
}

.upload-queue-item.error .file-hint {
  color: #b71c1c;
  opacity: 0.85;
}

.file-actions {
  display: flex;
  gap: 8px;
}

@media (max-width: 768px) {
  .upload-area {
    padding: 32px 20px;
  }

  .upload-icon {
    width: 40px;
    height: 40px;
  }

  .upload-hint {
    font-size: 15px;
  }

  .upload-subhint {
    font-size: 13px;
  }

  .upload-queue-item {
    gap: 10px;
    padding: 12px;
  }

  .item-main {
    flex-wrap: wrap;
  }

  .upload-toolbar {
    flex-direction: column;
    align-items: flex-start;
    gap: 12px;
  }

  .upload-actions {
    width: 100%;
  }

  .upload-actions .btn {
    flex: 1;
  }

  .file-actions {
    flex-wrap: wrap;
  }

  .file-actions .btn {
    flex: 1;
    min-width: 80px;
  }
}

@media (max-width: 480px) {
  .upload-area {
    padding: 24px 16px;
  }

  .upload-icon {
    width: 36px;
    height: 36px;
  }

  .upload-hint {
    font-size: 14px;
  }

  .upload-queue-item {
    padding: 10px;
  }
}
</style>
