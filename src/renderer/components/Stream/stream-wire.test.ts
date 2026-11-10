// The engine's binary frames (stream-wire.ts), fail loud, in plain node (no
// DOM globals): K2SV round-trips with codec 1 (H.264) and 2 (VP8), a
// message is read into a frame with its buffer kept for the worker, an
// unknown codec or a missing header is said so, and the offer is cut to
// what the probe found with JPEG as the floor.

import { describe, expect, it } from 'vitest'
import {
  decodeVideoFrame,
  encodeFrame,
  encodeVideoFrame,
  offerCodecs,
  readBinaryFrame,
  VIDEO_CODEC_H264,
  VIDEO_CODEC_VP8,
  videoCodecByte,
  videoCodecName,
} from './stream-wire'
import { H264_KEY, VP8_KEY } from './testing'

describe('stream wire', () => {
  it('runs with no DOM globals', () => {
    expect(typeof (globalThis as { document?: unknown }).document).toBe('undefined')
    expect(typeof (globalThis as { window?: unknown }).window).toBe('undefined')
  })

  it('names the K2SV codec bytes: 1 = H.264, 2 = VP8, anything else unknown', () => {
    expect([VIDEO_CODEC_H264, VIDEO_CODEC_VP8]).toEqual([1, 2])
    expect(videoCodecName(1)).toBe('h264')
    expect(videoCodecName(2)).toBe('vp8')
    expect(videoCodecName(0)).toBeNull()
    expect(videoCodecName(9)).toBeNull()
    expect(videoCodecByte('vp8')).toBe(2)
    expect(videoCodecByte('h264')).toBe(1)
  })

  it('reads a VP8 frame (codec 2) with its header fields and keeps the buffer', () => {
    const raw = encodeVideoFrame({ seq: 70000, widthPx: 1920, heightPx: 1080, widthPt: 1512, heightPt: 982, key: true, codec: VIDEO_CODEC_VP8, ageMs: 12, tsMs: 99 }, VP8_KEY)
    const h = decodeVideoFrame(raw)
    expect(h).toMatchObject({ seq: 70000, widthPx: 1920, heightPx: 1080, widthPt: 1512, heightPt: 982, codec: 2, key: true, ageMs: 12, tsMs: 99 })
    const r = readBinaryFrame(raw)
    if (!('frame' in r)) throw new Error(`expected a frame, got ${r.error}`)
    expect(r.frame.raw).toBe(raw)
    expect(r.frame.seq).toBe(70000)
    expect([r.frame.widthPt, r.frame.heightPt]).toEqual([1512, 982])
    const s = r.frame.source
    if (s.kind !== 'vp8') throw new Error(`expected vp8, got ${s.kind}`)
    expect({ key: s.key, w: s.widthPx, h: s.heightPx, data: Array.from(s.data) }).toEqual({ key: true, w: 1920, h: 1080, data: Array.from(VP8_KEY) })
  })

  it('reads H.264 and JPEG frames; an unknown codec or no header is an error', async () => {
    const h = readBinaryFrame(encodeVideoFrame({ seq: 1, widthPx: 4, heightPx: 2, widthPt: 4, heightPt: 2, key: false }, H264_KEY))
    expect('frame' in h && h.frame.source.kind).toBe('h264')
    const j = readBinaryFrame(encodeFrame({ seq: 2, widthPx: 4, heightPx: 2, widthPt: 4, heightPt: 2 }, new Uint8Array([0xff, 0xd8, 0xff])))
    if (!('frame' in j) || j.frame.source.kind !== 'jpeg') throw new Error('expected a jpeg frame')
    expect(j.frame.source.data.type).toBe('image/jpeg')
    expect(new Uint8Array(await j.frame.source.data.arrayBuffer())).toEqual(new Uint8Array([0xff, 0xd8, 0xff]))
    expect(readBinaryFrame(encodeVideoFrame({ seq: 3, widthPx: 1, heightPx: 1, widthPt: 1, heightPt: 1, key: false, codec: 9 }, H264_KEY))).toEqual({ error: 'video' })
    expect(readBinaryFrame(new Uint8Array([0x4b, 0x32, 0x53, 0x56, 0]).buffer)).toEqual({ error: 'video' })
    expect(readBinaryFrame(new Uint8Array([0xff, 0xd8]).buffer)).toEqual({ error: 'header' })
  })

  it('offers only what the probe found, in the source’s order, JPEG as the floor', () => {
    expect(offerCodecs({ h264: true, vp8: true, jpeg: true })).toEqual(['h264', 'vp8', 'jpeg'])
    expect(offerCodecs({ h264: false, vp8: true, jpeg: true })).toEqual(['vp8', 'jpeg'])
    expect(offerCodecs({ h264: false, vp8: false, jpeg: true })).toEqual(['jpeg'])
    expect(offerCodecs({ h264: true, vp8: true, jpeg: true }, ['vp8', 'jpeg'])).toEqual(['vp8', 'jpeg'])
    expect(offerCodecs({ h264: false, vp8: false, jpeg: false })).toEqual(['jpeg'])
  })
})
