<template>
  <div class="drive-view container">
    <div class="page-header">
      <h2>网盘</h2>
      <div class="header-actions">
        <router-link v-if="tokenStore.isAdmin" class="btn btn-sm btn-outline" to="/assist/drive/admin">
          管理公有文件夹
        </router-link>
        <button class="btn btn-sm btn-outline" :disabled="loading" @click="reload">刷新</button>
      </div>
    </div>

    <div v-if="!summary" class="empty-hint">加载中…</div>

    <div v-else-if="!summary.enabled" class="notice notice-warn">
      网盘功能当前已关闭。请联系管理员在「管理公有文件夹 → 网盘设置」中启用。
    </div>

    <div v-else class="drive-layout">
      <!-- 左侧：作用域 -->
      <aside class="scope-panel">
        <div class="scope-group">
          <div class="scope-group-title">我的</div>
          <button
            class="scope-item"
            :class="{ active: scope === 'private' }"
            @click="switchScope('private')"
          >
            <span class="scope-icon">🗂️</span>
            <span class="scope-name">我的网盘</span>
            <span class="scope-size">{{ formatSize(summary.used_bytes) }}</span>
          </button>
        </div>

        <div v-if="summary.public_folders.length" class="scope-group">
          <div class="scope-group-title">共享（公有文件夹）</div>
          <button
            v-for="f in summary.public_folders"
            :key="f.id"
            class="scope-item"
            :class="{ active: scope === `public:${f.id}` }"
            @click="switchScope(`public:${f.id}`)"
          >
            <span class="scope-icon">📁</span>
            <span class="scope-name">{{ f.name }}</span>
            <span class="scope-tag">{{ f.can_write ? '可写' : '只读' }}</span>
          </button>
        </div>
        <div v-else class="scope-empty">暂无公有文件夹</div>
      </aside>

      <!-- 右侧：文件区 -->
      <section class="file-panel" @dragover.prevent="dragging = true" @dragleave.prevent="dragging = false" @drop.prevent="onDrop">
        <div class="toolbar">
          <nav class="breadcrumb">
            <button class="crumb" @click="openPath('')">{{ listing?.label || '根目录' }}</button>
            <template v-for="c in listing?.breadcrumb || []" :key="c.path">
              <span class="crumb-sep">/</span>
              <button class="crumb" @click="openPath(c.path)">{{ c.name }}</button>
            </template>
          </nav>

          <div class="toolbar-actions">
            <span v-if="!listing?.can_write" class="readonly-tag">只读</span>
            <template v-if="listing?.can_write">
              <button class="btn btn-sm btn-outline" @click="openMkdir">新建文件夹</button>
              <button class="btn btn-sm" :disabled="uploading" @click="pickFiles">
                {{ uploading ? `上传中 ${uploadPercent}%` : '上传文件' }}
              </button>
            </template>
          </div>
        </div>

        <div class="usage-bar">
          <div class="usage-text">
            已用 {{ formatSize(listing?.used_bytes ?? 0) }}
            <template v-if="listing?.quota_bytes">
              / 配额 {{ formatSize(listing.quota_bytes) }}
            </template>
            <template v-else>（不限容量）</template>
            <span class="usage-hint">单文件上限 {{ listing?.max_upload_mb ?? summary.max_upload_mb }} MB</span>
          </div>
          <div v-if="listing?.quota_bytes" class="usage-track">
            <div class="usage-fill" :class="{ danger: usagePercent > 90 }" :style="{ width: usagePercent + '%' }"></div>
          </div>
        </div>

        <div class="table-wrap">
          <table class="file-table">
            <thead>
              <tr>
                <th>名称</th>
                <th class="col-size">大小</th>
                <th class="col-time">修改时间</th>
                <th class="col-actions">操作</th>
              </tr>
            </thead>
            <tbody>
              <tr v-if="listing?.path">
                <td colspan="4">
                  <button class="link-btn" @click="openPath(parentPath)">⬅ 返回上一级</button>
                </td>
              </tr>
              <tr v-for="e in listing?.entries || []" :key="e.name">
                <td>
                  <button class="entry-name" :class="{ dir: e.is_dir }" @click="activate(e)">
                    <span class="entry-icon">{{ e.is_dir ? '📁' : iconFor(e.ext) }}</span>
                    {{ e.name }}
                  </button>
                </td>
                <td class="col-size nowrap">{{ e.is_dir ? '—' : formatSize(e.size) }}</td>
                <td class="col-time nowrap">{{ formatTime(e.modified) }}</td>
                <td class="col-actions nowrap">
                  <button v-if="!e.is_dir" class="btn btn-sm btn-outline" @click="preview(e)">预览</button>
                  <a
                    v-if="!e.is_dir"
                    class="btn btn-sm btn-outline"
                    :href="downloadUrl(e.name)"
                    download
                  >下载</a>
                  <template v-if="listing?.can_write">
                    <button class="btn btn-sm btn-outline" @click="openRename(e)">重命名</button>
                    <button class="btn btn-sm btn-outline" @click="openMove(e)">移动</button>
                    <button class="btn btn-sm btn-outline-danger" @click="remove(e)">删除</button>
                  </template>
                </td>
              </tr>
              <tr v-if="listing && !listing.entries.length">
                <td colspan="4" class="empty-hint">
                  {{ dragging ? '松开鼠标即可上传到当前目录' : '此目录为空' }}
                </td>
              </tr>
            </tbody>
          </table>
        </div>

        <div v-if="dragging" class="drop-overlay">松开以上传到「{{ currentPathLabel }}」</div>
      </section>
    </div>

    <input ref="fileInput" type="file" multiple class="hidden-input" @change="onFilesPicked" />

    <!-- 新建文件夹 -->
    <div v-if="mkdirVisible" class="modal-overlay" @click.self="mkdirVisible = false">
      <div class="modal">
        <h3>新建文件夹</h3>
        <div class="form-group">
          <label>名称</label>
          <input v-model="mkdirName" type="text" placeholder="目录名" @keyup.enter="submitMkdir" />
        </div>
        <div class="modal-actions">
          <button class="btn" :disabled="busy" @click="submitMkdir">创建</button>
          <button class="btn btn-outline" @click="mkdirVisible = false">取消</button>
        </div>
      </div>
    </div>

    <!-- 重命名 -->
    <div v-if="renameTarget" class="modal-overlay" @click.self="renameTarget = null">
      <div class="modal">
        <h3>重命名</h3>
        <div class="form-group">
          <label>{{ renameTarget.name }}</label>
          <input v-model="renameName" type="text" @keyup.enter="submitRename" />
        </div>
        <div class="modal-actions">
          <button class="btn" :disabled="busy" @click="submitRename">确定</button>
          <button class="btn btn-outline" @click="renameTarget = null">取消</button>
        </div>
      </div>
    </div>

    <!-- 移动 -->
    <div v-if="moveTarget" class="modal-overlay" @click.self="moveTarget = null">
      <div class="modal">
        <h3>移动「{{ moveTarget.name }}」</h3>
        <div class="form-group">
          <label>目标目录（相对路径，留空表示根目录）</label>
          <input v-model="moveToDir" type="text" placeholder="如 docs/2024" @keyup.enter="submitMove" />
        </div>
        <p class="form-hint">目标目录不存在时会提示失败；可先在目标位置新建文件夹。</p>
        <div class="modal-actions">
          <button class="btn" :disabled="busy" @click="submitMove">移动</button>
          <button class="btn btn-outline" @click="moveTarget = null">取消</button>
        </div>
      </div>
    </div>

    <!-- 预览 -->
    <div v-if="previewTarget" class="modal-overlay preview-overlay" @click.self="closePreview">
      <div class="preview-box">
        <div class="preview-header">
          <span class="preview-title">{{ previewTarget.name }}</span>
          <div class="preview-actions">
            <a class="btn btn-sm btn-outline" :href="downloadUrl(previewTarget.name)" download>下载</a>
            <button class="btn btn-sm btn-outline" @click="closePreview">关闭</button>
          </div>
        </div>
        <div class="preview-body">
          <img v-if="previewKind === 'image'" :src="previewUrl" :alt="previewTarget.name" />
          <iframe v-else-if="previewKind === 'pdf'" :src="previewUrl" class="preview-frame" title="PDF 预览"></iframe>
          <pre v-else-if="previewKind === 'text'" class="preview-text">{{ previewText }}</pre>
          <div v-else class="preview-unsupported">
            此类型不支持在线预览，请下载后查看。
          </div>
        </div>
        <div v-if="previewTruncated" class="preview-foot">内容过大，仅显示前 512KB</div>
      </div>
    </div>

    <ConfirmDialog ref="confirmRef" />
  </div>
