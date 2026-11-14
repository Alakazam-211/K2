// The test kit's fakes for players of media files (testing.ts), fail loud:
// a frame's I420 planes through copyTo (and a frame that can't expose them),
// fetch with Range (206 and Content-Range, open and suffix ranges, 416, 404,
// 200, a stalled link, abort), a clock that moves only when told (timers in
// due order, timers set by timers), and the AudioDecoder fake (Opus yes or
// no, configure / decode / emit, flush settle and reset).

import { describe, expect, it } from 'vitest'
import { fakeAudioCodecs, fakeClock, fakeRangeFetch, fakeVideoFrame } from './testing'

describe('fakeVideoFrame', () => {
  it('copies Y, U, V back to back with their strides; odd sizes round the chroma up', async () => {
    const f = fakeVideoFrame({ timestamp: 3, width: 3, height: 3 })
    expect(f.allocationSize?.()).toBe(9 + 4 + 4)
    const buf = new Uint8Array(17)
    expect(await f.copyTo?.(buf)).toEqual([
      { offset: 0, stride: 3 },
      { offset: 9, stride: 2 },
      { offset: 13, stride: 2 },
    ])
    expect(Array.from(buf.subarray(0, 9))).toEqual([3, 4, 5, 6, 7, 8, 9, 10, 11])
    expect(Array.from(buf.subarray(9))).toEqual(new Array(8).fill(128))
  })

  it('given planes are what copyTo writes, into a view at an offset', async () => {
    const planes = [new Uint8Array([1, 2, 3, 4]), new Uint8Array([5]), new Uint8Array([6])] as const
    const f = fakeVideoFrame({ timestamp: 0, width: 2, height: 2, planes })
    const backing = new ArrayBuffer(10)
    await f.copyTo?.(new Uint8Array(backing, 2, 6))
    expect(Array.from(new Uint8Array(backing))).toEqual([0, 0, 1, 2, 3, 4, 5, 6, 0, 0])
  })

  it('format null, a conversion, a short buffer or a closed frame reject', async () => {
    const gpu = fakeVideoFrame({ timestamp: 0, width: 2, height: 2, format: null })
    expect(gpu.format).toBeNull()
    await expect(gpu.copyTo?.(new Uint8Array(6))).rejects.toThrow(/NotSupported/)
    const f = fakeVideoFrame({ timestamp: 0, width: 2, height: 2 })
    await expect(f.copyTo?.(new Uint8Array(6), { format: 'RGBA' })).rejects.toThrow(/NotSupported/)
    await expect(f.copyTo?.(new Uint8Array(5))).rejects.toThrow(/too small/)
    f.close()
    expect(f.closed).toBe(true)
    await expect(f.copyTo?.(new Uint8Array(6))).rejects.toThrow(/closed/)
  })
})

describe('fakeRangeFetch', () => {
  const file = Uint8Array.from({ length: 100 }, (_, i) => i)
  const net = () => fakeRangeFetch({ '/a.mp4': file })

  it('answers a range with 206, its bytes and Content-Range; open and suffix ranges; ranges clamp to the end', async () => {
    const n = net()
    const r = await n.fetch('/a.mp4', { headers: { Range: 'bytes=10-19' } })
    expect([r.ok, r.status, r.headers.get('content-range'), r.headers.get('Content-Length')]).toEqual([true, 206, 'bytes 10-19/100', '10'])
    expect(Array.from(new Uint8Array(await r.arrayBuffer()))).toEqual([10, 11, 12, 13, 14, 15, 16, 17, 18, 19])
    expect((await n.fetch('/a.mp4', { headers: { range: 'bytes=95-' } })).headers.get('Content-Range')).toBe('bytes 95-99/100')
    expect((await n.fetch('/a.mp4', { headers: { Range: 'bytes=-5' } })).headers.get('Content-Range')).toBe('bytes 95-99/100')
    expect((await n.fetch('/a.mp4', { headers: { Range: 'bytes=90-200' } })).headers.get('Content-Range')).toBe('bytes 90-99/100')
    expect(n.requests).toEqual([
      { url: '/a.mp4', range: 'bytes=10-19' },
      { url: '/a.mp4', range: 'bytes=95-' },
      { url: '/a.mp4', range: 'bytes=-5' },
      { url: '/a.mp4', range: 'bytes=90-200' },
    ])
  })

  it('no range: 200 and the whole file; past the end: 416; unknown: 404; failWith: that status', async () => {
    const n = net()
    const whole = await n.fetch('/a.mp4')
    expect(whole.status).toBe(200)
    expect((await whole.arrayBuffer()).byteLength).toBe(100)
    const past = await n.fetch('/a.mp4', { headers: { Range: 'bytes=100-' } })
    expect([past.ok, past.status, past.headers.get('Content-Range')]).toEqual([false, 416, 'bytes */100'])
    expect((await n.fetch('/b.mp4')).status).toBe(404)
    n.failWith('/a.mp4', 503)
    expect((await n.fetch('/a.mp4', { headers: { Range: 'bytes=0-1' } })).status).toBe(503)
  })

  it('a stalled link holds answers until resume; abort rejects, also while stalled', async () => {
    const n = net()
    n.stall()
    let got: number | null = null
    const held = n.fetch('/a.mp4', { headers: { Range: 'bytes=0-0' } }).then((r) => (got = r.status))
    const ac = new AbortController()
    const aborted = n.fetch('/a.mp4', { signal: ac.signal })
    await Promise.resolve()
    expect(got).toBeNull()
    ac.abort()
    await expect(aborted).rejects.toMatchObject({ name: 'AbortError' })
    n.resume()
    await held
    expect(got).toBe(206)
    const pre = new AbortController()
    pre.abort()
    await expect(n.fetch('/a.mp4', { signal: pre.signal })).rejects.toMatchObject({ name: 'AbortError' })
  })
})

