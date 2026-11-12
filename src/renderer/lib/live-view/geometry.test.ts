import { describe, expect, it } from 'vitest'
import { Axis, COL_PX, geometryOf, rectPx, ROW_PX } from './geometry'
import { play } from './test-helpers'
import { gridOf } from './workbook'

describe('Axis', () => {
  it('maps numbers to pixels and back, with hidden sizes of 0', () => {
    const a = new Axis(5, (n) => (n === 3 ? 0 : 10))
    expect([a.start(1), a.start(3), a.start(4), a.total]).toEqual([0, 20, 20, 40])
    expect([a.at(0), a.at(19), a.at(20), a.at(39), a.at(1000)]).toEqual([1, 2, 4, 5, 5])
  })
  it('windows 100,000 rows to the few on screen', () => {
    const a = new Axis(100_000, () => ROW_PX)
    const w = a.window(ROW_PX * 50_000, 400, 2)
    expect(w.first).toBe(50_001 - 2)
    expect(w.last - w.first).toBeLessThan(30)
  })
})

describe('geometryOf', () => {
  it('reads heights in points and widths in characters; the hidden row 17 is 0 high', () => {
    const g = gridOf(play().books[20], '@sheet:1')
    const G = geometryOf(g, { rows: 40, cols: 10 })
    expect(G.rows.size(1)).toBe(32)
    expect(G.rows.size(2)).toBe(ROW_PX)
    expect(G.rows.size(17)).toBe(0)
    expect(G.cols.size(1)).toBe(166)
    expect(G.cols.size(9)).toBe(COL_PX)
    expect(rectPx(G, 1, 1, 1, 2)).toMatchObject({ x: 0, y: 0, h: 32, clipB: false })
    expect(rectPx(G, 1, 1_048_576, 4, 4)).toMatchObject({ clipB: true })
  })
})
