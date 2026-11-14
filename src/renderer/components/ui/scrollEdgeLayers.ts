// The layers of a soft scroll edge: what a list fades and blurs under as it
// passes a bar. One place for the numbers, so the Thread's edges (ThreadZenKit,
// globals.css) and the other lists that fade the same way (ScrollEdgeFade:
// the Home nav) never drift apart. A leaf: no imports, so anything may read it
// at load without a cycle.

// ── Terminal: the Thread kit's strip behind the message box ─────────────

/** The frosted tint: nearly clear at the top, stronger toward the bottom
 *  ([% of the glass's height from its top, % of the background]). */
export const FOOTER_TINT_STOPS: ReadonlyArray<readonly [number, number]> = [
  [0, 0],
  [20, 6],
  [45, 18],
  [70, 32],
  [100, 46],
]

/** Reduced transparency (no blur): the gradient goes nearly opaque within
 *  6px of the glass's top ([px from the top, % of the background]). */
export const FOOTER_TINT_STOPS_OPAQUE: ReadonlyArray<readonly [number, number]> = [
  [0, 0],
  [1, 20],
  [2, 44],
  [3, 66],
  [4, 84],
  [6, 95],
]

/** The blur layers, light to heavy: [blur px, mask clear until %, fully on
 *  at %] of the glass's height from its top. Each starts before the one
 *  above it is fully on, so the masks overlap. */
export const FOOTER_BLUR_LAYERS: ReadonlyArray<readonly [number, number, number]> = [
  [1, 0, 12],
  [4, 8, 35],
  [10, 28, 60],
  [20, 52, 80],
]

/** Outside Glass, the Thread's header: the conversation fades as it passes
 *  under, the page's own colour near opaque at the bar ([% from the bar, %
 *  of the colour]; globals.css `.thread-float-scrim`). */
export const SCRIM_TINT_STOPS: ReadonlyArray<readonly [number, number]> = [
  [0, 97],
  [55, 88],
  [100, 0],
]

// ── Glass: the phone's header stack (globals.css `.k2-header-blur`) ─────

/** The band's height (px): `.k2-header-blur`. */
export const GLASS_BAND_PX = 40

/** The phone's four blurs, light to heavy: [blur px, solid until %, clear
 *  at %] of the band from the bar (`.k2-header-blur-1` … `-4`). */
export const GLASS_BLUR_LAYERS: ReadonlyArray<readonly [number, number, number]> = [
  [2, 55, 100],
  [6, 40, 80],
  [12, 25, 60],
  [24, 12, 40],
]

/** The phone's tint over them: [% from the bar, % of the colour]
 *  (`.k2-header-tint`). */
export const GLASS_TINT_STOPS: ReadonlyArray<readonly [number, number]> = [
  [0, 85],
  [45, 55],
  [100, 0],
]

/** Glass with no blur (`data-frost="tile"`: reduced transparency, or no
 *  backdrop-filter): the heavier ramp that keeps text off the rows. */
export const GLASS_TINT_STOPS_TILE: ReadonlyArray<readonly [number, number]> = [
  [0, 96],
  [60, 88],
  [100, 0],
]
