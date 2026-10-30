// Trackpad / mouse gestures → device input (sim-gestures), fail loud:
//  - a flick's release velocity in points per second, none for a stop;
//  - wheel → a one-finger drag against the delta, inside the safe band,
//    lifting and gripping again at its edge;
//  - pinch and ⌥-drag → two fingers.

import { describe, expect, it } from 'vitest'
import {
  altDragFingers,
  cssToPoints,
  mirrorPoint,
  pinchBegin,
  pinchPoints,
  pinchWheelScale,
  releaseVelocity,
  safeBand,
  trimSamples,
  twoFingers,
  wheelBegin,
  wheelDeltaPx,
  wheelEnd,
  wheelMove,
  withVelocity,
} from './sim-gestures'

// 200 × 400 CSS px canvas showing a 400 × 800 pt screen: 2 pt per px.
const box = { left: 0, top: 0, width: 200, height: 400 }
const screen = { widthPt: 400, heightPt: 800 }

describe('flick velocity', () => {
  it('measures the release velocity over the last 100 ms, in points per second', () => {
    const samples = [
      { clientX: 100, clientY: 300, at: 0 },
      { clientX: 100, clientY: 280, at: 16 },
      { clientX: 105, clientY: 260, at: 32 },
    ]
    // 40 px up, 5 px right in 32 ms → 80 pt / 10 pt in 0.032 s.
    expect(releaseVelocity(samples, 40, box, screen)).toEqual({ vx: 312.5, vy: -2500 })
  })

  it('a stop before lifting, a slow drag, or one sample has no velocity', () => {
    const moving = [
      { clientX: 100, clientY: 300, at: 0 },
      { clientX: 100, clientY: 260, at: 32 },
    ]
    expect(releaseVelocity(moving, 32 + 60, box, screen)).toBeNull()
    expect(releaseVelocity([{ clientX: 1, clientY: 1, at: 0 }], 5, box, screen)).toBeNull()
    const slow = [
      { clientX: 100, clientY: 300, at: 0 },
      { clientX: 100, clientY: 299, at: 90 },
    ]
    expect(releaseVelocity(slow, 95, box, screen)).toBeNull()
  })

  it('old samples drop out of the window', () => {
    const s = [
      { clientX: 0, clientY: 0, at: 0 },
      { clientX: 0, clientY: 0, at: 150 },
    ]
    expect(trimSamples(s, 200)).toEqual([{ clientX: 0, clientY: 0, at: 150 }])
  })

  it('only a touch up carries it', () => {
    const v = { vx: 1, vy: -2000 }
    expect(withVelocity({ t: 'touch', phase: 'up', x: 1, y: 2 }, v)).toEqual({ t: 'touch', phase: 'up', x: 1, y: 2, vx: 1, vy: -2000 })
    expect(withVelocity({ t: 'touch', phase: 'move', x: 1, y: 2 }, v)).toEqual({ t: 'touch', phase: 'move', x: 1, y: 2 })
    expect(withVelocity({ t: 'tap', x: 1, y: 2 }, v)).toEqual({ t: 'tap', x: 1, y: 2 })
    expect(withVelocity({ t: 'touch', phase: 'up', x: 1, y: 2 }, null)).toEqual({ t: 'touch', phase: 'up', x: 1, y: 2 })
  })
})

