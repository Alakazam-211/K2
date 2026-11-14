// Home groups: the group bar's colours follow the swipe. While a swipe is
// under way the current group's icon blends from the accent to muted and
// the group it heads to from muted to the accent, in step with the strip
// (home-groups.css mixes the two tokens in oklab by
// `--group-swipe-progress`). On release the blend glides on the strip's
// own curve and duration: forward on a commit, back on a cancel, so the
// landing changes nothing you can see.
//
// The value is set here on the bar's root from `groupSwipeLive`, in a rAF,
// never by React (no render per wheel event). `@property` in
// home-groups.css registers it as a number so the glide can transition it.

import { useLayoutEffect } from 'react'
import { groupSwipeLive, type SwipeFrame } from '@/hooks/useGroupSwipe'

export const GROUP_SWIPE_PROGRESS_VAR = '--group-swipe-progress'

/** Set the bar's blend for frame `f`. */
export function paintGroupInk(el: HTMLElement, f: SwipeFrame): void {
  if (f.kind === 'rest') {
    el.style.transition = ''
    el.style.removeProperty(GROUP_SWIPE_PROGRESS_VAR)
    return
  }
  const at = (share: number): string => String(Math.min(1, Math.abs(share)))
  el.style.transition = 'none'
  if (f.kind === 'track') {
    el.style.setProperty(GROUP_SWIPE_PROGRESS_VAR, at(f.offset))
    return
  }
  // The glide: from where the fingers left it, on the strip's curve.
  el.style.setProperty(GROUP_SWIPE_PROGRESS_VAR, at(f.from))
  void getComputedStyle(el).getPropertyValue(GROUP_SWIPE_PROGRESS_VAR)
  el.style.transition = `${GROUP_SWIPE_PROGRESS_VAR} ${f.ms}ms ${f.easing}`
  el.style.setProperty(GROUP_SWIPE_PROGRESS_VAR, f.outcome === 'commit' ? '1' : '0')
}

/** Keep the bar at `ref` painted from the live swipe (off when `live` is false). */
export function useGroupSwipeInk(ref: React.RefObject<HTMLElement | null>, live: boolean): void {
  useLayoutEffect(() => {
    if (!live) return
    let raf = 0
    let applied: SwipeFrame | null = null
    const apply = (): void => {
      raf = 0
      const f = groupSwipeLive.get()
      if (f === applied || !ref.current) return
      applied = f
      paintGroupInk(ref.current, f)
    }
    const off = groupSwipeLive.subscribe(() => {
      if (!raf) raf = requestAnimationFrame(apply)
    })
    apply()
    return () => {
      off()
      if (raf) cancelAnimationFrame(raf)
    }
  }, [ref, live])
}
