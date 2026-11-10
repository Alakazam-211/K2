// The worker player's protocol (fix 4), fail loud, with an in-process fake
// worker on a real MessageChannel: frames are transferred (the source
// buffer is detached), acks and `drawn` come back in order with times on
// this side's clock, a held view still acks, the centre pixel is sampled
// on request, the thumbnail round-trips, keyframe asks reach the pane, and a
// worker error hands the stream back to the main thread. Plus the first
// support check and its switch (the neutral key and the Mobile tab's old
// one).

import { afterEach, describe, expect, it } from 'vitest'
import { canUseWorkerPlayer, LEGACY_PLAYER_PREF_KEY, PLAYER_PREF_KEY, StreamPlayer, type PlayerHandlers, type WorkerLike } from './stream-player'
import { WorkerPlayerCore, type OffscreenLike, type ToWorker } from './stream-player.worker'
import { encodeFrame, encodeVideoFrame, VIDEO_CODEC_VP8, type StreamFrame } from './stream-wire'
import { fakeWebCodecs, VP8_DELTA } from './testing'

const ports: MessagePort[] = []

afterEach(() => {
  for (const p of ports.splice(0)) p.close()
})

/** A worker made of a MessageChannel and the real worker core. The canvas
 *  (not cloneable here) goes to the core directly. */
function channelWorker(canvas: OffscreenLike, codecs: ReturnType<typeof fakeWebCodecs> | null = null): WorkerLike & { fail(): void } {
  const { port1, port2 } = new MessageChannel()
  ports.push(port1, port2)
  const core = new WorkerPlayerCore({
    post: (m) => port2.postMessage(m),
    codecs,
    decodeJpeg: async () => ({ width: 4, height: 2, close: () => {} }),
    now: () => 1_000_250,
  })
  port2.onmessage = (ev) => core.handle(ev.data as ToWorker)
  const w: WorkerLike & { fail(): void } = {
    onmessage: null,
    onerror: null,
    postMessage(msg, transfer) {
      const m = msg as ToWorker
      if (m.t === 'init') {
        core.handle({ ...m, canvas })
        return
      }
      port1.postMessage(msg, transfer ?? [])
    },
    terminate() {
      port1.close()
      port2.close()
    },
    fail() {
      w.onerror?.({ message: 'boom' })
    },
  }
  port1.onmessage = (ev) => w.onmessage?.({ data: ev.data })
  return w
}

function offscreen(): OffscreenLike {
  return {
    width: 0,
    height: 0,
    getContext: () => ({ drawImage: () => {}, getImageData: () => ({ data: [9, 8, 7, 255] }) }),
    convertToBlob: async () => new Blob([new Uint8Array([0xff, 0xd8, 0xff, 0xd9])], { type: 'image/jpeg' }),
  }
}

function recorder() {
  const events: string[] = []
  const times: number[][] = []
  const lost: string[] = []
  const h: PlayerHandlers = {
    drawn: (seq, decodedAt, drawnAt, px) => {
      events.push(`drawn ${seq}${px ? ` ${px.join(',')}` : ''}`)
      times.push([decodedAt, drawnAt])
    },
    ack: (seq) => events.push(`ack ${seq}`),
    needKeyframe: () => events.push('keyframe'),
    videoFailed: (why) => events.push(`failed ${why}`),
    lost: (why) => lost.push(why),
  }
  return { h, events, times, lost }
}

function jpeg(seq: number): StreamFrame & { raw: ArrayBuffer } {
  const raw = encodeFrame({ seq, widthPx: 4, heightPx: 2, widthPt: 4, heightPt: 2 }, new Uint8Array([0xff, 0xd8, 0xff]))
  return { seq, widthPt: 4, heightPt: 2, source: { kind: 'jpeg', data: new Blob([]) }, raw }
}

async function until(pred: () => boolean): Promise<void> {
  for (let i = 0; i < 200 && !pred(); i++) await new Promise((r) => setTimeout(r, 5))
  expect(pred()).toBe(true)
}

