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
 * Content-box size of `el`, matching ResizeObserver `contentRect` under
 * border-box + CSS padding:
 *   contentW = clientWidth − paddingLeft − paddingRight
 *   contentH = clientHeight − paddingTop − paddingBottom
 *
 * Callers MUST pass this (or RO contentRect) into {@link measurePaneFit},
 * never a border-box (`getBoundingClientRect`) — otherwise spawn and RO
 * disagree by ~padding and the first RO fire is not a no-op.
 */
export function contentBoxSize(
  el: HTMLElement | null | undefined,
): PaneFitRect | null {
  if (!el) return null
  // jsdom / detached: client* can be 0 → treat as unmeasurable.
  const cs = getComputedStyle(el)
  const pl = parseFloat(cs.paddingLeft) || 0
  const pr = parseFloat(cs.paddingRight) || 0
  const pt = parseFloat(cs.paddingTop) || 0
  const pb = parseFloat(cs.paddingBottom) || 0
  const width = el.clientWidth - pl - pr
  const height = el.clientHeight - pt - pb
  if (!(width > 0) || !(height > 0)) return null
  return { width, height }
}

/**
 * Compute terminal cols/rows that fit `rect` given cell metrics.
 *
 * **Callers must pass a content-box rect** — the same box ResizeObserver
 * reports as `contentRect` (see {@link contentBoxSize}). Do not pass
 * `getBoundingClientRect()` (border box): with global border-box sizing
 * and pane padding, that double-applies pad inside the −4 floor and
 * shifts cols/rows by ±1.
 *
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

/** Inputs for the shared font-probe used by layout + measure-first spawn. */
export interface ProbeCellMetricsInput {
  fontFamily: string
  fontSize: number
  useWebgl: boolean
  dpr: number
  charTracking: number
  /** WebGL path line-height multiplier (settings store). */
  lineHeightMultiplier: number
  /** DOM path line-height multiplier (`config.font`). */
  configLineHeightMultiplier: number
}

/**
 * One-shot font probe for cell width/height. Shared by TerminalPane's
 * layout effect and measure-first spawn boot so the race path and the
 * steady-state path cannot drift.
 */
export function probeCellMetrics(
  input: ProbeCellMetricsInput,
): { width: number; height: number } {
  if (typeof document === 'undefined') {
    return { width: 0, height: 0 }
  }
  const span = document.createElement('span')
  span.style.cssText = `font-family: ${input.fontFamily}; font-size: ${input.fontSize}px; position: absolute; visibility: hidden; white-space: pre;`
  span.textContent = 'W'
  document.body.appendChild(span)
  const rect = span.getBoundingClientRect()
  document.body.removeChild(span)

  if (!input.useWebgl) {
    return {
      width: rect.width,
      height: Math.max(
        1,
        Math.ceil(input.fontSize * input.configLineHeightMultiplier),
      ),
    }
  }
  const dpr = input.dpr > 0 ? input.dpr : 1
  const measured = Math.floor(rect.width * dpr) / dpr
  const tracked = Math.max(0.5, measured * input.charTracking)
  return {
    width: Math.max(1 / dpr, Math.floor(tracked * dpr) / dpr),
    height: Math.max(1, Math.ceil(input.fontSize * input.lineHeightMultiplier)),
  }
}
