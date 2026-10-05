// prd-zen-mode-v1 Z29, Z30 and prd-zen-gardens-v1 G1–G3, G21, G34 — is
// this window showing Zen, and the gestures that change it.
//
// Zen is a mode of the window (G1). Shown = Zen exists here (desktop, a
// main-layout window), this window's switch is on, and Settings is closed
// (G2). The page doesn't matter: Zen covers whatever page the window is on,
// and that page stays put underneath. Everything that has to behave
// differently under Zen asks `zenShownNow()` (event handlers) or
// `useZenShown()` (React): one selector, never a copy.
//
// Gestures (G3):
//   - enter (the top-bar toggle, the app menu, the escape chord off Zen):
//     close Settings, turn this window's switch on. Shift held → safe mode.
//     Never a page change.
//   - exit (the page's own Zen toggle, the app menu, the chord in Zen, or a
//     page change while Zen is on, G34): turn this window's switch off.
//   - Settings (⌘,) hides Zen without turning it off.
//
// Safe mode (Z29) is per window and never written anywhere: it lasts until
// Try again, a `zen_changed`, or leaving Zen.

import { create } from 'zustand'
import { useSettingsStore } from '@/stores/settings'
import { useZenWindowStore } from './zen-window'
import { zenAvailable } from './zen-platform'
import type { ZenControlKind } from './zen-page'

/** Why this window is in safe mode. */
export type ZenSafeCause =
  | { kind: 'shift' }
  | { kind: 'crash'; message: string }
  | { kind: 'control'; control: ZenControlKind; problem: ZenControlProblem }
  | { kind: 'unreachable'; message: string }
  | { kind: 'unreadable'; message: string }
  | { kind: 'outdated' }

export type ZenControlProblem = 'undeclared' | 'missing' | 'not-wired' | 'invisible'

const CONTROL_NAME: Record<ZenControlKind, string> = {
  'zen-toggle': 'The Zen toggle',
  'garden-switcher': 'The Garden switcher',
  'drag-region': 'The window drag area',
}

const PROBLEM_TEXT: Record<ZenControlProblem, string> = {
  undeclared: 'isn’t declared by the page',
  missing: 'isn’t on the page',
  'not-wired': 'isn’t wired',
  invisible: 'isn’t visible',
}

/** G19: the daemon on this computer has no Gardens routes. */
export const ZEN_OUTDATED_TEXT = 'K2 on this computer is older than this app. Update it to use Gardens.'

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
    case 'outdated':
      return ZEN_OUTDATED_TEXT
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

/** Pure: the shown rule (G2). The page is not part of it. */
export function computeZenShown(input: { available: boolean; on: boolean; settingsOpen: boolean }): boolean {
  return input.available && input.on && !input.settingsOpen
}

/** Event handlers: is this window showing Zen right now? */
export function zenShownNow(): boolean {
  return computeZenShown({
    available: zenAvailable(),
    on: useZenWindowStore.getState().on,
    settingsOpen: useSettingsStore.getState().settingsOpen,
  })
}

/** React: is this window showing Zen? */
export function useZenShown(): boolean {
  const on = useZenWindowStore((s) => s.on)
  const settingsOpen = useSettingsStore((s) => s.settingsOpen)
  return computeZenShown({ available: zenAvailable(), on, settingsOpen })
}

/** Is Zen switched on in this window (shown, or hidden by Settings)? */
export function zenOnNow(): boolean {
  return zenAvailable() && useZenWindowStore.getState().on
}

/** Enter Zen in this window (the top-bar toggle; the chord off Zen; the app
 *  menu). `safe` (Shift held) starts it in safe mode. The page underneath
 *  stays where it is (G2). */
export function enterZen(opts: { safe?: boolean } = {}): void {
  if (!zenAvailable()) return
  useSettingsStore.getState().closeSettings()
  useZenViewStore.setState((s) => ({ safe: opts.safe ? { kind: 'shift' } : null, epoch: s.epoch + 1 }))
  useZenWindowStore.getState().setOn(true)
}

/** Exit Zen: turn this window's switch off. The page you were on shows. */
export function exitZen(): void {
  useZenWindowStore.getState().setOn(false)
  useZenViewStore.setState({ safe: null })
}

/** The escape hatch (menu item, ⌃⌘Z / Ctrl+Alt+Z): in Zen, exit; not in
 *  Zen, enter (Z30, G26). */
export function toggleZenFromEscape(): void {
  if (!zenAvailable()) return
  if (zenShownNow()) exitZen()
  else enterZen()
}

// Z32 / G26 / G52: ⌘1–9 / ⌘0 in Zen select row N of the page's first
// Agents widget, and never switch the window's server. The data verbs
// register the opener; until then the chord does nothing in Zen.
let zenRowSelect: ((index: number) => void) | null = null

/** Plug-in point: what ⌘N does in Zen. Returns the unregister. */
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
