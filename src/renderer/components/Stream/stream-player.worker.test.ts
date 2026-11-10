// The worker player's draw code (fix 4), fail loud, with a fake
// OffscreenCanvas and the kit's fake WebCodecs: stale JPEGs are acked but
// not drawn, keyframe asks pass through (once for a run of deltas), VP8
// frames go to a `vp8` decoder, the next keyframe after a decode error
// recovers, a held view acks without drawing, the probe reports what the
// worker decodes, the centre pixel is sampled only when asked, and the
// thumbnail comes from the canvas.

import { describe, expect, it, vi } from 'vitest'
import { WorkerPlayerCore, type FromWorker, type OffscreenLike, type ToWorker } from './stream-player.worker'
import type { WebCodecsLike } from './stream-video'
import { fakeWebCodecs, H264_DELTA as DELTA, H264_KEY as KEY, VP8_DELTA, VP8_KEY } from './testing'

function canvas(withContext = true): OffscreenLike & { drawImage: ReturnType<typeof vi.fn> } {
  const drawImage = vi.fn()
  return {
    width: 0,
    height: 0,
    drawImage,
    getContext: () => (withContext ? { drawImage, getImageData: () => ({ data: [200, 10, 20, 255] }) } : null),
    convertToBlob: async () => new Blob([new Uint8Array([0xff, 0xd8])], { type: 'image/jpeg' }),
  }
}

function core(opts: { codecs?: WebCodecsLike | null; ctx?: boolean; jpeg?: boolean } = {}) {
  const out: FromWorker[] = []
  const c = canvas(opts.ctx !== false)
  const decoded: number[] = []
  const w = new WorkerPlayerCore({
    post: (m) => out.push(m),
    codecs: opts.codecs ?? null,
    decodeJpeg:
      opts.jpeg === false
        ? null
        : async (blob) => {
            decoded.push((await blob.arrayBuffer()).byteLength)
            return { width: 4, height: 2, close: () => {} }
          },
    now: () => 5000,
  })
  w.handle({ t: 'init', canvas: c, sample: false })
  return { w, out, c, decoded }
}

function jpegFrame(seq: number): ToWorker {
  const buf = new ArrayBuffer(16 + 3)
  new Uint8Array(buf, 16).set([0xff, 0xd8, 0xff])
  return { t: 'frame', kind: 'jpeg', seq, buf, offset: 16, widthPx: 0, heightPx: 0, key: false }
}

function videoFrame(seq: number, key: boolean, kind: 'h264' | 'vp8' = 'h264'): ToWorker {
  const au = kind === 'vp8' ? (key ? VP8_KEY : VP8_DELTA) : key ? KEY : DELTA
  const buf = new ArrayBuffer(24 + au.byteLength)
  new Uint8Array(buf, 24).set(au)
  return { t: 'frame', kind, seq, buf, offset: 24, widthPx: 411, heightPx: 915, key }
}

const flush = async (): Promise<void> => {
  for (let i = 0; i < 10; i++) await Promise.resolve()
  await new Promise((r) => setTimeout(r, 0))
}

