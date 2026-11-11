// The engine's video decode (stream-video.ts) against the kit's fake
// WebCodecs, fail loud: SPS → codec string, VP8 keyframes by their start
// code, configure on the first keyframe, in-order decode with an ack per
// frame, held frames acked but not drawn, a delta with no keyframe dropped
// and a keyframe asked once (again only after the re-ask gap), decoder
// errors ask again, the next keyframe recovers, three in a row give up
// (JPEG fallback), and the support probe (H.264, VP8, JPEG; the pins).

import { afterEach, describe, expect, it, vi } from 'vitest'
import {
  codecFromAnnexB,
  DECODE_BACKLOG,
  decodeSupport,
  isVp8Keyframe,
  KEYFRAME_REASK_MS,
  MAX_DECODE_ERRORS,
  probeDecoders,
  resetVideoSupportForTests,
  splitNals,
  VideoSink,
  type WebCodecsLike,
} from './stream-video'
import { fakeWebCodecs, H264_DELTA as DELTA, H264_KEY as KEY, VP8_DELTA, VP8_KEY } from './testing'

function sinkWith(codecs: WebCodecsLike, held = false, clock = { t: 0 }) {
  const log: string[] = []
  const sink = new VideoSink({
    codecs,
    draw: (f, w, h) => log.push(`draw:${f.timestamp}:${w}x${h}`),
    ack: (seq) => log.push(`ack:${seq}`),
    needKeyframe: () => log.push('key?'),
    failed: (why) => log.push(`failed:${why}`),
    held: () => held,
    now: () => clock.t,
  })
  return { sink, log, clock }
}

const h264 = (seq: number, key: boolean, data: Uint8Array = key ? KEY : DELTA, widthPx = 4, heightPx = 8) => ({ seq, codec: 'h264' as const, key, data, widthPx, heightPx })
const vp8 = (seq: number, key: boolean) => ({ seq, codec: 'vp8' as const, key, data: key ? VP8_KEY : VP8_DELTA, widthPx: 640, heightPx: 360 })

afterEach(() => {
  resetVideoSupportForTests()
  vi.unstubAllGlobals()
})

describe('Annex B and VP8 helpers', () => {
  it('splits NAL units on 3- and 4-byte start codes and reads the codec from the SPS', () => {
    expect(splitNals(KEY).map((n) => n[0])).toEqual([0x67, 0x68, 0x65])
    expect(splitNals(new Uint8Array([0, 0, 1, 0x41, 1, 0, 0, 1, 0x41, 2])).map((n) => Array.from(n))).toEqual([
      [0x41, 1],
      [0x41, 2],
    ])
    expect(codecFromAnnexB(KEY)).toBe('avc1.640C1F')
    expect(codecFromAnnexB(DELTA)).toBeNull()
  })

  it('knows a VP8 key frame by its tag bit and start code', () => {
    expect(isVp8Keyframe(VP8_KEY)).toBe(true)
    expect(isVp8Keyframe(VP8_DELTA)).toBe(false)
    expect(isVp8Keyframe(VP8_KEY.subarray(0, 6))).toBe(false)
  })
})

