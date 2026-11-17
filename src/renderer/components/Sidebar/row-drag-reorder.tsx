// Drag-to-reorder for the agent rows in a left nav — one implementation for
// the Agents sidebar and the Home roster so the two look and feel the same:
// press on a row, move past a small threshold, and the row goes 40% (the
// ghost) and the cursor turns to `grabbing`. A press that never passes the
// threshold stays a click; a press that did drag never fires the row's
// click.
//
// While dragging, no row moves: a drop line (<RowDropLine>, the accent
// colour, 2px with a 6px cap at its left end) floats over the list where
// the row will land, centred on the seam between two rows or at the list's
// top or bottom. A list can say where a drop really lands (`landingGap`)
// and which area the pointer must stay in (`bounds`); where the row can't
// land, there's no line and a release does nothing.
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

/** The drop line's cap: a 6px square (Terminal) or dot (Glass) at its left
 *  end. The line itself is 2px. */
export const DROP_LINE_CAP_PX = 6
const DROP_LINE_PX = 2
/** Two rows closer than this (px) touch: the line goes on their seam. */
const ROWS_TOUCH_PX = 4

/** Where the drop line for gap `gap` sits: its centre, in px from the top
 *  of `container`. Between two rows that touch, on their seam. Where
 *  something sits between the two rows (a caption), against the row on
 *  the dragged row's side (`from`): after the row above when moving down,
 *  before the row below when moving up. Kept half a cap inside the
 *  container, so a list that clips its edges still shows the whole line.
 *  Null when the list has no rows. */
export function dropLineY(container: Element, rowSelector: string, gap: number, from: number): number | null {
  const rows = container.querySelectorAll(rowSelector)
  if (rows.length === 0) return null
  const box = container.getBoundingClientRect()
  let y: number
  if (gap <= 0) y = rows[0].getBoundingClientRect().top
  else if (gap >= rows.length) y = rows[rows.length - 1].getBoundingClientRect().bottom
  else {
    const above = rows[gap - 1].getBoundingClientRect().bottom
    const below = rows[gap].getBoundingClientRect().top
    y = below - above <= ROWS_TOUCH_PX ? (above + below) / 2 : from < gap ? above : below
  }
  const half = DROP_LINE_CAP_PX / 2
  return Math.min(Math.max(y - box.top, half), Math.max(half, box.height - half))
}

/** Whether the pointer is inside `el`'s box (no element: anywhere is in). */
export function pointerInside(el: Element | null, clientX: number, clientY: number): boolean {
  if (!el) return true
  const r = el.getBoundingClientRect()
  return clientX >= r.left && clientX <= r.right && clientY >= r.top && clientY <= r.bottom
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
  /** The gap a release over gap `gap` really lands in (a list whose rows
   *  keep to sections clamps it), or null where this row can't land: no
   *  line, and a release there does nothing. Default: the pointer's gap. */
  landingGap?: (from: number, gap: number) => number | null
  /** The area the pointer must be in (the list's own panel). Outside it
   *  there's no line and a release does nothing. Default: anywhere. */
  bounds?: () => Element | null
  /** Called once on release after a real drag, with the gap it lands in. */
  onDrop: (from: number, gap: number) => void
}

export interface RowDragReorder<Z extends string> {
  onRowMouseDown: (e: React.MouseEvent, start: RowDragStart<Z>) => void
  /** Put on each row wrapper: swallows the click a finished drag would fire. */
  onRowClickCapture: (e: React.MouseEvent) => void
  /** The dragged row (paint it as the ghost). */
  isDragging: (zone: Z, index: number) => boolean
  /** The drag would land in this gap of this list. */
  showsDropLineAt: (zone: Z, gap: number) => boolean
  /** Any drag in progress in this list. */
  draggingIn: (zone: Z) => boolean
  /** The drop line's centre (px from the list's top) while a release would
   *  move the row; null otherwise (no line). Feed it to <RowDropLine>. */
  dropLineTop: (zone: Z) => number | null
}

interface DropAt {
  gap: number
  /** The line's centre, or null when the drop leaves the row where it is. */
  top: number | null
}

export function useRowDragReorder<Z extends string>(): RowDragReorder<Z> {
  const [drag, setDrag] = useState<{ zone: Z; index: number } | null>(null)
  const [drop, setDrop] = useState<DropAt | null>(null)
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
      setDrop(null)
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
      if (!pointerInside(start.bounds?.() ?? null, ev.clientX, ev.clientY)) {
        gap = null
        setDrop(null)
        return
      }
      const over = start.gapAt ? start.gapAt(container, ev.clientX, ev.clientY) : dropGapAt(container, start.rowSelector, ev.clientY)
      gap = start.landingGap ? start.landingGap(start.index, over) : over
      if (gap === null) {
        setDrop(null)
        return
      }
      const top = dropTargetIndex(start.index, gap) === null ? null : dropLineY(container, start.rowSelector, gap, start.index)
      const next: DropAt = { gap, top }
      setDrop((prev) => (prev && prev.gap === next.gap && prev.top === next.top ? prev : next))
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
    showsDropLineAt: (zone, gap) => drag !== null && drag.zone === zone && drop !== null && drop.gap === gap,
    draggingIn: (zone) => drag !== null && drag.zone === zone,
    dropLineTop: (zone) => (drag !== null && drag.zone === zone && drop !== null ? drop.top : null),
  }
}

/** The agent nav row's text inset: the 2px selection bar plus 12px padding
 *  on the left, 12px on the right (AgentRowButton). */
export const NAV_ROW_TEXT_INSET = { left: 14, right: 12 } as const

/** The drop line: where a dragged row will land, floating over the list so
 *  no row moves. Put it first inside the list's `relative` element (first,
 *  so the last row stays the last child for the rules that read it: the
 *  nav's dividers, `divide-y`); `top` is the line's centre (`dropLineTop`,
 *  or `dropLineY`; null draws nothing).
 *  `inset` is the rows' text inset (px from each side), so the line spans
 *  what the rows show, not the panel's bleed. Square in Terminal and
 *  round-ended in Glass (the pill radius token). It glides between gaps on
 *  the fast motion token, and jumps under reduced motion. */
export function RowDropLine({ top, inset }: { top: number | null; inset: { left: number; right: number } }): React.JSX.Element | null {
  if (top === null) return null
  return (
    <div
      aria-hidden="true"
      data-row-drop-line=""
      className="pointer-events-none absolute z-[1] rounded-[var(--radius-pill)] bg-[var(--color-accent)] transition-[top] duration-[var(--motion-duration-fast)] ease-[var(--motion-ease-standard)] motion-reduce:transition-none"
      // border 0: a list that rules its children (divide-y) leaves the line unruled.
      style={{ top: top - DROP_LINE_PX / 2, left: inset.left, right: inset.right, height: DROP_LINE_PX, border: 0 }}
    >
      <span
        data-row-drop-line-cap=""
        className="absolute left-0 rounded-[var(--radius-pill)] bg-[var(--color-accent)]"
        style={{ top: (DROP_LINE_PX - DROP_LINE_CAP_PX) / 2, width: DROP_LINE_CAP_PX, height: DROP_LINE_CAP_PX }}
      />
    </div>
  )
}
