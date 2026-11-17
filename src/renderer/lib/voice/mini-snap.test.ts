// The floating call tile's motion maths (ported from K2 Companion): the
// corner a release goes to, the release velocity, the spring and the
// reduced-motion ease, and where the corners are on the desktop.

import { describe, expect, it } from 'vitest'
import {
  CORNERS,
  cornerSpots,
  EASE_DURATION,
  freeArea,
  pickCorner,
  planSnap,
  project,
  releaseVelocity,
  REST,
  SNAP_SPRING,
  springAt,
  type Corner,
  type Vec,
} from './mini-snap'

// A 1000 x 800 window, the tile's centre in each corner.
const HOMES: Record<Corner, Vec> = {
  'top-left': { x: 130, y: 120 },
  'top-right': { x: 870, y: 120 },
  'bottom-left': { x: 130, y: 680 },
  'bottom-right': { x: 870, y: 680 },
}

describe('pickCorner: position plus velocity to a corner', () => {
  it('a release with no velocity goes to the nearest corner', () => {
    expect(pickCorner({ x: 400, y: 300 }, { x: 0, y: 0 }, HOMES)).toBe('top-left')
    expect(pickCorner({ x: 600, y: 300 }, { x: 0, y: 0 }, HOMES)).toBe('top-right')
    expect(pickCorner({ x: 400, y: 500 }, { x: 0, y: 0 }, HOMES)).toBe('bottom-left')
    expect(pickCorner({ x: 600, y: 500 }, { x: 0, y: 0 }, HOMES)).toBe('bottom-right')
  })

  it('a slow release stays near: it coasts only a little', () => {
    // 100 px/s coasts about 25 px.
    expect(project(100)).toBeCloseTo(24.9, 1)
    expect(pickCorner({ x: 860, y: 670 }, { x: -100, y: -100 }, HOMES)).toBe('bottom-right')
  })

  it('a fling goes to the far corner', () => {
    // Let go right beside the bottom-right corner, flung up and left.
    expect(pickCorner({ x: 860, y: 670 }, { x: -2500, y: -2500 }, HOMES)).toBe('top-left')
    // Flung left only: the bottom-left corner.
    expect(pickCorner({ x: 860, y: 670 }, { x: -2500, y: 0 }, HOMES)).toBe('bottom-left')
    // Flung down from the top right: the bottom right.
    expect(pickCorner({ x: 860, y: 130 }, { x: 0, y: 2500 }, HOMES)).toBe('bottom-right')
  })
})

describe('releaseVelocity', () => {
  it('measures the last 100 ms of the drag', () => {
    const v = releaseVelocity([
      { t: 0, x: 0, y: 0 },
      { t: 500, x: 10, y: 0 },
      { t: 550, x: 60, y: 10 },
      { t: 600, x: 110, y: 20 },
    ])
    expect(v.x).toBeCloseTo(1000, 5)
    expect(v.y).toBeCloseTo(200, 5)
  })

  it('a pointer that had stopped, or one sample, has no velocity', () => {
    expect(releaseVelocity([{ t: 0, x: 0, y: 0 }])).toEqual({ x: 0, y: 0 })
    expect(releaseVelocity([
      { t: 0, x: 0, y: 0 },
      { t: 1, x: 50, y: 0 },
    ])).toEqual({ x: 0, y: 0 })
  })
})

describe('planSnap: the spring', () => {
  it('uses stiffness 400 and damping 30, overshoots a little and settles in about 0.4 s', () => {
    expect(SNAP_SPRING).toEqual({ mass: 1, stiffness: 400, damping: 30 })
    const plan = planSnap({ x: -300, y: 0, s: 1.04 }, { x: 0, y: 0 }, false)
    expect(plan.kind).toBe('spring')
    expect(plan.duration).toBeGreaterThan(0.3)
    expect(plan.duration).toBeLessThan(0.6)
    let most = -Infinity
    for (let t = 0; t <= plan.duration; t += 1 / 240) most = Math.max(most, plan.at(t).x)
    // About 3% overshoot past the corner (9 px of 300).
    expect(most).toBeGreaterThan(0)
    expect(most).toBeLessThan(15)
    expect(plan.at(plan.duration)).toEqual(REST)
  })

  it('carries the release velocity in', () => {
    const still = springAt(SNAP_SPRING, -200, 0, 0.02)
    const thrown = springAt(SNAP_SPRING, -200, 2000, 0.02)
    expect(thrown).toBeGreaterThan(still)
  })
})

