// Drawer handles (0.46.0 UI experiment): a slim pill, centred vertically
// on the edge where a drawer opens, replaces the top bar's drawer
// buttons. Click toggles the drawer. While the drawer is open the pill
// sits on the drawer's inner edge (half over the drawer, so it stays
// reachable over a Browser tab's native page) and can also be dragged to
// resize it; the drawer's own full-height resize edge still works above
// and below the pill.
//
// Open state stays per-window chrome (`usePanelsStore` →
// `k2.windowChrome.<label>`); this is only another gesture on it.
// Which pages use it: `drawer-handles.ts`.

import { useCallback, useRef } from 'react'
import { usePanelsStore } from '@/stores/panels'

/** Pointer travel (px) before a press on an open drawer's pill becomes a resize drag. */
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
  // so the pill cannot hang past it). Open: centred on the drawer border.
  const position =
    side === 'left'
      ? open
        ? 'left-0 -translate-x-1/2'
        : 'left-0'
      : open
        ? 'right-0 translate-x-1/2'
        : 'right-0'

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
      className={`group no-drag absolute top-1/2 z-20 flex h-14 w-3 -translate-y-1/2 cursor-pointer items-center justify-center rounded-full outline-none focus-visible:ring-1 focus-visible:ring-[var(--color-accent)] ${position}`}
    >
      <span
        aria-hidden="true"
        className="block h-8 w-1 rounded-full bg-[var(--color-border)] transition-all duration-150 group-hover:h-12 group-hover:w-1.5 group-hover:bg-[var(--color-text-secondary)] group-focus-visible:bg-[var(--color-accent)]"
      />
    </button>
  )
}