describe('VideoSink', () => {
  it('configures on the keyframe, decodes in order, draws cropped to the header size and acks each frame', () => {
    const codecs = fakeWebCodecs()
    const { sink, log } = sinkWith(codecs)
    sink.push(h264(10, true, KEY, 411, 915))
    sink.push(h264(11, false, DELTA, 411, 915))
    expect(codecs.decoders).toHaveLength(1)
    const d = codecs.decoders[0]
    expect(d.configs).toEqual([{ codec: 'avc1.640C1F', optimizeForLatency: true }])
    expect(d.chunks).toEqual([
      { type: 'key', timestamp: 10, bytes: KEY.length },
      { type: 'delta', timestamp: 11, bytes: DELTA.length },
    ])
    expect(sink.queued).toBe(2)
    const f = d.emit(10)
    d.emit(11)
    expect(f.closed).toBe(true)
    expect(log).toEqual(['draw:10:411x915', 'ack:10', 'draw:11:411x915', 'ack:11'])
    expect(sink.queued).toBe(0)
  })

  it('VP8: a keyframe configures a `vp8` decoder; deltas decode on it', () => {
    const codecs = fakeWebCodecs()
    const { sink, log } = sinkWith(codecs)
    sink.push(vp8(1, true))
    sink.push(vp8(2, false))
    expect(codecs.decoders).toHaveLength(1)
    const d = codecs.decoders[0]
    expect(d.configs).toEqual([{ codec: 'vp8', optimizeForLatency: true }])
    expect(d.chunks.map((c) => `${c.type}:${c.timestamp}`)).toEqual(['key:1', 'delta:2'])
    d.emit(1, 640, 360)
    d.emit(2, 640, 360)
    expect(log).toEqual(['draw:1:640x360', 'ack:1', 'draw:2:640x360', 'ack:2'])
    // A "key" whose bytes aren't a VP8 key frame is an error, not garbage.
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => {})
    sink.push({ ...vp8(3, true), data: VP8_DELTA })
    expect(d.state).toBe('closed')
    expect(log.slice(4)).toEqual(['ack:3', 'key?'])
    warn.mockRestore()
  })

  it('z3mbpZ: a decoder that holds frames never starves the window: acks on hand-off, once each', () => {
    const codecs = fakeWebCodecs()
    const { sink, log } = sinkWith(codecs)
    sink.push(h264(1, true))
    const d = codecs.decoders[0]
    d.decodeQueueSize = 0
    sink.push(h264(2, false))
    sink.push(h264(3, false))
    sink.push(h264(4, false))
    // Nothing output yet (the decoder holds them), but the window is free.
    expect(log).toEqual(['ack:2', 'ack:3', 'ack:4'])
    d.emit(1)
    d.emit(2)
    d.emit(3)
    d.emit(4)
    // Frame 1 was pushed before the queue size was known: acked on output.
    expect(log).toEqual(['ack:2', 'ack:3', 'ack:4', 'draw:1:4x8', 'ack:1', 'draw:2:4x8', 'draw:3:4x8', 'draw:4:4x8'])
  })

  it('a backed-up decoder (queue over DECODE_BACKLOG) acks on output, so slow decode still slows the stream', () => {
    const codecs = fakeWebCodecs()
    const { sink, log } = sinkWith(codecs)
    sink.push(h264(1, true))
    const d = codecs.decoders[0]
    d.decodeQueueSize = DECODE_BACKLOG + 1
    sink.push(h264(2, false))
    expect(log).toEqual([])
    d.emit(1)
    d.emit(2)
    expect(log).toEqual(['draw:1:4x8', 'ack:1', 'draw:2:4x8', 'ack:2'])
  })

  it('an H.264 keyframe reaches the decoder with the low-latency VUI in its SPS', () => {
    const hex = (s: string): number[] => s.match(/../g)!.map((b) => parseInt(b, 16))
    const real = new Uint8Array([0, 0, 0, 1, ...hex('27640c20ac568084091ea540'), 0, 0, 0, 1, ...hex('28ee3cb0'), 0, 0, 0, 1, 0x25, 0xb8, 0x04])
    const codecs = fakeWebCodecs()
    const { sink } = sinkWith(codecs)
    sink.push(h264(1, true, real))
    const chunk = codecs.decoders[0].chunks[0]
    expect(chunk.bytes).toBeGreaterThan(real.length)
  })

  it('acks a frame the decoder skipped when a later one comes out', () => {
    const codecs = fakeWebCodecs()
    const { sink, log } = sinkWith(codecs)
    sink.push(h264(1, true))
    sink.push(h264(2, false))
    sink.push(h264(3, false))
    codecs.decoders[0].emit(1)
    codecs.decoders[0].emit(3)
    expect(log).toEqual(['draw:1:4x8', 'ack:1', 'ack:2', 'draw:3:4x8', 'ack:3'])
  })

  it('a held view decodes and acks but does not draw', () => {
    const codecs = fakeWebCodecs()
    const { sink, log } = sinkWith(codecs, true)
    sink.push(h264(5, true))
    codecs.decoders[0].emit(5)
    expect(log).toEqual(['ack:5'])
  })

  it('drops a delta with no keyframe before it and asks for one, once; again only after the re-ask gap', () => {
    const codecs = fakeWebCodecs()
    const { sink, log, clock } = sinkWith(codecs)
    sink.push(h264(7, false))
    clock.t = KEYFRAME_REASK_MS - 1
    sink.push(h264(8, false))
    expect(codecs.decoders).toHaveLength(0)
    expect(log).toEqual(['ack:7', 'key?', 'ack:8'])
    // The daemon dropped the ask (its throttle): ask again.
    clock.t = KEYFRAME_REASK_MS
    sink.push(h264(9, false))
    expect(log.slice(3)).toEqual(['ack:9', 'key?'])
    sink.push(h264(10, true))
    expect(codecs.decoders).toHaveLength(1)
  })

  it('after a decoder error deltas are dropped until the next keyframe, which recovers on a new decoder', () => {
    const codecs = fakeWebCodecs()
    const { sink, log } = sinkWith(codecs)
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => {})
    sink.push(vp8(1, true))
    codecs.last().fail()
    sink.push(vp8(2, false))
    expect(log).toEqual(['ack:1', 'key?', 'ack:2'])
    expect(codecs.decoders).toHaveLength(1)
    sink.push(vp8(3, true))
    expect(codecs.decoders).toHaveLength(2)
    codecs.last().emit(3, 640, 360)
    expect(log.slice(3)).toEqual(['draw:3:640x360', 'ack:3'])
    warn.mockRestore()
  })

  it('a decoder error acks what was queued and asks for a keyframe; three in a row give up on video', () => {
    const codecs = fakeWebCodecs()
    const { sink, log } = sinkWith(codecs)
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => {})
    let seq = 0
    for (let round = 1; round <= MAX_DECODE_ERRORS; round++) {
      sink.push(h264(++seq, true))
      sink.push(h264(++seq, false))
      const d = codecs.decoders[codecs.decoders.length - 1]
      d.init.error(new Error(`decode failed ${round}`))
      expect(d.state).toBe('closed')
    }
    expect(log).toEqual(['ack:1', 'ack:2', 'key?', 'ack:3', 'ack:4', 'key?', 'ack:5', 'ack:6', 'failed:The video decoder failed 3 times in a row.'])
    // After giving up every frame is just acked.
    sink.push(h264(99, true))
    expect(log[log.length - 1]).toBe('ack:99')
    expect(codecs.decoders).toHaveLength(3)
    expect(warn).toHaveBeenCalled()
    warn.mockRestore()
  })

  it('a good frame between errors resets the count', () => {
    const codecs = fakeWebCodecs()
    const { sink, log } = sinkWith(codecs)
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => {})
    for (let i = 0; i < 5; i++) {
      sink.push(h264(i * 2 + 1, true))
      const d = codecs.decoders[codecs.decoders.length - 1]
      d.emit(i * 2 + 1)
      sink.push(h264(i * 2 + 2, false))
      d.init.error(new Error('glitch'))
    }
    expect(log.some((l) => l.startsWith('failed'))).toBe(false)
    warn.mockRestore()
  })

  it('a keyframe with a new codec reconfigures; outputs from a closed decoder are dropped', () => {
    const codecs = fakeWebCodecs()
    const { sink, log } = sinkWith(codecs)
    sink.push(h264(1, true))
    const SC = [0, 0, 0, 1]
    const main = new Uint8Array([...SC, 0x67, 0x4d, 0x40, 0x1f, ...SC, 0x68, 0xee, ...SC, 0x65, 0x88])
    sink.push(h264(2, true, main, 8, 4))
    expect(codecs.decoders).toHaveLength(2)
    expect(codecs.decoders[0].state).toBe('closed')
    expect(codecs.decoders[1].configs[0].codec).toBe('avc1.4D401F')
    // Frame 1 died with the old decoder: acked at the switch, never drawn.
    expect(log).toEqual(['ack:1'])
    const stale = codecs.decoders[0].emit(1)
    expect(stale.closed).toBe(true)
    codecs.decoders[1].emit(2)
    expect(log).toEqual(['ack:1', 'draw:2:8x4', 'ack:2'])
    // H.264 → VP8 (a source change) reconfigures too.
    sink.push(vp8(3, true))
    expect(codecs.decoders).toHaveLength(3)
    expect(codecs.decoders[2].configs[0].codec).toBe('vp8')
    sink.close()
    sink.push(h264(4, false, DELTA, 8, 4))
    expect(log[log.length - 1]).toBe('ack:4')
  })
})

