/** One ellipsis character (U+2026), never three dots. */
export const ELLIPSIS = '\u2026'

/** Cap on stem characters kept in front of the extension. Not a floor. */
const TAIL_CAP = 12

/**
 * Tree-row name slot.
 * `clientWidthPx` is the panel (or scroller) clientWidth, which includes
 * the scroller's `px-1` padding. Depth 0 at 180px is 128px; depth 1 is 112px.
 */
export function treeNameSlotWidth(clientWidthPx: number, depth: number): number {
  const paddingX = 8 // px-1
  const indent = depth * 16 + 8
  const chevron = 12 // w-3
  const gap = 4 // gap-1, twice
  const icon = 16 // w-4
  const contentWidth = clientWidthPx - paddingX
  return Math.max(0, contentWidth - indent - chevron - gap - icon - gap)
}

/**
 * Workspace-resource basename slot.
 * `clientWidthPx` is the panel clientWidth. The list is `px-1`, the row is
 * `px-2` with a `w-4` icon and `gap-1.5`. The missing badge is subtracted
 * only when that row is missing — callers measure the badge string once.
 */
export function resourceNameSlotWidth(
  clientWidthPx: number,
  opts?: { missing?: boolean; badgeWidthPx?: number },
): number {
  const outerPaddingX = 8 // px-1
  const buttonPaddingX = 16 // px-2
  const icon = 16 // w-4
  const gap = 6 // gap-1.5
  let slot = clientWidthPx - outerPaddingX - buttonPaddingX - icon - gap
  if (opts?.missing) {
    const badge = opts.badgeWidthPx
    slot -= gap
    if (typeof badge === 'number' && Number.isFinite(badge) && badge > 0) slot -= badge
  }
  return Math.max(0, slot)
}

function extensionOf(name: string, directory: boolean): string {
  if (directory) return ''
  const dot = name.lastIndexOf('.')
  // No dot, or the only dot is leading (`.env`, `.gitignore`).
  if (dot <= 0) return ''
  return name.slice(dot)
}

function fits(text: string, maxWidthPx: number, measure: (text: string) => number): boolean {
  return measure(text) <= maxWidthPx
}

/**
 * Shorten `name` so it fits in `maxWidthPx`, keeping the end visible.
 * `measure` is monotonic in string length for a monospace glyph width.
 * Returns `name` unchanged when it already fits — no ellipsis.
 */
export function middleEllipsis(
  name: string,
  maxWidthPx: number,
  measure: (text: string) => number,
  opts?: { directory?: boolean },
): string {
  if (!Number.isFinite(maxWidthPx)) return name
  if (fits(name, maxWidthPx, measure)) return name

  const ext = extensionOf(name, opts?.directory === true)
  const stem = ext ? name.slice(0, name.length - ext.length) : name
  const stemKeep = Math.min(TAIL_CAP, stem.length)
  const preferredTail = stem.slice(stem.length - stemKeep) + ext

  // Extra width goes to the head. The 12-character stem keep is a cap.
  // Head + tail must omit at least one character or we would rather show
  // the whole name (already rejected above when it fits).
  if (preferredTail.length < name.length && fits(ELLIPSIS + preferredTail, maxWidthPx, measure)) {
    const maxHead = name.length - preferredTail.length - 1
    let headLen = 0
    for (let n = maxHead; n >= 0; n--) {
      if (fits(name.slice(0, n) + ELLIPSIS + preferredTail, maxWidthPx, measure)) {
        headLen = n
        break
      }
    }
    if (headLen + preferredTail.length >= name.length) return name
    return name.slice(0, headLen) + ELLIPSIS + preferredTail
  }

  return shrinkToFit(name, ext, maxWidthPx, measure)
}

function shrinkToFit(
  name: string,
  ext: string,
  maxWidthPx: number,
  measure: (text: string) => number,
): string {
  if (ext && ext.length < name.length && fits(ELLIPSIS + ext, maxWidthPx, measure)) {
    const stem = name.slice(0, name.length - ext.length)
    const cap = Math.min(TAIL_CAP, stem.length)
    for (let k = cap; k >= 0; k--) {
      const tail = stem.slice(stem.length - k) + ext
      if (tail.length >= name.length) continue
      if (fits(ELLIPSIS + tail, maxWidthPx, measure)) return ELLIPSIS + tail
    }
  }

  // Extension itself does not fit (or there is none): show the end of the
  // full name, even if that cuts the extension.
  for (let k = name.length - 1; k >= 1; k--) {
    const suffix = name.slice(name.length - k)
    if (fits(ELLIPSIS + suffix, maxWidthPx, measure)) return ELLIPSIS + suffix
  }
  for (let k = name.length; k >= 1; k--) {
    const suffix = name.slice(name.length - k)
    if (fits(suffix, maxWidthPx, measure)) return suffix
  }
  return ''
}
