// ── 文本工具：全角 → 半角 ────────────────────────────────

/**
 * 常用中文标点 → 等价 ASCII 标点（不在全角 ASCII 区 U+FF01–U+FF5E 里的那几个）。
 *
 * DOI 里最容易误输入的就是这些：中文输入法下打 `。` `、` `《` `》` `“”` `‘’` `—`。
 */
const CJK_PUNCT_MAP: Record<string, string> = {
  '\u3000': ' ', // 全角空格
  '\u3002': '.', // 。
  '\u3001': ',', // 、
  '\u300a': '<', // 《
  '\u300b': '>', // 》
  '\u300c': '"',
  '\u300d': '"',
  '\u300e': '"',
  '\u300f': '"',
  '\u201c': '"', // “
  '\u201d': '"', // ”
  '\u2018': "'", // ‘
  '\u2019': "'", // ’
  '\u2014': '-', // —
  '\u2013': '-', // –
  '\u2026': '...' // …
}

/**
 * 把全角字符转成半角（用于 DOI 这类只接受 ASCII 的输入框）。
 *
 * - 全角 ASCII（`U+FF01`–`U+FF5E`，如 `． ／ ： （ ） Ａ`）→ 对应半角（偏移 `0xFEE0`）
 * - 全角空格 `U+3000` 与常见中文标点见 `CJK_PUNCT_MAP`
 * - 其他字符原样保留
 */
export function toHalfWidth(raw: string | null | undefined): string {
  return String(raw ?? '')
    .replace(/[\uFF01-\uFF5E]/g, (ch) => String.fromCharCode(ch.charCodeAt(0) - 0xfee0))
    .replace(/[\u3000\u3001\u3002\u300a\u300b\u300c\u300d\u300e\u300f\u201c\u201d\u2018\u2019\u2014\u2013\u2026]/g, (ch) => CJK_PUNCT_MAP[ch] ?? ch)
}
