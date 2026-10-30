// The H.264 decoder path (sim-video.ts) against a fake WebCodecs, fail
// loud: SPS → codec string, configure on the first keyframe, in-order
// decode with an ack per frame, held frames acked but not drawn, a delta
// with no keyframe skipped and a keyframe asked once, decoder errors ask
// again and three in a row give up (JPEG fallback), and the support probe.

import { afterEach, describe, expect, it, vi } from 'vitest'
import {
  codecFromAnnexB,
  H264Sink,
  MAX_DECODE_ERRORS,
  resetVideoSupportForTests,
  splitNals,
  videoDecodeSupport,
  type VideoDecoderConfigLike,
  type VideoDecoderInitLike,
  type VideoDecoderLike,
  type VideoFrameLike,
  type WebCodecsLike,
} from './sim-video'

const SC = [0, 0, 0, 1]
/** SPS (Constrained High 3.1, as VideoToolbox's low-latency mode sends) + PPS + IDR. */
const KEY = new Uint8Array([...SC, 0x67, 0x64, 0x0c, 0x1f, 0xac, ...SC, 0x68, 0xee, 0x3c, ...SC, 0x65, 0x88, 0x84])
const DELTA = new Uint8Array([...SC, 0x41, 0x9a, 0x02])

class FakeDecoder implements VideoDecoderLike {
  state = 'unconfigured'
  configs: VideoDecoderConfigLike[] = []
  chunks: { type: string; timestamp: number }[] = []
  constructor(readonly init: VideoDecoderInitLike) {}
  configure(c: VideoDecoderConfigLike): void {
    this.state = 'configured'
    this.configs.push(c)
  }
  decode(chunk: unknown): void {
    if (this.state !== 'configured') throw new Error('decode before configure')
    this.chunks.push(chunk as { type: string; timestamp: number })
  }
  close(): void {
    this.state = 'closed'
  }
  /** Emit the decoded frame for `timestamp`. */
  emit(timestamp: number, w = 412, h = 916): VideoFrameLike & { closed: boolean } {
    const f = {
      timestamp,
      displayWidth: w,
      displayHeight: h,
      closed: false,
      close() {
        f.closed = true
      },
    }
    this.init.output(f)
    return f
  }
}

function fakeCodecs(supported = true): WebCodecsLike & { decoders: FakeDecoder[] } {
  const decoders: FakeDecoder[] = []
  return {
    decoders,
    createDecoder: (init) => {
      const d = new FakeDecoder(init)
      decoders.push(d)
      return d
    },
    createChunk: (init) => ({ type: init.type, timestamp: init.timestamp, bytes: init.data.length }),
    isConfigSupported: async () => supported,
  }
}

function sinkWith(codecs: WebCodecsLike, held = false) {
  const log: string[] = []
  const sink = new H264Sink({
    codecs,
    draw: (f, w, h) => log.push(`draw:${f.timestamp}:${w}x${h}`),
    ack: (seq) => log.push(`ack:${seq}`),
    needKeyframe: () => log.push('key?'),
    failed: (why) => log.push(`failed:${why}`),
    held: () => held,
  })
  return { sink, log }
}

afterEach(() => {
  resetVideoSupportForTests()
  vi.unstubAllGlobals()
})

describe('Annex B helpers', () => {
  it('splits NAL units on 3- and 4-byte start codes and reads the codec from the SPS', () => {
    expect(splitNals(KEY).map((n) => n[0])).toEqual([0x67, 0x68, 0x65])
    expect(splitNals(new Uint8Array([0, 0, 1, 0x41, 1, 0, 0, 1, 0x41, 2])).map((n) => Array.from(n))).toEqual([
      [0x41, 1],
      [0x41, 2],
    ])
    expect(codecFromAnnexB(KEY)).toBe('avc1.640C1F')
    expect(codecFromAnnexB(DELTA)).toBeNull()
  })
})

