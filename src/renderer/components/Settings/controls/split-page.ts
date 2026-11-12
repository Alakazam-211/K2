// A Settings page with its own list beside the page (Styles, User Access):
// an 18rem list on the left, the chosen item on the right, each scrolling on
// its own. In a narrow Settings pane (a small window, the app zoomed in) the
// right side would be squeezed to nothing, so below SPLIT_PAGE_MIN_PX the
// list goes on top and the page scrolls as one.
//
// Measured from the page itself in CSS px (clientWidth), so it follows the
// Settings pane and the app zoom, not the window.

import { useState } from 'react'
import { useElementWidth } from '@/hooks/useElementWidth'

/** The list's 18rem plus 20rem for the chosen item's rows, in CSS px. */
export const SPLIT_PAGE_MIN_PX = 608

/** A ref for the page's root, and whether its list goes on top. */
export function useSplitPage(): [(el: HTMLDivElement | null) => void, boolean] {
  const [el, setEl] = useState<HTMLDivElement | null>(null)
  const width = useElementWidth(el)
  return [setEl, width > 0 && width < SPLIT_PAGE_MIN_PX]
}

/** The classes for each part, split or stacked. */
export const splitPage = {
  root: (stacked: boolean): string =>
    stacked ? 'h-full min-h-0 overflow-y-auto overflow-x-hidden flex flex-col' : 'h-full min-h-0 flex',
  list: (stacked: boolean): string =>
    stacked
      ? 'flex-shrink-0 flex flex-col border-b border-[var(--color-border)]'
      : 'w-72 flex-shrink-0 min-h-0 flex flex-col border-r border-[var(--color-border)]',
  /** The list's own scroller (split only). */
  listBody: (stacked: boolean, rest: string): string =>
    stacked ? rest : `flex-1 min-h-0 overflow-y-auto ${rest} [scrollbar-gutter:stable]`,
  pane: (stacked: boolean): string =>
    stacked ? 'min-w-0 p-6' : 'flex-1 min-w-0 min-h-0 overflow-y-auto p-6 [scrollbar-gutter:stable]',
}
