// A short move when a list's rows change places (K2 Design): each row
// slides from where it was to where it is now (FLIP), on the Style's medium
// motion (Terminal 150ms, Glass 180ms). Subs / Tokens uses it when the
// server default moves to the top of its card.
//
// Web Animations on the row's own box: the transform runs from the old
// place to none and simply ends; no fill, no inline styles to clean up.
// Places are `offsetTop` (page scroll doesn't change them; a running
// transform doesn't either, so a second move mid-move starts from the box),
// read after every commit so a change starts from the last one drawn.
//
// Never moves: on the first draw, when the rows themselves change (a new
// list, a server switch, a row added or removed), when the order is the
// same, or under reduced motion (instant).

import { useCallback, useEffect, useLayoutEffect, useRef } from 'react'
import { useReducedMotion } from '@/hooks/useReducedMotion'
import { mediumMotionTiming } from '@/lib/motion-timing'

/** `order`: the rows' ids, top to bottom. Returns a ref for each row. */
export function useReorderMotion(order: string[]): (id: string) => (el: HTMLElement | null) => void {
  const reduced = useReducedMotion()
  const els = useRef(new Map<string, HTMLElement>())
  const tops = useRef(new Map<string, number>())
  const prevOrder = useRef<string[] | null>(null)
  const anims = useRef(new Map<string, Animation>())
  const refs = useRef(new Map<string, (el: HTMLElement | null) => void>())
  const key = order.join('\u0000')

  const register = useCallback((id: string) => {
    let ref = refs.current.get(id)
    if (!ref) {
      ref = (el: HTMLElement | null) => {
        if (el) els.current.set(id, el)
        else els.current.delete(id)
      }
      refs.current.set(id, ref)
    }
    return ref
  }, [])

  // Every commit: anything above the rows (a note) moves them too.
  useLayoutEffect(() => {
    const was = prevOrder.current
    prevOrder.current = order
    const before = tops.current
    const now = new Map<string, number>()
    for (const id of order) {
      const el = els.current.get(id)
      if (el) now.set(id, el.offsetTop)
    }
    tops.current = now
    const sameRows = was !== null && was.length === order.length && was.every((id) => order.includes(id))
    if (!sameRows || was.join('\u0000') === key || reduced) return
    const { duration, easing } = mediumMotionTiming()
    for (const id of order) {
      const el = els.current.get(id)
      const from = before.get(id)
      const to = now.get(id)
      if (!el || from === undefined || to === undefined || from === to || typeof el.animate !== 'function') continue
      anims.current.get(id)?.cancel()
      let a: Animation
      try {
        a = el.animate([{ transform: `translateY(${from - to}px)` }, { transform: 'translateY(0px)' }], { duration, easing })
      } catch {
        continue
      }
      anims.current.set(id, a)
      const done = (): void => {
        if (anims.current.get(id) === a) anims.current.delete(id)
      }
      a.finished.then(done, done)
    }
  })

  useEffect(() => {
    const running = anims.current
    return () => {
      for (const a of running.values()) a.cancel()
      running.clear()
    }
  }, [])

  return register
}