describe('WorkerPlayerCore', () => {
  it('draws only the newest JPEG and acks every seq, in order', async () => {
    const { w, out, c, decoded } = core()
    w.handle(jpegFrame(1))
    w.handle(jpegFrame(2))
    await flush()
    expect(c.drawImage).toHaveBeenCalledTimes(1)
    // Only the JPEG bytes, past the 16-byte header.
    expect(decoded).toEqual([3])
    expect([c.width, c.height]).toEqual([4, 2])
    expect(out).toEqual([
      { t: 'ack', seq: 1 },
      { t: 'drawn', seq: 2, decodedAt: 5000, drawnAt: 5000, px: null },
      { t: 'ack', seq: 2 },
    ])
  })

  it('a held view acks without drawing; the centre pixel only while sampling', async () => {
    const { w, out, c } = core()
    w.handle({ t: 'hold', on: true })
    w.handle(jpegFrame(1))
    await flush()
    expect(c.drawImage).not.toHaveBeenCalled()
    expect(out).toEqual([{ t: 'ack', seq: 1 }])
    w.handle({ t: 'hold', on: false })
    w.handle({ t: 'sample', on: true })
    w.handle(jpegFrame(2))
    await flush()
    expect(out.find((m) => m.t === 'drawn')).toEqual({ t: 'drawn', seq: 2, decodedAt: 5000, drawnAt: 5000, px: [200, 10, 20] })
  })

  it('H.264: a delta before any keyframe is acked and asks for one; a keyframe decodes, draws cropped, acks', async () => {
    const codecs = fakeWebCodecs()
    const { w, out, c } = core({ codecs })
    w.handle(videoFrame(1, false))
    expect(out).toEqual([{ t: 'ack', seq: 1 }, { t: 'keyframe' }])
    w.handle(videoFrame(2, true))
    expect(codecs.last().chunks.map((k) => k.timestamp)).toEqual([2])
    const frame = { timestamp: 2, displayWidth: 412, displayHeight: 916, close: vi.fn() }
    codecs.last().init.output(frame)
    expect(c.drawImage).toHaveBeenCalledWith(frame, 0, 0, 411, 915, 0, 0, 411, 915)
    expect(frame.close).toHaveBeenCalled()
    expect(out.slice(2)).toEqual([
      { t: 'drawn', seq: 2, decodedAt: 5000, drawnAt: 5000, px: null },
      { t: 'ack', seq: 2 },
    ])
    // The daemon fell back to JPEG: what the decoder held is acked.
    w.handle(videoFrame(3, false))
    w.handle({ t: 'drop-video' })
    expect(out.at(-1)).toEqual({ t: 'ack', seq: 3 })
  })

  it('H.264 with no WebCodecs in the worker: ack and fail (the tab goes JPEG)', () => {
    const { w, out } = core({ codecs: null })
    w.handle(videoFrame(1, true))
    expect(out[0]).toEqual({ t: 'ack', seq: 1 })
    expect(out[1]).toMatchObject({ t: 'failed' })
  })

  it('VP8 frames go to a `vp8` decoder; deltas before the key are dropped with one keyframe ask', () => {
    const codecs = fakeWebCodecs()
    const { w, out, c } = core({ codecs })
    w.handle(videoFrame(1, false, 'vp8'))
    w.handle(videoFrame(2, false, 'vp8'))
    expect(codecs.decoders).toHaveLength(0)
    expect(out).toEqual([{ t: 'ack', seq: 1 }, { t: 'keyframe' }, { t: 'ack', seq: 2 }])
    w.handle(videoFrame(3, true, 'vp8'))
    w.handle(videoFrame(4, false, 'vp8'))
    const d = codecs.last()
    expect(d.configs).toEqual([{ codec: 'vp8', optimizeForLatency: true }])
    expect(d.chunks.map((k) => `${k.type}:${k.timestamp}`)).toEqual(['key:3', 'delta:4'])
    d.emit(3, 411, 915)
    expect(c.drawImage).toHaveBeenCalledTimes(1)
    expect(out.slice(3)).toEqual([
      { t: 'drawn', seq: 3, decodedAt: 5000, drawnAt: 5000, px: null },
      { t: 'ack', seq: 3 },
    ])
  })

  it('after a decode error nothing is drawn until the next keyframe, which recovers', () => {
    const codecs = fakeWebCodecs()
    const { w, out, c } = core({ codecs })
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => {})
    w.handle(videoFrame(1, true, 'vp8'))
    codecs.last().fail()
    w.handle(videoFrame(2, false, 'vp8'))
    expect(out).toEqual([{ t: 'ack', seq: 1 }, { t: 'keyframe' }, { t: 'ack', seq: 2 }])
    expect(c.drawImage).not.toHaveBeenCalled()
    w.handle(videoFrame(3, true, 'vp8'))
    expect(codecs.decoders).toHaveLength(2)
    codecs.last().emit(3, 411, 915)
    expect(c.drawImage).toHaveBeenCalledTimes(1)
    expect(out.slice(3)).toEqual([
      { t: 'drawn', seq: 3, decodedAt: 5000, drawnAt: 5000, px: null },
      { t: 'ack', seq: 3 },
    ])
    warn.mockRestore()
  })

  it('the probe says what the worker decodes; the thumbnail comes from the canvas', async () => {
    const yes = core({ codecs: fakeWebCodecs({ h264: true, vp8: true }) })
    yes.w.handle({ t: 'probe' })
    await flush()
    expect(yes.out).toEqual([{ t: 'probe', canvas2d: true, decode: { h264: true, vp8: true, jpeg: true } }])
    const vp8Only = core({ codecs: fakeWebCodecs({ h264: false, vp8: true }), jpeg: false })
    vp8Only.w.handle({ t: 'probe' })
    await flush()
    expect(vp8Only.out).toEqual([{ t: 'probe', canvas2d: true, decode: { h264: false, vp8: true, jpeg: false } }])
    const no = core({ codecs: null, ctx: false })
    no.w.handle({ t: 'probe' })
    await flush()
    expect(no.out).toEqual([{ t: 'probe', canvas2d: false, decode: { h264: false, vp8: false, jpeg: true } }])
    yes.c.width = 4
    yes.c.height = 2
    yes.w.handle({ t: 'thumb', id: 7 })
    await flush()
    const thumb = yes.out.at(-1)
    expect(thumb).toMatchObject({ t: 'thumb', id: 7 })
    expect(thumb && thumb.t === 'thumb' && thumb.blob ? thumb.blob.size : 0).toBe(2)
  })
})
