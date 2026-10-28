// @vitest-environment jsdom
// prd-garden-board-needs-v1 §14 "Renderer" (0.46.6): uploads.pick's press,
// rate and options (GP2); uploads.dropped (GP4); K2's picker on the desktop
// and the web (GP3, V4); the drop router's zen-widget target and its ring
// (V5); a big store.history reply reaching the frame whole (R5, GH12).
// Fixtures are made up (g-test0001, board-lite).
import { afterEach, describe, expect, it, vi } from 'vitest'
import type { ZenWidgetBridge } from './zen-bridge'
import type { ZenCustomWidgetPayload, ZenFrameReply } from './zen-custom-types'
import {
  createZenCustomLayer,
  type ZenCustomLayerDeps,
  type ZenCustomUploads,
  type ZenUploadedFile,
} from './zen-custom-bridge'
import { ZenUploadsError, zenFileAccepted, zenPick, zenPickFilters, type ZenUploadsDeps } from './zen-custom-uploads'
import { markZenDropOver, registerZenDropTarget, resetZenDropTargets, zenDropTargetAt, zenFramePoint } from './zen-drop-targets'
import type { ZenCustomStore } from './zen-custom-store'

vi.mock('@/stores/toast', () => ({ useToastStore: { getState: () => ({ addToast: () => undefined }) } }))

const REF = `k2blob:sha256-${'cd'.repeat(32)}`
const FILE: ZenUploadedFile = { ref: REF, name: 'Photo.png', size: 3, mime: 'image/png' }

function payload(caps: string[]): ZenCustomWidgetPayload {
  return {
    id: 'board',
    kind: 'custom',
    widget: 'board-lite',
    column: 0,
    props: { config: {} },
    caps: caps as ZenCustomWidgetPayload['caps'],
    requested: caps as ZenCustomWidgetPayload['requested'],
    source: 'user',
    name: 'Board',
    description: null,
    reasons: {},
    libs: [],
    hash: 'h1',
    state: 'ok',
    errors: [],
    warnings: [],
    origin: 'local',
    paused: null,
  }
}

class FakeUploads implements ZenCustomUploads {
  press = true
  picks: Array<{ accept: string[]; multiple: boolean; max: number }> = []
  answer: ZenUploadedFile[] = [FILE]
  drops: Array<(d: { files: ZenUploadedFile[]; x: number; y: number }) => void> = []
  pressed(): boolean {
    return this.press
  }
  async pick(opts: { accept: string[]; multiple: boolean; max: number }): Promise<ZenUploadedFile[]> {
    this.picks.push(opts)
    return this.answer
  }
  onDrop(fn: (d: { files: ZenUploadedFile[]; x: number; y: number }) => void): () => void {
    this.drops.push(fn)
    return () => {
      this.drops = this.drops.filter((f) => f !== fn)
    }
  }
}

function harness(caps = ['store:read', 'uploads:pick'], extra: Partial<ZenCustomLayerDeps> = {}) {
  const clock = { t: 1_000_000 }
  const uploads = new FakeUploads()
  const pushed: Array<[number, unknown]> = []
  const inner = { widgetId: 'board', caps: new Set<string>(), call: () => null } as unknown as ZenWidgetBridge
  const deps: ZenCustomLayerDeps = {
    widget: () => payload(caps),
    gardenId: () => 'g-test0001',
    inner,
    boundRows: () => [],
    ensureConversation: async () => undefined,
    push: (sub, v) => pushed.push([sub, v]),
    stop: () => undefined,
    paused: () => false,
    pauseForRunaway: () => undefined,
    theme: () => ({}),
    onThemeChange: () => () => undefined,
    now: () => clock.t,
    uploads,
    ...extra,
  }
  const layer = createZenCustomLayer(deps)
  let id = 0
  const send = async (verb: string, args: unknown[]): Promise<ZenFrameReply> => {
    const r = await layer.handle({ id: ++id, verb, args })
    if (!r) throw new Error('no reply')
    return r
  }
  return { layer, clock, uploads, pushed, send }
}

function ok(r: ZenFrameReply): unknown {
  if (!('ok' in r) || !r.ok) throw new Error(`refused: ${JSON.stringify(r)}`)
  return r.value
}

function code(r: ZenFrameReply): string {
  if (!('ok' in r) || r.ok) throw new Error(`not refused: ${JSON.stringify(r)}`)
  return r.error.code
}

