// Page switch chords (0.46.0, Rosson 2026-10-16): the top bar's tabs in
// order. Wiki and Settings joined in 0.46.4 (Rosson 2026-10-24).
//
//   ⌃⌘1 My Home · ⌃⌘2 Agents · ⌃⌘3 Projects · ⌃⌘4 Tickets
//   ⌃⌘5 Wiki · ⌃⌘6 Settings                                  (macOS)
//   Ctrl+Alt+1–6                                            (Linux / Windows)
//
// Rebindable: they are HOTKEYS entries (`@shared/hotkeys`, ids `goTo*`),
// listed in Settings → Key Bindings → Navigation, and the handler reads the
// effective binding on every keydown.
//
// ONE owner: the window-level digit handler (`useWorkspaceIndexShortcuts`),
// checked before ⌘1–9 / ⌥⌘1–9 / ⌥⌘0, so a page chord never also picks a row.
// No native menu accelerator: a fixed accelerator would keep taking ⌃⌘1–6
// after a rebind. Like ⌘1–9 these fire with focus in a terminal or text
// field (the terminal sends nothing for Control+Command or Control+Alt
// digits). Off macOS, AltGr (which Windows reports as Ctrl+Alt) is ignored,
// so AltGr+digit still types its character (², ³, #, { …).
//
// ⌘P stays as an alias for Projects (App.tsx, the View menu), ⌘, for
// Settings (App.tsx, the app menu).
//
// Wiki and Settings are not plain page switches: ⌃⌘5 is a click on the Wiki
// tab (`openWikiTab`, lib/wiki-open.ts: All of K2, the workspace on screen
// recorded; on the wiki already, it stays as it is), and ⌃⌘6 is ⌘, (the
// same `openSettings()`, no section).
//
// Clearing the bindings that already use digits:
//   ⌘1–9 / ⌘0     rows (Agents, Home, Zen), panes on Projects — no Control
//   ⌥⌘1–9 / ⌥⌘0   Homes on Home, Gardens in Zen, Agents rows — no Control
//   ⌃1–9          launch preset N — no Command / Alt
//
// In Zen, ⌃⌘1–4 switch the nav rail's view instead (`zenSwitchView`); the
// page under Zen stays put. The rail has no Wiki or Settings view, so:
//   - ⌃⌘5 leaves Zen and opens the wiki (a page change leaves Zen, G34, as
//     ⌘P does), also on a Garden without a rail;
//   - ⌃⌘6 opens Settings over Zen, as ⌘, does: Zen stays on, hidden while
//     Settings is open, and shows again when Settings closes.

import { useSettingsStore } from '@/stores/settings'
import { usePageViewStore, type AppPage } from '@/stores/page-view'
import { effectiveKeybinding, formatKeyCombo, getDefaultKeybindings } from '@shared/hotkeys'
import type { ZenRailView } from '@/lib/zen/zen-window'
import type { ZenPageKeys } from '@/lib/zen/zen-theme-switch'
import { exitZen, zenOnNow, zenShownNow, zenSwitchView } from '@/lib/zen/zen-view'
import { openWikiTab } from '@/lib/wiki-open'

export type PageShortcutId = 'goToHome' | 'goToAgents' | 'goToProjects' | 'goToTickets' | 'goToWiki' | 'goToSettings'

/** What a chord opens: a top-bar page, or Settings (the cog). */
export type PageShortcutTarget = AppPage | 'settings'

export interface PageShortcut {
  /** HOTKEYS id. */
  id: PageShortcutId
  target: PageShortcutTarget
  /** The Zen rail view the chord shows while Zen is shown; null when the
   *  rail has none (Wiki, Settings). */
  zenView: ZenRailView | null
}

/** In top-bar order (⌃⌘1 … ⌃⌘6). */
export const PAGE_SHORTCUTS: readonly PageShortcut[] = [
  { id: 'goToHome', target: 'home', zenView: 'home' },
  { id: 'goToAgents', target: 'agents', zenView: 'agents' },
  { id: 'goToProjects', target: 'projects', zenView: 'projects' },
  { id: 'goToTickets', target: 'feedback', zenView: 'tickets' },
  { id: 'goToWiki', target: 'wiki', zenView: null },
  { id: 'goToSettings', target: 'settings', zenView: null },
]

/** The chords that switch the Zen rail's view (⌃⌘1–4). */
const RAIL_SHORTCUTS = PAGE_SHORTCUTS.filter((s) => s.zenView !== null)

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

/** Act on a page chord, as a click on its top-bar tab does (closing
 *  Settings first). In Zen: ⌃⌘1–4 the rail's view; ⌃⌘5 leaves Zen and opens
 *  the wiki; ⌃⌘6 opens Settings over Zen (as ⌘, does). */
export function runPageShortcut(s: PageShortcut): void {
  if (s.target === 'settings') {
    // ⌘, (App.tsx): `openSettings()`, in Zen or not. Already open: leave
    // it on the page it shows, as the cog does.
    if (!useSettingsStore.getState().settingsOpen) useSettingsStore.getState().openSettings()
    return
  }
  if (s.target === 'wiki') {
    // Leave Zen first, even with Settings over it: the wiki may already be
    // the page under Zen, and then no page change would turn Zen off.
    if (zenOnNow()) exitZen()
    openWikiTab()
    return
  }
  if (zenShownNow()) {
    if (s.zenView) zenSwitchView(s.zenView)
    return
  }
  if (useSettingsStore.getState().settingsOpen) useSettingsStore.getState().closeSettings()
  usePageViewStore.getState().setPage(s.target)
}

/** A combo as painted: `⌃⌘1` on macOS, `Ctrl+Alt+1` elsewhere (Meta reads
 *  Super off macOS, as `metaKey` is there). */
export function pageComboLabel(combo: string, isMac: boolean): string {
  if (isMac) return formatKeyCombo(combo, true)
  return combo.replace(/Meta\+/g, 'Super+')
}

/** The chord shown for a tab's tooltip (the effective binding): a page
 *  tab, or `settings` for the cog. */
export function pageShortcutLabel(
  target: PageShortcutTarget,
  keybindings: Record<string, string>,
  isMac: boolean,
): string | null {
  const s = PAGE_SHORTCUTS.find((x) => x.target === target)
  if (!s) return null
  const combo = effectiveKeybinding(keybindings, s.id, isMac)
  return combo ? pageComboLabel(combo, isMac) : null
}

/** The four rail-view chords in one string (the Zen cheat sheet): `⌃⌘1–4`
 *  / `Ctrl+Alt+1–4` while every one is the default, else each in order. */
export function pageShortcutsSummary(keybindings: Record<string, string>, isMac: boolean): string {
  const defaults = getDefaultKeybindings(isMac)
  const combos = RAIL_SHORTCUTS.map((s) => effectiveKeybinding(keybindings, s.id, isMac))
  if (RAIL_SHORTCUTS.every((s, i) => combos[i] === defaults[s.id])) return isMac ? '⌃⌘1–4' : 'Ctrl+Alt+1–4'
  return combos.map((c) => (c ? pageComboLabel(c, isMac) : '—')).join(' / ')
}

/** Every page chord for the Zen cheat sheet: the rail views, Wiki and
 *  Settings, as painted (`—` for a cleared binding). */
export function pageShortcutKeys(keybindings: Record<string, string>, isMac: boolean): ZenPageKeys {
  return {
    views: pageShortcutsSummary(keybindings, isMac),
    wiki: pageShortcutLabel('wiki', keybindings, isMac) ?? '—',
    settings: pageShortcutLabel('settings', keybindings, isMac) ?? '—',
  }
}
