<template>
  <Teleport to="body">
    <div v-if="open" class="inline-pdf-overlay" @click.self="close">
      <div class="inline-pdf-panel">
        <div class="inline-pdf-header">
          <span class="inline-pdf-title" :title="title">{{ title || 'PDF 预览' }}</span>
          <span v-if="loading" class="inline-pdf-status">
            正在获取 PDF…{{ loadingText }}
          </span>
          <span v-else-if="error" class="inline-pdf-status is-error">{{ error }}</span>
          <span v-else-if="url" class="inline-pdf-status">已加载 {{ formatBytes(loaded) }}</span>
          <div class="inline-pdf-actions">
            <button
              class="btn btn-sm btn-outline"
              :disabled="!url"
              :title="`保存到本地（${saveName}）`"
              @click="saveFile"
            >保存</button>
            <button class="btn btn-sm btn-outline" :disabled="!url" @click="openInNewTab">新窗口打开</button>
            <button class="btn btn-sm" @click="close">关闭</button>
          </div>
        </div>
        <!-- 下载进度条：仅在拉取中显示 -->
        <div v-if="loading" class="inline-pdf-progress-row">
          <div class="inline-pdf-progress-track">
            <div
              class="inline-pdf-progress-bar"
              :class="{ 'is-indeterminate': total === 0 }"
              :style="total > 0 ? { width: percent + '%' } : undefined"
            ></div>
          </div>
          <span class="inline-pdf-progress-num">
            {{ total > 0 ? percent + '%' : formatBytes(loaded) }}
          </span>
        </div>
        <div class="inline-pdf-body">
          <iframe v-if="url" :src="url" class="inline-pdf-frame" title="PDF 页内预览"></iframe>
          <div v-else-if="loading" class="inline-pdf-placeholder">
            正在下载 PDF（#{{ docId }}）…
          </div>
          <div v-else class="inline-pdf-placeholder is-error">
            {{ error || '无法加载 PDF' }}
          </div>
        </div>
      </div>
    </div>
  </Teleport>
</template>

<script setup lang="ts">
import { ref, computed, watch, onBeforeUnmount } from 'vue'
import { fetchPdfBlob } from '@/api/documents'

/**
 * 页内 PDF 预览（需 Token 的 API 获取，不经过免 token 的 /assist/file/ 别名）。
 *
 * 实现要点：
 * - 用 axios（自动带 `X-Token`）以 `responseType: 'blob'` 拉 `/assist/api/documents/:id/pdf/`；
 * - 把 Blob 包成 `application/pdf` 后 `URL.createObjectURL` 挂给 `<iframe>`，浏览器原生 PDF 查看器渲染；
 * - 关闭 / 切换文档 / 卸载时统一 `revokeObjectURL`（避免大文件长期占内存）并 `abort()` 下载
 *   （否则关掉面板后 XHR 仍会在后台把整份 PDF 拉完）；
 * - 请求用自增序号做「最后一次生效」保护，快速开关时丢弃已过期的响应。
 */
const props = defineProps<{
  open: boolean
  docId: number | null
  title?: string
  /** 保存到本地时建议使用的文件名（缺省用标题 / 文档 ID） */
  filename?: string
}>()

const emit = defineEmits<{ (e: 'close'): void }>()