</template>

<script setup lang="ts">
import { computed, onMounted, ref } from 'vue'
import {
  createDriveFolder,
  deleteDriveEntry,
  driveDownloadUrl,
  driveRawUrl,
  getDriveSummary,
  getDriveText,
  listDrive,
  moveDriveEntry,
  renameDriveEntry,
  uploadDriveFiles
} from '@/api/drive'
import ConfirmDialog from '@/components/ConfirmDialog.vue'
import { useToast } from '@/composables/useToast'
import { useTokenStore } from '@/stores/token'
import type { DriveEntry, DriveListResponse, DriveScope, DriveSummary } from '@/types/drive'

const toast = useToast()
const tokenStore = useTokenStore()

const summary = ref<DriveSummary | null>(null)
const listing = ref<DriveListResponse | null>(null)
const scope = ref<DriveScope>('private')
const path = ref('')
const loading = ref(false)
const busy = ref(false)
const uploading = ref(false)
const uploadPercent = ref(0)
const dragging = ref(false)

const fileInput = ref<HTMLInputElement | null>(null)
const confirmRef = ref<InstanceType<typeof ConfirmDialog> | null>(null)

// 对话框
const mkdirVisible = ref(false)
const mkdirName = ref('')
const renameTarget = ref<DriveEntry | null>(null)
const renameName = ref('')
const moveTarget = ref<DriveEntry | null>(null)
const moveToDir = ref('')