describe('planSnap: reduced motion', () => {
  it('a plain 0.22 s ease-out: no overshoot, no velocity, always closer', () => {
    const plan = planSnap({ x: -300, y: 120, s: 1 }, { x: 4000, y: 4000 }, true)
    expect(plan.kind).toBe('ease')
    expect(plan.duration).toBe(EASE_DURATION)
    let prev = 300
    for (let t = 0; t <= plan.duration; t += 0.01) {
      const p = plan.at(t)
      expect(p.x).toBeLessThanOrEqual(0)
      expect(Math.abs(p.x)).toBeLessThanOrEqual(prev + 1e-9)
      prev = Math.abs(p.x)
    }
    expect(plan.at(EASE_DURATION)).toEqual(REST)
  })
})

describe('cornerSpots: where the corners are on the desktop', () => {
  const view = { width: 1000, height: 800 }
  const size = { width: 240, height: 200 }
  const insets = { top: 50, bottom: 12, side: 12, gap: 8 }

  it('keeps clear of the edges and the top bar', () => {
    expect(cornerSpots(view, size, insets)).toEqual({
      'top-left': { x: 12, y: 50 },
      'top-right': { x: 748, y: 50 },
      'bottom-left': { x: 12, y: 588 },
      'bottom-right': { x: 748, y: 588 },
    })
  })

  it('a bottom corner moves up above a composer, a top corner moves down below a bar', () => {
    const composer = { left: 600, top: 700, width: 400, height: 90 }
    const bar = { left: 0, top: 40, width: 300, height: 40 }
    const spots = cornerSpots(view, size, insets, [composer, bar])
    expect(spots['bottom-right']).toEqual({ x: 748, y: 700 - 8 - 200 })
    expect(spots['top-left']).toEqual({ x: 12, y: 40 + 40 + 8 })
    // The others weren't in the way.
    expect(spots['bottom-left']).toEqual({ x: 12, y: 588 })
    expect(spots['top-right']).toEqual({ x: 748, y: 50 })
  })

  it('moves past stacked boxes, and never off the window', () => {
    const a = { left: 700, top: 700, width: 300, height: 90 }
    const b = { left: 700, top: 560, width: 300, height: 100 }
    expect(cornerSpots(view, size, insets, [a, b])['bottom-right']).toEqual({ x: 748, y: 560 - 8 - 200 })
    const tall = { left: 0, top: 0, width: 1000, height: 800 }
    for (const c of CORNERS) {
      const s = cornerSpots(view, size, insets, [tall])[c]
      expect(s.y).toBeGreaterThanOrEqual(50)
      expect(s.y).toBeLessThanOrEqual(588)
    }
  })

  it('ignores boxes with no size (a hidden composer)', () => {
    expect(cornerSpots(view, size, insets, [{ left: 600, top: 700, width: 0, height: 0 }])['bottom-right']).toEqual({ x: 748, y: 588 })
  })
})

