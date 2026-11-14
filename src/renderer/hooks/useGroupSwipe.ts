// Home groups: the two-finger sideways swipe between groups (the logic is
// `lib/group-swipe.ts`). Mounted once per window by the Agents shell, on
// while the page is Home. It listens to wheel events on the window
// (passive: it never blocks a scroll) and skips any event that:
//   - is a pinch (ctrlKey: the trackpad zoom),
//   - something under the pointer already handled (defaultPrevented, e.g.
//     a terminal's own wheel),
//   - starts inside an element that scrolls sideways, or one marked
//     `data-group-swipe="off"`.
// Progress goes to `useGroupSwipeStore` for the group bar's visual; with
// prefers-reduced-motion it stays at rest (the step still happens).

import { useEffect } from 'react'
import { create } from 'zustand'
import { GroupSwipeTracker, SWIPE_QUIET_MS, SWIPE_REST, steppedGroupId, swipeAllowedFrom, type SwipeProgress } from '@/lib/group-swipe'
import { useHomesStore } from '@/stores/homes'

export const useGroupSwipeStore = create<{ swipe: SwipeProgress }>(() => ({ swipe: SWIPE_REST }))

function reducedMotion(): boolean {
  return typeof window.matchMedia === 'function' && window.matchMedia('(prefers-reduced-motion: reduce)').matches
}

function setSwipe(next: SwipeProgress): void {
  const cur = useGroupSwipeStore.getState().swipe
  if (cur.progress !== next.progress || cur.direction !== next.direction) useGroupSwipeStore.setState({ swipe: next })
}

/** Step the selected group by one (no wrap). True when it moved. */
export function stepGroup(step: -1 | 1): boolean {
  const st = useHomesStore.getState()
  const to = steppedGroupId(
    st.homes.map((h) => h.id),
    st.selectedId,
    step,
  )
  if (!to) return false
  st.selectHome(to)
  return true
}

export function useGroupSwipe(enabled: boolean): void {
  useEffect(() => {
    if (!enabled) return
    const tracker = new GroupSwipeTracker()
    let settleTimer: ReturnType<typeof setTimeout> | null = null
    const onWheel = (e: WheelEvent): void => {
      if (e.ctrlKey || e.defaultPrevented) return
      if (!swipeAllowedFrom(e.target)) return
      const now = performance.now()
      const answer = tracker.feed({ deltaX: e.deltaX, deltaY: e.deltaY, deltaMode: e.deltaMode, time: now })
      if (answer.step !== 0) stepGroup(answer.step)
      setSwipe(reducedMotion() ? SWIPE_REST : answer.swipe)
      if (settleTimer) clearTimeout(settleTimer)
      settleTimer = setTimeout(() => setSwipe(tracker.settle(performance.now())), SWIPE_QUIET_MS + 20)
    }
    window.addEventListener('wheel', onWheel, { passive: true })
    return () => {
      window.removeEventListener('wheel', onWheel)
      if (settleTimer) clearTimeout(settleTimer)
      setSwipe(SWIPE_REST)
    }
  }, [enabled])
}
