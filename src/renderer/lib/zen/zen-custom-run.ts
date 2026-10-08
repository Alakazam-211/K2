// prd-zen-user-widgets-v2 UW29, UW30, UW32, UWB9 — which custom widgets
// K2 has stopped in this window, and why.
//
// A stopped widget's frame is removed and K2 draws its card in the box; the
// rest of the Garden keeps running. Reasons:
//   - `not-started`: no `k2.ready()` within 10 s (UW30);
//   - `not-responding`: three missed pings after ready (UW30);
//   - `failing`: 20 uncaught errors in a minute (UW30);
//   - `rate`: ten `rate_limited` answers in a minute (UW29).
// The runaway guard (R6) never stops a frame: it pauses posting only. This
// window marks the pause at once (`pauseZenWidgetPosting`); the daemon holds
// it for every window (`paused` on the widget) until the person's Resume.
// The paused start after a freeze (UW32) is `zen-widgets-running.ts`
// (B3): a page with custom widgets waits for the window's boot decision
// (`zenPausedStartReady`), marks itself running while they're mounted, and
// while `zenPausedStart()` names its Garden no custom frame mounts: every
// custom widget shows "They're paused. [Run them]".
//
// This is per-window view state (which frames this window runs), never a
// canonical record: the runaway pause lives in the daemon.

import { useEffect, useState, useSyncExternalStore } from 'react'
import { create } from 'zustand'
import {
  clearZenWidgetsRunning,
  markZenWidgetsRunning,
  resumeZenPausedStart,
  subscribeZenPausedStart,
  zenPausedStart,
  zenPausedStartReady,
} from './zen-widgets-running'

export type ZenWidgetStopReason = 'not-started' | 'not-responding' | 'failing' | 'rate'

export interface ZenWidgetStop {
  reason: ZenWidgetStopReason
  at: number
}

interface RunState {
  /** `<gardenId>/<placementId>` → why it stopped. */
  stopped: Record<string, ZenWidgetStop>
  /** Bumped by Reload: remounts that frame. */
  generation: Record<string, number>
  /** `<gardenId>/<placementId>` → the runaway guard paused its posting in
   *  this window (until Resume; the daemon's pause covers other windows). */
  postingPaused: Record<string, true>
}

export const useZenCustomRunStore = create<RunState>(() => ({
  stopped: {},
  generation: {},
  postingPaused: {},
}))

/** The runaway guard tripped in this window: posting pauses at once. */
export function pauseZenWidgetPosting(key: string): void {
  useZenCustomRunStore.setState((s) => ({ postingPaused: { ...s.postingPaused, [key]: true } }))
}

/** Resume: posting may go again in this window. */
export function resumeZenWidgetPosting(key: string): void {
  useZenCustomRunStore.setState((s) => {
    const postingPaused = { ...s.postingPaused }
    delete postingPaused[key]
    return { postingPaused }
  })
}

export function isZenWidgetPostingPaused(key: string): boolean {
  return useZenCustomRunStore.getState().postingPaused[key] === true
}

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

/** Whether a Garden's custom frames may mount in this window (UW32):
 *  `waiting` until the boot decision, `paused` while it names this Garden. */
export type ZenWidgetsGate = 'waiting' | 'paused' | 'run'

let bootDecided = false
void zenPausedStartReady().then(() => {
  bootDecided = true
})

export function useZenWidgetsGate(gardenId: string): ZenWidgetsGate {
  const [ready, setReady] = useState(bootDecided)
  useEffect(() => {
    if (ready) return
    let live = true
    void zenPausedStartReady().then(() => {
      if (live) setReady(true)
    })
    return () => {
      live = false
    }
  }, [ready])
  const paused = useSyncExternalStore(subscribeZenPausedStart, zenPausedStart)
  if (!ready) return 'waiting'
  return paused && paused.garden === gardenId ? 'paused' : 'run'
}

/** The page: mark this window's Garden as running custom widgets while
 *  they're mounted (B3's freeze memory), clear it when they unmount. */
export function useZenWidgetsRunningMarker(gardenId: string, hasCustom: boolean): void {
  const gate = useZenWidgetsGate(gardenId)
  useEffect(() => {
    if (!hasCustom || !gardenId || gate !== 'run') return
    markZenWidgetsRunning(gardenId)
    return () => clearZenWidgetsRunning()
  }, [gardenId, hasCustom, gate])
}

/** "Run them". */
export function runZenWidgetsNow(): void {
  resumeZenPausedStart()
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
  }
}

/** The small inline notice while the runaway guard has posting paused. */
export const ZEN_WIDGET_PAUSED_POSTING_TEXT = 'Paused: too many posts.'

export const ZEN_WIDGETS_PAUSED_TEXT = 'K2 restarted while this Garden’s widgets were running. They’re paused.'

/** Tests only. */
export function __resetZenCustomRunForTests(): void {
  useZenCustomRunStore.setState({ stopped: {}, generation: {}, postingPaused: {} })
}
