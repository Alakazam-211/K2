// ── Passive scale-to-fit + pin-to-size layout ─────────────────────
// (kessel-hard-learnings §2.7 / §Wave 3; presence PRD §5.5 for the
// pinned branch.)
//
// Extracted from TerminalPane's scaleLayout useMemo so the decision
// table is unit-testable (S7b). Pure: geometry in, transform out.
//
// When ANOTHER viewer owns the PTY size (this pane never claimed
// active, or lost the claim), the grid can be bigger than our box.
// The only lossless treatments of a width-committed grid are scale,
// letterbox or clip — NEVER re-wrap (1:1 grid row → display row is
// preserved: we scale the whole strip uniformly). Scale factor is
// min(fitW, fitH, 1), centered/letterboxed, floored (see below)
// after which we clip instead (unreadably small is worse than
// clipped). An active pane renders 1:1 — its resizes drive the PTY,
// so any mismatch is transient (the hold-and-scale path covers it).
//
// S7b pin-to-size: while the session is PINNED the daemon clamps
// every resize, so the incoming grid dims converge on the pinned
// grid for EVERY viewer — including the active one. The active
// viewer therefore loses its centered-1:1 privilege when the pinned
// grid overflows its box: it letterboxes like a passive viewer
// (else it would clip a size it cannot change). A pinned grid that
// FITS still renders centered 1:1. The scale floor also drops from
// 0.4 to 0.25 — a deliberate pin means the user prefers
// shrink-to-fit over clipping; the un-pinned passive path keeps 0.4.

/** Floor for the un-pinned passive scale-to-fit path. */
export const PASSIVE_SCALE_FLOOR = 0.4
/** Lower floor while pinned — shrink-to-fit was asked for. */
export const PINNED_SCALE_FLOOR = 0.25

export interface ScaleLayoutResult {
  scale: number
  offsetX: number
  offsetY: number
  /** Drives the "viewing at C×R" pill. Never true while pinned —
   *  the pin badge is the pinned-state affordance. */
  passive: boolean
}

export interface ScaleLayoutInput {
  /** Grid dims of the current snapshot (0 while none). */
  snapCols: number
  snapRows: number
  /** Measured cell metrics (0 until the probe ran). */
  cellWidth: number
  cellHeight: number
  /** Pane container box (border-box px). */
  containerWidth: number
  containerHeight: number
  /** This pane holds the active-viewer claim. */
  isActiveViewer: boolean
  /** Session is pinned to a fixed size (S7b). The daemon clamps all
   *  resizes while pinned, so snapCols/snapRows equal the pinned
   *  grid once the frames converge. */
  pinned: boolean
  /** Resize hold-and-scale: an emitted resize whose frames haven't
   *  landed yet (active path only). */
  pendingResize: { cols: number; rows: number } | null
}

const IDENTITY: ScaleLayoutResult = {
  scale: 1,
  offsetX: 0,
  offsetY: 0,
  passive: false,
}

export function computeScaleLayout(input: ScaleLayoutInput): ScaleLayoutResult {
  const {
    snapCols,
    snapRows,
    cellWidth: cw,
    cellHeight: ch,
    containerWidth,
    containerHeight,
    isActiveViewer,
    pinned,
    pendingResize,
  } = input
  if (!snapCols || !snapRows || !cw || !ch) return IDENTITY
  // Same available-box formula as the ResizeObserver's cols/rows
  // fit, so a grid sized to THIS pane always computes fit ≥ 1 and
  // renders unscaled. Width subtracts only the LEFT 4px pad — the
  // right edge is padding-free so the column math gets that width
  // (the centered remainder supplies the visual right breathing room).
  const availW = Math.max(0, containerWidth - 4)
  const availH = Math.max(0, containerHeight - 4)
  if (!availW || !availH) return IDENTITY
  const gridW = snapCols * cw
  const gridH = snapRows * ch
  const fit = Math.min(availW / gridW, availH / gridH)
  const letterboxed = (scale: number, passive: boolean): ScaleLayoutResult => ({
    scale,
    offsetX: Math.max(0, (availW - gridW * scale) / 2),
    offsetY: Math.max(0, (availH - gridH * scale) / 2),
    passive,
  })
  // Unscaled (scale-1) rendering centers the sub-cell quantization
  // remainder: cols = floor(availW / cellW) leaves up to one cell
  // of slack, and anchoring the grid at the left padding parked ALL
  // of it on the right — users read that as a bigger right gap than
  // left. Splitting it (floored to whole px so scale-1 text stays
  // on the pixel grid) makes the gutters symmetric. These offsets
  // ARE the content origin: the scale wrapper translates the DOM
  // strip AND the WebGL canvas by them, and toGridXY subtracts them
  // for every pointer→cell mapping (selection, link hover, SGR
  // forwarding), so grid pixels and hit-testing move together. The
  // cursor overlay + IME shadow textarea add them explicitly via
  // contentOriginX/Y. The remainder gutter shows the container's
  // own background (same fill as the 4px padding), so no seam
  // appears on either side.
  const centered: ScaleLayoutResult = {
    scale: 1,
    offsetX: Math.floor(Math.max(0, availW - gridW) / 2),
    offsetY: Math.floor(Math.max(0, availH - gridH) / 2),
    passive: false,
  }
  // S7b pinned branch — REGARDLESS of isActiveViewer (see header).
  // Wins over the pendingResize hold too: while pinned no resize is
  // emitted, so any stale hold must not stretch a clamped grid.
  if (pinned) {
    if (fit >= 1) return centered
    return letterboxed(Math.max(fit, PINNED_SCALE_FLOOR), false)
  }
  if (!isActiveViewer) {
    if (fit >= 1) return centered
    return letterboxed(Math.max(fit, PASSIVE_SCALE_FLOOR), true)
  }
  // Active pane, resize in flight (hold-and-scale): frames still
  // carry the OLD geometry — stretch the last grid to the new box
  // (scale may exceed 1 when the box grew) until the first frame at
  // the requested dims lands or the hold times out. This is what
  // turns the container-resize window from a flash into a smooth
  // reflow.
  if (
    pendingResize &&
    (snapCols !== pendingResize.cols || snapRows !== pendingResize.rows)
  ) {
    return letterboxed(Math.max(fit, PASSIVE_SCALE_FLOOR), false)
  }
  return centered
}
