// Home groups: the two-finger sideways swipe between groups (the gesture
// logic is `lib/group-swipe.ts`). Mounted once per window by the Agents
// shell, on while the page is Home. It listens to wheel events on the
// window (passive: it never blocks a scroll) and takes only those that
// start in the Home nav (an element marked `data-group-swipe-zone`: the
// sidebar and the collapsed rail; the main pane stays). It skips any event
// that:
//   - is a pinch (ctrlKey: the trackpad zoom),
//   - something under the pointer already handled (defaultPrevented),
//   - starts inside an element that scrolls sideways (the group bar when
//     it overflows scrolls itself), or one marked `data-group-swipe="off"`.
//
// Two outputs:
//   - `groupSwipeLive`, outside React: a frame per wheel event for the
//     strip (`GroupSwipeStrip`), which moves it by transform in a rAF.
//     Track (follow the fingers), glide (from, to, ms, curve), rest.
//   - `useGroupSwipeStore`, for everything else: `swipe` (progress 0–1,
//     direction, and `settling` while it glides; refreshed every 5% of
//     travel and whenever it crosses SWIPE_COMMIT_AT, not per event) and
//     the last `release` (commit or cancel, with a sequence number).
// A commit glides the strip all the way to the next group first; the
// group switches when the glide lands, so the bar's accent moves then.
// There is no reduced-motion variant: the swipe always slides (Rosson).

import { useEffect } from 'react'
import { create } from 'zustand'
import {
  GroupSwipeTracker,
  SWIPE_COMMIT_AT,
  SWIPE_DISTANCE_PX,
  SWIPE_MIN_DISTANCE_PX,
  SWIPE_TUNING,
  glideFor,
  steppedGroupId,
  swipeAllowedFrom,
  type SwipeAnswer,
  type SwipeDecision,
  type SwipeDirection,
  type SwipeProgress,
  type SwipeRelease,
} from '@/lib/group-swipe'
import { useHomesStore } from '@/stores/homes'

export interface GroupSwipeState {
  swipe: SwipeProgress
  /** The last swipe's end; `seq` grows by one per release. */
  release: (SwipeRelease & { seq: number }) | null
}

export const useGroupSwipeStore = create<GroupSwipeState>(() => ({ swipe: null, release: null }))

/** One frame of the strip's motion. `offset`, `from` and `to` are signed
 *  shares of the strip's width: +1 is the next group fully in view. */
export type SwipeFrame =
  | { kind: 'rest'; landed: boolean }
  | { kind: 'track'; offset: number }
  | { kind: 'glide'; from: number; to: number; ms: number; easing: string; outcome: SwipeRelease['outcome'] }

let frame: SwipeFrame = { kind: 'rest', landed: false }
const watchers = new Set<(f: SwipeFrame) => void>()

/** The strip's motion, outside React (no render per wheel event). */
export const groupSwipeLive = {
  get(): SwipeFrame {
    return frame
  },
  subscribe(fn: (f: SwipeFrame) => void): () => void {
    watchers.add(fn)
    return () => {
      watchers.delete(fn)
    }
  },
}

function emit(next: SwipeFrame): void {
  frame = next
  for (const fn of watchers) fn(next)
}

/** The glide's timer waits this long past its duration, so the CSS
 *  transition has painted its last frame before the group switches. */
export const GLIDE_LAND_SLACK_MS = 16

/** The store's progress steps (it is refreshed once per step, not per event). */
const PROGRESS_STEPS = 20

function publishTrack(offset: number): void {
  const cur = useGroupSwipeStore.getState().swipe
  const progress = Math.min(1, Math.abs(offset))
  const direction: SwipeDirection = offset > 0 ? 'next' : offset < 0 ? 'prev' : (cur?.direction ?? 'next')
  if (
    cur &&
    !cur.settling &&
    cur.direction === direction &&
    Math.floor(cur.progress * PROGRESS_STEPS) === Math.floor(progress * PROGRESS_STEPS) &&
    cur.progress >= SWIPE_COMMIT_AT === progress >= SWIPE_COMMIT_AT
  ) {
    return
  }
  useGroupSwipeStore.setState({ swipe: { progress, direction } })
}

