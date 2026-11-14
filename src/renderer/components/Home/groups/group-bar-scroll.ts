// Home groups: the bar's scrolling. With more groups than fit, the row
// scrolls (sideways, or up and down in the rail), keeps the current group
// centred (or, mid-swipe, the point between it and the group the swipe is
// heading to), and fades an edge only while more groups sit past it.
//
// Overflow (home-groups notes, round mp-4): while the groups don't fit, the
// bar's root carries `data-overflow`, and `data-more-start` /
// `data-more-end` while more groups sit that way. The chevrons at the bar's
// ends show from those marks (home-groups.css) and page the bar by its
// visible extent (`pageBar`). Keyboard focus reveals the focused group
// (`revealInBar`).
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

/** The groups don't fit: their full length is more than the room the bar
 *  has (`extent`: the bar's whole width, chevrons included, so showing the
 *  chevrons never flips it back; the rail's capped stack height). */
export function barOverflows(content: number, extent: number): boolean {
  return content > extent + 1
}

/** The scroll offset one page towards `dir` (-1 back, 1 on): the visible
 *  extent, kept in range. */
export function pagedScroll(pos: number, view: number, content: number, dir: -1 | 1): number {
  const max = Math.max(0, content - view)
  return Math.round(Math.min(max, Math.max(0, pos + dir * view)))
}

/** The smallest scroll that brings an item (`start`, `size`) fully into a
 *  view at `pos` of `view` px; `pos` when it is already in view. */
export function revealScroll(start: number, size: number, pos: number, view: number): number {
  if (start < pos) return Math.max(0, Math.round(start))
  if (start + size > pos + view) return Math.round(start + size - view)
  return pos
}

/** Mark the fades on the frame and the overflow on the bar's root. */
export function syncBarEdges(frame: HTMLElement, scroller: HTMLElement, vertical: boolean): void {
  const f = vertical
    ? barFades(scroller.scrollTop, scroller.clientHeight, scroller.scrollHeight)
    : barFades(scroller.scrollLeft, scroller.clientWidth, scroller.scrollWidth)
  frame.toggleAttribute('data-fade-start', f.start)
  frame.toggleAttribute('data-fade-end', f.end)
  const root = frame.closest<HTMLElement>('[data-home-group-bar]')
  if (!root) return
  const over = vertical
    ? barOverflows(scroller.scrollHeight, scroller.clientHeight)
    : barOverflows(scroller.scrollWidth, root.clientWidth)
  root.toggleAttribute('data-overflow', over)
  root.toggleAttribute('data-more-start', over && f.start)
  root.toggleAttribute('data-more-end', over && f.end)
}

/** A chevron's click: one page of the bar towards `dir`. */
export function pageBar(frame: HTMLElement, scroller: HTMLElement, vertical: boolean, dir: -1 | 1): void {
  if (vertical) scroller.scrollTop = pagedScroll(scroller.scrollTop, scroller.clientHeight, scroller.scrollHeight, dir)
  else scroller.scrollLeft = pagedScroll(scroller.scrollLeft, scroller.clientWidth, scroller.scrollWidth, dir)
  syncBarEdges(frame, scroller, vertical)
}

/** Keyboard focus moved to `item`: scroll it fully into the bar's view. */
export function revealInBar(frame: HTMLElement, scroller: HTMLElement, item: HTMLElement, vertical: boolean): void {
  if (vertical) scroller.scrollTop = revealScroll(item.offsetTop, item.offsetHeight, scroller.scrollTop, scroller.clientHeight)
  else scroller.scrollLeft = revealScroll(item.offsetLeft, item.offsetWidth, scroller.scrollLeft, scroller.clientWidth)
  syncBarEdges(frame, scroller, vertical)
}

/** Centre `targets` (one id, or two: the midpoint) and keep the fades and
 *  the overflow marks true. */
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
    syncBarEdges(fr, sc, vertical)
  }, [frame, scroller, vertical, key, layoutKey])

  useLayoutEffect(() => {
    const sc = scroller.current
    const fr = frame.current
    if (!sc || !fr) return
    const update = (): void => syncBarEdges(fr, sc, vertical)
    sc.addEventListener('scroll', update, { passive: true })
    const ro = typeof ResizeObserver === 'undefined' ? null : new ResizeObserver(update)
    ro?.observe(sc)
    // The bar's own width too: a wider nav can fit every group again.
    const root = fr.closest('[data-home-group-bar]')
    if (root) ro?.observe(root)
    return () => {
      sc.removeEventListener('scroll', update)
      ro?.disconnect()
    }
  }, [frame, scroller, vertical])
}
