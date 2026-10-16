// Drawer handles (0.46.0 UI experiment): a small grip (a squarish tab
// with a column of dots), flush against the edge where a drawer opens and
// centred vertically, replaces the top bar's drawer buttons. Click
// toggles the drawer. Closed, the tab hangs off the room's edge into the
// room. Open, it hangs off the drawer's inner border over the drawer (so
// it stays reachable over a Browser tab's native page) and can also be
// dragged to resize it; the drawer's own full-height resize edge still
// works above and below the tab.
//
// Open state stays per-window chrome (`usePanelsStore` →
// `k2.windowChrome.<label>`); this is only another gesture on it.
// Which pages use it: `drawer-handles.ts`.

import { useCallback, useRef } from 'react'
import { usePanelsStore } from '@/stores/panels'

/** Pointer travel (px) before a press on an open drawer's tab becomes a resize drag. */
const DRAG_THRESHOLD_PX = 3

const SIDE_NAME: Record<'left' | 'right', string> = { left: 'Left', right: 'Right' }

export default function DrawerHandle({
  side,
  open,
  label,
}: {
  side: 'left' | 'right'
  open: boolean
  /** The drawer's tab names, e.g. "Files, Workspace". */
  label: string
}): React.JSX.Element {
  const toggle = usePanelsStore((s) => (side === 'left' ? s.toggleLeftPanel : s.toggleRightPanel))
  const drag = useRef<{ startX: number; startWidth: number; moved: boolean } | null>(null)
  const suppressClick = useRef(false)

  const onPointerDown = useCallback(
    (e: React.PointerEvent<HTMLButtonElement>) => {
      if (!open || e.button !== 0) return
      const s = usePanelsStore.getState()
      drag.current = {
        startX: e.clientX,
        startWidth: side === 'left' ? s.leftPanelWidth : s.rightPanelWidth,
        moved: false,
      }
      e.currentTarget.setPointerCapture?.(e.pointerId)
    },
    [open, side],
  )

  const onPointerMove = useCallback(
    (e: React.PointerEvent<HTMLButtonElement>) => {
      const d = drag.current
      if (!d) return
      const delta = e.clientX - d.startX
      if (!d.moved && Math.abs(delta) < DRAG_THRESHOLD_PX) return
      if (!d.moved) {
        d.moved = true
        document.body.style.cursor = 'col-resize'
        document.body.style.userSelect = 'none'
      }
      const s = usePanelsStore.getState()
      if (side === 'left') s.setLeftPanelWidth(d.startWidth + delta)
      else s.setRightPanelWidth(d.startWidth - delta)
    },
    [side],
  )

  const endDrag = useCallback((e: React.PointerEvent<HTMLButtonElement>) => {
    const d = drag.current
    drag.current = null
    if (!d) return
    e.currentTarget.releasePointerCapture?.(e.pointerId)
    if (d.moved) {
      document.body.style.cursor = ''
      document.body.style.userSelect = ''
      // The click that follows a drag must not also toggle the drawer.
      suppressClick.current = true
    }
  }, [])

  const onClick = useCallback(() => {
    if (suppressClick.current) {
      suppressClick.current = false
      return
    }
    toggle()
  }, [toggle])

  const action = open ? 'Hide' : 'Show'
  const name = `${SIDE_NAME[side]} drawer`
  const ariaLabel = `${action} ${name.toLowerCase()}`
  const tip = `${action} ${name.toLowerCase()} (${label})${open ? ' · drag to resize' : ''}`

  // Closed: flush inside the room's edge (the right edge is the window's,
  // so the tab cannot hang past it). Open: flush against the drawer
  // border, over the drawer.
  const position =
    side === 'left'
      ? open
        ? 'left-0 -translate-x-full justify-end'
        : 'left-0 justify-start'
      : open
        ? 'right-0 translate-x-full justify-start'
        : 'right-0 justify-end'

  return (
    <button
      type="button"
      aria-label={ariaLabel}
      aria-expanded={open}
      title={tip}
      data-drawer-handle={side}
      data-open={open ? 'true' : 'false'}
      onClick={onClick}
      onPointerDown={onPointerDown}
      onPointerMove={onPointerMove}
      onPointerUp={endDrag}
      onPointerCancel={endDrag}
      className={`group no-drag absolute top-1/2 z-20 flex h-11 w-3 -translate-y-1/2 cursor-pointer items-center outline-none focus-visible:ring-1 focus-visible:ring-[var(--color-accent)] ${position}`}
    >
      {/* Grip: theme radius (capped so it never turns into a pill), two
          columns of three dots in the background colour. */}
      <span
        aria-hidden="true"
        data-drawer-grip=""
        className="grid h-9 w-2.5 grid-cols-[2px_2px] content-center justify-center gap-[2px] rounded-[min(var(--radius-selector),3px)] bg-[var(--color-text-secondary)] transition-all duration-150 group-hover:h-10 group-hover:w-3 group-hover:bg-[var(--color-text-primary)] group-focus-visible:bg-[var(--color-accent)]"
      >
        {Array.from({ length: 6 }, (_, i) => (
          <span key={i} data-grip-dot="" className="block h-[2px] w-[2px] rounded-full bg-[var(--color-bg)]" />
        ))}
      </span>
    </button>
  )
}
