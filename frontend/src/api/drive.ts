/** 用户网盘 API */
import { apiDelete, apiGet, apiPost, apiPut, apiUpload } from '@/api/client'
import type {
  AdminDriveFolder,
  AdminDriveQuotasResponse,
  AdminDriveSettings,
  DriveListResponse,
  DriveScope,
  DriveSummary,
  DriveTextResponse,
  DriveUploadResponse
} from '@/types/drive'

/** 网盘概览：开关、配额、可访问的公有文件夹 */
export function getDriveSummary(): Promise<DriveSummary> {
  return apiGet<DriveSummary>('/assist/api/drive')
}

/** 列目录 */
export function listDrive(scope: DriveScope, path: string): Promise<DriveListResponse> {
  return apiGet<DriveListResponse>('/assist/api/drive/list', { scope, path })
}

/** 新建文件夹 */
export function createDriveFolder(
  scope: DriveScope,
  path: string,
  name: string
): Promise<{ ok: boolean; path: string }> {
  return apiPost('/assist/api/drive/mkdir', { scope, path, name })
}

/** 重命名 */
export function renameDriveEntry(
  scope: DriveScope,
  path: string,
  newName: string
): Promise<{ ok: boolean }> {
  return apiPost('/assist/api/drive/rename', { scope, path, new_name: newName })
}

/** 移动到目标目录 */
export function moveDriveEntry(
  scope: DriveScope,
  from: string,
  toDir: string
): Promise<{ ok: boolean; name?: string }> {
  return apiPost('/assist/api/drive/move', { scope, from, to_dir: toDir })
}

/** 删除文件/目录（目录需 recursive=true） */
export function deleteDriveEntry(
  scope: DriveScope,
  path: string,
  recursive = true
): Promise<{ ok: boolean }> {
  return apiDelete(
    `/assist/api/drive/file?scope=${encodeURIComponent(scope)}&path=${encodeURIComponent(path)}&recursive=${recursive ? 1 : 0}`
  )
}

/** 上传（可多选）；`scope`/`path` 必须在文件字段之前追加 */
export function uploadDriveFiles(
  scope: DriveScope,
  path: string,
  files: File[],
  onProgress?: (percent: number) => void
): Promise<DriveUploadResponse> {
  const form = new FormData()
  form.append('scope', scope)
  form.append('path', path)
  for (const file of files) form.append('file', file, file.name)
  return apiUpload<DriveUploadResponse>('/assist/api/drive/upload', form, {
    timeout: 0,
    onUploadProgress: (e) => {
      if (onProgress && e.total) onProgress(Math.round((e.loaded / e.total) * 100))
    }
  })
}

/** 下载地址（带 token 查询参数，便于 <a download> 直接使用） */
export function driveDownloadUrl(scope: DriveScope, path: string, token: string): string {
  return `/assist/api/drive/download?scope=${encodeURIComponent(scope)}&path=${encodeURIComponent(path)}&token=${encodeURIComponent(token)}`
}

/** 内联预览地址（图片 / PDF；带 token 以支持 <img>/<iframe>） */
export function driveRawUrl(scope: DriveScope, path: string, token: string): string {
  return `/assist/api/drive/raw?scope=${encodeURIComponent(scope)}&path=${encodeURIComponent(path)}&token=${encodeURIComponent(token)}`
}

/** 文本预览内容（≤512KB） */
export function getDriveText(scope: DriveScope, path: string): Promise<DriveTextResponse> {
  return apiGet<DriveTextResponse>('/assist/api/drive/text', { scope, path })
}

// ── 管理员：公有文件夹 / 配额 / 设置 ────────────────────

export function adminListDriveFolders(): Promise<{ folders: AdminDriveFolder[]; count: number }> {
  return apiGet('/assist/api/admin/drive/folders')
}

export function adminCreateDriveFolder(payload: {
  name: string
  read_role: string
  write_role: string
}): Promise<{ folder: AdminDriveFolder }> {
  return apiPost('/assist/api/admin/drive/folders', payload)
}

export function adminUpdateDriveFolder(
  id: string,
  patch: { name?: string; read_role?: string; write_role?: string }
): Promise<{ folder: AdminDriveFolder }> {
  return apiPut(`/assist/api/admin/drive/folders/${encodeURIComponent(id)}`, patch)
}

export function adminDeleteDriveFolder(id: string, force = false): Promise<{ ok: boolean }> {
  return apiDelete(`/assist/api/admin/drive/folders/${encodeURIComponent(id)}?force=${force ? 1 : 0}`)
}

export function adminListDriveQuotas(): Promise<AdminDriveQuotasResponse> {
  return apiGet('/assist/api/admin/drive/quotas')
}

/** 设置配额（MB）；`clear=true` 时删除覆盖值，回退默认配额 */
export function adminSetDriveQuota(
  userId: string,
  quotaMb: number,
  clear = false
): Promise<{ ok: boolean; quota_mb: number | null }> {
  return apiPut(`/assist/api/admin/drive/quotas/${encodeURIComponent(userId)}`, {
    quota_mb: quotaMb,
    clear
  })
}

export function adminGetDriveSettings(): Promise<AdminDriveSettings> {
  return apiGet('/assist/api/admin/drive/settings')
}

export function adminUpdateDriveSettings(
  patch: Partial<AdminDriveSettings>
): Promise<AdminDriveSettings> {
  return apiPut('/assist/api/admin/drive/settings', patch)
}
