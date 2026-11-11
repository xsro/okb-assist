// ── 语言码工具（与后端 `services::i18n::normalize_lang_key` 语义保持一致） ──

/** 常见语言码 → 中文名（未收录的语言直接显示大写语言码） */
export const LANG_LABELS: Record<string, string> = {
  en: '英文',
  zh: '中文',
  ja: '日文',
  ko: '韩文',
  fr: '法文',
  de: '德文',
  ru: '俄文',
  es: '西班牙文',
  pt: '葡萄牙文',
  it: '意大利文',
  ar: '阿拉伯文'
}

/** 语言码 → 展示名（`en-US` → 英文；未收录 → `EN`） */
export function langLabel(lang: string): string {
  const key = normalizeLangKey(lang) || lang.trim().toLowerCase()
  return LANG_LABELS[key] || key.toUpperCase()
}

/**
 * 归一化语言码：去空白、转小写、去区域后缀（`en-US` / `zh_CN` → `en` / `zh`）。
 * 只接受 2–3 个 ASCII 字母（ISO 639-1 / 常见 639-2），非法输入返回空串。
 */
export function normalizeLangKey(raw: string | null | undefined): string {
  const key = String(raw ?? '').trim().toLowerCase()
  if (!key) return ''
  const primary = key.split(/[-_]/)[0]?.trim() ?? ''
  if (primary.length < 2 || primary.length > 3 || !/^[a-z]+$/.test(primary)) return ''
  return primary
}
