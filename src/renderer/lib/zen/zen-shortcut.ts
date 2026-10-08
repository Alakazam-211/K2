// prd-zen-mode-v1 Z30, Z53, Z55, Z63 (Rosson 2026-10-04 answer 1) — the
// escape hatch that always works.
//
// Chord: ⌃⌘Z on macOS, Ctrl+Alt+Z on Linux and Windows. Checked against the
// other window chords: ⌘1–9 / ⌘0 (rows), ⌘⌥1–9 (Homes), ⌃1–9 (presets), ⌘L,
// ⌘⇧L, ⌘⇧N, ⌘⇧Z (Redo) — none uses Z with Control+Command or Control+Alt.
//
// ONE owner per platform (the ⌘L lesson: two owners double-toggle):
//   - macOS: the native View-menu item "Enter/Exit Zen Mode" with the
//     accelerator Ctrl+Cmd+Z (src-tauri/src/menu.rs). It emits
//     `menu:zen-toggle` to the focused window only. There is NO webview
//     keydown handler for the chord on macOS.
//   - Linux / Windows: a window-level keydown listener in the CAPTURE phase,
//     installed outside the Zen root, so no widget can swallow it. AltGr
//     (`getModifierState('AltGraph')`) is ignored, so AltGr+Z still types ż
//     on Polish layouts. Plus the app-menu item (`AppMenuPanel`), which
//     dispatches the same `menu:zen-toggle` name in this window.

import type { DesktopOs } from '@/lib/desktop-chrome'

/** The one event name every platform's menu path uses. */
export const ZEN_MENU_EVENT = 'menu:zen-toggle'

/** Display strings (the cheat sheet; not rebindable). */
export const ZEN_CHORD_LABEL: Record<'mac' | 'other', string> = {
  mac: '⌃⌘Z',
  other: 'Ctrl+Alt+Z',
}

/** The chord in `keyEventToCombo` form, per platform. */
export const ZEN_CHORD_COMBO: Record<'mac' | 'other', string> = {
  mac: 'Ctrl+Meta+Z',
  other: 'Ctrl+Alt+Z',
}

export interface ZenKeyEventLike {
  code: string
  ctrlKey: boolean
  altKey: boolean
  metaKey: boolean
  shiftKey: boolean
  getModifierState?(key: string): boolean
}

/** Linux / Windows: Ctrl+Alt+Z, never with AltGr, Meta or Shift. */
export function isZenChordNonMac(e: ZenKeyEventLike): boolean {
  if (e.code !== 'KeyZ') return false
  if (!e.ctrlKey || !e.altKey || e.metaKey || e.shiftKey) return false
  if (typeof e.getModifierState === 'function' && e.getModifierState('AltGraph')) return false
  return true
}

/**
 * Install the Linux / Windows chord owner (capture phase on `target`).
 * Returns the uninstall, or null on macOS (the native accelerator is the
 * only owner there) and on any non-desktop OS.
 */
export function installZenChordListener(
  os: DesktopOs,
  target: Pick<Window, 'addEventListener' | 'removeEventListener'>,
  onChord: () => void,
): (() => void) | null {
  if (os !== 'linux' && os !== 'windows') return null
  const handler = (e: KeyboardEvent): void => {
    if (e.repeat) return
    if (!isZenChordNonMac(e)) return
    e.preventDefault()
    e.stopPropagation()
    onChord()
  }
  target.addEventListener('keydown', handler, true)
  return () => target.removeEventListener('keydown', handler, true)
}

/** The Linux / Windows app menu's item: same event name, this window only
 *  (a DOM event, so no other window toggles). */
export function dispatchZenMenuToggle(): void {
  if (typeof window === 'undefined') return
  window.dispatchEvent(new Event(ZEN_MENU_EVENT))
}

/** Omarchy addition 4: the macOS View-menu "Zen Shortcuts" item and the
 *  Linux / Windows app-menu item open the Zen cheat sheet. */
export const ZEN_SHORTCUTS_MENU_EVENT = 'menu:zen-shortcuts'

/** The Linux / Windows app menu's "Zen Shortcuts": this window only. */
export function dispatchZenShortcutsMenu(): void {
  if (typeof window === 'undefined') return
  window.dispatchEvent(new Event(ZEN_SHORTCUTS_MENU_EVENT))
}

// ── Chords forwarded out of a sealed widget frame (prd-zen-user-widgets-v2
// UW33, UW58) ─────────────────────────────────────────────────────────────
//
// Keys typed in a focused widget frame never reach this window's listeners:
// not the Linux / Windows Ctrl+Alt+Z capture listener above, not the Garden
// keys (⌥⌘1–9, `lib/home-shortcuts.ts`), not the theme keys (⌃⌘. / ⌃⌘⇧.,
// Ctrl+Alt+. on Linux, `zen-theme-switch.ts`). K2's frame runtime (the shim,
// `sdk/k2-runtime.js`) watches keydown inside the frame and forwards a fixed
// set as `{chord: <name>}` over the widget's port. It must forward only
// trusted events (`e.isTrusted`), so a widget can't fake a key press by
// dispatching a synthetic event. The host acts on a forwarded chord only
// while that frame has focus, and at most once every 500 ms. On macOS ⌃⌘Z
// is a native menu accelerator and works whatever has focus, so the shim
// forwards no exit chord there. The Zen toggle stays clickable outside
// every frame.