describe('H264Sink', () => {
  it('configures on the keyframe, decodes in order, draws cropped to the header size and acks each frame', () => {
    const codecs = fakeCodecs()
    const { sink, log } = sinkWith(codecs)
    sink.push({ seq: 10, key: true, data: KEY, widthPx: 411, heightPx: 915 })
    sink.push({ seq: 11, key: false, data: DELTA, widthPx: 411, heightPx: 915 })
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

  it('acks a frame the decoder skipped when a later one comes out', () => {
    const codecs = fakeCodecs()
    const { sink, log } = sinkWith(codecs)
    sink.push({ seq: 1, key: true, data: KEY, widthPx: 4, heightPx: 8 })
    sink.push({ seq: 2, key: false, data: DELTA, widthPx: 4, heightPx: 8 })
    sink.push({ seq: 3, key: false, data: DELTA, widthPx: 4, heightPx: 8 })
    codecs.decoders[0].emit(1)
    codecs.decoders[0].emit(3)
    expect(log).toEqual(['draw:1:4x8', 'ack:1', 'ack:2', 'draw:3:4x8', 'ack:3'])
  })

  it('a held view decodes and acks but does not draw', () => {
    const codecs = fakeCodecs()
    const { sink, log } = sinkWith(codecs, true)
    sink.push({ seq: 5, key: true, data: KEY, widthPx: 4, heightPx: 8 })
    codecs.decoders[0].emit(5)
    expect(log).toEqual(['ack:5'])
  })

  it('skips a delta with no keyframe before it and asks for one, once', () => {
    const codecs = fakeCodecs()
    const { sink, log } = sinkWith(codecs)
    sink.push({ seq: 7, key: false, data: DELTA, widthPx: 4, heightPx: 8 })
    sink.push({ seq: 8, key: false, data: DELTA, widthPx: 4, heightPx: 8 })
    expect(codecs.decoders).toHaveLength(0)
    expect(log).toEqual(['ack:7', 'key?', 'ack:8'])
    sink.push({ seq: 9, key: true, data: KEY, widthPx: 4, heightPx: 8 })
    expect(codecs.decoders).toHaveLength(1)
  })

  it('a decoder error acks what was queued and asks for a keyframe; three in a row give up on video', () => {
    const codecs = fakeCodecs()
    const { sink, log } = sinkWith(codecs)
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => {})
    let seq = 0
    for (let round = 1; round <= MAX_DECODE_ERRORS; round++) {
      sink.push({ seq: ++seq, key: true, data: KEY, widthPx: 4, heightPx: 8 })
      sink.push({ seq: ++seq, key: false, data: DELTA, widthPx: 4, heightPx: 8 })
      const d = codecs.decoders[codecs.decoders.length - 1]
      d.init.error(new Error(`decode failed ${round}`))
      expect(d.state).toBe('closed')
    }
    expect(log).toEqual(['ack:1', 'ack:2', 'key?', 'ack:3', 'ack:4', 'key?', 'ack:5', 'ack:6', 'failed:The video decoder failed 3 times in a row.'])
    // After giving up every frame is just acked.
    sink.push({ seq: 99, key: true, data: KEY, widthPx: 4, heightPx: 8 })
    expect(log[log.length - 1]).toBe('ack:99')
    expect(codecs.decoders).toHaveLength(3)
    expect(warn).toHaveBeenCalled()
    warn.mockRestore()
  })

  it('a good frame between errors resets the count', () => {
    const codecs = fakeCodecs()
    const { sink, log } = sinkWith(codecs)
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => {})
    for (let i = 0; i < 5; i++) {
      sink.push({ seq: i * 2 + 1, key: true, data: KEY, widthPx: 4, heightPx: 8 })
      const d = codecs.decoders[codecs.decoders.length - 1]
      d.emit(i * 2 + 1)
      sink.push({ seq: i * 2 + 2, key: false, data: DELTA, widthPx: 4, heightPx: 8 })
      d.init.error(new Error('glitch'))
    }
    expect(log.some((l) => l.startsWith('failed'))).toBe(false)
    warn.mockRestore()
  })

  it('a keyframe with a new codec reconfigures; outputs from a closed decoder are dropped', () => {
    const codecs = fakeCodecs()
    const { sink, log } = sinkWith(codecs)
    sink.push({ seq: 1, key: true, data: KEY, widthPx: 4, heightPx: 8 })
    const main = new Uint8Array([...SC, 0x67, 0x4d, 0x40, 0x1f, ...SC, 0x68, 0xee, ...SC, 0x65, 0x88])
    sink.push({ seq: 2, key: true, data: main, widthPx: 8, heightPx: 4 })
    expect(codecs.decoders).toHaveLength(2)
    expect(codecs.decoders[0].state).toBe('closed')
    expect(codecs.decoders[1].configs[0].codec).toBe('avc1.4D401F')
    // Frame 1 died with the old decoder: acked at the switch, never drawn.
    expect(log).toEqual(['ack:1'])
    const stale = codecs.decoders[0].emit(1)
    expect(stale.closed).toBe(true)
    codecs.decoders[1].emit(2)
    expect(log).toEqual(['ack:1', 'draw:2:8x4', 'ack:2'])
    sink.close()
    sink.push({ seq: 3, key: false, data: DELTA, widthPx: 8, heightPx: 4 })
    expect(log[log.length - 1]).toBe('ack:3')
  })
})

describe('videoDecodeSupport', () => {
  it('is false with no WebCodecs, true when a VideoToolbox codec string is supported, and honours the JPEG pin', async () => {
    expect(await videoDecodeSupport(null)).toBe(false)
    resetVideoSupportForTests()
    const asked: string[] = []
    const codecs: WebCodecsLike = {
      ...fakeCodecs(),
      isConfigSupported: async (c) => {
        asked.push(c.codec)
        return c.codec === 'avc1.4D401F'
      },
    }
    expect(await videoDecodeSupport(codecs)).toBe(true)
    expect(asked).toEqual(['avc1.640C1F', 'avc1.4D401F'])
    resetVideoSupportForTests()
    expect(await videoDecodeSupport(fakeCodecs(false))).toBe(false)
    resetVideoSupportForTests()
    const store = new Map<string, string>([['k2.mobile.streamCodec', 'jpeg']])
    vi.stubGlobal('localStorage', { getItem: (k: string) => store.get(k) ?? null })
    expect(await videoDecodeSupport(fakeCodecs(true))).toBe(false)
    store.clear()
    expect(await videoDecodeSupport(fakeCodecs(true))).toBe(true)
  })
})
