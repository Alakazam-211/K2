// Turning and scaling a selection: both about the middle of the box around
// it; the turn a drag sweeps (Shift: 15° steps) and the factor a corner drag
// makes (Shift: 0.25 steps); where the handles sit and which one a press grabs.

import { describe, expect, it } from 'vitest'
import { centreOf, handleAt, handlePoints, normDeg, selectionBox, sizeFactor, sizePoint, turnDeg, turnPoint } from './transform'

describe('the selection’s centre', () => {
  it('is the middle of the box around everything selected', () => {
    const b = selectionBox([{ bbox: [0, 0, 10, 0] }, { bbox: [4, -2, 6, 8] }, { bbox: null }])
    expect(b).toEqual([0, -2, 10, 8])
    expect(centreOf(b!)).toEqual([5, 3])
    expect(selectionBox([{ bbox: null }])).toBeNull()
  })
})

describe('turning', () => {
  const c: [number, number] = [5, 3]
  it('a drag turns by the angle it sweeps about the centre, anticlockwise positive', () => {
    expect(turnDeg(c, [5, 13], [-5, 3], false)).toBe(90)
    expect(turnDeg(c, [5, 13], [15, 3], false)).toBe(-90)
    expect(turnDeg(c, [5, 13], [5, -7], false)).toBe(180)
    // Past half a turn reads the short way round.
    expect(turnDeg(c, [15, 3], [5 + 10 * Math.cos(-2.5), 3 + 10 * Math.sin(-2.5)], false)).toBe(-143.2)
  })
  it('to a tenth of a degree, or Shift’s 15° steps', () => {
    const at = (d: number): [number, number] => [5 + 10 * Math.cos(((90 + d) * Math.PI) / 180), 3 + 10 * Math.sin(((90 + d) * Math.PI) / 180)]
    expect(turnDeg(c, at(0), at(37.2834), false)).toBe(37.3)
    expect(turnDeg(c, at(0), at(37.2834), true)).toBe(30)
    expect(turnDeg(c, at(0), at(38), true)).toBe(45)
    expect(turnDeg(c, at(0), at(-7), true)).toBe(0)
  })
  it('the verb takes 0 up to 360', () => {
    expect(normDeg(-90)).toBe(270)
    expect(normDeg(450)).toBe(90)
    expect(normDeg(360)).toBe(0)
    expect(normDeg(-0.1)).toBe(359.9)
  })
  it('a point turns about the centre', () => {
    const p = turnPoint([15, 3], c, 90)
    expect(p[0]).toBeCloseTo(5, 9)
    expect(p[1]).toBeCloseTo(13, 9)
  })
})

describe('scaling', () => {
  const c: [number, number] = [0, 0]
  it('a corner drag scales by the pointer’s distance from the centre against where it began, the same both ways', () => {
    expect(sizeFactor(c, [3, 4], [6, 8], false)).toBe(2)
    expect(sizeFactor(c, [3, 4], [-1.5, 2], false)).toBe(0.5)
    expect(sizeFactor(c, [3, 4], [3.3, 4.4], false)).toBe(1.1)
  })
  it('to hundredths, or Shift’s 0.25 steps; never 0', () => {
    expect(sizeFactor(c, [3, 4], [3.3, 4.4], true)).toBe(1)
    expect(sizeFactor(c, [3, 4], [4.1, 5.5], true)).toBe(1.25)
    expect(sizeFactor(c, [3, 4], [0, 0], true)).toBe(0.25)
    expect(sizeFactor(c, [3, 4], [0, 0], false)).toBe(0.01)
  })
  it('a point scales about the centre', () => {
    expect(sizePoint([7, 3], [5, 3], 2)).toEqual([9, 3])
  })
})

describe('handles', () => {
  const b: [number, number, number, number] = [0, 0, 100, 50]
  it('the turn handle sits above the box, the scale handles on its corners, a few pixels out', () => {
    const h = handlePoints(b, 0.5)
    expect(h.turn).toEqual([50, 50 + 2.5 + 12])
    expect(h.corners).toEqual([[-2.5, -2.5], [102.5, -2.5], [102.5, 52.5], [-2.5, 52.5]])
  })
  it('a press near one grabs it; the turn handle first; elsewhere nothing', () => {
    expect(handleAt(b, [51, 64], 0.5)).toEqual({ kind: 'turn', at: [50, 64.5] })
    expect(handleAt(b, [103, 53], 0.5)).toEqual({ kind: 'size', corner: 2, at: [102.5, 52.5] })
    expect(handleAt(b, [50, 25], 0.5)).toBeNull()
  })
})
