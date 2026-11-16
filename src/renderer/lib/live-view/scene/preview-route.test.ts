// The docs preview route's scene answers, read from K2 Render's fixture
// (tests/fixtures/scene-preview: the living room drawn through the daemon's
// own path, the route's JSON as it answers): the listing (each object's
// local bounds, approx and why, a camera's display values), a still's
// answer (its camera, id table, look, shape and what it left out), the id
// image picking an object by its pixels and naming its address, the boxes
// (listed bounds through each object's transform, projected with the
// still's camera) holding every pixel the id image gives that object, and
// the pushes folded into the picture's state.

import { readFileSync } from 'node:fs'
import { join } from 'node:path'
import { PNG } from 'pngjs'
import { describe, expect, it } from 'vitest'
import { decodeIdPixels, extentOf, type IdLayer } from './ids'
import { projectPx } from './project'
import { addressOfId, foldPreviewPush, parseIdsImage, parseSceneImage, parseSceneListing, widthBucket } from './preview-route'
import type { PictureStatus } from './picture-source'

const DIR = join(__dirname, '..', '..', '..', '..', '..', 'tests', 'fixtures', 'scene-preview')
const json = (f: string): Record<string, unknown> => JSON.parse(readFileSync(join(DIR, f), 'utf8'))
const b64 = (f: string): string => readFileSync(join(DIR, f)).toString('base64')
/** The route's image answer as it comes (the fixture leaves out its base64). */
const answer = (f: string, still: string): Record<string, unknown> => ({ ...json(f), base64: b64(still) })
const idsAnswer = (f: string, png: string): Record<string, unknown> => {
  const a = json(f)
  return { kind: 'scene', container: a.container, render_hash: a.render_hash, rev: a.rev, width: a.width, state: 'ready', layer: 'ids', format: 'png', mime: 'image/png', base64: b64(png), size: a.size }
}
function idLayer(png: string, meta: Record<string, unknown>): IdLayer {
  const p = PNG.sync.read(readFileSync(join(DIR, png)))
  const ids = meta.ids as { objects: { index: number; id: string }[] }
  return { width: p.width, height: p.height, values: decodeIdPixels(p.data, p.width, p.height), objects: ids.objects }
}
/** K2SCENE1: the 8-byte magic, a little-endian u32 length, then the JSON header. */
function scene(): { objects: { id: string; key: string; transform: number[] }[] } {
  const b = readFileSync(join(DIR, 'living-room.k2scene'))
  const n = b.readUInt32LE(8)
  return JSON.parse(b.subarray(12, 12 + n).toString('utf8'))
}

describe('the scene listing', () => {
  const L = parseSceneListing({ kind: 'scene', ...json('listing.json') })

  it('names every view and object by its address, the scene first', () => {
    expect(L.containers[0]).toMatchObject({ key: '@scene:main', title: 'Scene' })
    expect(L.containers.map((c) => c.key)).toEqual(expect.arrayContaining(['@cam:1', '@cam:26', '@obj:4', '@obj:15', '@obj:23']))
    expect(L.containers.find((c) => c.key === '@obj:15')?.title).toBe('Sofa')
  })

  it('an object’s local bounds, approx as a bool with its reason (the sofa is drawn without its modifiers)', () => {
    const sofa = L.containers.find((c) => c.key === '@obj:15')!
    expect(sofa.bounds).toMatchObject({ approx: true, approx_reason: 'modifiers', hidden: false })
    expect(sofa.bounds!.min).toHaveLength(3)
    expect(L.containers.find((c) => c.key === '@obj:4')!.bounds).toMatchObject({ approx: false, approx_reason: null })
  })

  it('a camera’s display values; a view has no bounds', () => {
    const cam = L.containers.find((c) => c.key === '@cam:1')!
    expect(cam.camera).toMatchObject({ active: true, lensMm: 50 })
    expect(cam.bounds).toBeNull()
    expect(L.containers.find((c) => c.key === '@cam:26')!.camera?.active).toBe(false)
  })

  it('refuses an answer that isn’t a scene’s', () => {
    expect(() => parseSceneListing({ kind: 'deck', containers: [] })).toThrow(/scene/)
  })
})

describe('a still’s answer', () => {
  it('ready: its bytes, size, camera, id table, look, shape and what it left out', () => {
    const a = parseSceneImage(answer('scene.json', 'scene.webp'))
    expect(a).toMatchObject({ state: 'ready', container: '@scene:main', rev: 1, look: 'transparent', shape: 'wide', width: 640, height: 360, mime: 'image/webp', withoutModifiers: true, failedCode: null })
    expect(a.bytes!.length).toBe(readFileSync(join(DIR, 'scene.webp')).length)
    expect(a.camera!.view).toHaveLength(16)
    expect(a.camera!.proj).toHaveLength(16)
    expect(a.ids!.objects[10]).toEqual({ index: 10, id: 'Sofa' })
    expect(a.ids!.count).toBe(18)
    expect(a.meta).toEqual({ base_values: false, hidden_left_out: false, objects_hidden: 0, objects_not_evaluated: 0 })
  })

  it('the camera view keeps its camera’s aspect and names no shape', () => {
    const a = parseSceneImage(answer('cam-Camera.json', 'cam-Camera.webp'))
    expect(a.container).toBe('@cam:1')
    expect(a.camera).not.toBeNull()
  })

  it('rendering (no bytes yet), failed with its code, and stale with the older bytes', () => {
    expect(parseSceneImage({ kind: 'scene', state: 'rendering', render_hash: 'h', container: '@scene:main' })).toMatchObject({ state: 'rendering', bytes: null })
    expect(parseSceneImage({ kind: 'scene', state: 'failed', render_hash: 'h', failed: { code: 'timeout' } })).toMatchObject({ state: 'failed', failedCode: 'timeout' })
    expect(parseSceneImage({ ...answer('scene.json', 'scene.webp'), state: 'stale' }).bytes).not.toBeNull()
  })

  it('the id image’s answer: a PNG', () => {
    const a = parseIdsImage(idsAnswer('scene.json', 'scene.ids.png'))
    expect(a.mime).toBe('image/png')
    expect(a.bytes!.length).toBe(readFileSync(join(DIR, 'scene.ids.png')).length)
  })

  it('widths: the route’s buckets for the pixels asked', () => {
    expect([widthBucket(320), widthBucket(640), widthBucket(1920), widthBucket(1200)]).toEqual(['thumb', 'thumb2x', 'full', 'full'])
  })
})

