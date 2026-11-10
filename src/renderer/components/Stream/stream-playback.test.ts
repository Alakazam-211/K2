// The engine's main-thread player and the worker / main switch (S6b), fail
// loud: JPEG frames drawn newest-only with every seq acked (held: acked, not
// drawn), a stub's painted frame at 2×, H.264 through a fake WebCodecs
// (drawn cropped, acked per frame, a delta first asks for a keyframe), no
// acks for a socket that ended, the daemon's JPEG switch freeing the
// decoder; then the switch: no worker here means main from the start, a
// worker's probe settles it, and a worker lost mid-stream hands the same
// socket to this thread, where the next keyframe recovers the video.

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { MainThreadPlayer, type MainPlayerHandlers } from './stream-main-player'
import { StreamPlayback } from './stream-playback'
import { encodeFrame, encodeVideoFrame, type DecodeSupport, type StreamFrame } from './stream-wire'
import { fakePlayerWorker, fakeWebCodecs, H264_DELTA, H264_KEY, type FakeWebCodecs } from './testing'

function fakeCanvas() {
  const drawImage = vi.fn()
  const canvas = { width: 0, height: 0, getContext: () => ({ drawImage }), transferControlToOffscreen: vi.fn(() => ({ offscreen: true })) }
  return { canvas, el: canvas as unknown as HTMLCanvasElement, drawImage }
}

function recorder() {
  const events: string[] = []
  const decoded: (number | null)[] = []
  const kinds: string[] = []
  const h: MainPlayerHandlers & { player(kind: 'worker' | 'main', why: string | null): void } = {
    drawn: (seq, decodedAt) => {
      events.push(`drawn ${seq}`)
      decoded.push(decodedAt)
    },
    ack: (seq) => events.push(`ack ${seq}`),
    needKeyframe: () => events.push('keyframe'),
    videoFailed: (why) => events.push(`failed ${why}`),
    player: (kind, why) => kinds.push(why ? `${kind}: ${why}` : kind),
  }
  return { h, events, decoded, kinds }
}

const jpeg = (seq: number): StreamFrame => {
  const raw = encodeFrame({ seq, widthPx: 4, heightPx: 2, widthPt: 4, heightPt: 2 }, new Uint8Array([0xff, 0xd8]))
  return { seq, widthPt: 4, heightPt: 2, source: { kind: 'jpeg', data: new Blob([new Uint8Array([0xff, 0xd8])]) }, raw }
}

const h264 = (seq: number, key: boolean): StreamFrame => {
  const au = key ? H264_KEY : H264_DELTA
  const raw = encodeVideoFrame({ seq, widthPx: 411, heightPx: 915, widthPt: 411, heightPt: 915, key }, au)
  return { seq, widthPt: 411, heightPt: 915, source: { kind: 'h264', key, data: new Uint8Array(raw, 24), widthPx: 411, heightPx: 915 }, raw }
}

async function until(pred: () => boolean): Promise<void> {
  for (let i = 0; i < 200 && !pred(); i++) await new Promise((r) => setTimeout(r, 1))
  expect(pred()).toBe(true)
}

beforeEach(() => {
  vi.stubGlobal('createImageBitmap', async () => ({ width: 4, height: 2, close: () => {} }))
})

afterEach(() => {
  vi.unstubAllGlobals()
  vi.restoreAllMocks()
})

