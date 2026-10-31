<template>
  <div class="drive-admin container">
    <div class="page-header">
      <h2>网盘管理</h2>
      <router-link class="btn btn-sm btn-outline" to="/assist/drive">返回网盘</router-link>
    </div>

    <!-- 网盘设置 -->
    <section class="section">
      <h3>网盘设置</h3>
      <div class="settings-grid">
        <label class="setting">
          <input v-model="settings.enabled" type="checkbox" />
          <span>启用网盘功能</span>
        </label>
        <div class="setting">
          <span>单文件上限（MB）</span>
          <input v-model.number="settings.max_upload_mb" type="number" min="1" max="10240" />
        </div>
        <div class="setting">
          <span>默认配额（MB，0 = 不限）</span>
          <input v-model.number="settings.default_quota_mb" type="number" min="0" />
        </div>
      </div>
      <div class="settings-actions">
        <button class="btn btn-sm" :disabled="settingsBusy" @click="saveSettings">
          {{ settingsBusy ? '保存中…' : '保存设置' }}
        </button>
        <span class="form-hint">修改单文件上限后，上传接口的请求体上限会在服务重启后生效。</span>
      </div>
    </section>

    <!-- 公有文件夹 -->
    <section class="section">
      <h3>公有文件夹（{{ folders.length }}）</h3>
      <p class="section-hint">
        权限按<b>角色阈值</b>判定：角色等级 admin(3) &gt; view-upload(2) &gt; view-only(1)，
        用户角色等级 ≥ 阈值即可读/写。例如 `read_role=view-only` + `write_role=admin` 表示全站可读、仅管理员可写。
      </p>

      <div class="create-row">
        <input v-model="newFolder.name" type="text" placeholder="文件夹名称" />
        <select v-model="newFolder.read_role">
          <option value="view-only">读取：只读及以上</option>
          <option value="view-upload">读取：查看+上传及以上</option>
          <option value="admin">读取：仅管理员</option>
        </select>
        <select v-model="newFolder.write_role">
          <option value="view-upload">写入：查看+上传及以上</option>
          <option value="admin">写入：仅管理员</option>
          <option value="view-only">写入：所有人</option>
        </select>
        <button class="btn btn-sm" :disabled="busy" @click="createFolder">创建</button>
      </div>

      <div class="table-wrap">
        <table class="folder-table">
          <thead>
            <tr>
              <th>名称</th>
              <th>读取阈值</th>
              <th>写入阈值</th>
              <th>占用</th>
              <th>创建时间</th>
              <th class="col-actions">操作</th>
            </tr>
          </thead>
          <tbody>
            <tr v-for="f in folders" :key="f.id">
              <td>
                <input
                  class="name-input"
                  :value="f.name"
                  @change="renameFolder(f, ($event.target as HTMLInputElement).value)"
                />
              </td>
              <td>
                <select :value="f.read_role" @change="setRole(f, 'read_role', ($event.target as HTMLSelectElement).value)">
                  <option value="view-only">只读及以上</option>
                  <option value="view-upload">查看+上传及以上</option>
                  <option value="admin">仅管理员</option>
                </select>
              </td>
              <td>
                <select :value="f.write_role" @change="setRole(f, 'write_role', ($event.target as HTMLSelectElement).value)">
                  <option value="view-only">所有人</option>
                  <option value="view-upload">查看+上传及以上</option>
                  <option value="admin">仅管理员</option>
                </select>
              </td>
              <td class="nowrap">{{ formatSize(f.used_bytes) }}</td>
              <td class="nowrap">{{ formatTime(f.created_at) }}</td>
              <td class="col-actions nowrap">
                <button class="btn btn-sm btn-outline-danger" @click="removeFolder(f)">删除</button>
              </td>
            </tr>
            <tr v-if="!folders.length">
              <td colspan="6" class="empty-hint">还没有公有文件夹</td>
            </tr>
          </tbody>
        </table>
      </div>
    </section>

    <!-- 配额 -->
    <section class="section">
      <h3>用户配额</h3>
      <p class="section-hint">
        默认配额 {{ settings.default_quota_mb || 0 }} MB（0 = 不限）。填写覆盖值可单独调整；点「清除」回退默认。
      </p>
      <div class="table-wrap">
        <table class="folder-table">
          <thead>
            <tr>
              <th>用户</th>
              <th>角色</th>
              <th>已用</th>
              <th>生效配额</th>
              <th>覆盖值（MB）</th>
              <th class="col-actions">操作</th>
            </tr>
          </thead>
          <tbody>
            <tr v-for="u in quotas" :key="u.user_id">
              <td>
                <span class="mono">{{ u.username }}</span>
                <span v-if="u.display_name" class="muted">{{ u.display_name }}</span>
              </td>
              <td>{{ roleLabel(u.role) }}</td>
              <td class="nowrap">{{ formatSize(u.used_bytes) }}</td>
              <td class="nowrap">{{ u.quota_bytes ? formatSize(u.quota_bytes) : '不限' }}</td>
              <td>
                <input
                  class="quota-input"
                  type="number"
                  min="0"
                  :value="u.quota_override_mb ?? ''"
                  placeholder="默认"
                  @change="setQuota(u, ($event.target as HTMLInputElement).value)"
                />
              </td>
              <td class="col-actions nowrap">
                <button
                  class="btn btn-sm btn-outline"
                  :disabled="u.quota_override_mb === null"
                  @click="clearQuota(u)"
                >清除</button>
              </td>
            </tr>
            <tr v-if="!quotas.length">
              <td colspan="6" class="empty-hint">没有用户</td>
            </tr>
          </tbody>
        </table>
      </div>
    </section>

    <ConfirmDialog ref="confirmRef" />
  </div>
