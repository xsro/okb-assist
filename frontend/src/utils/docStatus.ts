/**
 * 文献状态的**前端推导**。
 *
 * 后端 `GET /assist/api/documents` 只返回「数据库记录 + 运行时事实」的原始字段：
 * `has_md` / `has_pdf` / `has_zip`、`synced` / `fs_synced_at`、`pdf_health` /
 * `pdf_health_message`、`mineru_task_id`、`runtime_task`（进程内活动任务）、
 * `last_error`（进程内最近失败原因）。
 *
 * 不再返回 `status` / `status_message` —— 展示用的状态与文案由本文件组合，
 * 因此可以比后端枚举给出更细的说明（无 PDF / 待解析 / 状态未知 / 解析中 42% …）。
 */

import type { Document } from '@/types/document'

/** 与后端 `DocStatus` 对齐的 4 个取值（用于配色 / 图标，不是文案） */
export type DocStatusKey = 'uploaded' | 'parsing' | 'markdown_done' | 'error'

export interface DerivedDocStatus {
  /** 粗粒度状态（只用于徽章配色 / 图标） */
  status: DocStatusKey
  /**
   * **徽章主文字：实际存在的文件类型**，如 `pdf|md|zip`、`pdf|md`、`md`、`无文件`、`未知`。
   * 直接反映磁盘（同步列）事实，比「已上传 / 解析完成」更直观。
   */
  files: string
  /** 正在跑的任务标签（`解析中` / `提取元数据中` / `索引中` …），无任务为 null */
  activity: string | null
  /** 补充说明（hover tooltip / 详情页「错误详情」行） */
  detail: string | null
  /** 0-100，仅 `parsing` 且有活动任务时非空 */
  progress: number | null
  /** 文件事实是否已同步过（false = 状态未知） */
  synced: boolean
  /** 原始 PDF 健康值：`ok` / `warning` / `error` / `missing` / `null`(未检查) */
  health: string | null
  healthDetail: string | null
}

/** PDF 健康 → 徽章里的短标注（`ok` / 未检查不加标注） */
export const PDF_HEALTH_SHORT: Record<string, string> = {
  warning: 'warn',
  error: 'bad',
  missing: 'missing'
}

/**
 * 由 `has_pdf` / `has_md` / `has_zip` 拼出「存在的文件类型」，顺序固定为 `pdf|md|zip`。
 *
 * PDF 健康有问题时直接在文件类型上标注：`pdf(bad)`（损坏）/ `pdf(warn)`（警告），
 * 例：`pdf(bad)|md`、`pdf(warn)|md|zip`。
 */
export function filesLabel(
  hasPdf: boolean,
  hasMd: boolean,
  hasZip: boolean,
  health?: string | null
): string {
  const parts: string[] = []
  if (hasPdf) {
    const short = health ? PDF_HEALTH_SHORT[health] : undefined
    parts.push(short ? `pdf(${short})` : 'pdf')
  }
  if (hasMd) parts.push('md')
  if (hasZip) parts.push('zip')
  return parts.length ? parts.join('|') : '无文件'
}

/** 活动任务类型 → 中文标签 */
const TASK_LABELS: Record<string, string> = {
  parse: '解析中',
  extract: '提取元数据中',
  index: '索引中',
  hash: '重算哈希中',
  'pdf-health': 'PDF 健康检查中',
  crossref: '获取 Crossref 中',
  'extract-pdf-meta': '读取 PDF 元信息中'
}

/** PDF 健康值 → 中文标签 */
export const HEALTH_LABELS: Record<string, string> = {
  ok: '正常',
  warning: '有警告',
  error: '损坏',
  missing: '缺失',
  unchecked: '未检查'
}

/**
 * 由原始字段推导展示状态。判断顺序与后端 `derive_state()` 保持一致，
 * 只是文案更细（例如 `uploaded` 会拆成「待解析 / 无 PDF / 状态未知」）。
 */
export function deriveDocStatus(doc: Document): DerivedDocStatus {
  const synced = doc.synced !== false
  const health = doc.pdf_health ?? null
  const healthDetail = doc.pdf_health_message ?? null
  const healthSuffix =
    health && health !== 'ok'
      ? `PDF 健康：${HEALTH_LABELS[health] || health}${healthDetail ? ` — ${healthDetail}` : ''}`
      : null

  // 主文字：已同步 → 真实文件类型；未同步 → 未知（不能谎报「无文件」）
  const files = synced ? filesLabel(doc.has_pdf, doc.has_md, doc.has_zip, health) : '未知'

  const base = {
    files,
    synced,
    health,
    healthDetail,
    detail: healthSuffix,
    progress: null as number | null,
    activity: null as string | null
  }

  // 1) 有活动任务（解析 / 提取 / 索引 / 维护任务）
  if (doc.runtime_task) {
    const t = doc.runtime_task
    const label = TASK_LABELS[t.task_type] || '处理中'
    return {
      ...base,
      status: 'parsing',
      activity: label,
      detail: t.message || label,
      progress: typeof t.progress === 'number' ? t.progress : null
    }
  }

  // 2) 进程内记录的失败原因
  if (doc.last_error) {
    return { ...base, status: 'error', detail: doc.last_error }
  }

  // 3) 有 Markdown = 已解析
  if (doc.has_md) {
    return { ...base, status: 'markdown_done', detail: healthSuffix || 'PDF 解析完成' }
  }

  // 4) 外部解析任务仍在（重启后等待/续跑上次 MinerU 任务）
  if (doc.mineru_task_id) {
    return {
      ...base,
      status: 'parsing',
      activity: '恢复解析任务中',
      detail: '正在等待上次提交的 MinerU 任务结果'
    }
  }

  // 5) 从未同步过文件事实 → 状态未知
  if (!synced) {
    return {
      ...base,
      status: 'uploaded',
      detail: '文件事实尚未从磁盘同步（打开详情页或重新同步后即可确定）'
    }
  }

  // 6) 有 PDF 待解析
  if (doc.has_pdf) {
    return { ...base, status: 'uploaded', detail: healthSuffix || 'PDF 已就绪，尚未解析' }
  }

  // 7) 已同步且确定没有 PDF（例如 BibTeX 只导入条目）
  return {
    ...base,
    status: 'uploaded',
    detail: healthDetail || '仅有书目条目，尚未上传 PDF'
  }
}

