// Zoom on a video tile: the scale's range, the point under the pointer
// staying put, the picture never leaving the frame (letterbox bars
// included), double-click, arrows, the wheel, and what resets it. Pure.

import { describe, expect, it } from 'vitest'
import {
  arrowPan,
  canvasImage,
  clampView,
  FIT,
  isZoomed,
  panBy,
  pictureRect,
  sinkZoom,
  toggleAt,
  viewFor,
  viewTransform,
  wheelFactor,
  ZOOM_MAX,
  ZOOM_STEP,
  zoomBy,
  zoomPercent,
  zoomResetKey,
  zoomStep,
  zoomTo,
  type Point,
  type ZoomView,
} from './share-zoom'

/** A 16:9 frame showing a 16:9 picture: the picture fills it. */
const frame = { width: 1600, height: 900 }
const image = { width: 3200, height: 1800 }
/** A 4:3 picture in the same frame: bars on the left and right. */
const square = { width: 1200, height: 900 }

/** Where the picture's own point (fit coordinates) lands under a view. */
function onScreen(v: ZoomView, p: Point): Point {
  return { x: v.x + p.x * v.scale, y: v.y + p.y * v.scale }
}

/** The picture's edges on screen. */
function edges(v: ZoomView, img = image) {
  const r = pictureRect(frame, img)
  const tl = onScreen(v, { x: r.left, y: r.top })
  const br = onScreen(v, { x: r.left + r.width, y: r.top + r.height })
  return { left: tl.x, top: tl.y, right: br.x, bottom: br.y }
}

describe('the picture inside the frame', () => {
  it('a picture of the same shape fills the frame; another shape is letterboxed and centred', () => {
    expect(pictureRect(frame, image)).toEqual({ left: 0, top: 0, width: 1600, height: 900 })
    expect(pictureRect(frame, square)).toEqual({ left: 200, top: 0, width: 1200, height: 900 })
  })

  it('with no picture yet (nothing drawn), the frame itself', () => {
    expect(pictureRect(frame, null)).toEqual({ left: 0, top: 0, width: 1600, height: 900 })
    expect(pictureRect(frame, { width: 0, height: 0 })).toEqual({ left: 0, top: 0, width: 1600, height: 900 })
  })

  it("reads the canvas's size, but not the HTML default before the first frame", () => {
    expect(canvasImage({ width: 1920, height: 1080 })).toEqual({ width: 1920, height: 1080 })
    expect(canvasImage({ width: 300, height: 150 })).toBeNull()
    expect(canvasImage({ width: 0, height: 0 })).toBeNull()
    expect(canvasImage(null)).toBeNull()
  })
})

describe('the clamp', () => {
  it('holds the scale between 1× and 6×, and anything at 1× is exactly fit', () => {
    expect(clampView({ scale: 9, x: 0, y: 0 }, frame, image).scale).toBe(ZOOM_MAX)
    expect(clampView({ scale: 0.3, x: -50, y: 20 }, frame, image)).toEqual(FIT)
    expect(clampView({ scale: 1.0001, x: -5, y: -5 }, frame, image)).toEqual(FIT)
    expect(clampView({ scale: Number.NaN, x: 0, y: 0 }, frame, image)).toEqual(FIT)
  })

  it('never lets the picture leave the frame, on any side', () => {
    for (const [x, y] of [
      [500, 500],
      [-99_999, -99_999],
      [-99_999, 500],
      [500, -99_999],
    ]) {
      const v = clampView({ scale: 3, x, y }, frame, image)
      const e = edges(v)
      expect(e.left).toBeLessThanOrEqual(0)
      expect(e.top).toBeLessThanOrEqual(0)
      expect(e.right).toBeGreaterThanOrEqual(frame.width)
      expect(e.bottom).toBeGreaterThanOrEqual(frame.height)
    }
    // Pushed past the top left: its top left corner sits on the frame's.
    expect(clampView({ scale: 3, x: 500, y: 500 }, frame, image)).toEqual({ scale: 3, x: 0, y: 0 })
    // Past the bottom right: its bottom right on the frame's.
    expect(clampView({ scale: 3, x: -99_999, y: -99_999 }, frame, image)).toEqual({ scale: 3, x: -3200, y: -1800 })
  })

  it('stops at the picture, not the letterbox bars', () => {
    // At 2× the 4:3 picture is 2400 wide: its edges reach the frame's, never past.
    const v = clampView({ scale: 2, x: 0, y: 0 }, frame, square)
    expect(edges(v, square).left).toBe(0)
    const w = clampView({ scale: 2, x: -99_999, y: 0 }, frame, square)
    expect(edges(w, square).right).toBe(frame.width)
  })

  it('a zoomed picture still narrower than the frame stays centred that way', () => {
    // A tall picture (9:16) in the wide frame: at 1.5× it's still narrower.
    const tall = { width: 900, height: 1600 }
    const v = clampView({ scale: 1.5, x: -300, y: 0 }, frame, tall)
    const e = edges(v, tall)
    expect((e.left + e.right) / 2).toBeCloseTo(frame.width / 2, 6)
    // Up and down it's bigger than the frame, so that axis is held, not centred.
    expect(e.top).toBeLessThanOrEqual(0)
    expect(e.bottom).toBeGreaterThanOrEqual(frame.height)
  })
})

