// Home groups: the bar's scrolling. With more groups than fit, the row
// scrolls (sideways, or up and down in the rail), keeps the current group
// centred (or, mid-swipe, the point between it and the group the swipe is
// heading to), and fades an edge only while more groups sit past it.
//
// Scrolls the bar's own scroller by hand: `scrollIntoView` would also
// scroll the nav and the window around it.

import { useLayoutEffect, type RefObject } from 'react'

/** The scroll offset that centres an item (`start`, `size`) in a view of
 *  `view` px over `content` px of content, kept in range. */
export function centredScroll(start: number, size: number, view: number, content: number): number {
  const max = Math.max(0, content - view)
  return Math.round(Math.min(max, Math.max(0, start + size / 2 - view / 2)))
}

/** Which ends have more groups past them. */
export function barFades(pos: number, view: number, content: number): { start: boolean; end: boolean } {
  const room = content - view
  if (room <= 1) return { start: false, end: false }
  return { start: pos > 1, end: pos < room - 1 }
}

function readFades(frame: HTMLElement, scroller: HTMLElement, vertical: boolean): void {
  const f = vertical
    ? barFades(scroller.scrollTop, scroller.clientHeight, scroller.scrollHeight)
    : barFades(scroller.scrollLeft, scroller.clientWidth, scroller.scrollWidth)
  frame.toggleAttribute('data-fade-start', f.start)
  frame.toggleAttribute('data-fade-end', f.end)
}

/** Centre `targets` (one id, or two: the midpoint) and keep the fades true. */
export function useGroupBarScroll(
  frame: RefObject<HTMLElement | null>,
  scroller: RefObject<HTMLElement | null>,
  vertical: boolean,
  targets: readonly string[],
  layoutKey: string,
): void {
  const key = targets.join('|')
  useLayoutEffect(() => {
    const sc = scroller.current
    const fr = frame.current
    if (!sc || !fr) return
    const all = Array.from(sc.querySelectorAll<HTMLElement>('[data-group-id]'))
    const items = targets
      .map((id) => all.find((el) => el.dataset.groupId === id))
      .filter((el): el is HTMLElement => el !== undefined)
    if (items.length > 0) {
      const start = items.reduce((a, el) => a + (vertical ? el.offsetTop : el.offsetLeft), 0) / items.length
      const size = vertical ? items[0].offsetHeight : items[0].offsetWidth
      if (vertical) sc.scrollTop = centredScroll(start, size, sc.clientHeight, sc.scrollHeight)
      else sc.scrollLeft = centredScroll(start, size, sc.clientWidth, sc.scrollWidth)
    }
    readFades(fr, sc, vertical)
  }, [frame, scroller, vertical, key, layoutKey])

  useLayoutEffect(() => {
    const sc = scroller.current
    const fr = frame.current
    if (!sc || !fr) return
    const update = (): void => readFades(fr, sc, vertical)
    sc.addEventListener('scroll', update, { passive: true })
    const ro = typeof ResizeObserver === 'undefined' ? null : new ResizeObserver(update)
    ro?.observe(sc)
    return () => {
      sc.removeEventListener('scroll', update)
      ro?.disconnect()
    }
  }, [frame, scroller, vertical])
}
