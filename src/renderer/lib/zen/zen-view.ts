// prd-zen-mode-v1 Z4, Z5, Z29, Z30 (Rosson 2026-10-04 answers 1–3) — is
// this window showing Zen, and the gestures that change it.
//
// Shown = Zen exists here (desktop, a main-layout window), the page is Home,
// Settings is closed, and the window's selected Home has Zen on. Everything
// that has to behave differently under Zen asks `zenShownNow()` (event
// handlers) or `useZenShown()` (React): one selector, never a copy.
//
// Gestures:
//   - enter (the toggle row on regular Home, or the escape chord off Zen):
//     go to Home, turn the selected Home's Zen on. Shift held → safe mode.
//   - exit (the page's own Zen toggle, the app menu, the chord): turn that
//     Home's Zen OFF.
//   - step away (Settings, ⌘P, the page's Home switcher picking another
//     Home): Zen stays on for the Home; coming back shows Zen again.
//
// Safe mode (Z29) is per window and never written anywhere: it lasts until
// Try again, a `zen_changed`, or leaving Zen.

import { create } from 'zustand'
import { useHomesStore, selectedHome } from '@/stores/homes'
import { usePageViewStore } from '@/stores/page-view'
import { useSettingsStore } from '@/stores/settings'
import { useZenHomesStore } from './zen-homes'
import { zenAvailable } from './zen-platform'
import type { ZenControlKind } from './zen-page'

/** Why this window is in safe mode. */
export type ZenSafeCause =
  | { kind: 'shift' }
  | { kind: 'crash'; message: string }
  | { kind: 'control'; control: ZenControlKind; problem: ZenControlProblem }
  | { kind: 'unreachable'; message: string }
  | { kind: 'unreadable'; message: string }

export type ZenControlProblem = 'undeclared' | 'missing' | 'not-wired' | 'invisible'

const CONTROL_NAME: Record<ZenControlKind, string> = {
  'zen-toggle': 'The Zen toggle',
  'home-switcher': 'The Home switcher',
  'drag-region': 'The window drag area',
}

const PROBLEM_TEXT: Record<ZenControlProblem, string> = {
  undeclared: 'isn’t declared by the page',
  missing: 'isn’t on the page',
  'not-wired': 'isn’t wired',
  invisible: 'isn’t visible',
}

/** The banner's cause line (Z29). */
export function zenSafeCauseText(cause: ZenSafeCause): string {
  switch (cause.kind) {
    case 'shift':
      return 'You held Shift while turning Zen on.'
    case 'crash':
      return `The page crashed: ${cause.message}`
    case 'control':
      return `${CONTROL_NAME[cause.control]} ${PROBLEM_TEXT[cause.problem]}.`
    case 'unreachable':
      return 'Can’t reach K2 on this computer.'
    case 'unreadable':
      return `K2 on this computer sent a Zen page this app can’t read: ${cause.message}`
  }
}

export const ZEN_SAFE_TITLE = 'Zen is in safe mode. Your files are untouched.'

interface ZenViewState {
  /** Non-null while this window is in safe mode. */
  safe: ZenSafeCause | null
  /** Bumped by enter / Try again / `zen_changed`: the page remounts and
   *  re-reads its config. */
  epoch: number
  enterSafeMode(cause: ZenSafeCause): void
  clearSafeMode(): void
}

export const useZenViewStore = create<ZenViewState>((set, get) => ({
  safe: null,
  epoch: 0,
  enterSafeMode(cause) {
    if (get().safe) return
    console.warn('[zen] safe mode:', zenSafeCauseText(cause))
    set({ safe: cause })
  },
  clearSafeMode() {
    set((s) => ({ safe: null, epoch: s.epoch + 1 }))
  },
}))

/** Pure: the shown rule (Z5). */
export function computeZenShown(input: {
  available: boolean
  page: string
  settingsOpen: boolean
  homeOn: boolean
}): boolean {
  return input.available && input.page === 'home' && !input.settingsOpen && input.homeOn
}

/** Is Zen on for the window's selected Home? */
function selectedHomeOn(): boolean {
  const home = selectedHome(useHomesStore.getState())
  return useZenHomesStore.getState().on[home.id] === true
}

/** Event handlers: is this window showing Zen right now? */
export function zenShownNow(): boolean {
  return computeZenShown({
    available: zenAvailable(),
    page: usePageViewStore.getState().page,
    settingsOpen: useSettingsStore.getState().settingsOpen,
    homeOn: selectedHomeOn(),
  })
}

/** React: is this window showing Zen? */
export function useZenShown(): boolean {
  const page = usePageViewStore((s) => s.page)
  const settingsOpen = useSettingsStore((s) => s.settingsOpen)
  const homeId = useHomesStore((s) => selectedHome(s).id)
  const homeOn = useZenHomesStore((s) => s.on[homeId] === true)
  return computeZenShown({ available: zenAvailable(), page, settingsOpen, homeOn })
}

/** React: is Zen on for the selected Home (whether or not it is shown)? */
export function useSelectedHomeZenOn(): boolean {
  const homeId = useHomesStore((s) => selectedHome(s).id)
  return useZenHomesStore((s) => s.on[homeId] === true)
}

/** Enter Zen for the window's selected Home (the toggle row; the chord off
 *  Zen). `safe` (Shift held) starts it in safe mode. */
export function enterZen(opts: { safe?: boolean } = {}): void {
  if (!zenAvailable()) return
  const home = selectedHome(useHomesStore.getState())
  useSettingsStore.getState().closeSettings()
  usePageViewStore.getState().setPage('home')
  useZenViewStore.setState((s) => ({ safe: opts.safe ? { kind: 'shift' } : null, epoch: s.epoch + 1 }))
  useZenHomesStore.getState().setOn(home.id, true)
}

/** Exit Zen: turn the selected Home's Zen OFF (Z5, answer 3). */
export function exitZen(): void {
  const home = selectedHome(useHomesStore.getState())
  useZenHomesStore.getState().setOn(home.id, false)
  useZenViewStore.setState({ safe: null })
}

/** The escape hatch (menu item, ⌃⌘Z / Ctrl+Alt+Z): in Zen, exit; not in
 *  Zen, go to Home and turn Zen on for the selected Home (Z30). */
export function toggleZenFromEscape(): void {
  if (!zenAvailable()) return
  if (zenShownNow()) exitZen()
  else enterZen()
}

// Z32/Z54: ⌘1–9 / ⌘0 in Zen select conversation N (the Home's row N) and
// never switch the window's server. The Agents widget (S6) registers the
// in-place opener; until then the chord does nothing in Zen.
let zenRowSelect: ((index: number) => void) | null = null

/** S6 plug-in point: what ⌘N does in Zen. Returns the unregister. */
export function registerZenRowSelect(fn: (index: number) => void): () => void {
  zenRowSelect = fn
  return () => {
    if (zenRowSelect === fn) zenRowSelect = null
  }
}

/** ⌘1–9 / ⌘0 while Zen is shown (`index` is 0-based). */
export function zenSelectRow(index: number): void {
  zenRowSelect?.(index)
}
