// Labels on the picture never touch: not each other, not the stage's own
// chrome (the view picker, compass, chips, key hint, ⓘ), and never past the
// picture's edges. Placement is measured sizes in, positions out: a tag
// sits above its box's top-left, flips to its top-right, then below it,
// then slides up or down; a selection label hangs under its point; an
// off-screen chip slides along its edge. 6 px between any two.

import { describe, expect, it } from 'vitest'
import { LABEL_GAP, hit, placeLabels, touchingPairs, type LabelReq, type Rect } from './labels'

const R = (x0: number, y0: number, x1: number, y1: number): Rect => ({ x0, y0, x1, y1 })
const FRAME = R(0, 0, 800, 450)
const tag = (id: string, anchor: Rect, w = 120, h = 22, prio = 1): LabelReq => ({ id, kind: 'tag', anchor, w, h, prio })
const box = (p: { x: number; y: number; w: number; h: number }): Rect => R(p.x, p.y, p.x + p.w, p.y + p.h)

/** Every pair, and every label against every obstacle, at least `gap` apart. */
function apart(placed: { x: number; y: number; w: number; h: number }[], obstacles: Rect[] = [], gap = LABEL_GAP): void {
  const rs = placed.map(box)
  expect(touchingPairs(rs, obstacles, gap)).toEqual([])
}

describe('placing labels', () => {
  it('puts a tag just above its box’s top-left, 6 px clear', () => {
    const [p] = placeLabels([tag('t', R(200, 200, 320, 300))], { frame: FRAME })
    expect(p).toMatchObject({ id: 't', x: 200, y: 200 - LABEL_GAP - 22, touching: false })
  })

  it('keeps a tag inside the picture: flips to the box’s right edge, then clamps', () => {
    const [p] = placeLabels([tag('t', R(740, 120, 790, 200), 160)], { frame: FRAME })
    expect(p.x + p.w).toBeLessThanOrEqual(FRAME.x1 - 4)
    expect(p.x).toBe(790 - 160) // the box's right edge
    const [q] = placeLabels([tag('t', R(20, 5, 80, 60))], { frame: FRAME })
    expect(q.y).toBeGreaterThanOrEqual(4) // no room above: below or clamped, never past the top
  })

  it('moves the second of two colliding tags clear of the first', () => {
    const placed = placeLabels([tag('a', R(100, 200, 260, 280)), tag('b', R(110, 205, 250, 260))], { frame: FRAME })
    expect(placed.every((p) => !p.touching)).toBe(true)
    apart(placed)
  })

  it('places higher priority first: the live change keeps its spot', () => {
    const placed = placeLabels([tag('quiet', R(100, 200, 260, 280), 120, 22, 5), tag('live', R(100, 200, 260, 280), 120, 22, 0)], { frame: FRAME })
    const live = placed.find((p) => p.id === 'live')!
    expect(live).toMatchObject({ x: 100, y: 200 - LABEL_GAP - 22 })
    apart(placed)
  })

  it('keeps clear of the stage’s chrome', () => {
    const picker = R(0, 160, 240, 200) // where the tag would go
    const placed = placeLabels([tag('t', R(100, 200, 260, 280))], { frame: FRAME, obstacles: [picker] })
    expect(placed[0].touching).toBe(false)
    apart(placed, [picker])
  })

  it('a crowd of twelve tags on one spot: none touch', () => {
    const reqs = Array.from({ length: 12 }, (_, i) => tag(`t${i}`, R(300 + i * 4, 220, 380 + i * 4, 300), 110 + i * 3))
    const placed = placeLabels(reqs, { frame: FRAME, obstacles: [R(0, 0, 230, 40), R(0, 400, 800, 450)] })
    expect(placed.filter((p) => p.touching)).toEqual([])
    apart(placed, [R(0, 0, 230, 40), R(0, 400, 800, 450)])
    for (const p of placed) {
      expect(p.x).toBeGreaterThanOrEqual(4)
      expect(p.y).toBeGreaterThanOrEqual(4)
      expect(p.x + p.w).toBeLessThanOrEqual(796)
      expect(p.y + p.h).toBeLessThanOrEqual(446)
    }
  })

  it('a selection label hangs centred under its point, else above it', () => {
    const [p] = placeLabels([{ id: 's', kind: 'point', anchor: R(400, 300, 400, 300), w: 60, h: 20, prio: 4 }], { frame: FRAME })
    expect(p).toMatchObject({ x: 370, y: 300 + LABEL_GAP })
    const [q] = placeLabels([{ id: 's', kind: 'point', anchor: R(400, 440, 400, 440), w: 60, h: 20, prio: 4 }], { frame: FRAME })
    expect(q.y + q.h).toBeLessThanOrEqual(440 - LABEL_GAP)
  })

  it('an edge chip slides along its edge, past whatever sits there', () => {
    const compass = R(18, 380, 62, 424)
    const req: LabelReq = { id: 'c', kind: 'edge', side: 'left', anchor: R(0, 400, 0, 400), w: 90, h: 26, prio: 0 }
    const [p] = placeLabels([req], { frame: R(0, 0, 800, 450), pad: 18, obstacles: [compass] })
    expect(p.x).toBe(18)
    expect(p.touching).toBe(false)
    apart([p], [compass])
    const [q] = placeLabels([{ ...req, side: 'right', anchor: R(800, 100, 800, 100) }], { frame: R(0, 0, 800, 450), pad: 18 })
    expect(q.x + q.w).toBe(800 - 18)
    expect(q.y).toBe(100 - 13)
  })

  it('a frame per label: chips use the stage, tags the picture', () => {
    const picture = R(100, 50, 700, 400)
    const placed = placeLabels(
      [
        { id: 'chip', kind: 'edge', side: 'top', anchor: R(400, 0, 400, 0), w: 80, h: 26, prio: 0, frame: R(0, 0, 800, 450) },
        tag('t', R(120, 60, 200, 120)),
      ],
      { frame: picture, pad: 4 },
    )
    expect(placed.find((p) => p.id === 'chip')!.y).toBe(4)
    expect(placed.find((p) => p.id === 't')!.y).toBeGreaterThanOrEqual(54)
  })

  it('says so when there is no room at all, and still keeps it inside', () => {
    const tiny = R(0, 0, 130, 30)
    const placed = placeLabels([tag('a', R(0, 0, 130, 30)), tag('b', R(0, 0, 130, 30))], { frame: tiny })
    expect(placed.some((p) => p.touching)).toBe(true)
  })

  it('hit and touchingPairs measure gaps exactly', () => {
    expect(hit(R(0, 0, 10, 10), R(10, 0, 20, 10))).toBe(false) // edges meet: not overlapping
    expect(touchingPairs([R(0, 0, 10, 10), R(15, 0, 25, 10)], [], 6)).toEqual([[0, 1]])
    expect(touchingPairs([R(0, 0, 10, 10), R(16, 0, 26, 10)], [], 6)).toEqual([])
    expect(touchingPairs([R(0, 0, 10, 10)], [R(12, 0, 20, 10)], 6)).toEqual([[0, 'obstacle']])
  })
})
