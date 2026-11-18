// Angle wrapping: modulo, never a loop, so huge or non-finite values return
// at once; ordinary angles wrap exactly as the drawing has always read them.

import { describe, expect, it } from 'vitest'
import { arcSweep, forwardSweep, normalizeAngle, TAU } from './angle'
import { turnDeg } from './edit/transform'
import { arcD, ellipsePoints } from './model/geom'

const EXTREMES = [1e308, -1e308, Number.MAX_VALUE, -Number.MAX_VALUE, NaN, Infinity, -Infinity]

describe('normalizeAngle', () => {
  it('lands in [0, TAU)', () => {
    expect(normalizeAngle(0)).toBe(0)
    expect(normalizeAngle(TAU)).toBe(0)
    expect(normalizeAngle(-TAU)).toBe(0)
    expect(normalizeAngle(1)).toBe(1)
    expect(normalizeAngle(-1)).toBeCloseTo(TAU - 1, 12)
    expect(normalizeAngle(TAU + 1)).toBeCloseTo(1, 12)
    expect(normalizeAngle(-1e-17)).toBe(0)
  })

  it('returns a finite angle for extreme values', () => {
    for (const v of EXTREMES) {
      const r = normalizeAngle(v)
      expect(r >= 0 && r < TAU).toBe(true)
    }
    expect(normalizeAngle(NaN)).toBe(0)
    expect(normalizeAngle(Infinity)).toBe(0)
    expect(normalizeAngle(-Infinity)).toBe(0)
  })
})

describe('arcSweep', () => {
  it('is (0, TAU]: no sweep or whole turns is a full circle', () => {
    expect(arcSweep(0)).toBe(TAU)
    expect(arcSweep(TAU)).toBe(TAU)
    expect(arcSweep(2 * TAU)).toBe(TAU)
    expect(arcSweep(-TAU)).toBe(TAU)
    expect(arcSweep(Math.PI / 2)).toBe(Math.PI / 2)
    expect(arcSweep(-Math.PI / 2)).toBeCloseTo((3 * Math.PI) / 2, 12)
    expect(arcSweep(TAU + 0.5)).toBeCloseTo(0.5, 12)
    expect(arcSweep(-1e-17)).toBe(TAU)
  })

  it('returns a finite sweep for extreme values', () => {
    for (const v of EXTREMES) {
      const r = arcSweep(v)
      expect(r > 0 && r <= TAU).toBe(true)
    }
  })
})

describe('forwardSweep', () => {
  it('wraps a backward sweep and keeps a forward one', () => {
    expect(forwardSweep(0)).toBe(TAU)
    expect(forwardSweep(-1)).toBeCloseTo(TAU - 1, 12)
    expect(forwardSweep(-3 * TAU - 1)).toBeCloseTo(TAU - 1, 9)
    expect(forwardSweep(1)).toBe(1)
    expect(forwardSweep(TAU)).toBe(TAU)
    expect(forwardSweep(1.5 * TAU)).toBe(1.5 * TAU)
    expect(forwardSweep(2 * TAU)).toBe(2 * TAU)
  })

  it('sheds whole turns past two, keeping the end angle and a full turn', () => {
    expect(forwardSweep(5.25 * TAU)).toBeCloseTo(1.25 * TAU, 9)
    expect(forwardSweep(4 * TAU)).toBe(2 * TAU)
  })

  it('returns a bounded sweep for extreme values', () => {
    for (const v of EXTREMES) {
      const r = forwardSweep(v)
      expect(r > 0 && r <= 2 * TAU).toBe(true)
    }
  })
})

describe('the drawing helpers on extreme angles', () => {
  it('arcD returns at once with finite path data', () => {
    const vals = [0, 1, 1e308, -1e308, Number.MAX_VALUE, -Number.MAX_VALUE]
    for (const a0 of vals)
      for (const a1 of vals) {
        const d = arcD([0, 0], 1, a0, a1)
        expect(d).toMatch(/^M/)
        expect(d).not.toMatch(/NaN|Infinity/)
      }
  })

  it('ellipsePoints returns a bounded set of finite points', () => {
    for (const [p0, p1] of [
      [0, 1e308],
      [1e308, 0],
      [-1e308, 1e308],
      [1e308, -1e308],
      [0, Number.MAX_VALUE],
    ]) {
      const { pts, full } = ellipsePoints([0, 0], [2, 0], 0.5, p0, p1)
      expect(pts.length).toBeLessThanOrEqual(2 * 72 + 1)
      expect(typeof full).toBe('boolean')
      for (const p of pts) expect(Number.isFinite(p[0]) && Number.isFinite(p[1])).toBe(true)
    }
  })

  it('keeps full circles and ellipses full, and partial ones partial', () => {
    expect(arcD([0, 0], 1, 0, 0)).toBe(arcD([0, 0], 1, 0, TAU))
    expect((arcD([0, 0], 1, 0, 0).match(/A/g) ?? []).length).toBe(2)
    expect((arcD([0, 0], 1, 0, Math.PI / 2).match(/A/g) ?? []).length).toBe(1)
    expect(arcD([0, 0], 1, 0, -Math.PI / 2)).toBe(arcD([0, 0], 1, 0, (3 * Math.PI) / 2))
    expect(ellipsePoints([0, 0], [2, 0], 0.5, 0, 0).full).toBe(true)
    expect(ellipsePoints([0, 0], [2, 0], 0.5, 0, TAU).full).toBe(true)
    expect(ellipsePoints([0, 0], [2, 0], 0.5, 1, 0).full).toBe(false)
    expect(ellipsePoints([0, 0], [2, 0], 0.5, 0, Math.PI).full).toBe(false)
    // Over a turn (but under two) still draws as before: every point, and full.
    const long = ellipsePoints([0, 0], [2, 0], 0.5, 0, 1.5 * TAU)
    expect(long.full).toBe(true)
    expect(long.pts.length).toBe(108 + 1)
  })

  it('turnDeg stays in (-180, 180] and a non-finite drag is no turn', () => {
    expect(turnDeg([0, 0], [1, 0], [0, 1], false)).toBe(90)
    expect(turnDeg([0, 0], [1, 0], [-1, 0], false)).toBe(180)
    expect(turnDeg([0, 0], [0, -1], [-1, 0.0001], false)).toBe(-90)
    expect(turnDeg([0, 0], [1, 0], [NaN, 1], false)).toBe(0)
    expect(turnDeg([0, 0], [1, 0], [Infinity, Infinity], false)).toBe(45)
  })
})