describe('StreamPlayer over a MessageChannel', () => {
  it('transfers each frame, gets acks and drawn back in order, still acks while held, samples on request', async () => {
    const w = channelWorker(offscreen())
    const r = recorder()
    const p = new StreamPlayer(w, {}, r.h, { transferCanvas: false, timeOrigin: 1_000_000 })
    expect(await p.ready).toEqual({ canvas2d: true, decode: { h264: false, vp8: false, jpeg: true } })
    const f1 = jpeg(1)
    expect(p.frame(f1)).toBe(true)
    expect(f1.raw.byteLength).toBe(0)
    await until(() => r.events.includes('ack 1'))
    p.frame(jpeg(2))
    await until(() => r.events.includes('ack 2'))
    expect(r.events).toEqual(['drawn 1', 'ack 1', 'drawn 2', 'ack 2'])
    expect(r.times[0]).toEqual([250, 250])
    expect(p.started).toBe(true)
    p.hold(true)
    p.frame(jpeg(3))
    await until(() => r.events.includes('ack 3'))
    expect(r.events.slice(4)).toEqual(['ack 3'])
    p.hold(false)
    p.sample(true)
    p.frame(jpeg(4))
    await until(() => r.events.includes('ack 4'))
    expect(r.events.slice(5)).toEqual(['drawn 4 9,8,7', 'ack 4'])
    expect(p.lastPx).toEqual([9, 8, 7])
    // A painted (stub) frame has no buffer to hand over.
    expect(p.frame({ seq: 5, widthPt: 1, heightPt: 1, source: { kind: 'paint', paint: () => {} } })).toBe(false)
    p.close()
  })

  it('the thumbnail round-trips; a worker error hands back to the main thread once', async () => {
    const w = channelWorker(offscreen())
    const r = recorder()
    const p = new StreamPlayer(w, {}, r.h, { transferCanvas: false })
    await p.ready
    p.frame(jpeg(1))
    await until(() => r.events.includes('ack 1'))
    const blob = await p.thumb(2000)
    expect(blob?.size).toBe(4)
    w.fail()
    w.fail()
    expect(r.lost).toEqual(['the player worker failed: boom'])
    expect(p.alive).toBe(false)
    expect(await p.thumb()).toBeNull()
    expect(p.frame(jpeg(2))).toBe(false)
  })
})

describe('StreamPlayer video', () => {
  it('a VP8 delta with no keyframe is transferred, dropped and asks the pane for a keyframe once', async () => {
    const codecs = fakeWebCodecs({ h264: false, vp8: true })
    const w = channelWorker(offscreen(), codecs)
    const r = recorder()
    const p = new StreamPlayer(w, {}, r.h, { transferCanvas: false })
    expect((await p.ready).decode).toEqual({ h264: false, vp8: true, jpeg: true })
    for (const seq of [1, 2]) {
      const raw = encodeVideoFrame({ seq, widthPx: 640, heightPx: 360, widthPt: 640, heightPt: 360, key: false, codec: VIDEO_CODEC_VP8 }, VP8_DELTA)
      const f: StreamFrame = { seq, widthPt: 640, heightPt: 360, source: { kind: 'vp8', key: false, data: new Uint8Array(raw, 24), widthPx: 640, heightPx: 360 }, raw }
      expect(p.frame(f)).toBe(true)
    }
    await until(() => r.events.includes('ack 2'))
    expect(r.events).toEqual(['ack 1', 'keyframe', 'ack 2'])
    expect(codecs.decoders).toHaveLength(0)
    p.close()
  })
})

describe('canUseWorkerPlayer', () => {
  const env = (over: Record<string, unknown> = {}) => ({
    Worker: function () {},
    HTMLCanvasElement: { prototype: { transferControlToOffscreen: () => ({}) } },
    createImageBitmap: () => {},
    localStorage: { getItem: () => null },
    ...over,
  })
  it('needs Worker, transferControlToOffscreen and createImageBitmap; the switch forces the main thread', () => {
    expect(canUseWorkerPlayer(env())).toBe(true)
    expect(canUseWorkerPlayer(env({ HTMLCanvasElement: { prototype: {} } }))).toBe(false)
    expect(canUseWorkerPlayer(env({ Worker: undefined }))).toBe(false)
    expect(canUseWorkerPlayer(env({ createImageBitmap: undefined }))).toBe(false)
    expect(canUseWorkerPlayer(env({ localStorage: { getItem: (k: string) => (k === PLAYER_PREF_KEY ? 'main' : null) } }))).toBe(false)
  })

  it('a stored Mobile-tab setting (k2.mobile.streamPlayer = main) still forces the main thread', () => {
    expect(LEGACY_PLAYER_PREF_KEY).toBe('k2.mobile.streamPlayer')
    expect(PLAYER_PREF_KEY).toBe('k2.stream.player')
    const stored = new Map([['k2.mobile.streamPlayer', 'main']])
    expect(canUseWorkerPlayer(env({ localStorage: { getItem: (k: string) => stored.get(k) ?? null } }))).toBe(false)
    stored.set('k2.mobile.streamPlayer', 'worker')
    expect(canUseWorkerPlayer(env({ localStorage: { getItem: (k: string) => stored.get(k) ?? null } }))).toBe(true)
  })
})