// 预览
const previewTarget = ref<DriveEntry | null>(null)
const previewKind = ref<'image' | 'pdf' | 'text' | 'none'>('none')
const previewText = ref('')
const previewTruncated = ref(false)

const parentPath = computed(() => {
  const parts = (listing.value?.path || '').split('/').filter(Boolean)
  parts.pop()
  return parts.join('/')
})
const currentPathLabel = computed(() => listing.value?.path || '根目录')
const usagePercent = computed(() => {
  const q = listing.value?.quota_bytes
  if (!q) return 0
  return Math.min(100, Math.round(((listing.value?.used_bytes || 0) / q) * 100))
})

const previewUrl = computed(() =>
  previewTarget.value ? driveRawUrl(scope.value, joinPath(previewTarget.value.name), tokenStore.token) : ''
)

function joinPath(name: string): string {
  const base = listing.value?.path || ''
  return base ? `${base}/${name}` : name
}

function formatSize(bytes: number): string {
  if (!bytes) return '0 B'
  const units = ['B', 'KB', 'MB', 'GB', 'TB']
  let value = bytes
  let i = 0
  while (value >= 1024 && i < units.length - 1) {
    value /= 1024
    i++
  }
  return `${value.toFixed(value >= 100 || i === 0 ? 0 : 1)} ${units[i]}`
}

function formatTime(v?: string | null): string {
  if (!v) return '—'
  const d = new Date(v)
  return Number.isNaN(d.getTime()) ? v : d.toLocaleString()
}

function iconFor(ext: string): string {
  const images = ['png', 'jpg', 'jpeg', 'gif', 'webp', 'bmp', 'svg']
  if (images.includes(ext)) return '🖼️'
  if (ext === 'pdf') return '📕'
  if (['md', 'markdown', 'txt', 'log'].includes(ext)) return '📄'
  if (['zip', 'gz', 'tar', 'rar', '7z'].includes(ext)) return '🗜️'
  if (['csv', 'xlsx', 'xls'].includes(ext)) return '📊'
  if (['mp3', 'wav', 'flac'].includes(ext)) return '🎵'
  if (['mp4', 'webm', 'mov'].includes(ext)) return '🎬'
  return '📎'
}

function errDetail(e: unknown, fallback: string): string {
  const err = e as { response?: { data?: { detail?: string } } }
  return err.response?.data?.detail || fallback
}

function downloadUrl(name: string): string {
  return driveDownloadUrl(scope.value, joinPath(name), tokenStore.token)
}

async function loadSummary() {
  try {
    summary.value = await getDriveSummary()
  } catch (e) {
    toast.showError(errDetail(e, '无法加载网盘信息'))
  }
}

async function loadListing() {
  loading.value = true
  try {
    listing.value = await listDrive(scope.value, path.value)
  } catch (e) {
    toast.showError(errDetail(e, '加载目录失败'))
    listing.value = null
  } finally {
    loading.value = false
  }
}

