// The lanes follow the monitor's playhead (T5):
//   - while it plays, by the View menu's "While playing": page (jump a
//     screen at 95% so it lands at 5%) or centred (it stays at 50%);
//   - while you follow someone who isn't playing, their edit point stays in
//     view (it comes to a third in when it would leave the screen).
// Only your own wheel, drag or keys on the lanes count as your scroll
// (`onUserScroll`), never these moves, so following stops only on yours.

import { useEffect, useLayoutEffect, useRef } from 'react'
import type { ScrollPref } from '@/lib/live-view/timeline/lanes-view'
import { scrollFor } from '@/lib/live-view/timeline/time'

export interface PlayScroll {
  playing: boolean
  following: boolean
  scroll: ScrollPref
}

export function usePlayScroll(scroller: () => HTMLElement | null, x: number | null, tw: number, f: PlayScroll | undefined, onUserScroll?: () => void): void {
  useLayoutEffect(() => {
    const s = scroller()
    if (!s || x == null || !f) return
    if (f.playing) {
      const next = scrollFor(f.scroll, x, s.scrollLeft, tw)
      if (next != null) s.scrollLeft = Math.max(0, next)
    } else if (f.following && (x < s.scrollLeft + 40 || x > s.scrollLeft + tw - 40)) s.scrollLeft = Math.max(0, x - tw * 0.35)
  })
  const cb = useRef(onUserScroll)
  cb.current = onUserScroll
  useEffect(() => {
    const s = scroller()
    if (!s) return
    const mine = (): void => cb.current?.()
    const key = (e: KeyboardEvent): void => {
      if (['ArrowLeft', 'ArrowRight', 'PageUp', 'PageDown'].includes(e.key) && e.target === s) mine()
    }
    s.addEventListener('wheel', mine, { passive: true })
    s.addEventListener('pointerdown', mine)
    s.addEventListener('keydown', key)
    return () => {
      s.removeEventListener('wheel', mine)
      s.removeEventListener('pointerdown', mine)
      s.removeEventListener('keydown', key)
    }
  }, [scroller])
}
