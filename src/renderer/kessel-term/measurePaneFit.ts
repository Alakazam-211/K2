// Pure pane-fit math for TerminalPane spawn + ResizeObserver.
//
// Matches the ResizeObserver path in TerminalPane (avail − 4px padding,
// floor to cell metrics, reject grids below MIN_FIT_*). Extracted so
// spawn can POST the same cols/rows the observer would emit — never the
// historical toy 120×40 when the pane box is measurable.

export interface PaneFitRect {
  width: number
  height: number
}

export interface PaneFit {
  cols: number
  rows: number
}

/** Minimum grid accepted by the ResizeObserver path (matches TerminalPane). */
export const MIN_FIT_COLS = 10
export const MIN_FIT_ROWS = 3

/**
 * Fallback ONLY when the pane box is unmeasurable (0×0 / hidden /
 * cell metrics not ready) and no last-known fit exists.
 *
 * Classic VT default 80×24 — deliberately NOT 120×40 (the old toy
 * happy-path that caused first-snapshot → full-window reflow churn).
 * Name is explicit so callers do not treat this as a measured fit.
 */
export const FALLBACK_SPAWN_COLS = 80
export const FALLBACK_SPAWN_ROWS = 24

/**
 * Compute terminal cols/rows that fit `rect` given cell metrics.
 * Same math as TerminalPane's ResizeObserver:
 *   availW = max(0, rect.width - 4)
 *   availH = max(0, rect.height - 4)
 *   cols = floor(availW / cellWidth), rows = floor(availH / cellHeight)
 * Returns null when inputs are invalid or the fit is below MIN_FIT_*.
 */
export function measurePaneFit(
  rect: PaneFitRect | null | undefined,
  cellWidth: number,
  cellHeight: number,
): PaneFit | null {
  if (!rect) return null
  if (!(cellWidth > 0) || !(cellHeight > 0)) return null
  if (!(rect.width > 0) || !(rect.height > 0)) return null

  const availW = Math.max(0, rect.width - 4)
  const availH = Math.max(0, rect.height - 4)
  const cols = Math.floor(availW / cellWidth)
  const rows = Math.floor(availH / cellHeight)

  if (cols < MIN_FIT_COLS || rows < MIN_FIT_ROWS) return null
  return { cols, rows }
}