/** The group one step `direction` from the selected one (no wrap), or null. */
function stepTarget(direction: SwipeDirection): string | null {
  const st = useHomesStore.getState()
  return steppedGroupId(
    st.homes.map((h) => h.id),
    st.selectedId,
    direction,
  )
}

/** Move the selected group one step (no wrap). True when it moved. */
export function stepGroup(direction: SwipeDirection): boolean {
  const to = stepTarget(direction)
  if (!to) return false
  useHomesStore.getState().selectHome(to)
  return true
}

/** The swipe's width for a gesture that starts in `zone`: its stage's
 *  width (`data-group-swipe-stage`), at least SWIPE_MIN_DISTANCE_PX. */
function swipeDistanceOf(zone: Element | null): number {
  const stage = zone?.querySelector('[data-group-swipe-stage]') ?? null
  const w = stage instanceof HTMLElement ? stage.clientWidth : 0
  return w > 0 ? Math.max(w, SWIPE_MIN_DISTANCE_PX) : SWIPE_DISTANCE_PX
}

export function useGroupSwipe(enabled: boolean): void {
  useEffect(() => {
    if (!enabled) return
    let zone: Element | null = null
    let settleTimer: ReturnType<typeof setTimeout> | null = null
    let glideTimer: ReturnType<typeof setTimeout> | null = null
    let gliding: SwipeRelease | null = null

    // The glide is over: switch on a commit, then the strip rests.
    const land = (): void => {
      if (glideTimer) clearTimeout(glideTimer)
      glideTimer = null
      const g = gliding
      gliding = null
      if (!g) return
      if (g.outcome === 'commit') stepGroup(g.direction)
      useGroupSwipeStore.setState({ swipe: null })
      emit({ kind: 'rest', landed: g.outcome === 'commit' })
    }

    const glide = (d: SwipeDecision): void => {
      land()
      const outcome = d.outcome === 'commit' && stepTarget(d.direction) ? 'commit' : 'cancel'
      const g = glideFor({ ...d, outcome })
      const seq = (useGroupSwipeStore.getState().release?.seq ?? 0) + 1
      useGroupSwipeStore.setState({
        release: { outcome, direction: d.direction, seq },
        swipe: { progress: outcome === 'commit' ? 1 : 0, direction: d.direction, settling: outcome },
      })
      gliding = { outcome, direction: d.direction }
      emit({ kind: 'glide', from: d.from, to: g.to, ms: g.ms, easing: g.easing, outcome })
      if (g.ms === 0) land()
      else glideTimer = setTimeout(land, g.ms + GLIDE_LAND_SLACK_MS)
    }

    const handle = (a: SwipeAnswer): void => {
      if (a.release) glide(a.release)
      if (a.offset !== null) {
        if (gliding) land()
        emit({ kind: 'track', offset: a.offset })
        publishTrack(a.offset)
      }
    }

    const tracker = new GroupSwipeTracker({
      distance: () => swipeDistanceOf(zone),
      canGo: (d) => stepTarget(d) !== null,
      // A new gesture during a glide: land it first, so this one starts
      // from the group it was heading to.
      onStart: land,
    })

    const onWheel = (e: WheelEvent): void => {
      if (e.ctrlKey || e.defaultPrevented) return
      const inZone = e.target instanceof Element ? e.target.closest('[data-group-swipe-zone]') : null
      if (!inZone || !swipeAllowedFrom(e.target)) return
      zone = inZone
      handle(tracker.feed({ deltaX: e.deltaX, deltaY: e.deltaY, deltaMode: e.deltaMode, time: performance.now() }))
      if (settleTimer) clearTimeout(settleTimer)
      settleTimer = setTimeout(() => handle(tracker.settle(performance.now())), SWIPE_TUNING.idleMs + 4)
    }
    window.addEventListener('wheel', onWheel, { passive: true })
    return () => {
      window.removeEventListener('wheel', onWheel)
      if (settleTimer) clearTimeout(settleTimer)
      land()
      useGroupSwipeStore.setState({ swipe: null })
      if (frame.kind !== 'rest') emit({ kind: 'rest', landed: false })
    }
  }, [enabled])
}
