// A top bar too narrow for its two clusters (a small window, the app zoomed
// in): it gives way a step at a time before anything overlaps.
//
//   compact  the server switcher shows its icon (and status light) only;
//   tight    the Usage pill shows its marks only (agent icons, or a small
//            gauge), not the percentages or the word;
//   minimal  the last resort (an 800 px window at 175 %): the dividers go,
//            and so does the server switcher until its menu opens (⌘L, as
//            Home's hidden trigger does), so the page tabs and the right
//            cluster never overlap.
//
// The bar carries the step in `data-top-bar-fit`; globals.css hides the
// labels (`data-top-bar-label`). Each pass starts from the full bar and
// steps down until the left cluster's content and the right cluster fit,
// measured in CSS px (scrollWidth, offsetWidth, clientWidth), so it is the
// same at any app zoom. Settings' bar uses it today; other pages' bars can
// pass their ref the same way.

import { useLayoutEffect, useRef } from 'react'

export const TOP_BAR_FIT_STEPS = ['full', 'compact', 'tight', 'minimal'] as const
export type TopBarFit = (typeof TOP_BAR_FIT_STEPS)[number]

/** Room kept between the two clusters, in CSS px. */
const MIN_GAP_PX = 8

const px = (v: string): number => parseFloat(v) || 0

/** Whether the bar's left content and right cluster fit side by side now. */
export function topBarFits(bar: HTMLElement, left: HTMLElement, right: HTMLElement): boolean {
  const s = getComputedStyle(bar)
  const room = bar.clientWidth - px(s.paddingLeft) - px(s.paddingRight)
  return left.scrollWidth + right.offsetWidth + MIN_GAP_PX <= room
}

/** Steps the bar down until it fits (or the last step), returning it. */
export function fitTopBar(bar: HTMLElement, left: HTMLElement, right: HTMLElement): TopBarFit {
  for (const step of TOP_BAR_FIT_STEPS) {
    bar.dataset.topBarFit = step
    if (topBarFits(bar, left, right)) return step
  }
  return TOP_BAR_FIT_STEPS[TOP_BAR_FIT_STEPS.length - 1]
}

/** Refs for the bar and its two clusters; keeps `data-top-bar-fit` current
 *  as the window, the zoom or the clusters' contents change. */
export function useTopBarFit(): {
  bar: React.RefObject<HTMLDivElement | null>
  left: React.RefObject<HTMLDivElement | null>
  right: React.RefObject<HTMLDivElement | null>
} {
  const bar = useRef<HTMLDivElement>(null)
  const left = useRef<HTMLDivElement>(null)
  const right = useRef<HTMLDivElement>(null)
  useLayoutEffect(() => {
    const b = bar.current
    const l = left.current
    const r = right.current
    if (!b || !l || !r) return
    const run = (): void => {
      // No layout (a test DOM): leave the full bar.
      if (b.clientWidth <= 0) return
      fitTopBar(b, l, r)
    }
    run()
    if (typeof ResizeObserver === 'undefined') return
    const ro = new ResizeObserver(run)
    ro.observe(b)
    for (const child of Array.from(l.children)) ro.observe(child)
    for (const child of Array.from(r.children)) ro.observe(child)
    return () => ro.disconnect()
  }, [])
  return { bar, left, right }
}
