// A column's resize edge: the drag handle between two panes, in the gap
// between their tiles. The app's `data-resize-edge` look (a hover tint in
// Terminal, a short accent pill in Glass), plus the keyboard: it is a
// focusable separator with its value, and ← / → move it a step (Home and
// End go to the ends).
//
// Drag live, settle on release (the dashboard-divider idiom): `onLive` runs
// while the pointer moves, `onCommit` once when it lets go or a key moves
// it. A fixed overlay holds the drag, so iframes and terminals under the
// pointer never swallow its moves. The pointer moves in on-screen px; the
// value is CSS px (lib/app-zoom).

import React, { useCallback, useState } from 'react'
import { clientToCssPx } from '@/lib/app-zoom'
import { clearStuckBodyUserSelect } from '@/components/common/SelectableText'

/** The edge's hit width (CSS px), centred on the line between the panes. */
export const RESIZE_EDGE_HIT_PX = 7
/** One arrow-key step. */
export const RESIZE_EDGE_KEY_STEP = 16

export function ResizeEdge({
  value,
  min,
  max,
  grows,
  onLive,
  onCommit,
  label,
  testId,
  style,
}: {
  /** The pane's width now. */
  value: number
  min: number
  max: number
  /** Which way the pane grows: 'right' for a pane on the left of the edge
   *  (dragging right widens it), 'left' for a pane on its right. */
  grows: 'left' | 'right'
  onLive: (width: number) => void
  onCommit: (width: number) => void
  /** "Resize the ticket list". */
  label: string
  testId?: string
  /** Where the edge sits (left / right offsets in the positioned parent). */
  style: React.CSSProperties
}): React.JSX.Element {
  const [dragging, setDragging] = useState(false)
  const sign = grows === 'right' ? 1 : -1
  const clamp = useCallback((n: number) => Math.round(Math.min(max, Math.max(min, n))), [min, max])

  const onMouseDown = (e: React.MouseEvent): void => {
    if (e.button !== 0) return
    e.preventDefault()
    const startX = e.clientX
    const base = value
    let last = base
    document.body.style.cursor = 'col-resize'
    setDragging(true)
    const onMove = (ev: MouseEvent): void => {
      last = clamp(base + sign * clientToCssPx(ev.clientX - startX))
      onLive(last)
    }
    const onUp = (): void => {
      document.removeEventListener('mousemove', onMove)
      document.removeEventListener('mouseup', onUp)
      document.body.style.cursor = ''
      clearStuckBodyUserSelect()
      setDragging(false)
      onCommit(last)
    }
    document.addEventListener('mousemove', onMove)
    document.addEventListener('mouseup', onUp)
  }

  const onKeyDown = (e: React.KeyboardEvent): void => {
    let next: number | null = null
    if (e.key === 'ArrowRight') next = value + sign * RESIZE_EDGE_KEY_STEP
    else if (e.key === 'ArrowLeft') next = value - sign * RESIZE_EDGE_KEY_STEP
    else if (e.key === 'Home') next = min
    else if (e.key === 'End') next = max
    if (next === null) return
    e.preventDefault()
    onCommit(clamp(next))
  }

  return (
    <>
      <div
        role="separator"
        aria-orientation="vertical"
        aria-label={label}
        aria-valuenow={Math.round(value)}
        aria-valuemin={Math.round(min)}
        aria-valuemax={Math.round(max)}
        tabIndex={0}
        title="Drag to resize"
        data-resize-edge=""
        data-testid={testId}
        className="k2-focus no-drag absolute top-0 bottom-0 z-10 cursor-col-resize outline-none hover:bg-[var(--color-accent)]/40 transition-colors"
        style={{ width: RESIZE_EDGE_HIT_PX, ...style }}
        onMouseDown={onMouseDown}
        onKeyDown={onKeyDown}
      />
      {dragging && <div className="fixed inset-0 z-50" style={{ cursor: 'col-resize' }} />}
    </>
  )
}
