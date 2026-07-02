// Selection model math for the WebGL painter (pure, node-safe).
//
// Native browser selection cannot exist over a canvas, so the painter
// keeps a tiny model in ABSOLUTE buffer-row coordinates (xterm's
// SelectionModel shape): absolute rows survive scrolling and
// scrollback appends for free — K2 row indices never shift within a
// snapshot generation (full snapshot replace clears the selection
// upstream). `endCol` is exclusive.

import { rowClusters } from '../runCols'
import type { WireCellRun } from '../gridWire'
import type { SelectionRange } from './painterTypes'

export interface SelectionPoint {
  abs: number
  col: number
}

/** Normalize an (anchor, focus) drag pair: start ≤ end regardless of
 *  drag direction; null for a collapsed selection. */
export function normalizeSelection(
  anchor: SelectionPoint,
  focus: SelectionPoint,
): SelectionRange | null {
  if (anchor.abs === focus.abs && anchor.col === focus.col) return null
  const forward =
    anchor.abs < focus.abs ||
    (anchor.abs === focus.abs && anchor.col < focus.col)
  const s = forward ? anchor : focus
  const e = forward ? focus : anchor
  return { startAbs: s.abs, startCol: s.col, endAbs: e.abs, endCol: e.col }
}

/** Double-click word range at a column, in COLUMN space. Simple
 *  whitespace boundaries (accepted v1 scope; xterm's separator list
 *  can layer on later). Null when the column holds no non-space
 *  content. */
export function wordRangeAtCol(
  row: WireCellRun[],
  col: number,
): { startCol: number; endCol: number } | null {
  const clusters = rowClusters(row)
  if (clusters.length === 0) return null
  let text = ''
  for (const run of row) text += run.text
  const isSpace = (i: number): boolean =>
    /^\s*$/.test(text.slice(clusters[i].utf16Start, clusters[i].utf16End))

  let hit = -1
  for (let i = 0; i < clusters.length; i++) {
    const c = clusters[i]
    if (col >= c.colStart && col < c.colStart + c.width) {
      hit = i
      break
    }
  }
  if (hit === -1 || isSpace(hit)) return null

  let lo = hit
  while (lo > 0 && !isSpace(lo - 1)) lo--
  let hi = hit
  while (hi < clusters.length - 1 && !isSpace(hi + 1)) hi++
  return {
    startCol: clusters[lo].colStart,
    endCol: clusters[hi].colStart + clusters[hi].width,
  }
}