describe('fakeClock', () => {
  it('moves only on advance, firing due timers in order (ties in set order), including timers set by timers', () => {
    const c = fakeClock(1000)
    const log: string[] = []
    c.setTimeout(() => log.push(`b@${c.now()}`), 20)
    c.setTimeout(() => log.push(`a@${c.now()}`), 10)
    c.setTimeout(() => log.push(`a2@${c.now()}`), 10)
    const gone = c.setTimeout(() => log.push('never'), 5)
    c.clearTimeout(gone)
    c.setTimeout(() => {
      log.push(`c@${c.now()}`)
      c.setTimeout(() => log.push(`d@${c.now()}`), 5)
    }, 15)
    expect(c.now()).toBe(1000)
    c.advance(19)
    expect(log).toEqual(['a@1010', 'a2@1010', 'c@1015'])
    expect(c.now()).toBe(1019)
    expect(c.pending).toBe(2)
    c.advance(1)
    expect(log).toEqual(['a@1010', 'a2@1010', 'c@1015', 'b@1020', 'd@1020'])
    expect(c.pending).toBe(0)
  })
})

describe('fakeAudioCodecs', () => {
  it('Opus is supported unless told otherwise; other codecs are not', async () => {
    const yes = fakeAudioCodecs()
    expect(await yes.isConfigSupported({ codec: 'opus', sampleRate: 48000, numberOfChannels: 2 })).toBe(true)
    expect(await yes.isConfigSupported({ codec: 'mp4a.40.2', sampleRate: 48000, numberOfChannels: 2 })).toBe(false)
    expect(yes.asked).toEqual(['opus', 'mp4a.40.2'])
    expect(await fakeAudioCodecs({ opus: false }).isConfigSupported({ codec: 'opus', sampleRate: 48000, numberOfChannels: 2 })).toBe(false)
  })

  it('records configure and decode, emits planar float audio, settles flush, and reset rejects pending flushes', async () => {
    const codecs = fakeAudioCodecs()
    const out: { timestamp: number; frames: number; first: number }[] = []
    const d = codecs.createDecoder({
      output: (a) => {
        const plane = new Float32Array(a.numberOfFrames)
        a.copyTo(plane, { planeIndex: 1 })
        out.push({ timestamp: a.timestamp, frames: a.numberOfFrames, first: plane[0] })
        a.close()
      },
      error: () => {},
    })
    expect(() => d.decode({})).toThrow(/before configure/)
    d.configure({ codec: 'opus', sampleRate: 48000, numberOfChannels: 2 })
    d.decode(codecs.createChunk({ type: 'key', timestamp: 0, data: new Uint8Array(7) }))
    expect(codecs.last().chunks).toEqual([{ type: 'key', timestamp: 0, bytes: 7 }])
    const a = codecs.last().emit(0, 960, 0.25)
    expect([a.duration, a.sampleRate, a.numberOfChannels, a.format, a.closed]).toEqual([20000, 48000, 2, 'f32-planar', true])
    expect(out).toEqual([{ timestamp: 0, frames: 960, first: 0.25 }])

    let flushed = false
    const flush = d.flush().then(() => (flushed = true))
    await Promise.resolve()
    expect(flushed).toBe(false)
    codecs.last().settle()
    await flush
    expect(flushed).toBe(true)

    const pending = d.flush()
    d.reset()
    await expect(pending).rejects.toMatchObject({ name: 'AbortError' })
    expect(d.state).toBe('unconfigured')
    expect(codecs.last().resets).toBe(1)
  })

  it('fail closes the decoder and calls error', () => {
    const codecs = fakeAudioCodecs()
    const errors: unknown[] = []
    codecs.createDecoder({ output: () => {}, error: (e) => errors.push(e) })
    codecs.last().fail()
    expect(codecs.last().state).toBe('closed')
    expect(errors).toHaveLength(1)
  })
})
