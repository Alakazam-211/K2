// Selection → clipboard text, rebuilt purely from the CellRun model.
// ONE implementation serves both paths:
//   - DOM strip: native selection → `data-abs-row` + Range-measured
//     UTF-16 offsets (TerminalPane.handleCopy) → buildCopyText.
//   - WebGL painter: grid-coordinate selection model → column
//     boundaries converted via the cluster math below → buildCopyText.
// Semantics (unchanged from the original handleCopy loop):
//   - per-line trailing whitespace is trimmed (padded-row wire format
//     otherwise hands out phantom spaces),
//   - empty rows contribute a bare newline,
//   - soft-wrapped rows join WITHOUT a newline and WITHOUT trim (the
//     break is mid-content; daemon marks the last run `wrapped`).

import type { WireCellRun } from './gridWire'
import { rowClusters } from './runCols'

export interface CopyGrid {
  grid: WireCellRun[][]
  scrollback: WireCellRun[][]
}

function rowToText(row: WireCellRun[]): string {
  let out = ''
  for (const run of row) out += run.text
  return out
}

function isRowWrapped(row: WireCellRun[]): boolean {
  return row.length > 0 && row[row.length - 1].wrapped === true
}

export function modelRowAt(snap: CopyGrid, abs: number): WireCellRun[] {
  if (abs < 0) return []
  if (abs < snap.scrollback.length) return snap.scrollback[abs] ?? []
  return snap.grid[abs - snap.scrollback.length] ?? []
}

/** Build the copied text for an inclusive row range with UTF-16 TEXT
 *  offsets at the boundaries (the unit `Range.toString().length` and
 *  `String.slice` share). */
export function buildCopyText(
  snap: CopyGrid,
  startAbs: number,
  startOffset: number,
  endAbs: number,
  endOffset: number,
): string {
  let out = ''
  for (let abs = startAbs; abs <= endAbs; abs++) {
    const row = modelRowAt(snap, abs)
    const text = rowToText(row)
    const from = abs === startAbs ? startOffset : 0
    const to = abs === endAbs ? endOffset : text.length
    const seg = text.slice(from, Math.max(from, to))
    if (abs < endAbs && isRowWrapped(row)) {
      // Soft-wrap continuation — no newline, no trim (the break is
      // mid-content).
      out += seg
    } else {
      out += seg.replace(/\s+$/, '')
      if (abs < endAbs) out += '\n'
    }
  }
  return out
}

/** UTF-16 start boundary for a terminal-column start boundary: the
 *  first cluster any of whose columns is ≥ `col` (a start landing on
 *  the second half of a wide char includes the whole char). Columns
 *  past the row's content map to the text length (empty segment). */
export function colToTextStart(row: WireCellRun[], col: number): number {
  const clusters = rowClusters(row)
  for (const c of clusters) {
    if (c.colStart + c.width > col) return c.utf16Start
  }
  const last = clusters[clusters.length - 1]
  return last ? last.utf16End : 0
}

/** UTF-16 end boundary for an EXCLUSIVE terminal-column end boundary:
 *  past the last cluster that starts before `col` (an end landing
 *  inside a wide char includes it — xterm's wide-char end-inclusion). */
export function colToTextEnd(row: WireCellRun[], col: number): number {
  const clusters = rowClusters(row)
  let end = 0
  for (const c of clusters) {
    if (c.colStart < col) end = c.utf16End
    else break
  }
  return end
}

/** Copy text for a normalized grid-coordinate selection (the WebGL
 *  painter's model): convert the column boundaries of the head/tail
 *  rows to text offsets, then share buildCopyText with the DOM path. */
export function copySelectionText(
  snap: CopyGrid,
  sel: {
    startAbs: number
    startCol: number
    endAbs: number
    endCol: number
  },
): string {
  const start = colToTextStart(modelRowAt(snap, sel.startAbs), sel.startCol)
  const end = colToTextEnd(modelRowAt(snap, sel.endAbs), sel.endCol)
  return buildCopyText(snap, sel.startAbs, start, sel.endAbs, end)
}