describe('the id image', () => {
  const meta = json('scene.json')
  const L = parseSceneListing({ kind: 'scene', ...json('listing.json') })
  const layer = idLayer('scene.ids.png', meta)

  it('picks the sofa by its pixels and names its address', () => {
    const box = extentOf(layer, 10)!
    expect(box).not.toBeNull()
    const cx = (box.x0 + box.x1) / 2
    // a pixel of the sofa near its middle row
    let x = box.x0
    while (x <= box.x1 && layer.values[Math.round((box.y0 + box.y1) / 2) * layer.width + x] !== 11) x++
    expect(x).toBeLessThanOrEqual(box.x1)
    expect(addressOfId(L, layer, x, Math.round((box.y0 + box.y1) / 2))).toBe('@obj:15')
    expect(cx).toBeGreaterThan(0)
    // the background names nothing
    expect(addressOfId(L, layer, 0, 0)).toBeNull()
  })

  it('every object’s pixels sit inside its box: listed bounds through its transform, projected with the still’s camera', () => {
    const cam = parseSceneImage(answer('scene.json', 'scene.webp')).camera!
    const objs = scene().objects
    let checked = 0
    for (const e of layer.objects) {
      const o = objs.find((x) => x.id === e.id)
      const b = L.containers.find((c) => c.title === e.id)?.bounds
      const ext = extentOf(layer, e.index)
      if (!o || !b || !ext) continue
      const T = o.transform
      const pts = [0, 1, 2, 3, 4, 5, 6, 7].map((k) => {
        const p = [k & 1 ? b.max[0] : b.min[0], k & 2 ? b.max[1] : b.min[1], k & 4 ? b.max[2] : b.min[2]]
        const w = [0, 1, 2].map((r) => T[r] * p[0] + T[4 + r] * p[1] + T[8 + r] * p[2] + T[12 + r])
        return projectPx(w, cam, { width: layer.width, height: layer.height })
      })
      const xs = pts.map((p) => p.x)
      const ys = pts.map((p) => p.y)
      expect(ext.x0, e.id).toBeGreaterThanOrEqual(Math.min(...xs) - 1.5)
      expect(ext.x1, e.id).toBeLessThanOrEqual(Math.max(...xs) + 1.5)
      expect(ext.y0, e.id).toBeGreaterThanOrEqual(Math.min(...ys) - 1.5)
      expect(ext.y1, e.id).toBeLessThanOrEqual(Math.max(...ys) + 1.5)
      checked++
    }
    expect(checked).toBeGreaterThanOrEqual(15)
  })
})

describe('the pushes, folded into the picture’s state', () => {
  const none: PictureStatus = { shown: null, want: null, state: 'none', renderMs: null, reusedFrom: null, withoutModifiers: false, reason: null, landed: false, recovered: false, retrying: false, again: false }
  const ready = (rev: number, extra: Record<string, unknown> = {}) => ({ type: 'preview_ready', container: '@scene:main', rev, state: 'ready', render_ms: 212, without_modifiers: true, ...extra })

  it('drawing the first, then ready', () => {
    const a = foldPreviewPush(none, { type: 'preview_rendering', container: '@scene:main', rev: 1 }, '@scene:main')
    expect(a).toMatchObject({ state: 'rendering', want: 1, shown: null })
    const b = foldPreviewPush(a, ready(1), '@scene:main')
    expect(b).toMatchObject({ state: 'ready', shown: 1, want: 1, renderMs: 212, withoutModifiers: true, landed: true })
  })

  it('a newer one drawing over the old is stale; a failure keeps the old; a retry that lands recovers', () => {
    const r1 = foldPreviewPush(none, ready(1), '@scene:main')
    const s = foldPreviewPush(r1, { type: 'preview_rendering', container: '@scene:main', rev: 2 }, '@scene:main')
    expect(s).toMatchObject({ state: 'stale', shown: 1, want: 2 })
    const f = foldPreviewPush(s, { type: 'preview_ready', container: '@scene:main', rev: 2, state: 'failed', failed: { code: 'timeout' } }, '@scene:main')
    expect(f).toMatchObject({ state: 'failed', shown: 1, reason: 'timeout' })
    const again = foldPreviewPush(f, { type: 'preview_rendering', container: '@scene:main', rev: 2 }, '@scene:main')
    expect(again).toMatchObject({ retrying: true })
    const ok = foldPreviewPush(again, ready(2), '@scene:main')
    expect(ok).toMatchObject({ state: 'ready', shown: 2, recovered: true })
  })

  it('reused from an older drawing (a rename): ready at once, saying so', () => {
    expect(foldPreviewPush(none, ready(3, { reused_from: '@obj:12' }), '@scene:main')).toMatchObject({ state: 'ready', reusedFrom: 3 })
  })

  it('another view’s push leaves this one alone', () => {
    expect(foldPreviewPush(none, ready(1, { container: '@cam:1' }), '@scene:main')).toBe(none)
  })
})
