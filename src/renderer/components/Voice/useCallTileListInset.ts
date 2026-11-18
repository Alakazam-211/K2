// A message list (Home chat's, the agent Thread's) under the floating call
// tile: while the tile rests in a bottom corner over the list's message
// column, the list makes room under its newest message (the tile's height
// over it plus the gap), so the tile never covers it. A list that was at its
// newest message stays at it as the room comes and goes; a reader who
// scrolled up stays where they were. A tile in a top corner, beside the
// column, or gone takes nothing.

import { useEffect, useLayoutEffect, useRef, useState } from 'react'
import { clientRectToCss } from '@/lib/app-zoom'
import { listInsetUnderTile, type Rect } from '@/lib/voice/mini-snap'
import { CALL_TILE_GAP, useCallTilePlace } from '@/stores/call-tile-place'

/** Within this of the end counts as at it (the views' own follow rule). */
const AT_END_PX = 32

function rectOf(el: Element): Rect {
  const r = clientRectToCss(el.getBoundingClientRect())
  return { left: r.left, top: r.top, width: r.width, height: r.height }
}

/** The room (px) the list `scroller` makes under its newest message for the
 *  tile. `following`: the list is at its newest message (its scroll rule). */
export function useCallTileListInset(scroller: () => HTMLElement | null, following: () => boolean): number {
  const rest = useCallTilePlace((s) => s.rest)
  const [inset, setInset] = useState(0)
  const look = useRef<() => void>(() => {})
  look.current = () => {
    const el = scroller()
    if (!el) return setInset(0)
    const column = el.querySelector('[data-thread-column]')
    setInset(listInsetUnderTile(rest, rectOf(el), column ? rectOf(column) : null, CALL_TILE_GAP))
  }
  // The tile comes to rest again whenever its spot moves (a drawer, a box
  // growing, the window), so its rest and the window's size are enough.
  useLayoutEffect(() => look.current(), [rest])
  useEffect(() => {
    const again = (): void => look.current()
    window.addEventListener('resize', again)
    return () => window.removeEventListener('resize', again)
  }, [])
  // The room came or went: a list that was at its newest message stays at it.
  const was = useRef(0)
  useLayoutEffect(() => {
    const delta = inset - was.current
    was.current = inset
    const el = scroller()
    if (!el || delta === 0) return
    const atEnd = el.scrollTop + el.clientHeight >= el.scrollHeight - delta - AT_END_PX
    if (following() || atEnd) el.scrollTop = el.scrollHeight
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [inset])
  return inset
}