describe('freeArea + cornerSpots: the corners of the free content area (under the tabs, beside drawers)', () => {
  // A 1000 x 800 window: top bar 38, sidebar 200, content column beside it.
  const view = { width: 1000, height: 800 }
  const size = { width: 240, height: 200 }
  const insets = { top: 50, bottom: 12, side: 12, gap: 8 }
  const content = { left: 200, top: 38, width: 800, height: 762 }
  const tabs = { left: 200, top: 38, width: 800, height: 36 }

  it('with no content column, no drawers and no tabs: the whole window (as before)', () => {
    expect(freeArea(view, {})).toEqual({ left: 0, top: 0, right: 1000, bottom: 800 })
    expect(cornerSpots(view, size, insets, [], freeArea(view, {}))).toEqual(cornerSpots(view, size, insets))
  })

  it('the tab strip: the top corners rest just under it, never on it', () => {
    const area = freeArea(view, { content, tabs: [tabs] })
    expect(area).toEqual({ left: 200, top: 38 + 36, right: 1000, bottom: 800 })
    const spots = cornerSpots(view, size, insets, [], area)
    expect(spots['top-left']).toEqual({ x: 200 + 12, y: 38 + 36 + 8 })
    expect(spots['top-right']).toEqual({ x: 1000 - 12 - 240, y: 38 + 36 + 8 })
    expect(spots['bottom-left']).toEqual({ x: 212, y: 800 - 12 - 200 })
    // A taller strip pushes it further.
    expect(cornerSpots(view, size, insets, [], freeArea(view, { content, tabs: [{ ...tabs, height: 64 }] }))['top-right'].y).toBe(38 + 64 + 8)
  })

  it('a left drawer open: the left corners sit beside it', () => {
    const drawer = { left: 200, top: 38, width: 260, height: 762 }
    const area = freeArea(view, { drawers: [{ side: 'left', rect: drawer }], tabs: [{ left: 460, top: 38, width: 540, height: 36 }] })
    expect(area.left).toBe(460)
    const spots = cornerSpots(view, size, insets, [], area)
    expect(spots['top-left']).toEqual({ x: 460 + 12, y: 38 + 36 + 8 })
    expect(spots['bottom-left']).toEqual({ x: 460 + 12, y: 588 })
    // The right corners don't move.
    expect(spots['bottom-right']).toEqual({ x: 748, y: 588 })
  })

  it('a right drawer open: the right corners sit beside it', () => {
    const drawer = { left: 700, top: 38, width: 300, height: 762 }
    const area = freeArea(view, { content: { ...content, width: 500 }, drawers: [{ side: 'right', rect: drawer }], tabs: [tabs] })
    expect(area.right).toBe(700)
    const spots = cornerSpots(view, size, insets, [], area)
    expect(spots['bottom-right']).toEqual({ x: 700 - 12 - 240, y: 588 })
    expect(spots['top-right']).toEqual({ x: 700 - 12 - 240, y: 38 + 36 + 8 })
    expect(spots['top-left']).toEqual({ x: 212, y: 82 })
  })

  it('both drawers, and a closed drawer (no width) takes nothing', () => {
    const area = freeArea(view, {
      drawers: [
        { side: 'left', rect: { left: 0, top: 38, width: 250, height: 762 } },
        { side: 'right', rect: { left: 750, top: 38, width: 250, height: 762 } },
        { side: 'right', rect: { left: 1000, top: 38, width: 0, height: 762 } },
      ],
    })
    expect(area).toEqual({ left: 250, top: 0, right: 750, bottom: 800 })
  })

  it("a tab strip outside the area (a drawer's, or low in the window) doesn't move the top", () => {
    const area = freeArea(view, {
      drawers: [{ side: 'right', rect: { left: 700, top: 38, width: 300, height: 762 } }],
      tabs: [
        { left: 720, top: 38, width: 280, height: 36 },
        { left: 0, top: 600, width: 700, height: 36 },
      ],
    })
    expect(area.top).toBe(0)
  })

  it('composers are still avoided inside the area', () => {
    const composer = { left: 200, top: 700, width: 500, height: 90 }
    const area = freeArea(view, { content, drawers: [{ side: 'right', rect: { left: 700, top: 38, width: 300, height: 762 } }], tabs: [tabs] })
    expect(cornerSpots(view, size, insets, [composer], area)['bottom-right']).toEqual({ x: 448, y: 700 - 8 - 200 })
  })
})
