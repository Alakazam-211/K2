// TerminalPainter seam — the contract between TerminalPane and the
// WebGL2 painter.
//
// The painter is a pure consumer sitting DOWNSTREAM of everything the
// pane already owns: WS lifecycle, `mergeDelta`, the rAF frame
// coalescer, wheel/keyboard/IME handlers, and all DOM overlays. It
// receives the merged snapshot + scroll position + selection + theme
// on React commits AND, while scrolling, directly from the pane's
// display-aligned wheel rAF (the vsync scroll pump — commit-cadence
// painting was the scroll-hop cause; docs/learnings/LEARNINGS-webgl-scroll.md). It
// never talks to the wire and never schedules its own frames —
// `render()` is the only entry point, which is what keeps the DOM
// strip a one-flag-flip fallback. Design:
// `.k2/notes/webgl-painter-brief.md` §6.

import type { WireCellRun } from '../gridWire'

/** Resolved theme colors as 0xRRGGBB (the wire's color unit). */
export interface PainterTheme {
  fg: number
  bg: number
  /** Selection rect tint; drawn translucent over the bg pass. */
  selection: number
  /**
   * Coverage-gamma exponent for tinted glyphs (`pow(coverage, gamma)`).
   * Live from the style store (per-style / per-scheme; Settings →
   * Styles) so the slider updates open WebGL tabs without remount.
   * Dev override `localStorage.K2SO_WEBGL_TEXT_GAMMA` still wins at
   * paint time.
   */
  textGamma: number
}

/** CSS-space font/cell measurements. The painter derives its own
 *  device-pixel grid from these (floor width / round height × dpr —
 *  brief §1.3's integer-device-grid discipline). */
export interface PainterMetrics {
  cssCellW: number
  cssCellH: number
  dpr: number
  fontFamily: string
  fontSize: number
}

/** Normalized selection in ABSOLUTE buffer-row coordinates (same
 *  space as `data-abs-row`): start ≤ end, `endCol` exclusive.
 *  Absolute coords survive scrolling and scrollback appends for free
 *  — K2 row indices never shift within a snapshot generation. */
export interface SelectionRange {
  startAbs: number
  startCol: number
  endAbs: number
  endCol: number
}

/** The subset of the merged TermGridSnapshot the painter reads.
 *  Structural, so TerminalPane's private snapshot type is assignable
 *  without an export churn. Row arrays are identity-stable across
 *  deltas (mergeDelta contract) — the painter's row cache keys on
 *  that identity as its damage test. */
export interface PainterGrid {
  cols: number
  rows: number
  grid: WireCellRun[][]
  scrollback: WireCellRun[][]
}

export interface PainterFrame {
  snapshot: PainterGrid
  /** Pixel scroll position (CSS px above buffer bottom) — the pane's
   *  `scrollPx` state, same unit `computeStripLayout` consumes. */
  scrollPx: number
  selection: SelectionRange | null
  theme: PainterTheme
}

export interface TerminalPainter {
  /** Attach to the canvas element the pane rendered. Creates the GL
   *  context; a missing WebGL2 fires `onFatal` (→ DOM fallback). */
  mount(canvas: HTMLCanvasElement): void
  /** Font/DPR change → rebuild the glyph atlas + device grid. */
  setMetrics(m: PainterMetrics): void
  /** Draw one frame. Idempotent; cheap when nothing changed. Grid
   *  dimension changes are detected here (no separate resize()). */
  render(frame: PainterFrame): void
  /** Context lost & unrestored, missing WebGL2, shader compile
   *  failure, pixel-readback sanity failure. Fires at most once; the
   *  pane responds by falling back to the DOM strip for its session. */
  onFatal(cb: (reason: string) => void): void
  dispose(): void
}
