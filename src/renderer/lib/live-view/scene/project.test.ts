// Projection and hit testing: a world point through K2 Render's camera
// (column-major view, infinite-far OpenGL projection with a shift) lands
// where the still drew it, and the id image names what is under a pixel
// (0 the background, v the object at index v − 1).

import { describe, expect, it } from 'vitest'
import { compassAxes, basis, fitBasis, project, projectPx, stillCamera, toPx, type StillCamera } from './project'
import { decodeIdPixels, entryOf, extentOf, idAt, idAtFraction, idIndexAt, locatorOf, type IdLayer } from './ids'
import { letterbox, shapeFor } from './picture-source'
import { StandInStills } from './stand-in/stand-in-stills'
import { DOCS } from './fold'
import { GOLD } from './samples'
import { worldOf } from './fold'

// A camera at (0, -5, 0) looking down +y, z up, 90° vertical, square.
const CAM: StillCamera = stillCamera(basis([0, -5, 0], [0, 1, 0], [0, 0, 1], 1, 1))
const SIZE = { width: 200, height: 200 }

describe('project', () => {
  it('puts the point the camera looks at in the middle', () => {
    const q = project([0, 0, 0], CAM)
    expect(q.inFront).toBe(true)
    expect(q.nx).toBeCloseTo(0)
    expect(q.ny).toBeCloseTo(0)
    expect(q.depth).toBeCloseTo(5)
    expect(toPx(q, SIZE)).toEqual({ x: 100, y: 100 })
  })

  it('maps right and up to the right and the top of the still (y down in pixels)', () => {
    const r = projectPx([5, 0, 0], CAM, SIZE)
    expect(r.x).toBeCloseTo(200)
    expect(r.y).toBeCloseTo(100)
    const u = projectPx([0, 0, 2.5], CAM, SIZE)
    expect(u.x).toBeCloseTo(100)
    expect(u.y).toBeCloseTo(50)
  })

  it('says a point behind the camera is not in front (and gives no position)', () => {
    const q = project([0, -10, 0], CAM)
    expect(q.inFront).toBe(false)
    expect(Number.isNaN(q.nx)).toBe(true)
  })

  it('applies the lens shift as K2 Render sends it (shift_x, shift_y in the projection)', () => {
    const b = basis([0, -5, 0], [0, 1, 0], [0, 0, 1], 1, 1)
    b.sx = 0.25
    b.sy = -0.5
    const cam = stillCamera(b)
    expect(cam.shift_x).toBe(0.25)
    const q = project([0, 0, 0], cam)
    expect(q.nx).toBeCloseTo(-0.25)
    expect(q.ny).toBeCloseTo(0.5)
  })

  it('reads the matrices column-major (translation in 12..14)', () => {
    expect(CAM.view[12]).toBeCloseTo(0)
    expect(CAM.view[14]).toBeCloseTo(-5)
    expect(CAM.proj[11]).toBe(-1)
  })

  it('fits a box inside its margin', () => {
    const pts = [[-1, -1, 0], [1, 1, 1], [-1, 1, 0], [1, -1, 1]]
    const cam = stillCamera(fitBasis(pts, [1, -1.3, 0.7], 0.27, 0.7, 16 / 9))
    for (const p of pts) {
      const q = project(p, cam)
      expect(Math.abs(q.nx)).toBeLessThanOrEqual(0.71)
      expect(Math.abs(q.ny)).toBeLessThanOrEqual(0.71)
    }
  })
})

describe('compass', () => {
  it('shows X right, Z up and Y into the picture for a camera looking down +y', () => {
    const ax = compassAxes(CAM)
    const by = Object.fromEntries(ax.map((a) => [a.axis, a]))
    expect(by.x.dx).toBeCloseTo(1)
    expect(by.z.dy).toBeCloseTo(-1)
    expect(by.y.away).toBeCloseTo(1)
    expect(ax[0].axis).toBe('y')
  })
})

