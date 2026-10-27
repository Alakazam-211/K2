// prd-widget-size-and-shared-libs-v1 WS12–WS14 (0.46.5): the owner's
// "Large widget" line. The daemon sends each custom widget's code size with
// the size K2 warns above (512 KB) and its limit (1 MB); the line shows in
// the widget's ⋯ menu (the Zen menu for a full-canvas widget) and next to
// its name in Settings → Gardens, never on the canvas (Rosson: Fleet Ops 2
// looks exactly as it does). Thin client: the numbers are the daemon's.

import type { ZenWidgetSize } from './zen-custom-types'

function isObj(v: unknown): v is Record<string, unknown> {
  return v !== null && typeof v === 'object' && !Array.isArray(v)
}

function count(v: unknown): number | null {
  return typeof v === 'number' && Number.isFinite(v) && v >= 0 ? v : null
}

/** The daemon's `size` (`{code, codeWarnAt, codeMax}`), or null when it
 *  sent none or something this client can't read (then no line shows). */
export function parseZenWidgetSize(raw: unknown): ZenWidgetSize | null {
  if (!isObj(raw)) return null
  const code = count(raw.code)
  const codeWarnAt = count(raw.codeWarnAt)
  const codeMax = count(raw.codeMax)
  if (code === null || codeWarnAt === null || codeMax === null) return null
  return { code, codeWarnAt, codeMax }
}

/** Strictly above the warning size (WS2: exactly 512 KB is not large). */
export function zenWidgetIsLarge(size: ZenWidgetSize | null | undefined): boolean {
  return !!size && size.code > size.codeWarnAt
}

/** Bytes in K2's limit words (1 KB = 1,024 bytes, as the limits are):
 *  "740 KB", "1 MB", "3.5 MB". The daemon's `bundle::human_bytes`. */
export function zenSizeText(n: number): string {
  const KB = 1024
  const MB = 1024 * 1024
  if (n >= MB) {
    const tenths = Math.round((n * 10) / MB)
    return tenths % 10 === 0 ? `${tenths / 10} MB` : `${Math.floor(tenths / 10)}.${tenths % 10} MB`
  }
  if (n >= KB) return `${Math.round(n / KB)} KB`
  return `${n} bytes`
}

/** WS13's one quiet line, or null when the widget isn't large. */
export function zenLargeWidgetText(size: ZenWidgetSize | null | undefined): string | null {
  if (!size || !zenWidgetIsLarge(size)) return null
  return `Large widget: ${zenSizeText(size.code)} of code (limit ${zenSizeText(size.codeMax)}). Ask your agent about splitting it or making it an App.`
}