async function reload() {
  await loadSummary()
  await loadListing()
}

async function switchScope(next: DriveScope) {
  scope.value = next
  path.value = ''
  await loadListing()
}

async function openPath(next: string) {
  path.value = next
  await loadListing()
}

function activate(entry: DriveEntry) {
  if (entry.is_dir) {
    openPath(joinPath(entry.name))
  } else {
    preview(entry)
  }
}

// ── 上传 ────────────────────────────────────────────────

function pickFiles() {
  fileInput.value?.click()
}

async function onFilesPicked(ev: Event) {
  const input = ev.target as HTMLInputElement
  const files = Array.from(input.files || [])
  input.value = ''
  await doUpload(files)
}

async function onDrop(ev: DragEvent) {
  dragging.value = false
  if (!listing.value?.can_write) {
    toast.showError('该目录只读，无法上传')
    return
  }
  const files = Array.from(ev.dataTransfer?.files || [])
  if (files.length) await doUpload(files)
}

async function doUpload(files: File[]) {
  if (!files.length) return
  const limit = (listing.value?.max_upload_mb ?? summary.value?.max_upload_mb ?? 200) * 1024 * 1024
  const tooBig = files.filter((f) => f.size > limit)
  if (tooBig.length) {
    toast.showError(`以下文件超过单文件上限 ${listing.value?.max_upload_mb} MB：${tooBig.map((f) => f.name).join('、')}`)
    return
  }
  uploading.value = true
  uploadPercent.value = 0
  try {
    const res = await uploadDriveFiles(scope.value, path.value, files, (p) => (uploadPercent.value = p))
    const ok = res.files.filter((f) => f.ok).length
    const failed = res.files.filter((f) => !f.ok)
    if (ok) toast.showSuccess(`已上传 ${ok} 个文件`)
    for (const f of failed) toast.showError(`${f.name}：${f.detail || '上传失败'}`)
    await reload()
  } catch (e) {
    toast.showError(errDetail(e, '上传失败'))
  } finally {
    uploading.value = false
    uploadPercent.value = 0
  }
}

// ── 目录/文件操作 ───────────────────────────────────────

function openMkdir() {
  mkdirName.value = ''
  mkdirVisible.value = true
}

async function submitMkdir() {
  const name = mkdirName.value.trim()
  if (!name) return
  busy.value = true
  try {
    await createDriveFolder(scope.value, path.value, name)
    mkdirVisible.value = false
    toast.showSuccess('已创建文件夹')
    await loadListing()
  } catch (e) {
    toast.showError(errDetail(e, '创建失败'))
  } finally {
    busy.value = false
  }
}

function openRename(entry: DriveEntry) {
  renameTarget.value = entry
  renameName.value = entry.name
}

async function submitRename() {
  if (!renameTarget.value) return
  const name = renameName.value.trim()
  if (!name || name === renameTarget.value.name) {
    renameTarget.value = null
    return
  }
  busy.value = true
  try {
    await renameDriveEntry(scope.value, joinPath(renameTarget.value.name), name)
    renameTarget.value = null
    toast.showSuccess('已重命名')
    await loadListing()
  } catch (e) {
    toast.showError(errDetail(e, '重命名失败'))
  } finally {
    busy.value = false
  }
}

function openMove(entry: DriveEntry) {
  moveTarget.value = entry
  moveToDir.value = ''
}

async function submitMove() {
  if (!moveTarget.value) return
  busy.value = true
  try {
    await moveDriveEntry(scope.value, joinPath(moveTarget.value.name), moveToDir.value.trim())
    moveTarget.value = null
    toast.showSuccess('已移动')
    await loadListing()
  } catch (e) {
    toast.showError(errDetail(e, '移动失败'))
  } finally {
    busy.value = false
  }
}

async function remove(entry: DriveEntry) {
  const ok = await confirmRef.value?.show(
    entry.is_dir ? '删除目录' : '删除文件',
    entry.is_dir
      ? `确定删除目录 ${entry.name} 及其全部内容？此操作不可撤销。`
      : `确定删除文件 ${entry.name}？此操作不可撤销。`
  )
  if (!ok) return
  busy.value = true
  try {
    await deleteDriveEntry(scope.value, joinPath(entry.name), true)
    toast.showSuccess('已删除')
    await loadListing()
  } catch (e) {
    toast.showError(errDetail(e, '删除失败'))
  } finally {
    busy.value = false
  }
}

