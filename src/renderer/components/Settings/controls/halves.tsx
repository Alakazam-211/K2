// Two halves side by side, each scrolling on its own (Settings → Projects,
// Settings → Providers / Models): a page's main settings on the left, its
// second list on the right, a hairline between. The page's header sits
// above, fixed. Below HALVES_MIN_WIDTH CSS px of body the halves stack,
// left above right, in one scroll, the right half under a hairline.
//
// The body is measured in CSS px (lib/app-zoom), so it follows the Settings
// pane and the app zoom, not the window. Each half fades at an edge while
// there is more to scroll that way (ScrollEdge).
//
// Use it as: the body `halves.body(stacked)` holding a ScrollEdge pair when
// stacked, then the left half (`halves.left`) and the right half
// (`halves.right`, its content under `halves.rightTop`), each holding its
// own ScrollEdge pair when split. Keep the same wrappers in both layouts,
// so crossing the breakpoint restyles them instead of remounting the rows.

import React, { useLayoutEffect, useRef, useState } from 'react'
import { cx } from '@/components/ui'
import { clientToCssPx } from '@/lib/app-zoom'

/** Below this width (CSS px) of the halves' body they stack. */
export const HALVES_MIN_WIDTH = 720

/** Width of an element in CSS px, live (ResizeObserver). `fallback` until
 *  the first measurement, or forever where ResizeObserver doesn't exist. */
export function useCssWidth(fallback: number): [React.RefObject<HTMLDivElement | null>, number] {
  const ref = useRef<HTMLDivElement>(null)
  const [width, setWidth] = useState(fallback)
  useLayoutEffect(() => {
    const el = ref.current
    if (!el || typeof ResizeObserver === 'undefined') return
    const measure = (): void => {
      // The rect is on-screen px; HALVES_MIN_WIDTH is CSS px (lib/app-zoom).
      const w = clientToCssPx(el.getBoundingClientRect().width)
      if (w > 0) setWidth(w)
    }
    measure()
    const ro = new ResizeObserver(measure)
    ro.observe(el)
    return () => ro.disconnect()
  }, [])
  return [ref, width]
}

/** The classes for each part, split or stacked. */
export const halves = {
  /** The body under the fixed header: two columns, or one scroll. */
  body: (stacked: boolean): string =>
    cx('flex-1 min-h-0', stacked ? 'overflow-y-auto' : 'grid grid-cols-2 divide-x divide-[var(--color-border)]'),
  left: (stacked: boolean): string => cx('px-6 pb-8 [scrollbar-gutter:stable]', stacked ? '' : 'overflow-y-auto min-h-0'),
  right: (stacked: boolean): string =>
    cx('px-6 pb-8 [scrollbar-gutter:stable]', stacked ? 'pt-5 border-t border-[var(--color-border)]' : 'overflow-y-auto min-h-0'),
  /** The right half's content: its top space when split (stacked, the half
   *  has it above the hairline). */
  rightTop: (stacked: boolean): string => (stacked ? '' : 'pt-5'),
}

/** A fade at the top or bottom edge of a scrolling half, shown while there
 *  is more to scroll that way, so a row cut by the edge reads as "more
 *  below" instead of clipped. A zero-height sticky child of the scroller:
 *  place the top one first and the bottom one last. `inset` stretches it
 *  over the scroller's side padding. The top one sits under a sticky bar
 *  in the half (Projects' dashboard bar, z-10: z-[5] < z-10); the bottom
 *  one over it. */
export function ScrollEdge({ at, inset = '' }: { at: 'top' | 'bottom'; inset?: string }): React.JSX.Element {
  const ref = useRef<HTMLDivElement>(null)
  const [more, setMore] = useState(false)
  useLayoutEffect(() => {
    const host = ref.current?.parentElement
    if (!host) return
    const update = (): void => {
      const room = host.scrollHeight - host.clientHeight
      setMore(room > 1 && (at === 'top' ? host.scrollTop > 1 : host.scrollTop < room - 1))
    }
    update()
    host.addEventListener('scroll', update, { passive: true })
    const ro = typeof ResizeObserver === 'undefined' ? null : new ResizeObserver(update)
    ro?.observe(host)
    for (const child of Array.from(host.children)) ro?.observe(child)
    return () => {
      host.removeEventListener('scroll', update)
      ro?.disconnect()
    }
  }, [at])
  return (
    <div
      ref={ref}
      aria-hidden="true"
      data-scroll-edge={at}
      data-more={more || undefined}
      className={cx('sticky h-0 pointer-events-none', at === 'top' ? 'top-0 z-[5]' : 'bottom-0 z-20', inset)}
    >
      <div
        className={cx(
          'absolute inset-x-0 h-6 transition-opacity duration-150',
          at === 'top'
            ? 'top-0 bg-gradient-to-b from-[var(--color-bg)] to-transparent'
            : 'bottom-0 bg-gradient-to-t from-[var(--color-bg)] to-transparent',
          more ? 'opacity-100' : 'opacity-0',
        )}
      />
    </div>
  )
}
