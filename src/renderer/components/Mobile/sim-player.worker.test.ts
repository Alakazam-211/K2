// The worker player's draw code (fix 4), fail loud, with a fake
// OffscreenCanvas and a fake WebCodecs: stale JPEGs are acked but not
// drawn, H.264 keyframe asks pass through, a held view acks without
// drawing, the probe reports what the worker has, the centre pixel is
// sampled only when asked, and the thumbnail comes from the canvas.

import { describe, expect, it, vi } from 'vitest'
import { WorkerPlayerCore, type FromWorker, type OffscreenLike, type ToWorker } from './sim-player.worker'
import type { VideoDecoderInitLike, WebCodecsLike } from './sim-video'

const SC = [0, 0, 0, 1]
const KEY = new Uint8Array([...SC, 0x67, 0x64, 0x0c, 0x1f, 0xac, ...SC, 0x68, 0xee, ...SC, 0x65, 0x88])
const DELTA = new Uint8Array([...SC, 0x41, 0x9a])

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

function fakeCodecs(supported = true): WebCodecsLike & { inits: VideoDecoderInitLike[]; decoded: number[] } {
  const inits: VideoDecoderInitLike[] = []
  const decoded: number[] = []
  return {
    inits,
    decoded,
    createDecoder(init) {
      inits.push(init)
      let state = 'unconfigured'
      return {
        get state() {
          return state
        },
        configure: () => {
          state = 'configured'
        },
        decode: (chunk: unknown) => decoded.push((chunk as { timestamp: number }).timestamp),
        close: () => {
          state = 'closed'
        },
      }
    },
    createChunk: (init) => init,
    isConfigSupported: async () => supported,
  }
}

function core(opts: { codecs?: WebCodecsLike | null; ctx?: boolean } = {}) {
  const out: FromWorker[] = []
  const c = canvas(opts.ctx !== false)
  const decoded: number[] = []
  const w = new WorkerPlayerCore({
    post: (m) => out.push(m),
    codecs: opts.codecs ?? null,
    decodeJpeg: async (blob) => {
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

function videoFrame(seq: number, key: boolean): ToWorker {
  const au = key ? KEY : DELTA
  const buf = new ArrayBuffer(24 + au.byteLength)
  new Uint8Array(buf, 24).set(au)
  return { t: 'frame', kind: 'h264', seq, buf, offset: 24, widthPx: 411, heightPx: 915, key }
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
    const codecs = fakeCodecs()
    const { w, out, c } = core({ codecs })
    w.handle(videoFrame(1, false))
    expect(out).toEqual([{ t: 'ack', seq: 1 }, { t: 'keyframe' }])
    w.handle(videoFrame(2, true))
    expect(codecs.decoded).toEqual([2])
    const frame = { timestamp: 2, displayWidth: 412, displayHeight: 916, close: vi.fn() }
    codecs.inits[0].output(frame)
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

  it('the probe says what the worker has; the thumbnail comes from the canvas', async () => {
    const yes = core({ codecs: fakeCodecs(true) })
    yes.w.handle({ t: 'probe' })
    await flush()
    expect(yes.out).toEqual([{ t: 'probe', canvas2d: true, video: true }])
    const no = core({ codecs: null, ctx: false })
    no.w.handle({ t: 'probe' })
    await flush()
    expect(no.out).toEqual([{ t: 'probe', canvas2d: false, video: false }])
    yes.c.width = 4
    yes.c.height = 2
    yes.w.handle({ t: 'thumb', id: 7 })
    await flush()
    const thumb = yes.out.at(-1)
    expect(thumb).toMatchObject({ t: 'thumb', id: 7 })
    expect(thumb && thumb.t === 'thumb' && thumb.blob ? thumb.blob.size : 0).toBe(2)
  })
})
