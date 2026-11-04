// What a board file's sealed frame needs from a tab that isn't a Garden (prd-board-tab-v1 BT-18, BT-22).

export const BOARD_FILE_WIDGET = 'k2:board-file@1'

/** Link sites and download consent are keyed by a Garden id on the local daemon; a tab uses this reserved one. It fits
 *  `valid_garden_id`, and no Garden can hold it (Garden ids are minted as `g-…`). */
export const boardFileConsentKey = 'board-file'

/** The tab's own hello: `k2.can` reads these. `board-file-v1` isn't in `ZEN_FRAME_FEATURES`, so these verbs answer
 *  `verb_unavailable` in a Garden frame. */
export const BOARD_FILE_FEATURES: readonly string[] = ['zen-v1', 'zen-widgets-v1', 'garden-blobs-v1', 'zen-files-v1', 'board-file-v1']

/** `--zen-*` lives on a Zen root, and a tab has none: map the app's tokens onto them. */
const ZEN_FROM_APP: Readonly<Record<string, string>> = {
  '--zen-canvas': '--color-bg-canvas',
  '--zen-surface': '--color-bg-surface',
  '--zen-surface-raised': '--color-bg-elevated',
  '--zen-text': '--color-text-primary',
  '--zen-text-muted': '--color-text-muted',
  '--zen-border': '--color-border',
  '--zen-accent': '--color-accent',
  '--zen-accent-text': '--color-on-accent',
  '--zen-danger': '--color-danger',
}

function luminance(css: string): number | null {
  const m = css.match(/\d+(\.\d+)?/g)
  if (css.startsWith('#')) {
    const h = css.slice(1)
    const full = h.length === 3 ? [...h].map((c) => c + c).join('') : h
    const n = parseInt(full.slice(0, 6), 16)
    if (Number.isNaN(n)) return null
    return (0.2126 * ((n >> 16) & 255) + 0.7152 * ((n >> 8) & 255) + 0.0722 * (n & 255)) / 255
  }
  if (!m || m.length < 3) return null
  const [r, g, b] = m.slice(0, 3).map(Number)
  return (0.2126 * r + 0.7152 * g + 0.0722 * b) / 255
}

export function boardFileTheme(el: HTMLElement | null): { vars: Record<string, string>; scheme: 'light' | 'dark'; reduced: boolean } {
  const source = el ?? document.documentElement
  const cs = getComputedStyle(source)
  const vars: Record<string, string> = {}
  for (const [zen, app] of Object.entries(ZEN_FROM_APP)) {
    const v = cs.getPropertyValue(app).trim()
    if (v) vars[zen] = v
  }
  const font = cs.getPropertyValue('font-family').trim()
  if (font) vars['--zen-font'] = font
  const lum = luminance(vars['--zen-canvas'] ?? vars['--zen-surface'] ?? '')
  const scheme = lum !== null && lum < 0.5 ? 'dark' : 'light'
  const reduced = typeof matchMedia === 'function' && matchMedia('(prefers-reduced-motion: reduce)').matches
  return { vars, scheme, reduced }
}

export interface BoardFileKey {
  key: string
  code: string
  meta: boolean
  ctrl: boolean
  shift: boolean
}

const OWN_KEY = /^(Digit[1-9]|KeyT|KeyW|KeyN|BracketLeft|BracketRight|Tab)$/

/** K2's own tab keys don't cross a sealed frame, so the frame hands them back and K2's handlers hear them here. Only
 *  these chords, so a frame can't press anything else in K2's window. */
export function redispatchBoardFileKey(k: BoardFileKey): boolean {
  if (!OWN_KEY.test(k.code) || !(k.meta || k.ctrl)) return false
  window.dispatchEvent(
    new KeyboardEvent('keydown', { key: k.key, code: k.code, metaKey: k.meta, ctrlKey: k.ctrl, shiftKey: k.shift, bubbles: true, cancelable: true }),
  )
  return true
}
