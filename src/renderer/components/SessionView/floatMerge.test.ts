// Thread view's merged objects: the phone's liquid glass (Glass) and the
// joined bar (Terminal). The distance they merge at, which pieces take
// part, and the shapes they make.
import { describe, expect, it } from 'vitest'
import {
  MERGE_K,
  MERGE_SETTLE_MS,
  boxGap,
  chamferLoop,
  joinBar,
  mergeNeighbours,
  mergedDistance,
  sdBox,
  shapeFromBox,
  smin,
  unionLoops,
  unionPath,
  type Box,
  type GlassShape,
} from './floatMerge'

const circle = (cx: number, cy: number, r: number): GlassShape => ({ cx, cy, hx: r, hy: r, r })
const box = (x0: number, y0: number, x1: number, y1: number): Box => ({ x0, y0, x1, y1 })

describe('the merge distance', () => {
  it('is the phone’s 16px, and the masks wait for the row to be still', () => {
    expect(MERGE_K).toBe(16)
    expect(MERGE_SETTLE_MS).toBeGreaterThanOrEqual(100)
    expect(MERGE_SETTLE_MS).toBeLessThanOrEqual(250)
  })

  it('takes in the neighbours closer than 16px to the name’s object, no others', () => {
    const plate = box(100, 40, 300, 76)
    const others = [
      box(70, 8, 84.5, 39), // 15.5px left of it: in
      box(316, 8, 347, 40), // exactly 16px right of it: out
      box(10, 8, 41, 39), // far: out
      box(150, 8, 181, 41), // over its top edge: in
    ]
    expect(mergeNeighbours(plate, others)).toEqual([0, 3])
    expect(boxGap(plate, others[1])).toBe(16)
  })
})

describe('Glass: the phone’s SDF (src/zen/glassMerge.ts)', () => {
  it('rounded box: negative inside, zero on the edge, distance outside', () => {
    const s = circle(0, 0, 18)
    expect(sdBox(0, 0, s)).toBeCloseTo(-18)
    expect(sdBox(18, 0, s)).toBeCloseTo(0)
    expect(sdBox(30, 0, s)).toBeCloseTo(12)
    const pill: GlassShape = { cx: 0, cy: 0, hx: 100, hy: 25, r: 25 }
    expect(sdBox(0, 25, pill)).toBeCloseTo(0)
    expect(sdBox(110, 0, pill)).toBeCloseTo(10)
  })

  it('smin: a plain min when far apart, a deeper join within k', () => {
    expect(smin(5, 40, MERGE_K)).toBe(5)
    expect(smin(5, 5, MERGE_K)).toBeLessThan(5)
    expect(smin(3, 7, 0)).toBe(3)
  })

  it('the name grows a neck into a neighbour within 16px; neighbours never merge with each other', () => {
    const a = circle(0, 0, 15.5)
    const b = circle(37, 0, 15.5) // 6px apart, as a side's objects
    expect(mergedDistance(18.5, 0, [a, b], null)).toBeCloseTo(3)
    // The name's object 6px below a: the gap is bridged (inside the union).
    const near: GlassShape = { cx: 0, cy: 15.5 + 6 + 18, hx: 120, hy: 18, r: 18 }
    expect(mergedDistance(0, 18.5, [a], near)).toBeLessThan(0)
    // 30px below: no bridge.
    const far: GlassShape = { cx: 0, cy: 15.5 + 30 + 18, hx: 120, hy: 18, r: 18 }
    expect(mergedDistance(0, 30.5, [a], far)).toBeGreaterThan(0)
  })

  it('a shape’s radius comes from its corners: a pill’s 9999px is half its height', () => {
    expect(shapeFromBox(box(10, 20, 110, 51), 9999)).toEqual({ cx: 60, cy: 35.5, hx: 50, hy: 15.5, r: 15.5 })
    expect(shapeFromBox(box(0, 0, 40, 40), 6).r).toBe(6)
  })
})

describe('Terminal: the joined bar', () => {
  // The stacked header: the objects' row (y 12–40) over the name's object,
  // which starts on their bottom hairline (y 39).
  const row = (x0: number, x1: number): Box => box(x0, 12, x1, 40)
  const plate = box(200, 39, 440, 76)

  it('joins an object the name comes within 16px of, and snaps the name out under it', () => {
    const others = [row(150, 186), row(20, 144), row(560, 600)]
    const bar = joinBar(plate, others, { cut: 5 })
    expect(bar).not.toBeNull()
    // 14px away: joined. Its neighbour 6px further on joins with it, so a
    // side's objects join as one. The right one, 120px away, stays apart.
    expect(bar!.joined).toEqual([0, 1])
    expect(bar!.plate.x0).toBe(20)
    expect(bar!.plate.x1).toBe(440)
  })

  it('joins closer than 16px, as Glass merges', () => {
    expect(joinBar(plate, [row(150, 184)], { cut: 5 })).toBeNull()
    expect(joinBar(plate, [row(150, 185)], { cut: 5 })?.joined).toEqual([0])
  })

  it('draws one outline, square inside and cut at every outer corner, never rounded', () => {
    const bar = joinBar(plate, [row(150, 186), row(20, 144)], { cut: 5 })!
    expect(bar.path.match(/M/g)).toHaveLength(1)
    expect(bar.path).not.toMatch(/[ACQSTacqst]/)
    const [loop] = unionLoops(bar.rects)
    // An L: the objects' row over the left of the name's object.
    expect(loop).toHaveLength(6)
    const cut = chamferLoop(loop, 5)
    // Five outer corners cut (two points each), one inner corner square.
    expect(cut).toHaveLength(5 * 2 + 1)
  })

  it('one hairline where two pieces meet, not two borders', () => {
    const bar = joinBar(plate, [row(150, 186), row(20, 144)], { cut: 5 })!
    // Between the two objects: the 6px gap filled, a 1px line at its middle.
    const v = bar.dividers.filter((d) => d.x1 - d.x0 === 1)
    expect(v).toHaveLength(1)
    expect((v[0].x0 + v[0].x1) / 2).toBe(147)
    // Along the objects' bottom edge, across the run of them.
    const hz = bar.dividers.filter((d) => d.y1 - d.y0 === 1)
    expect(hz).toEqual([{ x0: 20, y0: 39, x1: 186, y1: 40 }])
    // The gap between them is part of the bar.
    expect(bar.rects).toContainEqual({ x0: 144, y0: 12, x1: 150, y1: 40 })
  })

  it('in one line, joins the objects either side into one segmented bar', () => {
    const name = box(200, 8, 330, 36)
    const left = box(184, 8, 192, 36) // 8px off, as the row's column gap
    const right = box(345, 8, 380, 36) // 15px off
    const bar = joinBar(name, [left, right], { cut: 5 })!
    expect(bar.joined).toEqual([0, 1])
    expect(bar.path.match(/M/g)).toHaveLength(1)
    // A straight bar: four corners, all cut.
    expect(unionLoops(bar.rects)[0]).toHaveLength(4)
    expect(bar.dividers.map((d) => (d.x0 + d.x1) / 2)).toEqual([196, 337.5])
    expect(joinBar(name, [box(347, 8, 380, 36)], { cut: 5 })).toBeNull()
  })

  it('a corner never cuts more than half an edge', () => {
    const path = unionPath([box(0, 0, 6, 40)], 5)
    expect(path).toBe('M0 3L3 0L3 0L6 3L6 37L3 40L3 40L0 37Z')
  })
})
