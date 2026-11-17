// The message list shares its height with the box under it. When the box
// grows (the reply bar, the typing line, a longer draft) the list gets
// shorter, and the browser keeps its scroll offset, so the newest message
// slid under the box (or under a call's stage opening above). When the list
// was at its newest message, this keeps it there; a reader who scrolled up
// stays where they were. "Was at it" is read from the list's last size too:
// a list whose messages all fit has nothing to scroll, so no scroll event
// ever said it reached its end.

import { useLayoutEffect } from 'react'

/** Within this of the end counts as at it (the view's own follow rule). */
const AT_END_PX = 32

export function useKeepNewestInView(scroller: () => HTMLElement | null, following: () => boolean): void {
  useLayoutEffect(() => {
    const el = scroller()
    if (!el || typeof ResizeObserver === 'undefined') return
    let height = el.clientHeight
    const ro = new ResizeObserver(() => {
      const now = el.clientHeight
      const wasAtEnd = height > 0 && el.scrollTop + height >= el.scrollHeight - AT_END_PX
      if (now !== height && (following() || wasAtEnd)) el.scrollTop = el.scrollHeight
      height = now
    })
    ro.observe(el)
    return () => ro.disconnect()
    // The scroller and the follow flag are refs: read when it fires.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [])
}
