import { useLayoutEffect, useState } from 'react'

/** The element's width, kept current as it resizes; 0 until it has been
 *  measured (and in test DOMs, which have no layout). Measured before paint,
 *  so the first frame already has the right layout. Pass the element itself
 *  (a callback ref's state), so a root that changes with the layout is
 *  followed. Settings pages use it to switch between one and two columns at
 *  the pane's width, not the window's. */
export function useElementWidth(el: HTMLElement | null): number {
  const [width, setWidth] = useState(0)
  useLayoutEffect(() => {
    if (!el) return
    const read = (): void => setWidth(el.clientWidth)
    read()
    if (typeof ResizeObserver === 'undefined') return
    const ro = new ResizeObserver(read)
    ro.observe(el)
    return () => ro.disconnect()
  }, [el])
  return width
}
