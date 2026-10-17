// Page switch chords (0.46.0, Rosson 2026-10-16): the top bar's page tabs
// in order.
//
//   ⌃⌘1 My Home · ⌃⌘2 Agents · ⌃⌘3 Projects · ⌃⌘4 Tickets   (macOS)
//   Ctrl+Alt+1–4                                            (Linux / Windows)
//
// Rebindable: they are HOTKEYS entries (`@shared/hotkeys`, ids `goTo*`),
// listed in Settings → Key Bindings → Navigation, and the handler reads the
// effective binding on every keydown.
//
// ONE owner: the window-level digit handler (`useWorkspaceIndexShortcuts`),
// checked before ⌘1–9 / ⌥⌘1–9 / ⌥⌘0, so a page chord never also picks a row.
// No native menu accelerator: a fixed accelerator would keep taking ⌃⌘1–4
// after a rebind. Like ⌘1–9 these fire with focus in a terminal or text
// field (the terminal sends nothing for Control+Command or Control+Alt
// digits). Off macOS, AltGr (which Windows reports as Ctrl+Alt) is ignored,
// so AltGr+digit still types its character (², ³, #, { …).
//
// ⌘P stays as an alias for Projects (App.tsx, the View menu).
//
// Clearing the bindings that already use digits:
//   ⌘1–9 / ⌘0     rows (Agents, Home, Zen), panes on Projects — no Control
//   ⌥⌘1–9 / ⌥⌘0   Homes on Home, Gardens in Zen, Agents rows — no Control
//   ⌃1–9          launch preset N — no Command / Alt
//
// In Zen they switch the nav rail's view instead (`zenSwitchView`); the page
// under Zen stays put.

import { useSettingsStore } from '@/stores/settings'
import { usePageViewStore, type AppPage } from '@/stores/page-view'
import { effectiveKeybinding, formatKeyCombo, getDefaultKeybindings } from '@shared/hotkeys'
import type { ZenRailView } from '@/lib/zen/zen-window'
import { zenShownNow, zenSwitchView } from '@/lib/zen/zen-view'

export interface PageShortcut {
  /** HOTKEYS id. */
  id: 'goToHome' | 'goToAgents' | 'goToProjects' | 'goToTickets'
  page: Exclude<AppPage, 'wiki'>
  /** The Zen rail view the chord shows while Zen is shown. */
  zenView: ZenRailView
}

/** In top-bar order (⌃⌘1 … ⌃⌘4). */
export const PAGE_SHORTCUTS: readonly PageShortcut[] = [
  { id: 'goToHome', page: 'home', zenView: 'home' },
  { id: 'goToAgents', page: 'agents', zenView: 'agents' },
  { id: 'goToProjects', page: 'projects', zenView: 'projects' },
  { id: 'goToTickets', page: 'feedback', zenView: 'tickets' },
]

export interface PageKeyEventLike {
  key: string
  code: string
  metaKey: boolean
  ctrlKey: boolean
  altKey: boolean
  shiftKey: boolean
  getModifierState?(key: string): boolean
}

/** Does a keydown match `combo` (`@shared/hotkeys` form, e.g.
 *  `Ctrl+Meta+1`)? Modifiers must match exactly. A digit key is read from
 *  `e.code` (Option and AltGr change `e.key`; AZERTY's unshifted digit row
 *  types `&é"'`). */
export function keyEventMatchesCombo(e: PageKeyEventLike, combo: string): boolean {
  if (!combo) return false
  const parts = combo.split('+')
  const key = parts[parts.length - 1]
  const mods = new Set(parts.slice(0, -1))
  if (
    e.ctrlKey !== mods.has('Ctrl') ||
    e.metaKey !== mods.has('Meta') ||
    e.altKey !== mods.has('Alt') ||
    e.shiftKey !== mods.has('Shift')
  ) {
    return false
  }
  if (/^[0-9]$/.test(key)) return e.code === `Digit${key}`
  return (e.key.length === 1 ? e.key.toUpperCase() : e.key) === key
}

/** The page shortcut a keydown stands for, or null. */
export function pageShortcutForEvent(
  e: PageKeyEventLike,
  keybindings: Record<string, string>,
  isMac: boolean,
): PageShortcut | null {
  // AltGr types characters; Windows reports it as Ctrl+Alt.
  if (!isMac && e.ctrlKey && e.altKey && typeof e.getModifierState === 'function' && e.getModifierState('AltGraph')) {
    return null
  }
  for (const s of PAGE_SHORTCUTS) {
    if (keyEventMatchesCombo(e, effectiveKeybinding(keybindings, s.id, isMac))) return s
  }
  return null
}

/** Act on a page chord: in Zen, the rail's view; otherwise the page, as a
 *  click on its top-bar tab does (closing Settings first). */
export function runPageShortcut(s: PageShortcut): void {
  if (zenShownNow()) {
    zenSwitchView(s.zenView)
    return
  }
  if (useSettingsStore.getState().settingsOpen) useSettingsStore.getState().closeSettings()
  usePageViewStore.getState().setPage(s.page)
}

/** A combo as painted: `⌃⌘1` on macOS, `Ctrl+Alt+1` elsewhere (Meta reads
 *  Super off macOS, as `metaKey` is there). */
export function pageComboLabel(combo: string, isMac: boolean): string {
  if (isMac) return formatKeyCombo(combo, true)
  return combo.replace(/Meta\+/g, 'Super+')
}

/** The chord shown for a page's tab tooltip (the effective binding). */
export function pageShortcutLabel(page: AppPage, keybindings: Record<string, string>, isMac: boolean): string | null {
  const s = PAGE_SHORTCUTS.find((x) => x.page === page)
  if (!s) return null
  const combo = effectiveKeybinding(keybindings, s.id, isMac)
  return combo ? pageComboLabel(combo, isMac) : null
}

/** All four chords in one string (the Zen cheat sheet): `⌃⌘1–4` /
 *  `Ctrl+Alt+1–4` while every one is the default, else each in order. */
export function pageShortcutsSummary(keybindings: Record<string, string>, isMac: boolean): string {
  const defaults = getDefaultKeybindings(isMac)
  const combos = PAGE_SHORTCUTS.map((s) => effectiveKeybinding(keybindings, s.id, isMac))
  if (PAGE_SHORTCUTS.every((s, i) => combos[i] === defaults[s.id])) return isMac ? '⌃⌘1–4' : 'Ctrl+Alt+1–4'
  return combos.map((c) => (c ? pageComboLabel(c, isMac) : '—')).join(' / ')
}
