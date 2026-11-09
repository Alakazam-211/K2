// "Thread text width" (Settings → Workspaces / Agents → Workspace settings):
// how wide the text column may get in Thread view (prd-thread-view-focus-v1
// §5). Per-install localStorage, not daemon settings: appearance follows the
// person at the keyboard, and the daemon's settings.json is shared by every
// user of a server. Same home and shape as "New agents open in"
// (`sessionViewTab.ts`).

export const LS_THREAD_TEXT_WIDTH = 'k2.threadTextWidth'

export const THREAD_TEXT_WIDTH_MIN_PX = 480
/** Bubbles stop growing near here: `min(78%, 46rem)` reaches 46rem at a
 *  column of about 944px, so a wider limit only pushes the two sides apart. */
export const THREAD_TEXT_WIDTH_MAX_PX = 960
export const THREAD_TEXT_WIDTH_STEP_PX = 20
export const THREAD_TEXT_WIDTH_DEFAULT_PX = 720

export interface ThreadTextWidthPref {
  /** False is "Full width": no limit. `px` is kept for when it comes back on. */
  limited: boolean
  px: number
}

export const THREAD_TEXT_WIDTH_DEFAULT: ThreadTextWidthPref = {
  limited: true,
  px: THREAD_TEXT_WIDTH_DEFAULT_PX,
}

/** Coerce to a legal width: on the slider's steps, inside its range.
 *  Anything that isn't a finite number is the default. */
export function clampThreadTextWidthPx(raw: unknown): number {
  if (typeof raw !== 'number' || !Number.isFinite(raw)) return THREAD_TEXT_WIDTH_DEFAULT_PX
  const steps = Math.round((raw - THREAD_TEXT_WIDTH_MIN_PX) / THREAD_TEXT_WIDTH_STEP_PX)
  const snapped = THREAD_TEXT_WIDTH_MIN_PX + steps * THREAD_TEXT_WIDTH_STEP_PX
  return Math.min(THREAD_TEXT_WIDTH_MAX_PX, Math.max(THREAD_TEXT_WIDTH_MIN_PX, snapped))
}

/** Coerce anything to a legal pref. */
export function normalizeThreadTextWidth(raw: unknown): ThreadTextWidthPref {
  if (!raw || typeof raw !== 'object') return THREAD_TEXT_WIDTH_DEFAULT
  const obj = raw as { limited?: unknown; px?: unknown }
  return { limited: obj.limited !== false, px: clampThreadTextWidthPx(obj.px) }
}

export function parseThreadTextWidth(raw: string | null | undefined): ThreadTextWidthPref {
  if (!raw) return THREAD_TEXT_WIDTH_DEFAULT
  try {
    return normalizeThreadTextWidth(JSON.parse(raw))
  } catch {
    return THREAD_TEXT_WIDTH_DEFAULT
  }
}

export function readThreadTextWidth(): ThreadTextWidthPref {
  if (typeof localStorage === 'undefined') return THREAD_TEXT_WIDTH_DEFAULT
  try {
    return parseThreadTextWidth(localStorage.getItem(LS_THREAD_TEXT_WIDTH))
  } catch {
    return THREAD_TEXT_WIDTH_DEFAULT
  }
}

export function writeThreadTextWidth(pref: ThreadTextWidthPref): void {
  if (typeof localStorage === 'undefined') return
  try {
    localStorage.setItem(LS_THREAD_TEXT_WIDTH, JSON.stringify(normalizeThreadTextWidth(pref)))
  } catch {
    /* quota / private mode */
  }
}

/** The column's limit in px, or null for Full width. */
export function threadTextCapPx(pref: ThreadTextWidthPref | null | undefined): number | null {
  const safe = normalizeThreadTextWidth(pref)
  return safe.limited ? safe.px : null
}

/** Reset shows only away from the default. */
export function isThreadTextWidthDefault(pref: ThreadTextWidthPref): boolean {
  return pref.limited === THREAD_TEXT_WIDTH_DEFAULT.limited && pref.px === THREAD_TEXT_WIDTH_DEFAULT.px
}
