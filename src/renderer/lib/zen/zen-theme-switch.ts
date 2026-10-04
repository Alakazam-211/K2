// Omarchy additions 2 and 4 (prd-zen-mode-v1, Rosson 2026-10-04) — cycling
// Zen themes and the Zen shortcut cheat sheet.
//
// The daemon owns the active theme (`k2 zen theme next|prev|set|list`); the
// renderer shows which theme is active (from `/cli/zen/get`'s `theme.name`
// and `themes`) and asks for a change with
//   POST /cli/zen/theme/set {name}      on THIS computer's daemon
// then re-reads the page (the daemon's `zen_changed` follows). Next / prev
// are computed here from the daemon's list order, so they need only `set`.
//
// Keys (checked against every window chord: ⌘1–9 / ⌘0, ⌘⌥1–9, ⌃1–9,
// ⌃⌘Z, ⌘L, ⌘⇧L, ⌘⇧N, ⌘⇧Z, ⌘, ⌘K ⌘J ⌘P ⌘B ⌘⇧F, ⌘[ ⌘], ⌘= ⌘- ⌘0, and the
// ⌘T/W/D/N/O/F/K/← → tab keys — several of those handlers ignore Control,
// so the new chords avoid all of their keys):
//   next theme      ⌃⌘.    Ctrl+Alt+.        (e.code Period)
//   previous theme  ⌃⌘⇧.   Ctrl+Alt+Shift+.
//   cheat sheet     ?      (not while typing)  ⌃⌘/  Ctrl+Alt+/ (always)
// macOS reserves none of these; GNOME and KDE bind none by default. On
// Linux / Windows a keydown with AltGr held is ignored (AltGr+. and AltGr+/
// type characters on some layouts). One owner each: a capture-phase window
// listener installed while Zen is shown, with no native accelerator (the
// macOS "Zen Shortcuts" menu item has none). The ⌃⌘Z / Ctrl+Alt+Z escape
// hatch stays where it is (`zen-shortcut.ts`).

import { create } from 'zustand'
import { daemonCliPost } from '@/lib/daemon-cli'
import type { DesktopOs } from '@/lib/desktop-chrome'
import { zenLocalScope } from './zen-api'
import { ZEN_CHORD_LABEL } from './zen-shortcut'
import type { ZenThemeEntry } from './zen-page'

export { ZEN_SHORTCUTS_MENU_EVENT, dispatchZenShortcutsMenu } from './zen-shortcut'

/** K2's own transient Zen overlays: the cheat sheet and the theme picker. */
export const useZenOverlayStore = create<{ sheet: boolean; picker: boolean }>(() => ({ sheet: false, picker: false }))

export function openZenCheatSheet(): void {
  useZenOverlayStore.setState({ sheet: true, picker: false })
}
export function closeZenOverlays(): void {
  useZenOverlayStore.setState({ sheet: false, picker: false })
}

/** `POST /cli/zen/theme/set {name}` on this computer's daemon. */
export async function setZenTheme(name: string): Promise<void> {
  await daemonCliPost(zenLocalScope(), 'zen/theme/set', { name })
}

/** The theme `dir` steps from `active` in the daemon's list (wraps). */
export function cycleTarget(themes: readonly ZenThemeEntry[], active: string | null, dir: 1 | -1): string | null {
  if (themes.length === 0) return null
  const i = active ? themes.findIndex((t) => t.name === active) : -1
  if (i < 0) return themes[dir === 1 ? 0 : themes.length - 1].name
  if (themes.length === 1) return null
  return themes[(i + dir + themes.length) % themes.length].name
}

export interface ZenKeyLike {
  code: string
  key: string
  ctrlKey: boolean
  altKey: boolean
  metaKey: boolean
  shiftKey: boolean
  getModifierState?(key: string): boolean
}

function chordMods(e: ZenKeyLike, os: DesktopOs): boolean {
  if (os === 'mac') return e.ctrlKey && e.metaKey && !e.altKey
  if (!e.ctrlKey || !e.altKey || e.metaKey) return false
  return !(typeof e.getModifierState === 'function' && e.getModifierState('AltGraph'))
}