describe('zooming about the pointer', () => {
  it('keeps the point under the pointer where it is', () => {
    const at = { x: 400, y: 300 }
    const v = zoomTo(FIT, 2, at, frame, image)
    expect(v.scale).toBe(2)
    // The picture's point that was under the pointer is still there.
    expect(onScreen(v, at)).toEqual(at)
    // And again from there, zooming further about another point.
    const at2 = { x: 1000, y: 600 }
    const under = { x: (at2.x - v.x) / v.scale, y: (at2.y - v.y) / v.scale }
    const w = zoomBy(v, 1.5, at2, frame, image)
    expect(onScreen(w, under).x).toBeCloseTo(at2.x, 6)
    expect(onScreen(w, under).y).toBeCloseTo(at2.y, 6)
  })

  it('near an edge, the clamp wins over the pointer (the picture never pulls away from it)', () => {
    // At the top left corner the corner stays in the corner.
    expect(zoomTo(FIT, 4, { x: 0, y: 0 }, frame, image)).toEqual({ scale: 4, x: 0, y: 0 })
    // At the bottom right, the bottom right stays put.
    expect(zoomTo(FIT, 4, { x: 1600, y: 900 }, frame, image)).toEqual({ scale: 4, x: -4800, y: -2700 })
  })

  it('zooming out to 1× is fit, wherever it was', () => {
    const v = zoomTo(FIT, 3, { x: 1200, y: 100 }, frame, image)
    expect(zoomTo(v, 1, { x: 10, y: 10 }, frame, image)).toEqual(FIT)
    expect(zoomBy(v, 0.01, { x: 10, y: 10 }, frame, image)).toEqual(FIT)
  })

  it('stops at 6×', () => {
    expect(zoomBy(FIT, 100, { x: 800, y: 450 }, frame, image).scale).toBe(ZOOM_MAX)
  })

  it('the buttons step ×1.5 about the middle, to 6× and back to fit', () => {
    let v = FIT
    const seen: number[] = []
    for (let i = 0; i < 6; i++) {
      v = zoomStep(v, 1, frame, image)
      seen.push(Math.round(v.scale * 1000) / 1000)
    }
    expect(seen).toEqual([1.5, 2.25, 3.375, 5.063, 6, 6])
    // About the middle: the middle stays the middle.
    const once = zoomStep(FIT, 1, frame, image)
    expect(onScreen(once, { x: 800, y: 450 })).toEqual({ x: 800, y: 450 })
    for (let i = 0; i < 6; i++) v = zoomStep(v, -1, frame, image)
    expect(v).toEqual(FIT)
    expect(ZOOM_STEP).toBe(1.5)
  })
})

