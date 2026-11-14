// Home groups: the two-finger sideways swipe between groups (the logic is
// `lib/group-swipe.ts`). Mounted once per window by the Agents shell, on
// while the page is Home. It listens to wheel events on the window
// (passive: it never blocks a scroll) and takes only those that start in
// the Home nav (an element marked `data-group-swipe-zone`: the sidebar and
// the collapsed rail; K2 Design's mock: "two fingers sideways over the
// nav", the main pane stays). It skips any event that:
//   - is a pinch (ctrlKey: the trackpad zoom),
//   - something under the pointer already handled (defaultPrevented),
//   - starts inside an element that scrolls sideways (the group bar when
//     it overflows scrolls itself), or one marked `data-group-swipe="off"`.
//
// `useGroupSwipeStore` carries the live progress (0–1 and next/prev, null
// at rest) and the last release (commit or cancel, with a sequence number)
// for the bar's visual. A commit past the last group is a cancel (no
// wrap). There is no reduced-motion variant: the swipe always slides,
// with the peek, whatever prefers-reduced-motion says.

import { useEffect } from 'react'
import { create } from 'zustand'
import {
  GroupSwipeTracker,
  SWIPE_QUIET_MS,
  steppedGroupId,
  swipeAllowedFrom,
  type SwipeAnswer,
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

function setSwipe(next: SwipeProgress): void {
  const cur = useGroupSwipeStore.getState().swipe
  if (cur?.progress === next?.progress && cur?.direction === next?.direction) return
  useGroupSwipeStore.setState({ swipe: next })
}

/** Move the selected group one step (no wrap). True when it moved. */
export function stepGroup(direction: SwipeDirection): boolean {
  const st = useHomesStore.getState()
  const to = steppedGroupId(
    st.homes.map((h) => h.id),
    st.selectedId,
    direction,
  )
  if (!to) return false
  st.selectHome(to)
  return true
}

function act(answer: SwipeAnswer): void {
  if (answer.release) {
    const r = answer.release
    const moved = r.outcome === 'commit' && stepGroup(r.direction)
    const seq = (useGroupSwipeStore.getState().release?.seq ?? 0) + 1
    useGroupSwipeStore.setState({ release: { outcome: moved ? 'commit' : 'cancel', direction: r.direction, seq } })
  }
  setSwipe(answer.swipe)
}

export function useGroupSwipe(enabled: boolean): void {
  useEffect(() => {
    if (!enabled) return
    const tracker = new GroupSwipeTracker()
    let settleTimer: ReturnType<typeof setTimeout> | null = null
    const onWheel = (e: WheelEvent): void => {
      if (e.ctrlKey || e.defaultPrevented) return
      const inZone = e.target instanceof Element && e.target.closest('[data-group-swipe-zone]') !== null
      if (!inZone || !swipeAllowedFrom(e.target)) return
      act(tracker.feed({ deltaX: e.deltaX, deltaY: e.deltaY, deltaMode: e.deltaMode, time: performance.now() }))
      if (settleTimer) clearTimeout(settleTimer)
      settleTimer = setTimeout(() => act(tracker.settle(performance.now())), SWIPE_QUIET_MS + 20)
    }
    window.addEventListener('wheel', onWheel, { passive: true })
    return () => {
      window.removeEventListener('wheel', onWheel)
      if (settleTimer) clearTimeout(settleTimer)
      setSwipe(null)
    }
  }, [enabled])
}