/** ⌃⌘. / ⌃⌘⇧. (Ctrl+Alt+. / Ctrl+Alt+Shift+.): +1, −1, or null. */
export function zenThemeCycleDir(e: ZenKeyLike, os: DesktopOs): 1 | -1 | null {
  if (e.code !== 'Period' || !chordMods(e, os)) return null
  return e.shiftKey ? -1 : 1
}

function isEditable(target: EventTarget | null): boolean {
  const el = target as HTMLElement | null
  if (!el || typeof el.closest !== 'function') return false
  return el.closest('input, textarea, select, [contenteditable=""], [contenteditable="true"]') !== null
}

/** `?` outside a text field, or ⌃⌘/ (Ctrl+Alt+/) anywhere. */
export function isZenCheatSheetKey(e: ZenKeyLike & { target?: EventTarget | null }, os: DesktopOs): boolean {
  if (e.code === 'Slash' && !e.shiftKey && chordMods(e, os)) return true
  return e.key === '?' && !e.ctrlKey && !e.metaKey && !e.altKey && !isEditable(e.target ?? null)
}

/** Install the theme-cycle and cheat-sheet keys (capture phase). */
export function installZenThemeKeys(
  os: DesktopOs,
  target: Pick<Window, 'addEventListener' | 'removeEventListener'>,
  on: { cycle(dir: 1 | -1): void; sheet(): void },
): () => void {
  const handler = (e: KeyboardEvent): void => {
    const dir = zenThemeCycleDir(e, os)
    if (dir !== null) {
      e.preventDefault()
      e.stopPropagation()
      if (!e.repeat) on.cycle(dir)
      return
    }
    if (isZenCheatSheetKey(e, os)) {
      e.preventDefault()
      e.stopPropagation()
      on.sheet()
    }
  }
  target.addEventListener('keydown', handler, true)
  return () => target.removeEventListener('keydown', handler, true)
}

export interface ZenShortcutRow {
  keys: string
  what: string
}
export interface ZenShortcutGroup {
  title: string
  rows: ZenShortcutRow[]
}

/** Every Zen shortcut, for the cheat sheet (Omarchy 4). */
export function zenShortcutGroups(os: DesktopOs): ZenShortcutGroup[] {
  const mac = os === 'mac'
  // The window's own chords are `metaKey` on every platform and paint as ⌘
  // glyphs everywhere (lib/home-shortcuts.ts); only the Zen chords differ.
  const cmd = '⌘'
  const alt = '⌥⌘'
  const chord = mac ? '⌃⌘' : 'Ctrl+Alt+'
  const shiftChord = mac ? '⌃⌘⇧' : 'Ctrl+Alt+Shift+'
  return [
    {
      title: 'Zen',
      rows: [
        { keys: mac ? ZEN_CHORD_LABEL.mac : ZEN_CHORD_LABEL.other, what: 'Exit Zen Mode (always works)' },
        { keys: `${chord}.`, what: 'Next theme' },
        { keys: `${shiftChord}.`, what: 'Previous theme' },
        { keys: `?  or  ${chord}/`, what: 'Show these shortcuts' },
        { keys: 'Esc', what: 'Close this sheet or the theme picker' },
      ],
    },
    {
      title: 'Agents and Homes',
      rows: [
        { keys: `${cmd}1–9`, what: 'Open conversation 1–9' },
        { keys: `${alt}1–9`, what: 'Switch to Home 1–9' },
        { keys: `${cmd}↑ / ${cmd}↓`, what: 'Previous / next conversation' },
      ],
    },
    {
      title: 'Messages',
      rows: [
        { keys: 'Enter', what: 'Send' },
        { keys: 'Shift+Enter', what: 'New line' },
        { keys: 'Esc', what: 'Clear the message box' },
      ],
    },
    {
      title: 'The rest of K2',
      rows: [
        { keys: `${cmd},`, what: 'Settings (Zen stays on)' },
        { keys: `${cmd}P`, what: 'Projects' },
        { keys: `${cmd}J`, what: 'Running Agents' },
        { keys: `${cmd}K`, what: 'Command Palette' },
        { keys: mac ? '⌘⇧N' : 'Ctrl+Shift+N', what: 'New window' },
      ],
    },
  ]
}
