// k1 grid merge + one-batch apply. Pure; the pane owns WS / React / painter I/O.

import { anchorScrollPx, clampScrollPx } from './scrollMath'
import { computeResyncScrollPx } from './resyncAnchor'

export interface CellRun {
  text: string
  fg: number | null
  bg: number | null
  bold: boolean
  italic: boolean
  underline: boolean
  inverse: boolean
  dim: boolean
  strikeout: boolean
  /** Present (true) on a row's LAST run when the row soft-wraps into
   *  the next one. Daemons that predate the field never send it. */
  wrapped?: boolean
  /** Terminal-column span, present only when it differs from the
   *  run's char count (double-width CJK/emoji, zero-width combining
   *  chars). Absent ⇒ one column per char. */
  cols?: number
}

export interface CursorSnapshot {
  row: number
  col: number
  visible: boolean
}

export interface TermGridSnapshot {
  paneId: string
  cols: number
  rows: number
  grid: CellRun[][]
  scrollback: CellRun[][]
  cursor: CursorSnapshot
  version: number
  displayOffset: number
  /** Sticky; daemon re-sends only on full snapshots. */
  mouseReport?: boolean
  sgrMouse?: boolean
  altScreen?: boolean
}

export interface DamagedRow {
  row: number
  runs: CellRun[]
}

export interface TermGridDelta {
  paneId: string
  cols: number
  rows: number
  damagedRows: DamagedRow[]
  scrollbackAppended: CellRun[][]
  cursor: CursorSnapshot
  version: number
  displayOffset: number
}

export type PendingFrame =
  | { kind: 'snapshot'; payload: TermGridSnapshot }
  | { kind: 'delta'; payload: TermGridDelta }

export interface ApplyFrameBatchInput {
  pending: PendingFrame[]
  live: TermGridSnapshot | null
  rendered: TermGridSnapshot | null
  scrollPx: number
  cellHeight: number
  resizeHoldActive: boolean
}

export interface ApplyFrameBatchResult {
  live: TermGridSnapshot | null
  /** True when a resize-hold parks a blank merge off-screen. */
  suppressRender: boolean
  scrollPx: number
  /** Highest applied version; 0 means no ack. */
  ackVersion: number
}

/** True when the visible grid has no non-whitespace ink. */
export function isGridEmpty(snap: TermGridSnapshot): boolean {
  for (const row of snap.grid) {
    for (const run of row) {
      if (run.text && run.text.trim().length > 0) return false
    }
  }
  return true
}

/** Merge a delta into a prior snapshot. Returns `prev` unchanged
 *  when no snapshot exists yet (delta-before-snapshot). */
export function mergeDelta(
  prev: TermGridSnapshot | null,
  delta: TermGridDelta,
): TermGridSnapshot | null {
  if (!prev) return prev
  const nextGrid: CellRun[][] = prev.grid.slice()
  while (nextGrid.length < delta.rows) nextGrid.push([])
  if (nextGrid.length > delta.rows) nextGrid.length = delta.rows
  for (const dr of delta.damagedRows) {
    if (dr.row < 0 || dr.row >= delta.rows) continue
    nextGrid[dr.row] = dr.runs
  }
  const nextScrollback =
    delta.scrollbackAppended.length > 0
      ? prev.scrollback.concat(delta.scrollbackAppended)
      : prev.scrollback
  return {
    paneId: prev.paneId,
    cols: delta.cols,
    rows: delta.rows,
    grid: nextGrid,
    scrollback: nextScrollback,
    cursor: delta.cursor,
    version: delta.version,
    displayOffset: delta.displayOffset,
    // Mode bits are snapshot-only; carry last-known across deltas.
    mouseReport: prev.mouseReport,
    sgrMouse: prev.sgrMouse,
    altScreen: prev.altScreen,
  }
}

/** Fold one coalesced flush: merge, re-anchor scroll, suppress a
 *  resize-hold blank, and report the ack version. */
export function applyFrameBatch(
  input: ApplyFrameBatchInput,
): ApplyFrameBatchResult {
  const { pending, live, rendered, scrollPx, cellHeight, resizeHoldActive } =
    input
  const ch = cellHeight || 20
  let next: TermGridSnapshot | null = live
  const prevSnap = next
  const prevTotal =
    (next?.scrollback.length ?? 0) + (next?.grid.length ?? 0)
  let appendedRows = 0
  let sawSnapshot = false
  let nextScrollPx = scrollPx
  for (const f of pending) {
    if (f.kind === 'snapshot') {
      sawSnapshot = true
      next = f.payload
    } else {
      appendedRows += f.payload.scrollbackAppended.length
      next = mergeDelta(next, f.payload)
    }
  }
  if (sawSnapshot) {
    const nextTotal =
      (next?.scrollback.length ?? 0) + (next?.grid.length ?? 0)
    appendedRows = Math.max(appendedRows, nextTotal - prevTotal)
    if (nextScrollPx > 0 && prevSnap && next) {
      const re = computeResyncScrollPx(prevSnap, next, nextScrollPx, ch)
      if (re !== null) {
        nextScrollPx = clampScrollPx(re, next.scrollback.length, ch)
        appendedRows = 0
      }
    }
  }
  if (appendedRows > 0 && nextScrollPx > 0) {
    const sbLen = next?.scrollback.length ?? 0
    nextScrollPx = anchorScrollPx(nextScrollPx, appendedRows, sbLen, ch)
  }
  const suppressRender = Boolean(
    resizeHoldActive &&
      rendered &&
      !isGridEmpty(rendered) &&
      next &&
      isGridEmpty(next),
  )
  let ackVersion = 0
  for (const f of pending) {
    if (f.payload.version > ackVersion) ackVersion = f.payload.version
  }
  return {
    live: next,
    suppressRender,
    scrollPx: nextScrollPx,
    ackVersion,
  }
}
