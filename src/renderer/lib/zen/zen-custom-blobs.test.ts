// prd-garden-board-needs-v1 §14 "Renderer": the custom layer's byte rules
// (GB12, V1), blobs.put / blobs.get over the widget door (chunked), the
// reply transfer list (V2), k2.download's press, rate, name and 60 s
// ticket (GS2, GS3, GS7). Fixtures are made up (g-test0001, board-lite).
import { describe, expect, it } from 'vitest'
import type { ZenWidgetBridge } from './zen-bridge'
import type { ZenCustomWidgetPayload, ZenFrameReply } from './zen-custom-types'
import { createZenCustomLayer, type ZenCustomDownloads, type ZenCustomLayerDeps } from './zen-custom-bridge'
import {
  ZEN_BLOB_CHUNK_BYTES,
  ZEN_BLOB_MAX_BYTES,
  ZEN_DOWNLOADS,
  base64ToBytes,
  bytesToBase64,
  createZenCustomBlobs,
  zenBytesReader,
  zenDownloadName,
  type ZenCustomBlobs,
} from './zen-custom-blobs'
import { startZenFrameHost } from './zen-custom-host'
import type { DownloadSliceReader } from '../fs-transfer'

const REF = `k2blob:sha256-${'ab'.repeat(32)}`

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

class FakeBlobs implements ZenCustomBlobs {
  puts: Array<{ bytes: number[]; opts: unknown }> = []
  gets: Array<[string, unknown]> = []
  readers: string[] = []
  async put(bytes: ArrayBuffer, opts: { mime?: string; name?: string }) {
    this.puts.push({ bytes: [...new Uint8Array(bytes)], opts })
    return { ref: REF, size: bytes.byteLength, mime: 'image/png', name: opts.name ?? null, new: true }
  }
  async get(ref: string, opts: { px?: number; from?: string }) {
    this.gets.push([ref, opts])
    return { info: { ref, size: 3, mime: 'image/png', name: 'a.png' }, bytes: new Uint8Array([1, 2, 3]).buffer }
  }
  reader(ref: string): DownloadSliceReader {
    this.readers.push(ref)
    return zenBytesReader(new Uint8Array([9, 9]).buffer)
  }
}

class FakeDownloads implements ZenCustomDownloads {
  press = true
  saved: Array<{ name: string; bytes: number[] }> = []
  answer: string | null = '/Users/x/Downloads/Board (1).png'
  pressed(): boolean {
    return this.press
  }
  async save(name: string, read: DownloadSliceReader): Promise<string | null> {
    const out: number[] = []
    let offset = 0
    for (;;) {
      const s = await read(offset, 4)
      out.push(...base64ToBytes(s.base64))
      offset += s.len
      if (s.eof) break
    }
    this.saved.push({ name, bytes: out })
    return this.answer
  }
}

function harness(caps = ['store:read', 'store:write', 'downloads:save']) {
  const clock = { t: 1_000_000 }
  const blobs = new FakeBlobs()
  const downloads = new FakeDownloads()
  const inner = { widgetId: 'board', caps: new Set<string>(), call: () => null } as unknown as ZenWidgetBridge
  const deps: ZenCustomLayerDeps = {
    widget: () => payload(caps),
    gardenId: () => 'g-test0001',
    inner,
    boundRows: () => [],
    ensureConversation: async () => undefined,
    push: () => undefined,
    stop: () => undefined,
    paused: () => false,
    pauseForRunaway: () => undefined,
    theme: () => ({}),
    onThemeChange: () => () => undefined,
    now: () => clock.t,
    blobs,
    downloads,
  }
  const layer = createZenCustomLayer(deps)
  let id = 0
  const send = async (verb: string, args: unknown[], bytes?: unknown): Promise<ZenFrameReply> => {
    const r = await layer.handle({ id: ++id, verb, args, ...(bytes !== undefined ? { bytes } : {}) })
    if (!r) throw new Error('no reply')
    return r
  }
  return { layer, clock, blobs, downloads, send }
}

function ok(r: ZenFrameReply): unknown {
  if (!('ok' in r) || !r.ok) throw new Error(`refused: ${JSON.stringify(r)}`)
  return r.value
}