describe('MainThreadPlayer', () => {
  it('draws only the newest JPEG, acks every seq, acks without drawing while held, paints a stub frame at 2×', async () => {
    const { el, canvas, drawImage } = fakeCanvas()
    const r = recorder()
    let held = false
    const p = new MainThreadPlayer(r.h, { canvas: () => el, held: () => held })
    for (const seq of [1, 2, 3]) p.frame(jpeg(seq))
    await until(() => r.events.includes('ack 3'))
    expect(r.events).toEqual(['ack 1', 'ack 2', 'drawn 3', 'ack 3'])
    expect(drawImage).toHaveBeenCalledTimes(1)
    expect([canvas.width, canvas.height]).toEqual([4, 2])
    expect(typeof r.decoded[0]).toBe('number')
    held = true
    p.frame(jpeg(4))
    await until(() => r.events.includes('ack 4'))
    expect(r.events.slice(4)).toEqual(['ack 4'])
    held = false
    const paint = vi.fn()
    p.frame({ seq: 5, widthPt: 10, heightPt: 20, source: { kind: 'paint', paint } })
    await until(() => r.events.includes('ack 5'))
    expect(paint).toHaveBeenCalledWith(expect.anything(), 20, 40)
    expect(r.events.slice(5)).toEqual(['drawn 5', 'ack 5'])
    // A painted frame has no decode step.
    expect(r.decoded[1]).toBeNull()
  })

  it('decodes H.264: a delta first asks for a keyframe; each output drawn cropped and acked; not drawn while held', () => {
    const codecs = fakeWebCodecs({ h264: true })
    const { el, canvas, drawImage } = fakeCanvas()
    const r = recorder()
    let held = false
    const p = new MainThreadPlayer(r.h, { canvas: () => el, held: () => held, codecs: () => codecs })
    p.frame(h264(1, false))
    expect(r.events).toEqual(['ack 1', 'keyframe'])
    expect(codecs.decoders).toHaveLength(0)
    p.frame(h264(2, true))
    expect(codecs.last().configs).toEqual([{ codec: 'avc1.640C1F', optimizeForLatency: true }])
    const f2 = codecs.last().emit(2)
    expect(drawImage).toHaveBeenCalledWith(f2, 0, 0, 411, 915, 0, 0, 411, 915)
    expect([canvas.width, canvas.height]).toEqual([411, 915])
    expect(f2.closed).toBe(true)
    p.frame(h264(3, false))
    held = true
    codecs.last().emit(3)
    expect(drawImage).toHaveBeenCalledTimes(1)
    expect(r.events.slice(2)).toEqual(['drawn 2', 'ack 2', 'ack 3'])
  })

  it('a socket that ended acks nothing more; the daemon switching to JPEG frees what the decoder held', async () => {
    const codecs = fakeWebCodecs({ h264: true })
    const { el, drawImage } = fakeCanvas()
    const r = recorder()
    const p = new MainThreadPlayer(r.h, { canvas: () => el, held: () => false, codecs: () => codecs })
    p.frame(h264(1, true))
    p.frame(jpeg(2))
    p.reset()
    codecs.decoders[0].emit(1)
    await new Promise((res) => setTimeout(res, 5))
    expect(r.events).toEqual([])
    expect(drawImage).not.toHaveBeenCalled()
    // The next socket starts fresh: video waits for a keyframe.
    p.frame(h264(10, true))
    p.frame(h264(11, false))
    expect(p.queued).toBe(2)
    p.dropVideo()
    expect(r.events).toEqual(['ack 10', 'ack 11'])
    expect(codecs.last().state).toBe('closed')
  })

  it('no WebCodecs here: the frame is acked and video reported failed', () => {
    vi.spyOn(console, 'warn').mockImplementation(() => {})
    const r = recorder()
    const p = new MainThreadPlayer(r.h, { canvas: () => fakeCanvas().el, held: () => false, codecs: () => null })
    p.frame(h264(1, true))
    expect(r.events).toEqual(['failed no WebCodecs here', 'ack 1'])
  })
})