describe('uploads.pick: K2’s picker, from a click (GP2)', () => {
  it('opens only after a press, answers refs, and passes the checked options', async () => {
    const h = harness()
    expect(ok(await h.send('uploads.pick', [{ accept: ['image/*', '.PDF'], multiple: true, max: 5 }]))).toEqual({ files: [FILE] })
    expect(h.uploads.picks).toEqual([{ accept: ['image/*', '.pdf'], multiple: true, max: 5 }])
    h.clock.t += 3_000
    h.uploads.press = false
    expect(code(await h.send('uploads.pick', []))).toBe('failed')
  })

  it('defaults to one file, and cuts the answer to max', async () => {
    const h = harness()
    h.uploads.answer = [FILE, { ...FILE, name: 'b.png' }, { ...FILE, name: 'c.png' }]
    expect(ok(await h.send('uploads.pick', []))).toEqual({ files: [FILE] })
    expect(h.uploads.picks[0]).toEqual({ accept: [], multiple: false, max: 1 })
    h.clock.t += 3_000
    expect((ok(await h.send('uploads.pick', [{ multiple: true, max: 2 }])) as { files: unknown[] }).files).toHaveLength(2)
  })

  it('a cancel is an empty list', async () => {
    const h = harness()
    h.uploads.answer = []
    expect(ok(await h.send('uploads.pick', [{}]))).toEqual({ files: [] })
  })

  it('refuses bad options before opening anything', async () => {
    const h = harness()
    for (const bad of [
      [{ accept: 'image/*' }],
      [{ accept: ['image'] }],
      [{ accept: ['<svg>'] }],
      [{ accept: Array.from({ length: 21 }, () => '.png') }],
      [{ multiple: 'yes' }],
      [{ max: 0 }],
      [{ max: 21 }],
      [{ path: '/etc' }],
      ['image/*'],
    ]) {
      expect(code(await h.send('uploads.pick', bad)), JSON.stringify(bad)).toBe('bad_args')
    }
    expect(h.uploads.picks).toEqual([])
  })

  it('is rate limited: once every 2 s, 10 a minute', async () => {
    const h = harness()
    ok(await h.send('uploads.pick', []))
    expect(code(await h.send('uploads.pick', []))).toBe('rate_limited')
    for (let i = 0; i < 9; i++) {
      h.clock.t += 2_000
      ok(await h.send('uploads.pick', []))
    }
    h.clock.t += 2_000
    expect(code(await h.send('uploads.pick', []))).toBe('rate_limited')
  })

  it('needs uploads:pick, and a host that has a picker', async () => {
    expect(code(await harness(['store:read']).send('uploads.pick', []))).toBe('cap_not_granted')
    expect(code(await harness(undefined, { uploads: undefined }).send('uploads.pick', []))).toBe('verb_unavailable')
  })
})

describe('uploads.dropped: the files K2 hands the frame (GP4)', () => {
  it('pushes {files, x, y} to a subscriber, and stops on unsubscribe', async () => {
    const h = harness()
    expect(await h.layer.handle({ sub: 7, verb: 'uploads.dropped', args: [] })).toBeNull()
    expect(h.uploads.drops).toHaveLength(1)
    h.uploads.drops[0]({ files: [FILE], x: 12, y: 34 })
    expect(h.pushed).toEqual([[7, { files: [FILE], x: 12, y: 34 }]])
    await h.layer.handle({ unsub: 7 })
    expect(h.uploads.drops).toHaveLength(0)
  })

  it('needs uploads:pick', async () => {
    const h = harness(['store:read'])
    const r = await h.layer.handle({ sub: 1, verb: 'uploads.dropped', args: [] })
    expect(r && 'error' in r ? r.error.code : null).toBe('cap_not_granted')
  })
})

