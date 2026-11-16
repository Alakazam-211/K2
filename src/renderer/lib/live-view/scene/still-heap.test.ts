// What filled the sample scene page's heap (CDP heap profile, 2026-11-16):
// React's dev build logs each component's changed props into its
// performance track, walking every enumerable key down three levels, and
// the browser keeps those measures. A still's pixel and id arrays (millions
// of indexes each at 1920 px) went through that walk on every step: about
// 7.6 GB allocated between two steps, 2 GB still held after a collection.
// So a still's big arrays aren't enumerable (still readable), and the
// stand-in's rasteriser reuses its scratch buffers instead of allocating
// four full-size arrays a still.

import { describe, expect, it } from 'vitest'
import { DOCS } from './fold'
import { sealStill, type Still } from './picture-source'
import { StandInStills } from './stand-in/stand-in-stills'
import { rasterScratch } from './stand-in/raster'

const keysIn = (o: object): string[] => {
  const out: string[] = []
  for (const k in o) out.push(k)
  return out
}

describe('a still’s big arrays stay out of prop walks', () => {
  it('sealStill hides the pixels and the id values from for…in and Object.keys, and they still read', () => {
    const data = new Uint8ClampedArray(16)
    const values = new Uint32Array(4)
    const st: Still = { rev: 1, view: '@scene:main', shape: 'wide', width: 2, height: 2, image: { kind: 'rgba', data }, camera: null, ids: { width: 2, height: 2, values, objects: [] }, renderMs: 1 }
    sealStill(st)
    expect(keysIn(st.image)).toEqual(['kind'])
    expect(Object.keys(st.ids!)).toEqual(['width', 'height', 'objects'])
    expect(st.image.kind === 'rgba' && st.image.data).toBe(data)
    expect(st.ids!.values).toBe(values)
  })

  it('the stand-in’s stills come sealed, cached copies too', () => {
    const src = new StandInStills((rev) => DOCS[rev] ?? null)
    const a = src.draw({ rev: 60, view: '@scene:main', shape: 'wide', width: 64, ids: true })!
    const b = src.draw({ rev: 60, view: '@scene:main', shape: 'wide', width: 64, ids: true })!
    for (const st of [a, b]) {
      expect(keysIn(st.image)).toEqual(['kind'])
      expect(keysIn(st.ids!)).not.toContain('values')
      expect(st.image.kind === 'rgba' && st.image.data.length).toBe(st.width * st.height * 4)
    }
  })
})

describe('the rasteriser’s scratch buffers', () => {
  it('are reused from one still to the next, and grow only for a bigger one', () => {
    const src = new StandInStills((rev) => DOCS[rev] ?? null)
    src.draw({ rev: 60, view: '@scene:main', shape: 'wide', width: 640 })
    const first = rasterScratch()
    src.draw({ rev: 60, view: '@cam:1', shape: 'wide', width: 640 })
    const second = rasterScratch()
    expect(second.depth).toBe(first.depth)
    expect(second.occlusion).toBe(first.occlusion)
    expect(second.blur).toBe(first.blur)
    src.draw({ rev: 60, view: '@scene:main', shape: 'wide', width: 1280 })
    expect(rasterScratch().depth.length).toBeGreaterThanOrEqual(1280 * 720)
  })

  it('draw the same picture whether fresh or reused', () => {
    const one = new StandInStills((rev) => DOCS[rev] ?? null).draw({ rev: 61, view: '@scene:main', shape: 'wide', width: 640 })!
    new StandInStills((rev) => DOCS[rev] ?? null).draw({ rev: 60, view: '@cam:1', shape: 'tall', width: 640 })
    const two = new StandInStills((rev) => DOCS[rev] ?? null).draw({ rev: 61, view: '@scene:main', shape: 'wide', width: 640 })!
    expect(one.image.kind === 'rgba' && two.image.kind === 'rgba' && Buffer.from(one.image.data.buffer).equals(Buffer.from(two.image.data.buffer))).toBe(true)
  })
})
