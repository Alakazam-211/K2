// One sink's frames through WebCodecs onto a surface (Sew draft §B7.4, §B9.2),
// over the fake WebCodecs.

import { beforeEach, describe, expect, it, vi } from 'vitest'
import { encodeFrame, FRAME_HEADER_BYTES, h264CodecOf, H264_PROBE_CODEC, parseFrame } from './video-frame'
import { MAX_DECODE_QUEUE, probeDecode, SinkDecoder } from './video-decoder'
import { FakeVideoDecoder, FakeVideoFrame, fakeWebCodecs, RecordingSurface } from './fake-webcodecs'

const key = (payload = new Uint8Array([0x9d, 0x01, 0x2a]), over = {}) => encodeFrame({ kind: 'encoded', codec: 'vp8', keyframe: true, width: 640, height: 360, ...over }, payload)
const delta = (over = {}) => encodeFrame({ kind: 'encoded', codec: 'vp8', ...over }, new Uint8Array([1, 2, 3]))

/** Annex B: an SPS (profile 0x64 High, constraint 0x00, level 0x28) then an IDR slice. */
const h264Key = (profile = 0x64, level = 0x28) => new Uint8Array([0, 0, 0, 1, 0x67, profile, 0x00, level, 0xac, 0, 0, 0, 1, 0x68, 0xee, 0, 0, 1, 0x65, 0x88])

beforeEach(() => FakeVideoDecoder.reset())

function setup() {
  const surface = new RecordingSurface()
  const requestKeyframe = vi.fn()
  let t = 0
  const d = new SinkDecoder({ env: fakeWebCodecs(), surface: () => surface, requestKeyframe, now: () => t })
  return { d, surface, requestKeyframe, tick: (ms: number) => (t += ms), dec: () => FakeVideoDecoder.instances.at(-1) as FakeVideoDecoder }
}

describe('the frame header', () => {
  it('round-trips every field', () => {
    const buf = encodeFrame(
      { kind: 'encoded', codec: 'h264', keyframe: true, discontinuity: true, selfView: true, sinkId: 0xfffffffe, timestampUs: 2 ** 40 + 3, width: 1920, height: 1080, seq: 77 },
      new Uint8Array([5, 6]),
    )
    expect(buf.byteLength).toBe(FRAME_HEADER_BYTES + 2)
    const { header, payload } = parseFrame(buf)
    expect(header).toEqual({ kind: 'encoded', codec: 'h264', keyframe: true, discontinuity: true, selfView: true, sinkId: 0xfffffffe, timestampUs: 2 ** 40 + 3, width: 1920, height: 1080, seq: 77 })
    expect([...payload]).toEqual([5, 6])
  })

  it('reads the bytes little-endian at the draft offsets', () => {
    const b = new Uint8Array(encodeFrame({ kind: 'rgba', sinkId: 0x01020304, width: 0x0280, height: 0x0168, seq: 9 }, new Uint8Array(0)))
    expect([...b.subarray(0, 8)]).toEqual([1, 3, 0, 0, 0x04, 0x03, 0x02, 0x01])
    expect([...b.subarray(16, 20)]).toEqual([0x80, 0x02, 0x68, 0x01])
    expect([...b.subarray(24, 32)]).toEqual([0, 0, 0, 0, 0, 0, 0, 0])
  })

  it('refuses a frame it cannot read', () => {
    const b = new Uint8Array(key())
    b[0] = 2
    expect(() => parseFrame(b)).toThrow(/version 2/)
    expect(() => parseFrame(new Uint8Array(10))).toThrow(/shorter than the header/)
    expect(() => parseFrame(new Uint8Array(key()).subarray(0, FRAME_HEADER_BYTES + 1))).toThrow(/header says 3 payload bytes/)
    const k = new Uint8Array(key())
    k[1] = 9
    expect(() => parseFrame(k)).toThrow(/unknown kind 9/)
  })

  it("names an H.264 keyframe's codec from its SPS", () => {
    expect(h264CodecOf(h264Key())).toBe('avc1.640028')
    expect(h264CodecOf(new Uint8Array([0, 0, 1, 0x65, 0x88]))).toBeNull()
  })
})

describe('the probe', () => {
  it('asks WebCodecs for VP8 and Constrained Baseline H.264', async () => {
    expect(await probeDecode(fakeWebCodecs())).toEqual(['vp8', 'h264'])
    FakeVideoDecoder.supported = new Set(['vp8'])
    expect(await probeDecode(fakeWebCodecs())).toEqual(['vp8'])
    expect(await probeDecode(null)).toEqual([])
  })
})

