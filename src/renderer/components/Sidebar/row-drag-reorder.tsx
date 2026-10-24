// Drag-to-reorder for the agent rows in a left nav — one implementation for
// the Agents sidebar and the Home roster so the two look and feel the same:
// press on a row, move past a small threshold, and the row goes 40% (the
// ghost), the cursor turns to `grabbing`, and a 2px accent line marks the
// gap it will land in. A press that never passes the threshold stays a
// click; a press that did drag never fires the row's click.
//
// Only the gesture lives here. What a drop writes is the caller's: Home
// moves a row in this device's Home (localStorage), the Agents nav writes
// the daemon's canonical workspace order (`projects/reorder`).

import React, { useCallback, useEffect, useRef, useState } from 'react'

/** How far (px) the pointer must travel before a press becomes a drag. */
export const ROW_DRAG_THRESHOLD = { x: 3, y: 5 } as const

/** In-row controls that keep their own press (tag chips, worktree rows,
 *  the hide button). The row shell itself (`data-agent-row`, or any button
 *  marked `data-row-drag-handle`, e.g. a Garden's Agents widget rows) is a
 *  button too — it is the drag handle, so it is not exempt. */
const DRAG_EXEMPT =
  '[role="button"]:not([data-row-drag-handle]), button:not([data-agent-row]):not([data-row-drag-handle]), a, input, textarea, select'

export function isRowDragExempt(target: EventTarget | null): boolean {
  if (!(target instanceof Element)) return true
  return target.closest(DRAG_EXEMPT) !== null
}

export function passedRowDragThreshold(dx: number, dy: number): boolean {
  return Math.abs(dx) > ROW_DRAG_THRESHOLD.x || Math.abs(dy) > ROW_DRAG_THRESHOLD.y
}

/** The gap (0..rows) under `clientY`: after every row whose middle is above it. */
export function dropGapAt(container: Element, rowSelector: string, clientY: number): number {
  const rows = container.querySelectorAll(rowSelector)
  let gap = 0
  for (let i = 0; i < rows.length; i++) {
    const rect = rows[i].getBoundingClientRect()
    if (clientY > rect.top + rect.height / 2) gap = i + 1
  }
  return gap
}

/** Where a row taken from `from` and dropped in gap `gap` ends up, or null
 *  when the drop leaves it where it is (its own gap either side). */
export function dropTargetIndex(from: number, gap: number): number | null {
  if (from < 0 || gap < 0 || gap === from || gap === from + 1) return null
  return gap > from ? gap - 1 : gap
}

/** The full canonical order after one visible section (`zoneIds`, in the
 *  order the nav shows it) is reordered by moving `movedId` into `gap`.
 *  The section's rows keep the slots they already hold in `allIds` and are
 *  refilled in their new order; every row outside the section (the other
 *  pinned/unpinned list, focus groups not on screen) keeps its slot. The
 *  result is a full permutation, so `projects/reorder` leaves no two rows
 *  on the same tab order. Null when nothing moves. */
export function reorderedZoneIds(
  allIds: readonly string[],
  zoneIds: readonly string[],
  movedId: string,
  gap: number,
): string[] | null {
  const from = zoneIds.indexOf(movedId)
  const to = dropTargetIndex(from, Math.min(gap, zoneIds.length))
  if (to === null) return null
  const zone = [...zoneIds]
  zone.splice(to, 0, ...zone.splice(from, 1))
  const inZone = new Set(zoneIds)
  let next = 0
  const out = allIds.map((id) => (inZone.has(id) ? zone[next++] : id))
  // A section row the full list doesn't know (a stale render) — refuse
  // rather than drop it from the order.
  if (next !== zone.length) return null
  return out
}