function code(r: ZenFrameReply): string {
  if (!('ok' in r) || r.ok) throw new Error(`not refused: ${JSON.stringify(r)}`)
  return r.error.code
}

describe('bytes ride beside the message, checked by the layer (GB12, V1)', () => {
  it('blobs.put takes one ArrayBuffer and hands it to the widget door', async () => {
    const h = harness()
    const v = ok(await h.send('blobs.put', [null, { name: 'shot.png' }], new Uint8Array([1, 2, 3]).buffer))
    expect(v).toEqual({ ref: REF, size: 3, mime: 'image/png', name: 'shot.png', new: true })
    expect(h.blobs.puts).toEqual([{ bytes: [1, 2, 3], opts: { name: 'shot.png' } }])
  })

  it('refuses bytes on a verb that takes none, bytes that are not an ArrayBuffer, and too many', async () => {
    const h = harness()
    expect(code(await h.send('blobs.get', [REF], new ArrayBuffer(1)))).toBe('bad_args')
    expect(code(await h.send('theme.get', [], new ArrayBuffer(1)))).toBe('bad_args')
    expect(code(await h.send('blobs.put', [null, {}], new Uint8Array(4)))).toBe('bad_args')
    expect(code(await h.send('blobs.put', [null, {}], 'text'))).toBe('bad_args')
    expect(code(await h.send('blobs.put', [null, {}], new ArrayBuffer(ZEN_BLOB_MAX_BYTES + 1)))).toBe('too_large')
    expect(code(await h.send('blobs.put', [null, {}]))).toBe('bad_args')
    expect(code(await h.send('blobs.put', ['inline data', {}], new ArrayBuffer(1)))).toBe('bad_args')
    expect(code(await h.send('blobs.put', [null, { path: '/etc' }], new ArrayBuffer(1)))).toBe('bad_args')
    expect(h.blobs.puts).toEqual([])
  })

  it('still applies the 64 KB rule to everything else', async () => {
    const h = harness()
    expect(code(await h.send('blobs.put', [null, { name: 'x'.repeat(70_000) }], new ArrayBuffer(1)))).toBe('too_large')
  })

  it('needs the cap the catalog row names', async () => {
    const h = harness(['store:read'])
    expect(code(await h.send('blobs.put', [null, {}], new ArrayBuffer(1)))).toBe('cap_not_granted')
    const r = harness(['store:write'])
    expect(code(await r.send('blobs.get', [REF]))).toBe('cap_not_granted')
  })

  it('blobs.get answers its info and moves the bytes back', async () => {
    const h = harness()
    const r = await h.send('blobs.get', [REF, { px: 256 }])
    expect(ok(r)).toEqual({ ref: REF, size: 3, mime: 'image/png', name: 'a.png' })
    expect('bytes' in r && r.bytes instanceof ArrayBuffer ? [...new Uint8Array(r.bytes)] : null).toEqual([1, 2, 3])
    expect(h.blobs.gets).toEqual([[REF, { px: 256, from: undefined }]])
    expect(code(await h.send('blobs.get', ['k2blob:sha256-../x']))).toBe('bad_args')
    expect(code(await h.send('blobs.get', [REF, { px: 300 }]))).toBe('bad_args')
  })
})