describe('wheel → drag', () => {
  it('converts wheel units to CSS px and px to points', () => {
    expect(wheelDeltaPx({ deltaX: 0, deltaY: 3, deltaMode: 1 }, box)).toEqual({ dx: 0, dy: 48 })
    expect(wheelDeltaPx({ deltaX: 0, deltaY: 1, deltaMode: 2 }, box)).toEqual({ dx: 0, dy: 400 })
    expect(wheelDeltaPx({ deltaX: -7, deltaY: 12.5, deltaMode: 0 }, box)).toEqual({ dx: -7, dy: 12.5 })
    expect(cssToPoints({ dx: 10, dy: 25 }, box, screen)).toEqual({ dx: 20, dy: 50 })
  })

  it('the finger goes down inside the safe band, away from the edge gestures', () => {
    expect(safeBand(screen)).toEqual({ minX: 60, maxX: 340, minY: 160, maxY: 640 })
    expect(wheelBegin({ x: 200, y: 300 }, screen).send).toEqual([{ t: 'touch', phase: 'down', x: 200, y: 300 }])
    // Under the home indicator: the finger goes down above it.
    expect(wheelBegin({ x: 5, y: 790 }, screen).send).toEqual([{ t: 'touch', phase: 'down', x: 60, y: 640 }])
  })

  it('moves against the scroll delta (scrolling down drags the content up)', () => {
    const { drag } = wheelBegin({ x: 200, y: 400 }, screen)
    const r = wheelMove(drag, { dx: 0, dy: 100 }, screen)
    expect(r.send).toEqual([{ t: 'touch', phase: 'move', x: 200, y: 300 }])
    const back = wheelMove(r.drag, { dx: -30, dy: -50 }, screen)
    expect(back.send).toEqual([{ t: 'touch', phase: 'move', x: 230, y: 350 }])
    expect(wheelEnd(back.drag)).toEqual([{ t: 'touch', phase: 'up', x: 230, y: 350 }])
  })

  it('at the band edge it lifts and grips again on the far side, like scrolling a long list', () => {
    const { drag } = wheelBegin({ x: 200, y: 300 }, screen)
    const r = wheelMove(drag, { dx: 0, dy: 400 }, screen)
    expect(r.send).toEqual([
      { t: 'touch', phase: 'move', x: 200, y: 160 },
      { t: 'touch', phase: 'up', x: 200, y: 160 },
      { t: 'touch', phase: 'down', x: 200, y: 640 },
      { t: 'touch', phase: 'move', x: 200, y: 380 },
    ])
    expect(r.drag.finger).toEqual({ x: 200, y: 380 })
  })

  it('a huge delta grips at most a few times per frame, never loops', () => {
    const { drag } = wheelBegin({ x: 200, y: 400 }, screen)
    const r = wheelMove(drag, { dx: 0, dy: 1e9 }, screen)
    expect(r.send.filter((m) => m.t === 'touch' && m.phase === 'down')).toHaveLength(4)
    const all = r.send as { x: number; y: number }[]
    expect(all.every((m) => m.y >= 160 && m.y <= 640)).toBe(true)
  })
})

describe('pinch → two fingers', () => {
  it('starts a quarter of the short side apart around the pointer, and spreads with the scale', () => {
    const p = pinchBegin({ x: 200, y: 400 }, screen)
    expect(pinchPoints(p, screen)).toEqual([
      { x: 150, y: 400 },
      { x: 250, y: 400 },
    ])
    expect(pinchPoints({ ...p, scale: 2 }, screen)).toEqual([
      { x: 100, y: 400 },
      { x: 300, y: 400 },
    ])
    // Pinched in hard: the fingers keep 20 pt apart; spread wide: on screen.
    expect(pinchPoints({ ...p, scale: 0.01 }, screen)).toEqual([
      { x: 190, y: 400 },
      { x: 210, y: 400 },
    ])
    expect(pinchPoints({ ...p, scale: 50 }, screen)).toEqual([
      { x: 20, y: 400 },
      { x: 380, y: 400 },
    ])
  })

  it('Ctrl+wheel scales by exp(-dy / 100) per step', () => {
    const p = pinchBegin({ x: 200, y: 400 }, screen)
    expect(pinchWheelScale(p, -100).scale).toBeCloseTo(Math.E, 10)
    expect(pinchWheelScale(pinchWheelScale(p, 50), -50).scale).toBeCloseTo(1, 10)
  })

  it('⌥-drag mirrors the second finger around the centre', () => {
    expect(mirrorPoint({ x: 100, y: 300 }, screen)).toEqual({ x: 300, y: 500 })
    expect(altDragFingers('down', 50, 150, box, screen)).toEqual({ t: 'touch', phase: 'down', x: 100, y: 300, x2: 300, y2: 500 })
    expect(twoFingers('up', { x: 1.04, y: 2 }, { x: 3, y: 4.06 })).toEqual({ t: 'touch', phase: 'up', x: 1, y: 2, x2: 3, y2: 4.1 })
  })
})
