// Angle wrapping for the drawing's arcs and ellipses, by modulo rather than
// by adding a turn in a loop. A loop never ends on a huge value (1e308 + 2π
// is still 1e308) and spins on NaN or Infinity; these always return at once
// with a finite number.

export const TAU = 2 * Math.PI

/** An angle (radians) in [0, TAU). A non-finite angle is 0. */
export function normalizeAngle(a: number): number {
  if (!Number.isFinite(a)) return 0
  const r = a % TAU
  const out = r < 0 ? r + TAU : r
  // A tiny negative remainder plus a turn rounds to TAU itself; -0 is 0.
  return out >= TAU || out === 0 ? 0 : out
}

/** An arc's counter-clockwise sweep in (0, TAU]: no sweep, or whole turns,
 *  is a full circle (the drawing's arc rule). A non-finite sweep is a full
 *  circle. */
export function arcSweep(span: number): number {
  if (!Number.isFinite(span)) return TAU
  const s = normalizeAngle(span)
  return s === 0 ? TAU : s
}

/** A sweep that runs forward from its start, as an ellipse or a hatch arc
 *  reads it: one at or before its start wraps on into (0, TAU]; one past its
 *  start stays as it is up to two turns. A longer one sheds whole turns into
 *  (TAU, 2·TAU], so it ends at the same angle, still covers a full turn, and
 *  its sample count stays bounded. A non-finite sweep is a full turn. */
export function forwardSweep(span: number): number {
  if (!Number.isFinite(span)) return TAU
  if (span <= 0) return arcSweep(span)
  if (span <= 2 * TAU) return span
  return TAU + arcSweep(span)
}