describe('the id image', () => {
  // 4 × 2: background, object 0 (value 1), object 3 (value 4), and an index the table cut.
  const layer: IdLayer = { width: 4, height: 2, values: [0, 1, 1, 4, 0, 0, 4, 9], objects: [{ index: 0, id: '@obj:2' }, { index: 3, id: '@obj:7' }] }

  it('reads 0 as the background, never as object 0', () => {
    expect(idIndexAt(layer, 0, 0)).toBeNull()
    expect(idAt(layer, 0, 0)).toBeNull()
    expect(idIndexAt(layer, 1, 0)).toBe(0)
    expect(idAt(layer, 1, 0)).toBe('@obj:2')
  })

  it('maps v to index v − 1 and its locator o<v − 1>', () => {
    expect(idIndexAt(layer, 3, 0)).toBe(3)
    expect(locatorOf(idIndexAt(layer, 3, 0) as number)).toBe('o3')
    expect(idAt(layer, 2.9, 1.5)).toBe('@obj:7')
  })

  it('answers null outside the image and for an index the table left out', () => {
    expect(idAt(layer, -1, 0)).toBeNull()
    expect(idAt(layer, 4, 0)).toBeNull()
    expect(idIndexAt(layer, 3, 1)).toBe(8)
    expect(idAt(layer, 3, 1)).toBeNull()
    expect(entryOf(layer, 8)).toBeNull()
  })

  it('reads a point of the picture as shown, by fractions', () => {
    expect(idAtFraction(layer, 0.3, 0.2)).toBe('@obj:2')
  })

  it('decodes the PNG’s RGB as r << 16 | g << 8 | b, ignoring alpha', () => {
    const rgba = [0, 0, 0, 0, 0, 0, 1, 255, 0, 1, 0, 255, 1, 0, 2, 0]
    expect(Array.from(decodeIdPixels(rgba, 4, 1))).toEqual([0, 1, 256, 65538])
  })

  it('finds an object’s extent', () => {
    expect(extentOf(layer, 3)).toEqual({ x0: 2, y0: 0, x1: 3, y1: 1 })
    expect(extentOf(layer, 5)).toBeNull()
  })
})

describe('the stand-in still (sample)', () => {
  const D = DOCS[GOLD.state.upto]
  const src = new StandInStills(() => D)

  it('draws with alpha: nothing drawn is transparent, and its ids are 0 there', () => {
    const st = src.draw({ rev: D.rev, view: '@scene:main', shape: 'wide', width: 192 })!
    expect(st.width).toBe(192)
    expect(st.height).toBe(108)
    const rgba = st.image.kind === 'rgba' ? st.image.data : new Uint8ClampedArray()
    const ids = st.ids!.values
    let bg = 0
    for (let i = 0; i < ids.length; i++) if (ids[i] === 0) { bg++; expect(rgba[i * 4 + 3]).toBe(0) } else expect(rgba[i * 4 + 3]).toBe(255)
    expect(bg).toBeGreaterThan(0)
    expect(bg).toBeLessThan(ids.length)
  })

  it('names what is under the projected centre of Table (its camera and id image agree)', () => {
    const st = src.draw({ rev: D.rev, view: '@obj:Table', shape: 'wide', width: 320 })!
    const w = worldOf(D, '@obj:10')!
    // The table top's centre, 0.77 m up in its own space.
    const p = [w[8] * 0.77 + w[12], w[9] * 0.77 + w[13], w[10] * 0.77 + w[14]]
    const q = projectPx(p, st.camera!, st)
    expect(q.inFront).toBe(true)
    expect(idAt(st.ids!, q.x, q.y + 2)).toBe('@obj:10')
  })

  it('caches by the doc and the view', () => {
    const a = src.draw({ rev: D.rev, view: '@scene:main', shape: 'wide', width: 192 })!
    const b = src.draw({ rev: D.rev, view: '@scene:main', shape: 'wide', width: 192 })!
    expect(b.image).toBe(a.image)
  })
})

describe('letterbox', () => {
  it('keeps 16:9 in a wide pane, centred, inside a 16 px margin', () => {
    const lb = letterbox(1232, 696)
    expect(lb.shape).toBe('wide')
    expect(lb.top).toBe(16)
    expect(lb.height).toBeCloseTo(664)
    expect(lb.width).toBeCloseTo(664 * (16 / 9))
    expect(lb.left).toBeCloseTo((1232 - lb.width) / 2)
  })

  it('asks for a 4:3 still in a tall pane (narrower than 3:2)', () => {
    expect(shapeFor(700, 600)).toBe('tall')
    const lb = letterbox(732, 632)
    expect(lb.shape).toBe('tall')
    expect(lb.width / lb.height).toBeCloseTo(4 / 3)
  })
})