</template>

<script setup lang="ts">
import { onMounted, reactive, ref } from 'vue'
import {
  adminCreateDriveFolder,
  adminDeleteDriveFolder,
  adminGetDriveSettings,
  adminListDriveFolders,
  adminListDriveQuotas,
  adminSetDriveQuota,
  adminUpdateDriveFolder,
  adminUpdateDriveSettings
} from '@/api/drive'
import ConfirmDialog from '@/components/ConfirmDialog.vue'
import { useToast } from '@/composables/useToast'
import type { AdminDriveFolder, AdminDriveQuotaRow, AdminDriveSettings } from '@/types/drive'

const toast = useToast()
const confirmRef = ref<InstanceType<typeof ConfirmDialog> | null>(null)

const folders = ref<AdminDriveFolder[]>([])
const quotas = ref<AdminDriveQuotaRow[]>([])
const settings = reactive<AdminDriveSettings>({
  enabled: true,
  max_upload_mb: 200,
  default_quota_mb: 0
})
const settingsBusy = ref(false)
const busy = ref(false)
const newFolder = reactive({ name: '', read_role: 'view-only', write_role: 'admin' })

function roleLabel(role: string): string {
  const map: Record<string, string> = { admin: '管理员', 'view-only': '只读', 'view-upload': '查看+上传' }
  return map[role] || role
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

function formatTime(v?: string): string {
  if (!v) return '—'
  const d = new Date(v)
  return Number.isNaN(d.getTime()) ? v : d.toLocaleString()
}

function errDetail(e: unknown, fallback: string): string {
  const err = e as { response?: { data?: { detail?: string } } }
  return err.response?.data?.detail || fallback
}

async function loadAll() {
  try {
    const [f, q, s] = await Promise.all([
      adminListDriveFolders(),
      adminListDriveQuotas(),
      adminGetDriveSettings()
    ])
    folders.value = f.folders
    quotas.value = q.users
    Object.assign(settings, s)
  } catch (e) {
    toast.showError(errDetail(e, '加载网盘管理数据失败'))
  }
}

async function saveSettings() {
  settingsBusy.value = true
  try {
    const res = await adminUpdateDriveSettings({ ...settings })
    Object.assign(settings, res)
    toast.showSuccess('设置已保存')
  } catch (e) {
    toast.showError(errDetail(e, '保存失败'))
  } finally {
    settingsBusy.value = false
  }
}

async function createFolder() {
  const name = newFolder.name.trim()
  if (!name) {
    toast.showError('请填写文件夹名称')
    return
  }
  busy.value = true
  try {
    await adminCreateDriveFolder({
      name,
      read_role: newFolder.read_role,
      write_role: newFolder.write_role
    })
    newFolder.name = ''
    toast.showSuccess('已创建公有文件夹')
    await loadAll()
  } catch (e) {
    toast.showError(errDetail(e, '创建失败'))
  } finally {
    busy.value = false
  }
}

async function updateFolder(id: string, patch: { name?: string; read_role?: string; write_role?: string }) {
  try {
    await adminUpdateDriveFolder(id, patch)
    toast.showSuccess('已更新')
    await loadAll()
  } catch (e) {
    toast.showError(errDetail(e, '更新失败'))
    await loadAll()
  }
}

function renameFolder(f: AdminDriveFolder, name: string) {
  const trimmed = name.trim()
  if (!trimmed || trimmed === f.name) return
  updateFolder(f.id, { name: trimmed })
}

function setRole(f: AdminDriveFolder, key: 'read_role' | 'write_role', value: string) {
  if (f[key] === value) return
  updateFolder(f.id, { [key]: value })
}

async function removeFolder(f: AdminDriveFolder) {
  const ok = await confirmRef.value?.show(
    '删除公有文件夹',
    `确定删除「${f.name}」？该文件夹内的文件会一并删除且不可恢复。`
  )
  if (!ok) return
  try {
    await adminDeleteDriveFolder(f.id, true)
    toast.showSuccess('已删除')
    await loadAll()
  } catch (e) {
    toast.showError(errDetail(e, '删除失败'))
  }
}

async function setQuota(u: AdminDriveQuotaRow, raw: string) {
  const value = Number(raw)
  if (!Number.isFinite(value) || value < 0) {
    toast.showError('配额需为非负整数（MB）')
    return
  }
  try {
    await adminSetDriveQuota(u.user_id, Math.round(value))
    toast.showSuccess(`已设置 ${u.username} 的配额为 ${value} MB`)
    await loadAll()
  } catch (e) {
    toast.showError(errDetail(e, '设置失败'))
    await loadAll()
  }
}

async function clearQuota(u: AdminDriveQuotaRow) {
  try {
    await adminSetDriveQuota(u.user_id, 0, true)
    toast.showSuccess(`已清除 ${u.username} 的配额覆盖`)
    await loadAll()
  } catch (e) {
    toast.showError(errDetail(e, '操作失败'))
  }
}

onMounted(loadAll)
</script>

<style scoped>
.page-header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
  margin-bottom: 16px;
}
.page-header h2 {
  font-size: 20px;
  font-weight: 600;
}
.section-hint {
  font-size: 13px;
  color: var(--text-secondary);
  margin-bottom: 12px;
  line-height: 1.7;
}
.settings-grid {
  display: grid;
  grid-template-columns: repeat(auto-fit, minmax(220px, 1fr));
  gap: 12px 20px;
}
.setting {
  display: flex;
  align-items: center;
  gap: 8px;
  font-size: 14px;
}
.setting input[type='number'] {
  width: 110px;
  padding: 6px 8px;
  border: 1px solid var(--border);
  border-radius: 6px;
}
.settings-actions {
  display: flex;
  align-items: center;
  gap: 12px;
  margin-top: 14px;
  flex-wrap: wrap;
}
.form-hint {
  font-size: 12px;
  color: var(--text-muted);
}
.create-row {
  display: flex;
  gap: 8px;
  flex-wrap: wrap;
  margin-bottom: 14px;
}
.create-row input[type='text'] {
  flex: 1;
  min-width: 160px;
  padding: 8px 10px;
  border: 1px solid var(--border);
  border-radius: 6px;
  font-size: 13px;
}
.create-row select,
.folder-table select {
  padding: 7px 8px;
  border: 1px solid var(--border);
  border-radius: 6px;
  font-size: 13px;
  background: var(--bg-white);
}
.table-wrap {
  overflow-x: auto;
}
.folder-table {
  width: 100%;
  border-collapse: collapse;
  font-size: 13px;
  min-width: 720px;
}
.folder-table th,
.folder-table td {
  text-align: left;
  padding: 8px 10px;
  border-bottom: 1px solid var(--border);
  vertical-align: middle;
}
.folder-table th {
  background: var(--bg-table-header);
  color: var(--text-secondary);
  font-weight: 500;
  white-space: nowrap;
}
.name-input {
  width: 100%;
  min-width: 120px;
  padding: 6px 8px;
  border: 1px solid transparent;
  border-radius: 5px;
  font-size: 13px;
  background: transparent;
}
.name-input:hover,
.name-input:focus {
  border-color: var(--border);
  background: var(--bg-white);
  outline: none;
}
.quota-input {
  width: 100px;
  padding: 6px 8px;
  border: 1px solid var(--border);
  border-radius: 5px;
  font-size: 13px;
}
.col-actions {
  text-align: right;
}
.nowrap {
  white-space: nowrap;
}
.mono {
  font-family: 'SF Mono', 'Cascadia Code', monospace;
}
.muted {
  color: var(--text-muted);
  font-size: 12px;
  margin-left: 6px;
}
.empty-hint {
  text-align: center;
  color: var(--text-muted);
  font-size: 13px;
  padding: 18px 0;
}
</style>
