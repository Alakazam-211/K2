// Drawer handles (0.46.0 UI experiment): a small dog-ear tab (a page
// corner folded over), flush against the edge where a drawer opens and
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
  // border, over the drawer. `attached` is the tab's edge that touches the
  // border; the folded corner is on the other (free) side, at the top.
  const position =
    side === 'left'
      ? open
        ? 'left-0 -translate-x-full justify-end'
        : 'left-0 justify-start'
      : open
        ? 'right-0 translate-x-full justify-start'
        : 'right-0 justify-end'
  const attached: 'left' | 'right' = (side === 'left') !== open ? 'left' : 'right'

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
      className={`group no-drag absolute top-1/2 z-20 flex h-12 w-3 -translate-y-1/2 cursor-pointer items-center outline-none focus-visible:ring-1 focus-visible:ring-[var(--color-accent)] ${position}`}
    >
      {/* Drawn attached on the left with the top-right corner folded;
          mirrored when the tab is attached on the right. */}
      <svg
        aria-hidden="true"
        data-drawer-dogear={attached}
        viewBox="0 0 12 32"
        preserveAspectRatio="none"
        className="block h-7 w-2.5 transition-all duration-150 group-hover:h-8 group-hover:w-3"
        style={attached === 'right' ? { transform: 'scaleX(-1)' } : undefined}
      >
        <path
          data-part="body"
          d="M0 0 H6 L12 6 V30 Q12 32 10 32 H0 Z"
          className="fill-[var(--color-text-secondary)] transition-colors duration-150 group-hover:fill-[var(--color-text-primary)] group-focus-visible:fill-[var(--color-accent)]"
        />
        <path
          data-part="fold"
          d="M6 0 V6 H12 Z"
          className="fill-[var(--color-text-muted)] transition-colors duration-150 group-hover:fill-[var(--color-text-secondary)]"
        />
      </svg>
    </button>
  )
}