/** Every chord a widget frame may forward (UW33). */
export const ZEN_FORWARDED_CHORDS = [
  'zen-exit',
  'garden-1',
  'garden-2',
  'garden-3',
  'garden-4',
  'garden-5',
  'garden-6',
  'garden-7',
  'garden-8',
  'garden-9',
  'theme-next',
  'theme-prev',
] as const

export type ZenForwardedChord = (typeof ZEN_FORWARDED_CHORDS)[number]

export function isZenForwardedChord(x: unknown): x is ZenForwardedChord {
  return typeof x === 'string' && (ZEN_FORWARDED_CHORDS as readonly string[]).includes(x)
}

/** A keydown as the shim sees it. */
export interface ZenFrameKeyLike extends ZenKeyEventLike {
  isTrusted?: boolean
  repeat?: boolean
}

/**
 * The chord a keydown inside a widget frame stands for, or null: the
 * reference the shim mirrors (the runtime keeps its own copy in plain JS;
 * a test should check the two agree). Untrusted and repeated events are
 * never forwarded.
 */
export function zenForwardedChordForKey(e: ZenFrameKeyLike, os: DesktopOs): ZenForwardedChord | null {
  if (e.isTrusted === false || e.repeat) return null
  const altGr = typeof e.getModifierState === 'function' && e.getModifierState('AltGraph')
  // Exit Zen: Ctrl+Alt+Z off macOS (the native accelerator covers macOS).
  if (os !== 'mac' && isZenChordNonMac(e)) return 'zen-exit'
  // Garden 1–9: ⌥⌘1–9 on every platform (HOME_SWITCH_BINDING).
  const digit = /^Digit([1-9])$/.exec(e.code)
  if (digit && e.metaKey && e.altKey && !e.ctrlKey && !e.shiftKey) {
    return `garden-${digit[1]}` as ZenForwardedChord
  }
  // Theme next / previous: ⌃⌘. / ⌃⌘⇧. on macOS, Ctrl+Alt+(Shift+). elsewhere.
  if (e.code === 'Period') {
    const mods = os === 'mac' ? e.ctrlKey && e.metaKey && !e.altKey : e.ctrlKey && e.altKey && !e.metaKey && !altGr
    if (mods) return e.shiftKey ? 'theme-prev' : 'theme-next'
  }
  return null
}

/** What a forwarded chord does, supplied by the page that hosts the frames. */
export interface ZenChordActions {
  exitZen(): void
  /** 1–9: the Nth Garden. */
  switchGarden(n: number): void
  cycleTheme(dir: 1 | -1): void
}

/** At most one forwarded chord per this many ms (UW33). */
export const ZEN_FORWARDED_CHORD_GAP_MS = 500

export interface ZenChordGate {
  /** The chord when it may run now (a known chord, from a frame that has
   *  focus, not within the gap of the last accepted one), else null. */
  accept(chord: unknown, frameFocused: boolean, now: number): ZenForwardedChord | null
  /** `accept`, then act. Returns whether it ran. */
  run(chord: unknown, frameFocused: boolean, now: number, actions: ZenChordActions): boolean
}

/** The host's gate for forwarded chords: one per page, so all of its
 *  widget frames share the 500 ms budget. */
export function createZenChordGate(gapMs: number = ZEN_FORWARDED_CHORD_GAP_MS): ZenChordGate {
  let last = Number.NEGATIVE_INFINITY
  const accept = (chord: unknown, frameFocused: boolean, now: number): ZenForwardedChord | null => {
    if (!frameFocused || !isZenForwardedChord(chord)) return null
    if (now - last < gapMs) return null
    last = now
    return chord
  }
  return {
    accept,
    run(chord, frameFocused, now, actions) {
      const c = accept(chord, frameFocused, now)
      if (!c) return false
      if (c === 'zen-exit') actions.exitZen()
      else if (c === 'theme-next') actions.cycleTheme(1)
      else if (c === 'theme-prev') actions.cycleTheme(-1)
      else actions.switchGarden(Number(c.slice('garden-'.length)))
      return true
    },
  }
}

/** Whether `frame` is the focused element of its document (the host's
 *  focus test for a forwarded chord). */
export function zenFrameHasFocus(frame: Element | null, doc: Pick<Document, 'activeElement'> = document): boolean {
  return frame !== null && doc.activeElement === frame
}
