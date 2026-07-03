// Seam color — iTerm-style "extend background color to edges".
//
// A fullscreen TUI paints its own background per-cell over exactly
// cols×rows, but the pane's pixel size rarely divides evenly by the
// cell size: the quantization remainder (right edge + bottom edge, up
// to one cell each plus rounding) shows the pane container's THEME
// background instead of the TUI's, producing a visible seam. Fix:
// examine the boundary cells of the visible grid and, when a strict
// majority of them share one explicit background color, paint the
// container with that color so the remainder blends into the TUI.
//
// Pure and allocation-light — this runs once per committed frame.

/** The minimal slice of a CellRun this module reads. TerminalPane's
 *  wire `CellRun` (and gridWire's `WireCellRun`) are structurally
 *  assignable. */
export interface SeamRun {
  text: string
  fg: number | null
  bg: number | null
  inverse: boolean
  /** Terminal-column span, present only when it differs from the
   *  run's char count (wide/zero-width content). Absent ⇒ one column
   *  per char. */
  cols?: number
}

/** Terminal-column span of one run. `text.length` counts UTF-16 code
 *  units, but runs where columns differ from the char count carry an
 *  explicit `cols` — for the rest this is exact enough (seam picking
 *  weights, it doesn't position). */
function runSpan(run: SeamRun): number {
  return run.cols ?? run.text.length
}

/** Background color a run actually paints, or null for the terminal
 *  default. Inverse swaps fg into the background (runStyle parity);
 *  an inverse run with null fg paints the theme's default FOREGROUND
 *  — not a color we can name here, so it counts as non-explicit. */
function effectiveBg(run: SeamRun): number | null {
  return run.inverse ? run.fg : run.bg
}

/**
 * Pick the color the pane container should use to fill the sub-cell
 * remainder at the right/bottom edges of the grid, or null to keep
 * the theme background.
 *
 * Boundary sample = every cell of the bottom row + the right-edge
 * cell of every other row (weighted by cell count via run col-spans).
 * Returns a color only when a STRICT majority (>50%) of the examined
 * boundary cells share one explicit (non-null) background — a plain
 * shell (mostly default backgrounds) always returns null.
 *
 * `rows` should be the rows actually being rendered (the visible
 * strip), so scrollback scrolling follows what's on screen. `cols`,
 * when provided, marks rows whose runs span fewer than `cols` columns
 * as ending in default-background cells (the daemon trims trailing
 * blanks); without it the last run is assumed to reach the right
 * edge.
 *
 * O(total runs), one small Map allocation per call.
 */
export function pickSeamColor(
  rows: ReadonlyArray<ReadonlyArray<SeamRun>>,
  cols?: number,
): number | null {
  const n = rows.length
  if (n === 0) return null

  const counts = new Map<number, number>()
  let total = 0
  const tally = (color: number | null, cells: number): void => {
    if (cells <= 0) return
    total += cells
    if (color !== null) counts.set(color, (counts.get(color) ?? 0) + cells)
  }

  // Bottom row: every cell, weighted by run span. Cells past the last
  // run (trimmed trailing blanks) are default-background.
  const bottom = rows[n - 1]
  let bottomSpan = 0
  for (const run of bottom) {
    const span = runSpan(run)
    bottomSpan += span
    tally(effectiveBg(run), span)
  }
  if (cols !== undefined && bottomSpan < cols) tally(null, cols - bottomSpan)

  // Right-edge cell of every other row (the bottom row's corner is
  // already in its full-row tally). A row that doesn't reach `cols`
  // ends in trimmed default cells → its right edge is default.
  for (let r = 0; r < n - 1; r++) {
    const row = rows[r]
    if (row.length === 0) {
      tally(null, 1)
      continue
    }
    if (cols !== undefined) {
      let span = 0
      for (const run of row) span += runSpan(run)
      if (span < cols) {
        tally(null, 1)
        continue
      }
    }
    tally(effectiveBg(row[row.length - 1]), 1)
  }

  if (total === 0) return null
  let best: number | null = null
  let bestCount = 0
  for (const [color, count] of counts) {
    if (count > bestCount) {
      best = color
      bestCount = count
    }
  }
  // Strict majority: exactly half (e.g. a vertically split TUI) is
  // ambiguous → keep the theme background.
  return bestCount * 2 > total ? best : null
}