// ── 预览 ────────────────────────────────────────────────

async function preview(entry: DriveEntry) {
  previewTarget.value = entry
  previewText.value = ''
  previewTruncated.value = false
  const ext = entry.ext
  if (['png', 'jpg', 'jpeg', 'gif', 'webp', 'bmp', 'svg'].includes(ext)) {
    previewKind.value = 'image'
    return
  }
  if (ext === 'pdf') {
    previewKind.value = 'pdf'
    return
  }
  const textLike = [
    'txt', 'md', 'markdown', 'json', 'csv', 'log', 'yml', 'yaml', 'xml',
    'rs', 'ts', 'js', 'py', 'sh', 'c', 'h', 'cpp', 'go', 'java', 'toml',
    'ini', 'conf', 'css', 'sql', 'html', 'htm'
  ]
  if (textLike.includes(ext)) {
    previewKind.value = 'text'
    try {
      const res = await getDriveText(scope.value, joinPath(entry.name))
      previewText.value = res.content
      previewTruncated.value = res.truncated
    } catch (e) {
      toast.showError(errDetail(e, '读取文本失败'))
    }
    return
  }
  previewKind.value = 'none'
}

function closePreview() {
  previewTarget.value = null
  previewText.value = ''
}

onMounted(async () => {
  await loadSummary()
  if (summary.value?.enabled) {
    await loadListing()
  }
})
</script>

