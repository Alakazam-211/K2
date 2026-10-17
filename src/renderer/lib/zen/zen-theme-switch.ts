// Omarchy additions 2 and 4 (prd-zen-mode-v1, Rosson 2026-10-04) — cycling
// Zen themes and the Zen shortcut cheat sheet.
//
// The daemon owns the active theme (`k2 zen theme next|prev|set|list`); the
// renderer shows which theme is active (from `/cli/zen/get`'s `theme.name`
// and `themes`) and asks for a change on THIS computer's daemon:
//   POST /cli/zen/theme/set  {name}      the picker
//   POST /cli/zen/theme/next {}          ⌃⌘.
//   POST /cli/zen/theme/prev {}          ⌃⌘⇧.
// each with `garden` added when the Garden has its own pick (`theme.scope =
// "garden"`, prd-zen-gardens-v1 G16, G30), so the switch changes what this
// Garden shows. The daemon owns the order and the wrap. Then the page is
// re-read (`zen_changed` follows too).
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
import { daemonCliGet, daemonCliPost } from '@/lib/daemon-cli'
import type { DesktopOs } from '@/lib/desktop-chrome'
import { zenLocalScope } from './zen-api'
import { ZEN_CHORD_LABEL } from './zen-shortcut'
import { ZenPageParseError, parseZenThemeEntries, type ZenThemeEntry, type ZenThemeScope } from './zen-page'

export { ZEN_SHORTCUTS_MENU_EVENT, dispatchZenShortcutsMenu } from './zen-shortcut'

/** K2's own transient Zen overlays: the cheat sheet and the theme picker. */
export const useZenOverlayStore = create<{ sheet: boolean; picker: boolean }>(() => ({ sheet: false, picker: false }))

export function openZenCheatSheet(): void {
  useZenOverlayStore.setState({ sheet: true, picker: false })
}
export function closeZenOverlays(): void {
  useZenOverlayStore.setState({ sheet: false, picker: false })
}

/** Where a switch applies: the Garden itself only when it has its own pick. */
function scopeBody(scope: ZenThemeScope, gardenId: string): { garden?: string } {
  return scope === 'garden' ? { garden: gardenId } : {}
}

/** `POST /cli/zen/theme/set {name, garden?}` on this computer's daemon. */
export async function setZenTheme(name: string, scope: ZenThemeScope, gardenId: string): Promise<void> {
  await daemonCliPost(zenLocalScope(), 'zen/theme/set', { name, ...scopeBody(scope, gardenId) })
}

/** One Garden's themes (`GET /cli/zen/theme/list?garden=`): the list in
 *  the daemon's order and the theme the Garden shows now. */
export interface ZenGardenThemes {
  active: string | null
  themes: ZenThemeEntry[]
}

/** Parse a `GET /cli/zen/theme/list` body. Throws `ZenPageParseError` when
 *  there is no theme list in it. */
export function parseZenGardenThemes(raw: unknown): ZenGardenThemes {
  if (raw === null || typeof raw !== 'object' || Array.isArray(raw)) throw new ZenPageParseError('no themes in the answer')
  const body = raw as Record<string, unknown>
  if (!Array.isArray(body.themes)) throw new ZenPageParseError('no themes in the answer')
  const active = typeof body.active === 'string' && body.active ? body.active : null
  return { active, themes: parseZenThemeEntries(body.themes) }
}

/** `GET /cli/zen/theme/list?garden=<id>` on this computer's daemon (the
 *  Garden settings' Theme row). */
export async function loadZenGardenThemes(gardenId: string): Promise<ZenGardenThemes> {
  return parseZenGardenThemes(await daemonCliGet<unknown>(zenLocalScope(), 'zen/theme/list', { garden: gardenId }))
}

/** `POST /cli/zen/theme/next|prev {garden?}` on this computer's daemon. */
export async function cycleZenTheme(dir: 1 | -1, scope: ZenThemeScope, gardenId: string): Promise<void> {
  await daemonCliPost(zenLocalScope(), dir === 1 ? 'zen/theme/next' : 'zen/theme/prev', scopeBody(scope, gardenId))
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

/** Every Zen shortcut, for the cheat sheet (Omarchy 4). `pageKeys`: the
 *  page chords as painted (`pageShortcutsSummary`, lib/page-shortcuts.ts;
 *  the defaults when omitted). */
export function zenShortcutGroups(os: DesktopOs, pageKeys?: string): ZenShortcutGroup[] {
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
      title: 'Agents and Gardens',
      rows: [
        { keys: `${cmd}1–9`, what: 'Open conversation 1–9' },
        { keys: `${alt}1–9`, what: 'Switch to Garden 1–9' },
        // 0.46.0: the page chords switch the nav rail's view (a Garden with a rail).
        { keys: pageKeys ?? `${chord}1–4`, what: 'My Home / Agents / Projects / Tickets view' },
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
        { keys: `${cmd}P`, what: 'Projects (leaves Zen)' },
        { keys: `${cmd}J`, what: 'Running Agents' },
        { keys: `${cmd}K`, what: 'Command Palette' },
        { keys: mac ? '⌘⇧N' : 'Ctrl+Shift+N', what: 'New window' },
      ],
    },
  ]
}
