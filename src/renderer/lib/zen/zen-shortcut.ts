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
