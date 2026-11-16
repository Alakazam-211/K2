// Older messages landing above the oldest loaded one (an "Earlier
// messages" page) keep the message being read where it is on screen.
//
// The row is read in the render that draws the page (the DOM is still the
// one the reader sees) and put back in that commit's layout phase, so
// nothing the reader or the list did while the page was on its way is
// undone. The chats used to keep the list's height (and scrollTop) from
// when the page was asked for: a re-pin meanwhile (a trim, an arrival, the
// activity bar) spent that height and threw the reader to the page's first
// row, at the very top; a scrollTop kept from the ask undid any scrolling
// done while the page loaded.
//
// Rows are the direct children of `rows()` with `data-zen-message` (the
// Thread's bubble root), as for the render window.

import { useLayoutEffect, useRef } from 'react'

const ROW_SELECTOR = ':scope > [data-zen-message]'

/** The first row (partly) on screen in `scroller` and its offset in the
 *  content: where the reader is. */
export function readingRow(
  scroller: HTMLElement | null,
  rows: HTMLElement | null,
): { el: HTMLElement; top: number } | null {
  if (!scroller || !rows || !(scroller.clientHeight > 0)) return null
  const top = scroller.getBoundingClientRect().top
  for (const r of Array.from(rows.querySelectorAll<HTMLElement>(ROW_SELECTOR))) {
    if (r.getBoundingClientRect().bottom > top + 1) return { el: r, top: r.offsetTop }
  }
  return null
}

export interface PrependHoldOptions {
  /** Every row's key in memory, oldest first. */
  keys: readonly string[]
  /** The scrolling element. */
  scroller: () => HTMLElement | null
  /** The rows' parent. */
  rows: () => HTMLElement | null
  /** Following the newest: the list's own pin wins, nothing is held. */
  following: () => boolean
  /** The render window's `wrote`: the hold's scroll is not the reader's. */
  wrote: () => void
  /** The hold wrote scrollTop (a saved position to keep up to date). */
  onScrollWrite?: (top: number) => void
}

/** Call after the render window (its layout effect runs first) and after
 *  any layout effect that restores a saved scrollTop. */
export function usePrependHold(o: PrependHoldOptions): void {
  const opts = useRef(o)
  opts.current = o
  const first = o.keys.length > 0 ? o.keys[0] : null
  const firstRef = useRef(first)
  const hold = useRef<{ el: HTMLElement; top: number } | null>(null)
  if (firstRef.current !== first) {
    const was = firstRef.current
    firstRef.current = first
    // Rows came in above the old first one (it is still here, further
    // down); a new chat or a re-read that dropped it holds nothing.
    const prepended = was !== null && o.keys.indexOf(was) > 0
    hold.current = prepended && !o.following() ? readingRow(o.scroller(), o.rows()) : null
  }

  useLayoutEffect(() => {
    const a = hold.current
    if (!a) return
    hold.current = null
    const { scroller, wrote, onScrollWrite } = opts.current
    const el = scroller()
    if (!el || !a.el.isConnected) return
    const dy = a.el.offsetTop - a.top
    if (Math.abs(dy) < 1) return
    el.scrollTop += dy
    wrote()
    onScrollWrite?.(el.scrollTop)
  })
}