describe('the decode probe', () => {
  it('reports H.264, VP8 and JPEG from what isConfigSupported says', async () => {
    const both = fakeWebCodecs({ h264: ['avc1.4D401F'], vp8: true })
    expect(await probeDecoders(both, true)).toEqual({ h264: true, vp8: true, jpeg: true })
    expect(both.asked).toEqual(['avc1.640C1F', 'avc1.4D401F', 'vp8'])
    expect(await probeDecoders(fakeWebCodecs({ h264: false, vp8: true }), false)).toEqual({ h264: false, vp8: true, jpeg: false })
    expect(await probeDecoders(fakeWebCodecs({ h264: true, vp8: false }), true)).toEqual({ h264: true, vp8: false, jpeg: true })
    expect(await probeDecoders(null, true)).toEqual({ h264: false, vp8: false, jpeg: true })
  })

  it('decodeSupport: none with no WebCodecs; probed once; the JPEG pin (new key and the Mobile tab’s old one) turns video off', async () => {
    vi.stubGlobal('createImageBitmap', () => {})
    expect(await decodeSupport(null)).toEqual({ h264: false, vp8: false, jpeg: true })
    const codecs = fakeWebCodecs({ h264: true, vp8: true })
    expect(await decodeSupport(codecs)).toEqual({ h264: true, vp8: true, jpeg: true })
    await decodeSupport(codecs)
    expect(codecs.asked).toEqual(['avc1.640C1F', 'vp8'])
    resetVideoSupportForTests()
    expect(await decodeSupport(fakeWebCodecs({ h264: false, vp8: false }))).toEqual({ h264: false, vp8: false, jpeg: true })
    for (const key of ['k2.stream.codec', 'k2.mobile.streamCodec']) {
      resetVideoSupportForTests()
      const store = new Map<string, string>([[key, 'jpeg']])
      vi.stubGlobal('localStorage', { getItem: (k: string) => store.get(k) ?? null })
      expect(await decodeSupport(fakeWebCodecs())).toEqual({ h264: false, vp8: false, jpeg: true })
      store.clear()
      expect(await decodeSupport(fakeWebCodecs())).toMatchObject({ h264: true, vp8: true })
    }
  })
})
