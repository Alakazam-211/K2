// DocsPreviewPictureSource against K2 Render's fixture, through a transport
// that answers as the route does: whether the daemon serves the doc (a
// scene listing, or 409 no_preview), a still with its camera and id layer
// (asked with `look=transparent`, its shape and the width's bucket, then
// `layer=ids`), sealed for React, an object picked by its pixels, a still
// still drawing (polled until it lands, and said as it goes), a failure
// said with its code and its retry, and a push that makes a view's still
// stale.

import { readFileSync } from 'node:fs'
import { join } from 'node:path'
import { PNG } from 'pngjs'
import { describe, expect, it } from 'vitest'
import { DaemonHttpError } from '@/lib/daemon-http-error'
import { DocsPreviewPictureSource, type PictureEvent, type PreviewTransport } from './docs-preview-source'

const DIR = join(__dirname, '..', '..', '..', '..', '..', 'tests', 'fixtures', 'scene-preview')
const json = (f: string): Record<string, unknown> => JSON.parse(readFileSync(join(DIR, f), 'utf8'))
const b64 = (f: string): string => readFileSync(join(DIR, f)).toString('base64')
const VIEWS: Record<string, string> = { '@scene:main': 'scene', '@cam:1': 'cam-Camera', '@obj:15': 'obj-Sofa' }

/** The route over the fixture: `script` can answer a call first (rendering, failed). */
function route(script: (p: Record<string, string>, n: number) => unknown | undefined = () => undefined): PreviewTransport & { calls: Record<string, string>[] } {
  const calls: Record<string, string>[] = []
  return {
    calls,
    async get(p) {
      calls.push(p)
      const s = script(p, calls.length)
      if (s !== undefined) return s
      if (!p.container) return { kind: 'scene', ...json('listing.json') }
      const f = VIEWS[p.container]
      if (!f) return { kind: 'scene', container: p.container, render_hash: 'x', state: 'failed', failed: { code: 'unsupported_part' } }
      const meta = json(`${f}.json`)
      if (p.layer === 'ids') return { kind: 'scene', container: p.container, render_hash: meta.render_hash, state: 'ready', layer: 'ids', format: 'png', mime: 'image/png', base64: b64(`${f}.ids.png`), size: meta.size }
      return { ...meta, base64: b64(`${f}.webp`) }
    },
  }
}
const decoders = {
  image: async (bytes: Uint8Array) => ({ width: 640, height: 360, bytes: bytes.length }) as unknown as CanvasImageSource & { width: number; height: number },
  pixels: async (bytes: Uint8Array) => {
    const p = PNG.sync.read(Buffer.from(bytes))
    return { width: p.width, height: p.height, data: p.data }
  },
}
const req = (view = '@scene:main', ids = true) => ({ rev: 1, view, shape: 'wide' as const, width: 640, ids })

describe('whether the daemon serves the doc', () => {
  it('a scene listing: served', async () => {
    expect(await DocsPreviewPictureSource.serves(route())).toBe(true)
  })
  it('409 no_preview (the mesh read isn’t built, so no doc is served): not served, the sample stays', async () => {
    const t = route(() => {
      throw new DaemonHttpError('K2 draws no previews of this kind of file yet', 409, 'no_preview')
    })
    expect(await DocsPreviewPictureSource.serves(t)).toBe(false)
  })
  it('another kind of answer: not served', async () => {
    expect(await DocsPreviewPictureSource.serves(route(() => ({ kind: 'deck', containers: [] })))).toBe(false)
  })
})