<style scoped>
.page-header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
  margin-bottom: 16px;
  flex-wrap: wrap;
}
.page-header h2 {
  font-size: 20px;
  font-weight: 600;
}
.header-actions {
  display: flex;
  gap: 8px;
}
.notice-warn {
  background: var(--warning-bg);
  color: var(--warning-text);
  padding: 12px 14px;
  border-radius: 8px;
  font-size: 14px;
}
.drive-layout {
  display: grid;
  grid-template-columns: 220px 1fr;
  gap: 16px;
  align-items: start;
}
.scope-panel {
  background: var(--bg-white);
  border: 1px solid var(--border);
  border-radius: 8px;
  padding: 12px;
  position: sticky;
  top: 12px;
}
.scope-group + .scope-group {
  margin-top: 14px;
}
.scope-group-title {
  font-size: 12px;
  color: var(--text-muted);
  padding: 0 6px 6px;
}
.scope-item {
  display: flex;
  align-items: center;
  gap: 8px;
  width: 100%;
  padding: 8px 10px;
  border: none;
  background: transparent;
  border-radius: 6px;
  cursor: pointer;
  text-align: left;
  font-size: 14px;
  color: var(--text);
  min-height: 40px;
}
.scope-item:hover {
  background: var(--bg);
}
.scope-item.active {
  background: var(--primary-light);
  color: var(--primary-dark);
  font-weight: 600;
}
.scope-name {
  flex: 1;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.scope-size,
.scope-tag {
  font-size: 11px;
  color: var(--text-muted);
}
.scope-empty {
  font-size: 12px;
  color: var(--text-muted);
  padding: 4px 6px;
}
.file-panel {
  position: relative;
  background: var(--bg-white);
  border: 1px solid var(--border);
  border-radius: 8px;
  padding: 12px 14px 16px;
  min-height: 320px;
}
.toolbar {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 10px;
  flex-wrap: wrap;
  margin-bottom: 10px;
}
.breadcrumb {
  display: flex;
  align-items: center;
  gap: 4px;
  flex-wrap: wrap;
  min-width: 0;
}
.crumb {
  border: none;
  background: transparent;
  color: var(--primary);
  cursor: pointer;
  font-size: 13px;
  padding: 2px 4px;
  border-radius: 4px;
}
.crumb:hover {
  background: var(--bg);
}
.crumb-sep {
  color: var(--text-muted);
  font-size: 12px;
}
.toolbar-actions {
  display: flex;
  align-items: center;
  gap: 8px;
}
.readonly-tag {
  font-size: 12px;
  color: var(--text-muted);
  background: var(--bg);
  border-radius: 4px;
  padding: 2px 8px;
}
.usage-bar {
  margin-bottom: 12px;
}
.usage-text {
  font-size: 12px;
  color: var(--text-secondary);
  display: flex;
  gap: 8px;
  flex-wrap: wrap;
  align-items: center;
}
.usage-hint {
  color: var(--text-muted);
}
.usage-track {
  height: 4px;
  background: var(--bg-muted);
  border-radius: 2px;
  margin-top: 6px;
  overflow: hidden;
}
.usage-fill {
  height: 100%;
  background: var(--primary);
  transition: width 0.2s;
}
.usage-fill.danger {
  background: var(--danger);
}
.table-wrap {
  overflow-x: auto;
}
.file-table {
  width: 100%;
  border-collapse: collapse;
  font-size: 13px;
  min-width: 620px;
}
.file-table th,
.file-table td {
  text-align: left;
  padding: 8px 10px;
  border-bottom: 1px solid var(--border);
  vertical-align: middle;
}
.file-table th {
  color: var(--text-secondary);
  font-weight: 500;
  background: var(--bg-table-header);
}
.col-size {
  width: 90px;
}
.col-time {
  width: 170px;
}
.col-actions {
  text-align: right;
}
.col-actions .btn + .btn,
.col-actions .btn + a,
.col-actions a + .btn {
  margin-left: 6px;
}
.nowrap {
  white-space: nowrap;
}
.entry-name {
  border: none;
  background: transparent;
  cursor: pointer;
  font-size: 13px;
  color: var(--text);
  padding: 2px 4px;
  border-radius: 4px;
  display: inline-flex;
  align-items: center;
  gap: 6px;
  text-align: left;
}
.entry-name:hover {
  background: var(--bg);
  color: var(--primary);
}
.entry-name.dir {
  font-weight: 500;
}
.entry-icon {
  font-size: 15px;
}
.link-btn {
  border: none;
  background: transparent;
  color: var(--primary);
  cursor: pointer;
  font-size: 13px;
  padding: 2px 4px;
}
.empty-hint {
  text-align: center;
  color: var(--text-muted);
  font-size: 13px;
  padding: 20px 0;
}
.drop-overlay {
  position: absolute;
  inset: 0;
  background: rgba(25, 118, 210, 0.08);
  border: 2px dashed var(--primary);
  border-radius: 8px;
  display: flex;
  align-items: center;
  justify-content: center;
  color: var(--primary-dark);
  font-weight: 600;
  pointer-events: none;
}
.hidden-input {
  display: none;
}
.form-hint {
  font-size: 12px;
  color: var(--text-muted);
}
.modal .form-group input {
  width: 100%;
  padding: 10px 12px;
  border: 1px solid var(--border);
  border-radius: 6px;
  font-size: 14px;
}
/* 预览 */
.preview-overlay {
  z-index: 120;
}
.preview-box {
  background: var(--bg-white);
  border-radius: 10px;
  width: min(1100px, 94vw);
  height: min(88vh, 900px);
  display: flex;
  flex-direction: column;
  overflow: hidden;
}
.preview-header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 10px;
  padding: 10px 14px;
  border-bottom: 1px solid var(--border);
  background: var(--bg-footer);
}
.preview-title {
  font-size: 14px;
  font-weight: 600;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.preview-actions {
  display: flex;
  gap: 8px;
}
.preview-body {
  flex: 1;
  min-height: 0;
  display: flex;
  align-items: center;
  justify-content: center;
  background: var(--bg);
  overflow: auto;
  padding: 12px;
}
.preview-body img {
  max-width: 100%;
  max-height: 100%;
  object-fit: contain;
}
.preview-frame {
  width: 100%;
  height: 100%;
  border: none;
  background: #fff;
}
.preview-text {
  width: 100%;
  height: 100%;
  margin: 0;
  padding: 14px;
  overflow: auto;
  background: var(--bg-white);
  font-family: 'SF Mono', 'Cascadia Code', monospace;
  font-size: 12.5px;
  line-height: 1.6;
  white-space: pre-wrap;
  word-break: break-word;
}
.preview-unsupported {
  color: var(--text-secondary);
  font-size: 14px;
}
.preview-foot {
  padding: 6px 14px;
  font-size: 12px;
  color: var(--text-muted);
  border-top: 1px solid var(--border);
}
@media (max-width: 768px) {
  .drive-layout {
    grid-template-columns: 1fr;
  }
  .scope-panel {
    position: static;
    display: flex;
    gap: 12px;
    overflow-x: auto;
  }
  .scope-group + .scope-group {
    margin-top: 0;
  }
  .scope-item {
    white-space: nowrap;
  }
}
</style>
