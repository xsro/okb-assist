/**
 * 把「上传失败」的原始错误翻译成一句能看懂的话 + 一句该怎么做的提示。
 *
 * 后端各失败路径的响应体并不统一（`{detail}` / `{error, message, existing_id}`，
 * 甚至有 axios 的网络层错误），这里统一收口，避免界面上直接出现
 * `Request failed with status code 500` 这种对用户毫无意义的文案。
 */

export type UploadFailureKind =
  | 'duplicate' // 内容重复
  | 'auth' // 未登录 / 登录失效
  | 'permission' // 角色不够
  | 'too-large' // 超过大小上限
  | 'invalid' // 文件本身不被接受
  | 'timeout' // 传输超时
  | 'network' // 连不上后端
  | 'canceled' // 主动取消
  | 'not-found' // 接口不存在（前后端版本不一致）
  | 'server' // 服务端 5xx
  | 'unknown'

export interface UploadFailure {
  kind: UploadFailureKind
  /** 主提示：一句话说清出了什么事 */
  message: string
  /** 补充提示：接下来该怎么做（可能为空） */
  hint?: string
  /** 原始 detail / 错误信息，用于悬停查看或排查 */
  detail?: string
  /** 重复文件时后端返回的已有文档 ID */
  existingId?: number
}

interface AxiosLikeError {
  code?: string
  message?: string
  response?: { status?: number; data?: unknown }
}

/** 单份 PDF 的上限，与后端 `MAX_DOC_UPLOAD_BYTES` 保持一致 */
export const MAX_UPLOAD_MB = 200

function pickDetail(raw: unknown): string {
  if (typeof raw === 'string') return raw
  if (!raw || typeof raw !== 'object') return ''
  const data = raw as Record<string, unknown>
  for (const key of ['detail', 'message', 'error']) {
    const value = data[key]
    if (typeof value === 'string' && value.trim()) return value.trim()
  }
  return ''
}

/** 把任意上传异常翻译成可读的失败描述 */
export function describeUploadError(err: unknown): UploadFailure {
  const e = (err ?? {}) as AxiosLikeError
  const status = e.response?.status
  const data = (e.response?.data ?? {}) as Record<string, unknown>
  const detail = pickDetail(e.response?.data) || (typeof e.message === 'string' ? e.message : '')

  // 1) 内容重复（本地查重伪造的 409，或服务端 file_hash 冲突）
  if (data.error === 'duplicate' || (status === 409 && /已存在|重复|duplicate/i.test(detail))) {
    const rawId = data.existing_id
    const existingId = typeof rawId === 'number' ? rawId : undefined
    return {
      kind: 'duplicate',
      message: '文献库里已有这份 PDF（内容完全相同）',
      hint: existingId
        ? '点「查看已有文献」打开它；若确实需要第二份记录，可用「强制上传」'
        : '若确实需要第二份记录，可用「强制上传」',
      detail,
      existingId
    }
  }

  // 2) 没有响应：网络层错误（断开 / 超时 / 被中断）
  if (status === undefined) {
    if (e.code === 'HASH_FAILED') {
      return {
        kind: 'invalid',
        message: '读取文件内容失败，无法计算校验值',
        hint: '文件可能已被移动或删除；超大文件也可能让浏览器内存不足，请重新选择文件（或改用按路径登记）',
        detail
      }
    }
    if (e.code === 'ECONNABORTED' || e.code === 'ETIMEDOUT') {
      return {
        kind: 'timeout',
        message: '上传超时，传输被中断',
        hint: '检查网络后重试；文件较大时确保网络稳定，必要时换有线网络或分批上传',
        detail
      }
    }
    if (e.code === 'ERR_CANCELED') {
      return { kind: 'canceled', message: '上传已取消', detail }
    }
    if (/network error|failed to fetch|load failed|networkerror/i.test(detail)) {
      return {
        kind: 'network',
        message: '连不上后端服务',
        hint: '确认后端已启动、地址与端口正确（默认 5001），然后重试',
        detail
      }
    }
    return {
      kind: 'network',
      message: '上传请求失败，没有收到服务端响应',
      hint: '可能是网络中断或后端进程重启，检查后重试',
      detail
    }
  }

  // 3) 有响应：按状态码 / 响应文案分类
  if (status === 401) {
    return {
      kind: 'auth',
      message: '登录状态已失效，无法上传',
      hint: '请重新登录后再试（令牌可能已过期或被撤销）',
      detail
    }
  }
  if (status === 403) {
    return {
      kind: 'permission',
      message: '当前账号没有上传权限',
      hint: '上传需要「管理员」或「查看+上传」角色，请联系管理员调整',
      detail
    }
  }
  if (status === 404) {
    return {
      kind: 'not-found',
      message: '上传接口不存在',
      hint: '前端与后端版本可能不一致，确认后端已更新到对应版本',
      detail
    }
  }
  if (status === 413) {
    return {
      kind: 'too-large',
      message: `文件超过服务端接收上限（${MAX_UPLOAD_MB} MB）`,
      hint: '可先压缩 PDF 或拆分后再上传',
      detail
    }
  }
  if (/过大|超过.{0,6}上限|too large|payload/i.test(detail)) {
    return {
      kind: 'too-large',
      message: detail || '文件超过大小上限',
      hint: `单份 PDF 上限 ${MAX_UPLOAD_MB} MB，可压缩或拆分后再上传`,
      detail
    }
  }
  if (status === 400) {
    return {
      kind: 'invalid',
      message: detail || '文件不被接受',
      hint: '只支持扩展名为 .pdf 且内容非空的文件',
      detail
    }
  }
  if (status >= 500) {
    return {
      kind: 'server',
      message: `服务端处理失败（HTTP ${status}）`,
      hint: '文件可能已部分写入，稍后重试即可；持续失败请查看后端日志',
      detail
    }
  }
  return {
    kind: 'unknown',
    message: detail || `上传失败（HTTP ${status}）`,
    detail
  }
}