describe('a still', () => {
  it('asks transparent, in its shape and width bucket, then its id image; hands back a sealed still with its camera', async () => {
    const t = route()
    const src = new DocsPreviewPictureSource(t, decoders)
    const st = (await src.still(req()))!
    expect(t.calls.filter((c) => c.container)).toEqual([
      { container: '@scene:main', width: 'thumb2x', look: 'transparent', shape: 'wide' },
      { container: '@scene:main', width: 'thumb2x', look: 'transparent', shape: 'wide', layer: 'ids' },
    ])
    expect(st).toMatchObject({ view: '@scene:main', width: 640, height: 360, shape: 'wide' })
    expect(st.image.kind).toBe('bitmap')
    expect(st.camera!.view).toHaveLength(16)
    expect(st.ids!.values.length).toBe(640 * 360)
    expect(st.ids!.objects).toHaveLength(18)
    // sealed: React's dev prop walk never enumerates the pixels
    expect(Object.keys(st.ids!)).not.toContain('values')
    src.close()
  })

  it('the second ask is the cache’s', async () => {
    const t = route()
    const src = new DocsPreviewPictureSource(t, decoders)
    await src.still(req())
    const n = t.calls.length
    await src.still(req())
    expect(t.calls.length).toBe(n)
  })

  it('without ids: one call', async () => {
    const t = route()
    const src = new DocsPreviewPictureSource(t, decoders)
    expect((await src.still(req('@cam:1', false)))!.ids).toBeNull()
    expect(t.calls.filter((c) => c.container)).toHaveLength(1)
  })

  it('an object’s bounds from the listing, once it is in; and the object under a pixel by its address', async () => {
    const src = new DocsPreviewPictureSource(route(), decoders)
    await src.ready()
    expect(src.bounds!(null as never, '@obj:15')).toMatchObject({ approx: true, approx_reason: 'modifiers' })
    expect(src.bounds!(null as never, '@cam:1')).toBeNull()
    const st = (await src.still(req()))!
    const L = st.ids!
    let hit: [number, number] | null = null
    for (let i = 0; i < L.values.length && !hit; i++) if (L.values[i] === 11) hit = [i % L.width, Math.floor(i / L.width)]
    expect(src.addressAt(st, hit![0], hit![1])).toBe('@obj:15')
  })
})

describe('drawing, failing and trying again', () => {
  it('rendering: says so, asks again until it lands, then says ready', async () => {
    const t = route((p, n) => (p.container && !p.layer && n <= 3 ? { kind: 'scene', container: p.container, render_hash: 'h', rev: 1, state: 'rendering' } : undefined))
    const src = new DocsPreviewPictureSource(t, decoders, { pollMs: 1 })
    const seen: PictureEvent[] = []
    src.subscribe((e) => seen.push(e))
    const st = await src.still(req('@scene:main', false))
    expect(st).not.toBeNull()
    expect(seen.filter((e) => e.type !== 'listing').map((e) => (e.type === 'preview_ready' ? `ready:${e.state}` : 'rendering'))).toEqual(['rendering', 'rendering', 'ready:ready'])
  })

  it('failed: no still, the failure said with its code; a retry asks again and lands', async () => {
    let fail = true
    const t = route((p) => (p.container && !p.layer && fail ? { kind: 'scene', container: p.container, render_hash: 'h', rev: 1, state: 'failed', failed: { code: 'timeout' } } : undefined))
    const src = new DocsPreviewPictureSource(t, decoders)
    const seen: PictureEvent[] = []
    src.subscribe((e) => seen.push(e))
    expect(await src.still(req())).toBeNull()
    expect(seen.at(-1)).toMatchObject({ type: 'preview_ready', container: '@scene:main', state: 'failed', failed: { code: 'timeout' } })
    fail = false
    src.retry('@scene:main')
    expect(seen.at(-1)).toMatchObject({ type: 'preview_rendering', container: '@scene:main' })
    expect(await src.still(req())).not.toBeNull()
    expect(seen.at(-1)).toMatchObject({ type: 'preview_ready', state: 'ready' })
  })

  it('a view K2 can’t draw: failed with its code', async () => {
    const src = new DocsPreviewPictureSource(route(), decoders)
    const seen: PictureEvent[] = []
    src.subscribe((e) => seen.push(e))
    expect(await src.still(req('@obj:23'))).toBeNull()
    expect(seen.at(-1)).toMatchObject({ state: 'failed', failed: { code: 'unsupported_part' } })
  })

  it('a push for a view drops its cached still (the next ask fetches the new one) and is passed on', async () => {
    const t = route()
    const src = new DocsPreviewPictureSource(t, decoders)
    const seen: PictureEvent[] = []
    src.subscribe((e) => seen.push(e))
    await src.still(req())
    const n = t.calls.length
    src.push({ type: 'preview_ready', container: '@scene:main', render_hash: 'new', width: 'thumb2x', state: 'ready', failed: null, rev: 2, rendered_at_ms: 0 })
    expect(seen.at(-1)).toMatchObject({ type: 'preview_ready', rev: 2 })
    await src.still(req())
    expect(t.calls.length).toBeGreaterThan(n)
  })
})
