/** 网盘相关类型定义 */

export type DriveScope = 'private' | `public:${string}`

export interface DrivePublicFolder {
  id: string
  name: string
  read_role: string
  write_role: string
  can_write?: boolean
}

export interface DriveSummary {
  enabled: boolean
  max_upload_mb: number
  quota_bytes: number | null
  used_bytes: number
  public_folders: DrivePublicFolder[]
  user: { id: string; username: string; role: string }
}

export interface DriveEntry {
  name: string
  is_dir: boolean
  size: number
  modified: string | null
  ext: string
}

export interface DriveBreadcrumb {
  name: string
  path: string
}

export interface DriveListResponse {
  scope: string
  label: string
  can_write: boolean
  path: string
  breadcrumb: DriveBreadcrumb[]
  entries: DriveEntry[]
  used_bytes: number
  quota_bytes: number | null
  max_upload_mb: number
}

export interface DriveUploadResult {
  name: string
  ok: boolean
  size?: number
  detail?: string
}

export interface DriveUploadResponse {
  files: DriveUploadResult[]
  used_bytes: number
  quota_bytes: number | null
}

export interface DriveTextResponse {
  content: string
  truncated: boolean
  size: number
  name: string
}

/** 管理员视角的公有文件夹 */
export interface AdminDriveFolder {
  id: string
  name: string
  read_role: string
  write_role: string
  created_at: string
  used_bytes: number
}

export interface AdminDriveQuotaRow {
  user_id: string
  username: string
  display_name: string
  role: string
  used_bytes: number
  quota_bytes: number | null
  quota_override_mb: number | null
}

export interface AdminDriveQuotasResponse {
  users: AdminDriveQuotaRow[]
  default_quota_mb: number
  max_upload_mb: number
}

export interface AdminDriveSettings {
  enabled: boolean
  max_upload_mb: number
  default_quota_mb: number
  /** 实际存储根目录（只读，来自 system.json 的 drive_root，改后需重启） */
  root?: string
}