describe('encoded frames', () => {
  it('waits for a keyframe, configures for latency, decodes and draws, and closes every frame', () => {
    const s = setup()
    s.d.push(delta())
    expect(s.surface.drawn).toEqual([])
    expect(s.d.stats.dropped).toBe(1)
    expect(s.requestKeyframe).toHaveBeenCalledTimes(1)
    s.d.push(key())
    s.d.push(delta())
    expect(s.dec().configs).toEqual([{ codec: 'vp8', optimizeForLatency: true }])
    expect(s.dec().chunks.map((c) => c.type)).toEqual(['key', 'delta'])
    expect([...s.dec().chunks[0].data]).toEqual([0x9d, 0x01, 0x2a])
    expect(s.surface.drawn).toEqual([
      { kind: 'frame', width: 640, height: 360 },
      { kind: 'frame', width: 640, height: 360 },
    ])
    expect(s.d.stats.decoded).toBe(2)
    expect(FakeVideoFrame.open).toBe(0)
  })

  it('resets and reconfigures before a discontinuity', () => {
    const s = setup()
    s.d.push(key())
    s.d.push(key(undefined, { discontinuity: true }))
    expect(s.dec().resets).toBe(1)
    expect(s.dec().configs).toHaveLength(2)
    expect(s.dec().chunks).toHaveLength(2)
  })

  it('on a decoder error starts a new decoder at the next keyframe, and asks for one once', () => {
    const s = setup()
    s.d.push(key())
    const first = s.dec()
    first.fail()
    expect(s.requestKeyframe).toHaveBeenCalledTimes(1)
    s.d.push(delta())
    s.d.push(delta())
    // Asked already: the plugin rate-limits too, but we don't spam it.
    expect(s.requestKeyframe).toHaveBeenCalledTimes(1)
    expect(s.d.stats.dropped).toBe(2)
    s.d.push(key())
    expect(s.dec()).not.toBe(first)
    expect(s.dec().chunks).toHaveLength(1)
    s.dec().fail()
    expect(s.requestKeyframe).toHaveBeenCalledTimes(2)
  })

  it(`drops deltas and asks for a keyframe when more than ${MAX_DECODE_QUEUE} frames wait`, () => {
    const s = setup()
    s.d.push(key())
    s.dec().decodeQueueSize = MAX_DECODE_QUEUE + 1
    s.d.push(delta())
    expect(s.requestKeyframe).toHaveBeenCalledTimes(1)
    s.dec().decodeQueueSize = 0
    s.d.push(delta())
    expect(s.dec().chunks).toHaveLength(1)
    s.d.push(key())
    s.d.push(delta())
    expect(s.dec().chunks.map((c) => c.type)).toEqual(['key', 'key', 'delta'])
  })

  it('configures H.264 without a description, from the SPS, and again when the profile changes', () => {
    const s = setup()
    s.d.push(encodeFrame({ kind: 'encoded', codec: 'h264', keyframe: true }, h264Key()))
    s.d.push(encodeFrame({ kind: 'encoded', codec: 'h264' }, new Uint8Array([0, 0, 1, 0x41, 0x9a])))
    s.d.push(encodeFrame({ kind: 'encoded', codec: 'h264', keyframe: true }, h264Key()))
    expect(s.dec().configs).toEqual([{ codec: 'avc1.640028', optimizeForLatency: true }])
    s.d.push(encodeFrame({ kind: 'encoded', codec: 'h264', keyframe: true }, h264Key(0x42, 0x1f)))
    expect(s.dec().configs.map((c) => c.codec)).toEqual(['avc1.640028', 'avc1.42001F'])
    expect(s.dec().configs.every((c) => !('description' in c))).toBe(true)
    // A keyframe without an SPS keeps what it had, or the probe's codec.
    const t = setup()
    t.d.push(encodeFrame({ kind: 'encoded', codec: 'h264', keyframe: true }, new Uint8Array([0, 0, 1, 0x65])))
    expect(t.dec().configs[0].codec).toBe(H264_PROBE_CODEC)
  })

  it('a reset frame waits for the next keyframe', () => {
    const s = setup()
    s.d.push(key())
    s.d.push(encodeFrame({ kind: 'reset' }, new Uint8Array(0)))
    s.d.push(delta())
    expect(s.dec().resets).toBe(1)
    expect(s.dec().chunks).toHaveLength(1)
  })

  it('counts a long gap between pictures as a freeze, but not after a hold', () => {
    const s = setup()
    s.d.push(key())
    s.tick(100)
    s.d.push(delta())
    s.tick(900)
    s.d.push(delta())
    expect(s.d.stats).toMatchObject({ freezes: 1, freezeMs: 900 })
    s.d.hold()
    s.tick(5000)
    s.d.push(delta())
    expect(s.d.stats.freezes).toBe(1)
  })

  it('resume (unmuted, shown again) waits for a keyframe and asks for it', () => {
    const s = setup()
    s.d.push(key())
    s.d.resume()
    expect(s.requestKeyframe).toHaveBeenCalledTimes(1)
    s.d.push(delta())
    expect(s.dec().chunks).toHaveLength(1)
  })

  it('close closes the decoder and ignores later frames', () => {
    const s = setup()
    s.d.push(key())
    s.d.close()
    expect(s.dec().state).toBe('closed')
    s.d.push(key())
    expect(s.dec().chunks).toHaveLength(1)
  })
})

describe('decoded frames (the fallback)', () => {
  it('draws I420 through a VideoFrame, keeping the last size when the header says 0', () => {
    const s = setup()
    s.d.push(encodeFrame({ kind: 'i420', width: 4, height: 2 }, new Uint8Array(12)))
    s.d.push(encodeFrame({ kind: 'i420' }, new Uint8Array(12)))
    expect(s.surface.drawn).toEqual([
      { kind: 'frame', width: 4, height: 2 },
      { kind: 'frame', width: 4, height: 2 },
    ])
    expect(FakeVideoFrame.open).toBe(0)
    expect(FakeVideoDecoder.instances).toHaveLength(0)
  })

  it('draws RGBA with no WebCodecs at all', () => {
    const surface = new RecordingSurface()
    const d = new SinkDecoder({ env: null, surface: () => surface, requestKeyframe: vi.fn(), now: () => 0 })
    d.push(encodeFrame({ kind: 'rgba', width: 2, height: 2 }, new Uint8Array(16)))
    expect(surface.drawn).toEqual([{ kind: 'rgba', width: 2, height: 2 }])
    // An encoded frame can't be shown without WebCodecs: dropped, never thrown.
    d.push(key())
    expect(d.stats.dropped).toBe(1)
  })
})
