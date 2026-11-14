// The decode probe with a caller's own strings (stream-video.ts), fail loud:
// the live stream's probe asks exactly what it always did (the same strings,
// in order, each with optimizeForLatency: true, stopping at the first yes)
// and reports exactly { h264, vp8, jpeg }; a file player's probe asks AV1
// and its own H.264 string, may leave the latency flag out, can report every
// string it asked, and is cached apart from the stream's (the stream's JPEG
// pin doesn't touch it). Plus the widened decoder config and frame types
// reaching the fake decoder.

import { afterEach, describe, expect, it, vi } from 'vitest'
import {
  decodeSupport,
  H264_PROBE_CODECS,
  probeDecoders,
  resetVideoSupportForTests,
  STREAM_DECODE_PROBE,
  VP8_PROBE_CODECS,
  type DecodeProbe,
  type VideoDecoderConfigLike,
  type WebCodecsLike,
} from './stream-video'
import { fakeVideoFrame, fakeWebCodecs, type FakeVideoFrame } from './testing'

afterEach(() => {
  resetVideoSupportForTests()
  vi.unstubAllGlobals()
})

/** `codecs`, recording every config `isConfigSupported` is asked with. */
function recording(codecs: WebCodecsLike): { codecs: WebCodecsLike; configs: VideoDecoderConfigLike[] } {
  const configs: VideoDecoderConfigLike[] = []
  return {
    configs,
    codecs: {
      ...codecs,
      isConfigSupported: (c) => {
        configs.push(c)
        return codecs.isConfigSupported(c)
      },
    },
  }
}

const FILE_PROBE: DecodeProbe = {
  h264: ['avc1.64002A'],
  vp8: [],
  av01: ['av01.0.01M.08', 'av01.0.08M.08', 'av01.0.09M.08'],
  everyString: true,
}

describe('the live stream probe is unchanged', () => {
  it('keeps its strings and its latency flag', () => {
    expect(H264_PROBE_CODECS).toEqual(['avc1.640C1F', 'avc1.4D401F'])
    expect(VP8_PROBE_CODECS).toEqual(['vp8'])
    expect(STREAM_DECODE_PROBE).toStrictEqual({ h264: ['avc1.640C1F', 'avc1.4D401F'], vp8: ['vp8'], optimizeForLatency: true })
  })

  it('asks each config with optimizeForLatency: true, stops at the first yes, and reports only h264, vp8 and jpeg', async () => {
    const r = recording(fakeWebCodecs({ h264: ['avc1.4D401F'], vp8: true, av01: true }))
    const got = await probeDecoders(r.codecs, true)
    expect(got).toStrictEqual({ h264: true, vp8: true, jpeg: true })
    expect(r.configs).toStrictEqual([
      { codec: 'avc1.640C1F', optimizeForLatency: true },
      { codec: 'avc1.4D401F', optimizeForLatency: true },
      { codec: 'vp8', optimizeForLatency: true },
    ])
    expect(await probeDecoders(null, false)).toStrictEqual({ h264: false, vp8: false, jpeg: false })
  })
})

describe('a caller’s own probe', () => {
  it('asks AV1 and the H.264 rung’s string, with no latency flag, and reports each string', async () => {
    const r = recording(fakeWebCodecs({ h264: ['avc1.64002A'], vp8: true, av01: ['av01.0.08M.08'] }))
    const got = await probeDecoders(r.codecs, false, FILE_PROBE)
    expect(got).toStrictEqual({
      h264: true,
      vp8: false,
      jpeg: false,
      av01: true,
      byCodec: { 'avc1.64002A': true, 'av01.0.01M.08': false, 'av01.0.08M.08': true, 'av01.0.09M.08': false },
    })
    // Every string asked (no stop at the first yes), none with the flag.
    expect(r.configs).toStrictEqual([{ codec: 'avc1.64002A' }, { codec: 'av01.0.01M.08' }, { codec: 'av01.0.08M.08' }, { codec: 'av01.0.09M.08' }])
  })

  it('AV1 is false by default in the fake, and absent from results whose probe didn’t ask', async () => {
    expect(await probeDecoders(fakeWebCodecs(), false, FILE_PROBE)).toMatchObject({ av01: false, h264: true })
    expect(await probeDecoders(fakeWebCodecs({ av01: true }), false, { h264: [], vp8: [], av01: ['av01.0.09M.08'] })).toStrictEqual({
      h264: false,
      vp8: false,
      jpeg: false,
      av01: true,
    })
    expect(await probeDecoders(null, true, FILE_PROBE)).toStrictEqual({ h264: false, vp8: false, jpeg: true, av01: false, byCodec: {} })
  })

  it('an explicit latency flag is passed as given', async () => {
    const r = recording(fakeWebCodecs({ av01: true }))
    await probeDecoders(r.codecs, false, { h264: [], vp8: [], av01: ['av01.0.08M.08'], optimizeForLatency: false })
    expect(r.configs).toStrictEqual([{ codec: 'av01.0.08M.08', optimizeForLatency: false }])
  })

  it('decodeSupport caches per probe, and the stream’s JPEG pin leaves other probes alone', async () => {
    const codecs = fakeWebCodecs({ h264: true, vp8: true, av01: true })
    expect(await decodeSupport(codecs)).toStrictEqual({ h264: true, vp8: true, jpeg: false })
    expect((await decodeSupport(codecs, FILE_PROBE)).av01).toBe(true)
    await decodeSupport(codecs)
    await decodeSupport(codecs, FILE_PROBE)
    expect(codecs.asked).toEqual(['avc1.640C1F', 'vp8', 'avc1.64002A', 'av01.0.01M.08', 'av01.0.08M.08', 'av01.0.09M.08'])

    resetVideoSupportForTests()
    vi.stubGlobal('localStorage', { getItem: (k: string) => (k === 'k2.stream.codec' ? 'jpeg' : null) })
    expect(await decodeSupport(fakeWebCodecs({ h264: true, vp8: true, av01: true }))).toStrictEqual({ h264: false, vp8: false, jpeg: false })
    expect(await decodeSupport(fakeWebCodecs({ h264: true, vp8: true, av01: true }), FILE_PROBE)).toMatchObject({ h264: true, av01: true })
  })
})

describe('the widened decoder config and frame', () => {
  it('a config with a description and coded size reaches the decoder as given', () => {
    const codecs = fakeWebCodecs({ av01: true })
    const d = codecs.createDecoder({ output: () => {}, error: () => {} })
    const av1C = new Uint8Array([0x81, 0x08, 0x0c, 0x00])
    d.configure({ codec: 'av01.0.08M.08', description: av1C, codedWidth: 1920, codedHeight: 1080 })
    expect(codecs.last().configs).toEqual([{ codec: 'av01.0.08M.08', description: av1C, codedWidth: 1920, codedHeight: 1080 }])
  })

  it('emitFrame hands a readable frame to the decoder’s output', async () => {
    const codecs = fakeWebCodecs()
    const out: FakeVideoFrame[] = []
    codecs.createDecoder({ output: (f) => out.push(f as FakeVideoFrame), error: () => {} })
    const f = codecs.last().emitFrame(fakeVideoFrame({ timestamp: 40000, width: 4, height: 2 }))
    expect(out).toEqual([f])
    expect(f.format).toBe('I420')
    const buf = new Uint8Array(f.allocationSize?.() ?? 0)
    expect(await f.copyTo?.(buf)).toEqual([
      { offset: 0, stride: 4 },
      { offset: 8, stride: 2 },
      { offset: 10, stride: 2 },
    ])
  })
})