describe('panning', () => {
  it('moves a zoomed picture, held inside the frame', () => {
    const v = zoomTo(FIT, 2, { x: 800, y: 450 }, frame, image)
    expect(v).toEqual({ scale: 2, x: -800, y: -450 })
    expect(panBy(v, 100, -50, frame, image)).toEqual({ scale: 2, x: -700, y: -500 })
    expect(panBy(v, 5000, 5000, frame, image)).toEqual({ scale: 2, x: 0, y: 0 })
    expect(panBy(v, -5000, -5000, frame, image)).toEqual({ scale: 2, x: -1600, y: -900 })
  })

  it('does nothing at fit', () => {
    expect(panBy(FIT, 100, 100, frame, image)).toEqual(FIT)
  })

  it('arrow keys show more of that side (the picture moves the other way); Shift goes four times as far', () => {
    expect(arrowPan('ArrowRight', frame)).toEqual({ x: -160, y: 0 })
    expect(arrowPan('ArrowLeft', frame)).toEqual({ x: 160, y: 0 })
    expect(arrowPan('ArrowDown', frame)).toEqual({ x: 0, y: -90 })
    expect(arrowPan('ArrowUp', frame)).toEqual({ x: 0, y: 90 })
    expect(arrowPan('ArrowRight', frame, true)).toEqual({ x: -640, y: 0 })
    expect(arrowPan('a', frame)).toBeNull()
  })
})

describe('double-click', () => {
  it('fit → 2× at the click; zoomed → fit', () => {
    const at = { x: 1200, y: 300 }
    const v = toggleAt(FIT, at, frame, image)
    expect(v.scale).toBe(2)
    expect(onScreen(v, at)).toEqual(at)
    expect(toggleAt(v, at, frame, image)).toEqual(FIT)
    expect(toggleAt(zoomTo(FIT, 5, at, frame, image), at, frame, image)).toEqual(FIT)
  })
})

describe('the wheel', () => {
  it('scrolling up zooms in, down zooms out, by the same amount each way', () => {
    expect(wheelFactor(-100)).toBeGreaterThan(1)
    expect(wheelFactor(100)).toBeLessThan(1)
    expect(wheelFactor(-100) * wheelFactor(100)).toBeCloseTo(1, 12)
    expect(wheelFactor(0)).toBe(1)
  })

  it('lines and pages count as pixels', () => {
    expect(wheelFactor(-1, 1)).toBeCloseTo(wheelFactor(-16, 0), 12)
    expect(wheelFactor(-1, 2)).toBeCloseTo(wheelFactor(-400, 0), 12)
  })
})

describe('what shows, and what the sink asks for', () => {
  it('the % shows only while zoomed', () => {
    expect(zoomPercent(FIT)).toBeNull()
    expect(zoomPercent({ scale: 2.5, x: 0, y: 0 })).toBe('250%')
    expect(isZoomed(FIT)).toBe(false)
  })

  it('the transform: none at fit, translate then scale while zoomed', () => {
    expect(viewTransform(FIT)).toBeUndefined()
    expect(viewTransform({ scale: 2, x: -10, y: -20 })).toBe('translate(-10px, -20px) scale(2)')
  })

  it("the sink's zoom comes in steps (1, 2, 4, 6), so a wheel doesn't ask for a new size every notch", () => {
    expect([1, 1.2, 2, 2.01, 3.9, 4, 4.5, 6].map((scale) => sinkZoom({ scale, x: 0, y: 0 }))).toEqual([1, 2, 2, 4, 4, 4, 6, 6])
  })
})

describe('resets', () => {
  it('another track, or the share ending, is a new key: fit again', () => {
    const live = zoomResetKey('TR_S7', 'live')
    expect(zoomResetKey('TR_S7', 'paused')).toBe(live)
    expect(zoomResetKey('TR_S7', 'off')).not.toBe(live)
    expect(zoomResetKey('TR_S7', 'unsupported')).not.toBe(live)
    expect(zoomResetKey('TR_S9', 'live')).not.toBe(live)
  })

  it('a view kept for one key is fit for any other', () => {
    const v = { scale: 3, x: -100, y: -100 }
    const kept = { key: zoomResetKey('TR_S7', 'live'), view: v }
    expect(viewFor(kept, zoomResetKey('TR_S7', 'live'))).toBe(v)
    expect(viewFor(kept, zoomResetKey('TR_S9', 'live'))).toEqual(FIT)
    expect(viewFor(kept, zoomResetKey('TR_S7', 'off'))).toEqual(FIT)
  })
})