describe('K2’s picker (GP3, V4)', () => {
  function deps(over: Partial<ZenUploadsDeps> = {}): ZenUploadsDeps & { calls: string[] } {
    const calls: string[] = []
    return {
      calls,
      web: () => false,
      native: async (o) => {
        calls.push(`native ${JSON.stringify(o)}`)
        return ['/Users/x/Desktop/a.png', '/Users/x/Desktop/b.png']
      },
      browser: async (o) => {
        calls.push(`browser ${JSON.stringify(o)}`)
        return []
      },
      ingest: async (paths, accept) => {
        calls.push(`ingest ${paths.length} ${accept.join(',')}`)
        return paths.map((p) => ({ ...FILE, name: p.split('/').pop() as string }))
      },
      put: async (_b, o) => ({ ref: REF, size: 3, mime: 'image/png', name: o.name ?? null, new: true }),
      ...over,
    }
  }

  it('desktop: the OS dialog with extension filters, then the daemon reads the paths', async () => {
    const d = deps()
    const files = await zenPick(d, { accept: ['image/png', '.jpg'], multiple: true, max: 1 })
    expect(files.map((f) => f.name)).toEqual(['a.png'])
    expect(d.calls).toEqual(['native {"filters":["png","jpg"],"multiple":true}', 'ingest 1 image/png,.jpg'])
  })

  it('desktop: a cancel never reaches the daemon', async () => {
    const d = deps({ native: async () => null })
    expect(await zenPick(d, { accept: [], multiple: false, max: 1 })).toEqual([])
    expect(d.calls).toEqual([])
  })

  it('web: K2’s own file input, then blobs.put; the accept list is checked first', async () => {
    const png = new File([new Uint8Array([137, 80, 78, 71])], 'shot.png', { type: 'image/png' })
    const txt = new File(['hello'], 'notes.txt', { type: 'text/plain' })
    const seen: string[] = []
    const d = deps({ web: () => true, browser: async (o) => (seen.push(JSON.stringify(o)), [png]) })
    expect(await zenPick(d, { accept: ['image/*'], multiple: false, max: 1 })).toEqual([{ ref: REF, name: 'shot.png', size: 3, mime: 'image/png' }])
    expect(seen).toEqual(['{"accept":"image/*","multiple":false}'])
    const bad = deps({ web: () => true, browser: async () => [txt] })
    await expect(zenPick(bad, { accept: ['image/*'], multiple: false, max: 1 })).rejects.toBeInstanceOf(ZenUploadsError)
  })

  it('filters: extensions only when every entry can be spelled as some', () => {
    expect(zenPickFilters([])).toEqual([])
    expect(zenPickFilters(['.PDF', 'image/jpeg'])).toEqual(['pdf', 'jpg', 'jpeg'])
    expect(zenPickFilters(['image/*', 'application/x-unknown'])).toEqual([])
    expect(zenFileAccepted(['image/*'], 'a.heic', 'image/heic')).toBe(true)
    expect(zenFileAccepted(['.md'], 'README.MD', '')).toBe(true)
    expect(zenFileAccepted(['image/*'], 'a.png', 'text/plain')).toBe(false)
  })
})

describe('the drop router’s widget target (V5)', () => {
  afterEach(() => resetZenDropTargets())

  it('only a listening frame is a target; the point is in frame coordinates; the ring follows the drag', () => {
    const frame = document.createElement('iframe')
    document.body.appendChild(frame)
    frame.getBoundingClientRect = () => ({ left: 100, top: 50, right: 500, bottom: 450, width: 400, height: 400, x: 100, y: 50, toJSON: () => ({}) })
    expect(zenDropTargetAt(frame)).toBeNull()
    const got: Array<[string[], { x: number; y: number }]> = []
    const off = registerZenDropTarget(frame, (paths, at) => got.push([paths, at]))
    const t = zenDropTargetAt(frame)
    expect(t?.frame).toBe(frame)
    t?.handler(['/x/a.png'], zenFramePoint(frame, { x: 130.4, y: 80.6 }))
    expect(got).toEqual([[['/x/a.png'], { x: 30, y: 31 }]])
    expect(zenDropTargetAt(document.body)).toBeNull()
    markZenDropOver(frame)
    expect(frame.hasAttribute('data-zen-drop-over')).toBe(true)
    off()
    expect(frame.hasAttribute('data-zen-drop-over')).toBe(false)
    expect(zenDropTargetAt(frame)).toBeNull()
    frame.remove()
  })

  it('the router sends a drop over a listening frame to zen-widget, and an unlistened one falls through to miss', async () => {
    const { hitTestExternalDrop } = await import('../external-drop-router')
    const frame = document.createElement('iframe')
    document.body.appendChild(frame)
    const doc = { elementFromPoint: () => frame, querySelectorAll: () => [] } as unknown as Document
    expect(hitTestExternalDrop({ x: 10, y: 10 }, doc).kind).toBe('miss')
    const off = registerZenDropTarget(frame, () => undefined)
    const t = hitTestExternalDrop({ x: 10, y: 10 }, doc)
    expect(t.kind).toBe('zen-widget')
    off()
    expect(hitTestExternalDrop({ x: 10, y: 10 }, doc).kind).toBe('miss')
    frame.remove()
  })
})

describe('replies into the frame are not capped (R5, GH12)', () => {
  it('a 120 KiB store.history page reaches the frame whole', async () => {
    const big = { head: 2, oldest: 0, items: [{ key: 'k', rev: 2, deleted: false, value: 'x'.repeat(120 * 1024), prev: null, prevKind: 'none', by: 'you', at: 1 }], next: null }
    const store = { call: async () => big, subscribe: () => () => undefined } as unknown as ZenCustomStore
    const h = harness(['store:read'], { store })
    const v = ok(await h.send('store.history', [{ limit: 1 }])) as typeof big
    expect(v.items[0].value).toHaveLength(120 * 1024)
  })
})