describe('StreamPlayback', () => {
  const H264_ONLY: DecodeSupport = { h264: true, vp8: false, jpeg: true }
  const workerEnv = {
    Worker: function Worker() {},
    HTMLCanvasElement: { prototype: { transferControlToOffscreen: () => ({}) } },
    createImageBitmap: () => {},
    localStorage: { getItem: () => null },
  }

  it('no worker here: main from the start, frames drawn on this thread', async () => {
    const { el, drawImage } = fakeCanvas()
    const r = recorder()
    const pb = new StreamPlayback(r.h, { canvas: () => el, held: () => false, env: { ...workerEnv, Worker: undefined } })
    expect(pb.kind).toBe('main')
    pb.start()
    expect(r.kinds).toEqual([])
    pb.reset()
    pb.frame(jpeg(1))
    await until(() => r.events.includes('ack 1'))
    expect(r.events).toEqual(['drawn 1', 'ack 1'])
    expect(drawImage).toHaveBeenCalledTimes(1)
    expect(pb.workerDecode).toBeNull()
  })

  it('the stored k2.stream.player = main pin keeps it on this thread', () => {
    const pinned = { ...workerEnv, localStorage: { getItem: (k: string) => (k === 'k2.stream.player' ? 'main' : null) } }
    expect(new StreamPlayback(recorder().h, { canvas: () => null, held: () => false, env: pinned }).kind).toBe('main')
  })

  it('a worker with no OffscreenCanvas 2D context, or no canvas yet, falls back to main', async () => {
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => {})
    const r = recorder()
    const w = fakePlayerWorker({ canvas2d: false, decode: H264_ONLY })
    const pb = new StreamPlayback(r.h, { canvas: () => fakeCanvas().el, held: () => false, env: workerEnv, startWorker: () => w })
    expect(pb.kind).toBe('pending')
    pb.start()
    await until(() => pb.kind !== 'pending')
    expect(pb.kind).toBe('main')
    expect(r.kinds).toEqual(['main: no OffscreenCanvas 2D context in the worker'])
    const r2 = recorder()
    const pb2 = new StreamPlayback(r2.h, { canvas: () => null, held: () => false, env: workerEnv, startWorker: () => w })
    pb2.start()
    expect(r2.kinds).toEqual(['main: no canvas yet'])
    expect(warn).toHaveBeenCalledTimes(2)
  })

  it('a worker lost mid-stream: the same socket goes to this thread, acks per frame, and the next keyframe recovers', async () => {
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => {})
    const codecs: FakeWebCodecs = fakeWebCodecs({ h264: true })
    const first = fakeCanvas()
    let el = first.el
    const r = recorder()
    const w = fakePlayerWorker({ canvas2d: true, decode: H264_ONLY })
    const pb = new StreamPlayback(r.h, { canvas: () => el, held: () => false, env: workerEnv, startWorker: () => w, codecs: () => codecs })
    pb.start({ sample: true })
    await until(() => pb.kind === 'worker')
    expect(r.kinds).toEqual(['worker'])
    expect(pb.workerDecode).toEqual(H264_ONLY)
    expect(first.canvas.transferControlToOffscreen).toHaveBeenCalledTimes(1)
    expect(w.posted[0].msg).toMatchObject({ t: 'init', canvas: { offscreen: true }, sample: true })
    pb.reset()
    // On the worker: frames are transferred; its acks come back.
    const f1 = h264(1, true)
    pb.frame(f1)
    const sent = w.posted.find((p) => p.msg.t === 'frame')
    expect(sent?.msg).toMatchObject({ t: 'frame', kind: 'h264', seq: 1, offset: 24, key: true })
    expect(sent?.transfer).toEqual([f1.raw])
    w.reply({ t: 'drawn', seq: 1, decodedAt: 0, drawnAt: 0, px: null })
    w.reply({ t: 'ack', seq: 1 })
    expect(r.events).toEqual(['drawn 1', 'ack 1'])
    // A painted (stub) frame can't go to the worker: acked at once.
    pb.frame({ seq: 2, widthPt: 1, heightPt: 1, source: { kind: 'paint', paint: () => {} } })
    expect(r.events.slice(2)).toEqual(['ack 2'])
    // The worker dies. The pane re-keys its canvas (a new element).
    w.onerror?.({ message: 'worker crashed' })
    expect(pb.kind).toBe('main')
    expect(r.kinds).toEqual(['worker', 'main: the player worker failed: worker crashed'])
    expect(pb.workerDecode).toBeNull()
    const second = fakeCanvas()
    el = second.el
    // This thread has no decoder yet: deltas are acked and a keyframe asked.
    pb.frame(h264(3, false))
    pb.frame(h264(4, false))
    expect(r.events.slice(3)).toEqual(['ack 3', 'keyframe', 'ack 4'])
    pb.frame(h264(5, true))
    pb.frame(h264(6, false))
    codecs.last().emit(5)
    codecs.last().emit(6)
    expect(r.events.slice(6)).toEqual(['drawn 5', 'ack 5', 'drawn 6', 'ack 6'])
    expect(second.drawImage).toHaveBeenCalledTimes(2)
    expect(first.drawImage).not.toHaveBeenCalled()
    // Nothing more reaches the dead worker.
    expect(w.posted.filter((p) => p.msg.t === 'frame')).toHaveLength(1)
    expect(warn).toHaveBeenCalledTimes(1)
  })

  it('a probe that never answers falls back after PROBE_TIMEOUT_MS', () => {
    vi.useFakeTimers()
    try {
      vi.spyOn(console, 'warn').mockImplementation(() => {})
      const r = recorder()
      const silent = { posted: [] as unknown[], onmessage: null, onerror: null, postMessage() {}, terminate: vi.fn() }
      const pb = new StreamPlayback(r.h, { canvas: () => fakeCanvas().el, held: () => false, env: workerEnv, startWorker: () => silent })
      pb.start()
      vi.advanceTimersByTime(3000)
      expect(pb.kind).toBe('main')
      expect(r.kinds).toEqual(['main: the worker did not answer its probe'])
      expect(silent.terminate).toHaveBeenCalled()
    } finally {
      vi.useRealTimers()
    }
  })
})