export interface RowDragStart<Z extends string> {
  /** Which list the row is in; a drag never leaves its list. */
  zone: Z
  /** The row's index in that list. */
  index: number
  /** The list's element (holds the rows matched by `rowSelector`). */
  container: () => Element | null
  rowSelector: string
  /** The gap under the pointer (default: by `clientY` only, a vertical list;
   *  a grid passes its own reading-order rule). */
  gapAt?: (container: Element, clientX: number, clientY: number) => number
  /** Called once on release after a real drag, with the gap it ended over. */
  onDrop: (from: number, gap: number) => void
}

export interface RowDragReorder<Z extends string> {
  onRowMouseDown: (e: React.MouseEvent, start: RowDragStart<Z>) => void
  /** Put on each row wrapper: swallows the click a finished drag would fire. */
  onRowClickCapture: (e: React.MouseEvent) => void
  /** The dragged row (paint it as the ghost). */
  isDragging: (zone: Z, index: number) => boolean
  /** Paint the drop line in this gap of this list. */
  showsDropLineAt: (zone: Z, gap: number) => boolean
  /** Any drag in progress in this list. */
  draggingIn: (zone: Z) => boolean
}

export function useRowDragReorder<Z extends string>(): RowDragReorder<Z> {
  const [drag, setDrag] = useState<{ zone: Z; index: number } | null>(null)
  const [dropGap, setDropGap] = useState<number | null>(null)
  const suppressClickRef = useRef(false)
  const endRef = useRef<(() => void) | null>(null)

  // Unmounted mid-drag: drop the listeners and give the cursor back.
  useEffect(() => () => endRef.current?.(), [])

  const onRowMouseDown = useCallback((e: React.MouseEvent, start: RowDragStart<Z>) => {
    if (e.button !== 0 || isRowDragExempt(e.target)) return
    endRef.current?.()
    const startX = e.clientX
    const startY = e.clientY
    let started = false
    let gap: number | null = null

    const end = (): void => {
      document.removeEventListener('mousemove', onMove)
      document.removeEventListener('mouseup', onUp)
      document.body.style.cursor = ''
      document.body.style.userSelect = ''
      endRef.current = null
      setDrag(null)
      setDropGap(null)
    }
    const onMove = (ev: MouseEvent): void => {
      if (!started && passedRowDragThreshold(ev.clientX - startX, ev.clientY - startY)) {
        started = true
        setDrag({ zone: start.zone, index: start.index })
        document.body.style.cursor = 'grabbing'
        document.body.style.userSelect = 'none'
      }
      if (!started) return
      const container = start.container()
      if (!container) return
      gap = start.gapAt ? start.gapAt(container, ev.clientX, ev.clientY) : dropGapAt(container, start.rowSelector, ev.clientY)
      setDropGap(gap)
    }
    const onUp = (): void => {
      end()
      if (!started) return
      // The click this release may fire (when it lands on a row) is the
      // drag's, not a pick. Clear the flag after it would have fired so a
      // release off the rows doesn't eat the next real click.
      suppressClickRef.current = true
      setTimeout(() => { suppressClickRef.current = false }, 0)
      if (gap !== null) start.onDrop(start.index, gap)
    }
    endRef.current = end
    document.addEventListener('mousemove', onMove)
    document.addEventListener('mouseup', onUp)
  }, [])

  const onRowClickCapture = useCallback((e: React.MouseEvent) => {
    if (!suppressClickRef.current) return
    suppressClickRef.current = false
    e.stopPropagation()
    e.preventDefault()
  }, [])

  return {
    onRowMouseDown,
    onRowClickCapture,
    isDragging: (zone, index) => drag !== null && drag.zone === zone && drag.index === index,
    showsDropLineAt: (zone, gap) => drag !== null && drag.zone === zone && dropGap === gap,
    draggingIn: (zone) => drag !== null && drag.zone === zone,
  }
}

/** The 2px accent line in the gap a dragged row will land in. */
export function RowDropLine(): React.JSX.Element {
  return <div className="h-[2px] bg-[var(--color-accent)] mx-3" data-row-drop-line="" />
}