describe('the frame host moves reply bytes (V2) and keeps replies uncapped (GH12)', () => {
  it('posts a bytes reply with its buffer in the transfer list', async () => {
    const sent: Array<{ msg: unknown; transfer: unknown }> = []
    const port = {
      onmessage: null as ((e: MessageEvent) => void) | null,
      postMessage: (msg: unknown, transfer?: unknown) => void sent.push({ msg, transfer }),
      close: () => undefined,
    }
    const bytes = new Uint8Array([7, 8]).buffer
    const big = 'y'.repeat(120 * 1024)
    const host = startZenFrameHost(
      { postMessage: () => undefined },
      {
        hello: { k2: 'hello', v: 1, caps: [], features: [], widget: { id: 'b', name: 'B', garden: 'g' }, config: {}, motion: { reduced: false } },
        label: 'b',
        channel: () => ({ port1: port, port2: {} }) as unknown as MessageChannel,
        stop: () => undefined,
        focused: () => true,
        onChord: () => undefined,
        makeLayer: () => ({
          handle: async (m) => {
            const id = (m as { id: number }).id
            return id === 1 ? { id, ok: true, value: { ref: REF }, bytes } : { id, ok: true, value: { items: [{ value: big }] } }
          },
          boundChanged: () => undefined,
          subscriptions: 0,
          held: 0,
          dispose: () => undefined,
        }),
      },
    )
    port.onmessage?.({ data: { id: 1, verb: 'blobs.get', args: [REF] } } as MessageEvent)
    port.onmessage?.({ data: { id: 2, verb: 'store.list', args: [] } } as MessageEvent)
    await new Promise((r) => setTimeout(r, 0))
    expect(sent[0]).toEqual({ msg: { id: 1, ok: true, value: { ref: REF }, bytes }, transfer: [bytes] })
    // A 120 KiB reply reaches the frame whole (replies have no size cap).
    expect(JSON.stringify(sent[1].msg).length).toBeGreaterThan(120 * 1024)
    expect(sent[1].transfer).toBeUndefined()
    host.dispose()
  })
})

describe('k2.download: a press, the rate, the name and the 60 s ticket (GS2, GS3, GS7)', () => {
  it('saves bytes after a press and answers the name it landed as, never a path', async () => {
    const h = harness()
    const r = ok(await h.send('downloads.save', ['Board.png', null, {}], new Uint8Array([1, 2, 3, 4, 5]).buffer))
    expect(r).toEqual({ saved: true, name: 'Board (1).png' })
    expect(h.downloads.saved).toEqual([{ name: 'Board.png', bytes: [1, 2, 3, 4, 5] }])
  })

  it('needs a press, the downloads:save cap, and an allowed extension', async () => {
    const h = harness()
    h.downloads.press = false
    expect(code(await h.send('downloads.save', ['a.png', null, {}], new ArrayBuffer(1)))).toBe('failed')
    expect(code(await h.send('downloads.begin', ['a.png']))).toBe('failed')
    h.downloads.press = true
    h.clock.t += 10_000
    expect(code(await h.send('downloads.save', ['run.html', null, {}], new ArrayBuffer(1)))).toBe('bad_args')
    expect(code(await h.send('downloads.save', ['tool.exe', null, {}], new ArrayBuffer(1)))).toBe('bad_args')
    const n = harness(['store:read'])
    expect(code(await n.send('downloads.save', ['a.png', null, {}], new ArrayBuffer(1)))).toBe('cap_not_granted')
    expect(h.downloads.saved).toEqual([])
  })

  it('is at most one every 2 seconds and 10 a minute', async () => {
    const h = harness()
    ok(await h.send('downloads.save', ['a.txt', null, {}], new ArrayBuffer(1)))
    expect(code(await h.send('downloads.save', ['b.txt', null, {}], new ArrayBuffer(1)))).toBe('rate_limited')
    for (let i = 0; i < ZEN_DOWNLOADS.perMinute - 1; i++) {
      h.clock.t += ZEN_DOWNLOADS.everyMs
      ok(await h.send('downloads.save', [`n${i}.txt`, null, {}], new ArrayBuffer(1)))
    }
    h.clock.t += ZEN_DOWNLOADS.everyMs
    expect(code(await h.send('downloads.save', ['over.txt', null, {}], new ArrayBuffer(1)))).toBe('rate_limited')
  })

  it('begin checks the press now; the bytes may follow with the ticket within 60 s, once', async () => {
    const h = harness()
    const t = ok(await h.send('downloads.begin', ['Board.png', { mime: 'image/png' }])) as { ticket: string }
    expect(typeof t.ticket).toBe('string')
    h.downloads.press = false
    h.clock.t += 30_000
    expect(ok(await h.send('downloads.save', ['Board.png', null, { ticket: t.ticket }], new ArrayBuffer(2)))).toEqual({
      saved: true,
      name: 'Board (1).png',
    })
    expect(code(await h.send('downloads.save', ['Board.png', null, { ticket: t.ticket }], new ArrayBuffer(2)))).toBe('failed')
    h.downloads.press = true
    h.clock.t += 5_000
    const late = ok(await h.send('downloads.begin', ['late.png'])) as { ticket: string }
    h.clock.t += ZEN_DOWNLOADS.ticketMs + 1
    h.downloads.press = false
    expect(code(await h.send('downloads.save', ['late.png', null, { ticket: late.ticket }], new ArrayBuffer(2)))).toBe('failed')
    expect(code(await h.send('downloads.save', ['late.png', null, { ticket: 'dl-forged' }], new ArrayBuffer(2)))).toBe('failed')
    // A ticket belongs to this placement's layer: another layer never knows it.
    const other = harness()
    other.downloads.press = false
    expect(code(await other.send('downloads.save', ['late.png', null, { ticket: late.ticket }], new ArrayBuffer(2)))).toBe('failed')
  })

  it('{blob: ref} reads from the daemon, never through the frame', async () => {
    const h = harness()
    expect(ok(await h.send('downloads.save', ['photo.png', { blob: REF }, {}]))).toEqual({ saved: true, name: 'Board (1).png' })
    expect(h.blobs.readers).toEqual([REF])
    expect(h.downloads.saved[0]).toEqual({ name: 'photo.png', bytes: [9, 9] })
  })

  it('answers saved: false when the person cancels', async () => {
    const h = harness()
    h.downloads.answer = null
    expect(ok(await h.send('downloads.save', ['a.json', null, {}], new ArrayBuffer(1)))).toEqual({ saved: false })
  })

  it('cleans names', () => {
    expect(zenDownloadName('../../etc/board.png')).toEqual({ name: 'board.png' })
    expect(zenDownloadName('.hidden.txt')).toEqual({ name: 'hidden.txt' })
    expect(zenDownloadName('Board', 'image/png')).toEqual({ name: 'Board.png' })
    expect(zenDownloadName('Board')).toHaveProperty('problem')
    expect(zenDownloadName('a\u0000b.csv')).toEqual({ name: 'ab.csv' })
    const long = zenDownloadName(`${'x'.repeat(300)}.pdf`)
    expect('name' in long && long.name.length === 120 && long.name.endsWith('.pdf')).toBe(true)
  })
})