const url = ref('')
const loading = ref(false)
const error = ref('')
/** 已下载字节数 / 总字节数（服务端未回 Content-Length 时 total = 0） */
const loaded = ref(0)
const total = ref(0)
const percent = computed(() =>
  total.value > 0 ? Math.min(100, Math.round((loaded.value / total.value) * 100)) : 0
)
/** 头部状态行的补充说明：百分比 + 已下载 / 总量 */
const loadingText = computed(() => {
  if (total.value > 0) {
    return ` ${percent.value}%（${formatBytes(loaded.value)} / ${formatBytes(total.value)}）`
  }
  return loaded.value > 0 ? ` 已接收 ${formatBytes(loaded.value)}` : ''
})
/** 保存文件名：优先文件名 prop，其次标题，最后 doc-<id>.pdf */
const saveName = computed(() => {
  const raw = (props.filename || props.title || '').trim()
  const base = raw.replace(/[\\/:*?"<>|]+/g, '_') || (props.docId ? `doc-${props.docId}` : 'document')
  return /\.pdf$/i.test(base) ? base : `${base}.pdf`
})
/** 请求代号：只有最新的请求能把结果写进状态 */
let requestId = 0
/** 当前下载的中止器：同一时刻只允许一个下载在跑 */
let controller: AbortController | null = null

/** 判断是否为「我们主动取消」（关闭面板 / 换文档），取消不该弹错误提示 */
function isCanceled(e: unknown): boolean {
  const err = e as { code?: string; name?: string }
  return err.code === 'ERR_CANCELED' || err.name === 'CanceledError' || err.name === 'AbortError'
}

/** 中止当前下载并释放中止器 */
function abortDownload() {
  if (controller) {
    controller.abort()
    controller = null
  }
}

function formatBytes(bytes: number): string {
  if (!bytes || bytes <= 0) return '0 B'
  if (bytes < 1024) return bytes + ' B'
  if (bytes < 1024 * 1024) return (bytes / 1024).toFixed(1) + ' KB'
  if (bytes < 1024 * 1024 * 1024) return (bytes / (1024 * 1024)).toFixed(1) + ' MB'
  return (bytes / (1024 * 1024 * 1024)).toFixed(2) + ' GB'
}

function revoke() {
  if (url.value) {
    URL.revokeObjectURL(url.value)
    url.value = ''
  }
}

async function load() {
  abortDownload()
  revoke()
  error.value = ''
  loaded.value = 0
  total.value = 0
  if (!props.docId) {
    error.value = '缺少文档 ID'
    return
  }
  const myId = ++requestId
  const ctrl = new AbortController()
  controller = ctrl
  loading.value = true
  try {
    const blob = await fetchPdfBlob(props.docId, {
      signal: ctrl.signal,
      onProgress: (info) => {
        if (myId !== requestId) return
        loaded.value = info.loaded
        total.value = info.total
      }
    })
    if (myId !== requestId) return
    const pdfBlob =
      blob.type === 'application/pdf' ? blob : new Blob([blob], { type: 'application/pdf' })
    if (pdfBlob.size === 0) {
      error.value = 'PDF 文件为空'
      return
    }
    // 下载完成后用 Blob 真实体积修正（避免服务端未回 Content-Length 时头部显示 0 B）
    loaded.value = pdfBlob.size
    total.value = pdfBlob.size
    url.value = URL.createObjectURL(pdfBlob)
  } catch (e) {
    if (myId !== requestId || isCanceled(e)) return
    const status = (e as { response?: { status?: number } }).response?.status
    if (status === 404) error.value = 'PDF 文件不存在，请先上传'
    else if (status === 401 || status === 403) error.value = '没有权限查看该 PDF，请重新登录'
    else error.value = '无法加载 PDF'
  } finally {
    if (myId === requestId) loading.value = false
    if (controller === ctrl) controller = null
  }
}

function teardown() {
  requestId++
  loading.value = false
  abortDownload()
  document.removeEventListener('keydown', onKeydown)
  revoke()
}

function close() {
  teardown()
  emit('close')
}

/** 新窗口打开：把同一个 Blob URL 直接交给浏览器标签页 */
function openInNewTab() {
  if (url.value) window.open(url.value, '_blank', 'noopener')
}

/**
 * 保存到本地：用临时 `<a download>` 触发浏览器下载。
 * Blob URL 同源，不会触发额外的网络请求（内容已在内存里）。
 */
function saveFile() {
  if (!url.value) return
  const a = document.createElement('a')
  a.href = url.value
  a.download = saveName.value
  a.rel = 'noopener'
  document.body.appendChild(a)
  a.click()
  document.body.removeChild(a)
}

/** Esc 关闭 */
function onKeydown(e: KeyboardEvent) {
  if (e.key === 'Escape') close()
}

watch(
  () => props.open,
  (isOpen) => {
    if (isOpen) {
      document.addEventListener('keydown', onKeydown)
      load()
    } else {
      teardown()
    }
  },
  { immediate: true }
)

/** 文档切换时重新拉取（面板保持打开时点另一篇） */
watch(
  () => props.docId,
  () => {
    if (props.open) load()
  }
)

onBeforeUnmount(() => {
  teardown()
})
</script>

<style scoped>
.inline-pdf-overlay {
  position: fixed;
  inset: 0;
  z-index: 1000;
  background: rgba(0, 0, 0, 0.55);
  display: flex;
  align-items: center;
  justify-content: center;
  padding: 24px;
}

.inline-pdf-panel {
  display: flex;
  flex-direction: column;
  width: min(1100px, 100%);
  height: min(92vh, 100%);
  min-height: 0;
  background: var(--bg-white, #fff);
  border-radius: 10px;
  box-shadow: 0 12px 40px rgba(0, 0, 0, 0.28);
  overflow: hidden;
}

.inline-pdf-header {
  display: flex;
  align-items: center;
  gap: 12px;
  flex-wrap: wrap;
  padding: 10px 16px;
  border-bottom: 1px solid var(--border, #e5e7eb);
  background: var(--bg-table-header, #f7f7f7);
}

.inline-pdf-title {
  flex: 1 1 240px;
  min-width: 0;
  font-size: 14px;
  font-weight: 600;
  color: var(--text, #333);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.inline-pdf-status {
  font-size: 12px;
  color: var(--text-secondary, #666);
  font-variant-numeric: tabular-nums;
}
.inline-pdf-status.is-error {
  color: var(--danger, #f44336);
}

/* 下载进度条（拉取中显示） */
.inline-pdf-progress-row {
  display: flex;
  align-items: center;
  gap: 10px;
  padding: 0 16px 8px;
  background: var(--bg-table-header, #f7f7f7);
  border-bottom: 1px solid var(--border, #e5e7eb);
}
.inline-pdf-progress-track {
  flex: 1;
  height: 6px;
  border-radius: 3px;
  background: var(--border, #e5e7eb);
  overflow: hidden;
}
.inline-pdf-progress-bar {
  height: 100%;
  width: 0;
  border-radius: 3px;
  background: var(--primary, #1976d2);
  transition: width 0.2s ease-out;
}
/* 拿不到总大小时走不定长流动动画 */
.inline-pdf-progress-bar.is-indeterminate {
  width: 35%;
  animation: inline-pdf-indeterminate 1.2s ease-in-out infinite;
}
@keyframes inline-pdf-indeterminate {
  0% {
    transform: translateX(-100%);
  }
  100% {
    transform: translateX(300%);
  }
}
.inline-pdf-progress-num {
  min-width: 52px;
  text-align: right;
  font-size: 12px;
  color: var(--text-secondary, #666);
  font-variant-numeric: tabular-nums;
}

.inline-pdf-actions {
  display: flex;
  gap: 8px;
  margin-left: auto;
}

.inline-pdf-body {
  flex: 1;
  min-height: 0;
  display: flex;
  background: #525659;
}

.inline-pdf-frame {
  flex: 1;
  min-height: 0;
  width: 100%;
  border: 0;
  background: #fff;
}

.inline-pdf-placeholder {
  margin: auto;
  padding: 24px;
  font-size: 13px;
  color: #f3f4f6;
  text-align: center;
  line-height: 1.7;
}
.inline-pdf-placeholder.is-error {
  color: #fecaca;
}

@media (max-width: 768px) {
  .inline-pdf-overlay {
    padding: 0;
  }
  .inline-pdf-panel {
    width: 100%;
    height: 100%;
    border-radius: 0;
  }
  .inline-pdf-title {
    flex-basis: 100%;
  }
  .inline-pdf-actions {
    margin-left: 0;
    width: 100%;
    justify-content: flex-end;
  }
}
</style>
