// prd-zen-user-widgets-v2 UW29, UW30, UW32, UWB9 — which custom widgets
// K2 has stopped in this window, and why.
//
// A stopped widget's frame is removed and K2 draws its card in the box; the
// rest of the Garden keeps running. Reasons:
//   - `not-started`: no `k2.ready()` within 10 s (UW30);
//   - `not-responding`: three missed pings after ready (UW30);
//   - `failing`: 20 uncaught errors in a minute (UW30);
//   - `rate`: ten `rate_limited` answers in a minute (UW29);
//   - `runaway`: the runaway guard tripped (UWB9); the daemon also holds a
//     pause on the grant, which only the owner's Resume clears.
// `pausedStart` is the paused start after a crash with widgets running
// (UW32): the per-window marker is B3's (`k2.zen.widgetsRunning.<label>`);
// while it is set no custom frame mounts, and every custom widget shows
// "They're paused. [Run them]".
//
// This is per-window view state (which frames this window runs), never a
// canonical record: the grant and its pause live in the daemon.

import { create } from 'zustand'

export type ZenWidgetStopReason = 'not-started' | 'not-responding' | 'failing' | 'rate' | 'runaway'

export interface ZenWidgetStop {
  reason: ZenWidgetStopReason
  at: number
}

interface RunState {
  /** `<gardenId>/<placementId>` → why it stopped. */
  stopped: Record<string, ZenWidgetStop>
  /** Bumped by Reload: remounts that frame. */
  generation: Record<string, number>
  /** UW32: K2 restarted while this window's widgets ran. */
  pausedStart: boolean
  /** What "Run them" does (B3 clears its marker); default just unpauses. */
  onRunThem: (() => void) | null
}

export const useZenCustomRunStore = create<RunState>(() => ({
  stopped: {},
  generation: {},
  pausedStart: false,
  onRunThem: null,
}))

export function zenPlacementKey(gardenId: string, placementId: string): string {
  return `${gardenId}/${placementId}`
}

export function stopZenWidget(key: string, reason: ZenWidgetStopReason, now: number = Date.now()): void {
  useZenCustomRunStore.setState((s) => ({ stopped: { ...s.stopped, [key]: { reason, at: now } } }))
}

/** Reload (corner menu, or a card's button): clears the stop and remounts. */
export function reloadZenWidget(key: string): void {
  useZenCustomRunStore.setState((s) => {
    const stopped = { ...s.stopped }
    delete stopped[key]
    return { stopped, generation: { ...s.generation, [key]: (s.generation[key] ?? 0) + 1 } }
  })
}

/** B3 (UW32): hold every custom frame in this window until "Run them". */
export function setZenWidgetsPausedStart(paused: boolean, onRunThem: (() => void) | null = null): void {
  useZenCustomRunStore.setState({ pausedStart: paused, onRunThem })
}

/** "Run them": unpause this window's widgets. */
export function runZenWidgetsNow(): void {
  const { onRunThem } = useZenCustomRunStore.getState()
  useZenCustomRunStore.setState({ pausedStart: false, onRunThem: null })
  onRunThem?.()
}

/** K2's card text for a stop. */
export function zenWidgetStopText(reason: ZenWidgetStopReason): string {
  switch (reason) {
    case 'not-started':
      return 'This widget didn’t start.'
    case 'not-responding':
      return 'This widget stopped responding.'
    case 'failing':
      return 'This widget keeps failing.'
    case 'rate':
      return 'This widget kept going over its limits, so K2 stopped it.'
    case 'runaway':
      return 'This widget sent a lot of messages very quickly, so K2 stopped it and turned its sending off.'
  }
}

export const ZEN_WIDGETS_PAUSED_TEXT = 'K2 restarted while this Garden’s widgets were running. They’re paused.'

/** Tests only. */
export function __resetZenCustomRunForTests(): void {
  useZenCustomRunStore.setState({ stopped: {}, generation: {}, pausedStart: false, onRunThem: null })
}
