// The message list shares its height with the box under it. When the box
// grows (the reply bar, the typing line, a longer draft) the list gets
// shorter, and the browser keeps its scroll offset, so the newest message
// slid under the box. When the list was at its newest message, this keeps
// it there; a reader who scrolled up stays where they were.

import { useLayoutEffect } from 'react'

export function useKeepNewestInView(scroller: () => HTMLElement | null, following: () => boolean): void {
  useLayoutEffect(() => {
    const el = scroller()
    if (!el || typeof ResizeObserver === 'undefined') return
    let height = el.clientHeight
    const ro = new ResizeObserver(() => {
      const now = el.clientHeight
      if (now !== height && following()) el.scrollTop = el.scrollHeight
      height = now
    })
    ro.observe(el)
    return () => ro.disconnect()
    // The scroller and the follow flag are refs: read when it fires.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [])
}