describe('the widget door client chunks at 8 MiB', () => {
  it('uploads in order with the upload id and reads back whole', async () => {
    const bodies: Array<Record<string, unknown>> = []
    let store = new Uint8Array(0)
    const blobs = createZenCustomBlobs({
      put: async (body) => {
        bodies.push({ ...body, base64: (body.base64 as string).length })
        const add = base64ToBytes(body.base64 as string)
        const next = new Uint8Array(store.length + add.length)
        next.set(store)
        next.set(add, store.length)
        store = next
        return body.last ? { ref: REF, size: store.length, mime: 'application/octet-stream', name: null, new: true } : { upload: 'up-1', received: store.length }
      },
      get: async (p) => {
        const offset = Number(p.offset)
        const len = Number(p.len)
        const slice = store.subarray(offset, offset + len)
        return { ref: REF, size: store.length, mime: 'application/octet-stream', name: null, offset, len: slice.length, eof: offset + slice.length >= store.length, base64: bytesToBase64(slice) }
      },
    })
    const n = ZEN_BLOB_CHUNK_BYTES * 2 + 5
    const src = new Uint8Array(n)
    for (let i = 0; i < n; i++) src[i] = i % 251
    const info = await blobs.put(src.buffer, { name: 'big.bin' })
    expect(info.ref).toBe(REF)
    expect(bodies.map((b) => [b.offset, b.upload ?? null, b.last])).toEqual([
      [0, null, false],
      [ZEN_BLOB_CHUNK_BYTES, 'up-1', false],
      [ZEN_BLOB_CHUNK_BYTES * 2, 'up-1', true],
    ])
    const back = await blobs.get(REF, {})
    expect(back.bytes.byteLength).toBe(n)
    expect(new Uint8Array(back.bytes).every((b, i) => b === i % 251)).toBe(true)
  })
})
