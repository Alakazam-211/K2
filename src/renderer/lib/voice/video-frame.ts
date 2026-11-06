// Video in voice rooms (Sew's plugin video API, draft §B7.4): one message on a
// sink's Tauri raw Channel is a 32-byte little-endian header, then the frame.
//
//   0 u8 version (1) · 1 u8 kind · 2 u8 codec · 3 u8 flags · 4 u32 sink_id
//   8 u64 timestamp_us · 16 u16 width · 18 u16 height (0 = unchanged)
//   20 u32 sequence · 24 u32 payload length · 28 u32 reserved (0)
//
// Kinds: 1 encoded, 2 I420, 3 RGBA, 4 reset. Codecs: 1 VP8, 2 H.264, 0 when
// decoded. Flags: bit0 keyframe, bit1 discontinuity (reset the decoder first),
// bit2 self-view.

export const FRAME_HEADER_BYTES = 32
export const FRAME_VERSION = 1

export const FRAME_KIND = { encoded: 1, i420: 2, rgba: 3, reset: 4 } as const
export type FrameKind = keyof typeof FRAME_KIND

export type VideoCodec = 'vp8' | 'h264'

const CODEC_BYTE: Record<VideoCodec, number> = { vp8: 1, h264: 2 }

const FLAG_KEYFRAME = 1
const FLAG_DISCONTINUITY = 2
const FLAG_SELF_VIEW = 4

export interface FrameHeader {
  kind: FrameKind
  /** Null for decoded frames and resets. */
  codec: VideoCodec | null
  keyframe: boolean
  /** Reset the decoder before this frame. */
  discontinuity: boolean
  selfView: boolean
  sinkId: number
  /** Microseconds: arrival, or capture for self-view. */
  timestampUs: number
  width: number
  /** 0 = unchanged. */
  height: number
  seq: number
}

export interface Frame {
  header: FrameHeader
  payload: Uint8Array
}

/** A frame this client can't read is a bug on one side: it throws, loudly. */
export function parseFrame(data: ArrayBuffer | Uint8Array): Frame {
  const bytes = data instanceof Uint8Array ? data : new Uint8Array(data)
  if (bytes.byteLength < FRAME_HEADER_BYTES) throw new Error(`video frame: ${bytes.byteLength} bytes, shorter than the header`)
  const v = new DataView(bytes.buffer, bytes.byteOffset, FRAME_HEADER_BYTES)
  const version = v.getUint8(0)
  if (version !== FRAME_VERSION) throw new Error(`video frame: version ${version}, this client reads ${FRAME_VERSION}`)
  const kindByte = v.getUint8(1)
  const kind = (Object.keys(FRAME_KIND) as FrameKind[]).find((k) => FRAME_KIND[k] === kindByte)
  if (!kind) throw new Error(`video frame: unknown kind ${kindByte}`)
  const codecByte = v.getUint8(2)
  const codec = codecByte === 0 ? null : codecByte === 1 ? 'vp8' : codecByte === 2 ? 'h264' : undefined
  if (codec === undefined) throw new Error(`video frame: unknown codec ${codecByte}`)
  if (kind === 'encoded' && codec === null) throw new Error('video frame: an encoded frame with no codec')
  const flags = v.getUint8(3)
  const length = v.getUint32(24, true)
  if (FRAME_HEADER_BYTES + length !== bytes.byteLength) {
    throw new Error(`video frame: header says ${length} payload bytes, the message has ${bytes.byteLength - FRAME_HEADER_BYTES}`)
  }
  return {
    header: {
      kind,
      codec,
      keyframe: (flags & FLAG_KEYFRAME) !== 0,
      discontinuity: (flags & FLAG_DISCONTINUITY) !== 0,
      selfView: (flags & FLAG_SELF_VIEW) !== 0,
      sinkId: v.getUint32(4, true),
      timestampUs: Number(v.getBigUint64(8, true)),
      width: v.getUint16(16, true),
      height: v.getUint16(18, true),
      seq: v.getUint32(20, true),
    },
    payload: bytes.subarray(FRAME_HEADER_BYTES),
  }
}

/** The Rust side's framing, for fakes and tests. */
export function encodeFrame(h: Partial<FrameHeader> & { kind: FrameKind }, payload: Uint8Array): ArrayBuffer {
  const out = new Uint8Array(FRAME_HEADER_BYTES + payload.byteLength)
  const v = new DataView(out.buffer)
  v.setUint8(0, FRAME_VERSION)
  v.setUint8(1, FRAME_KIND[h.kind])
  v.setUint8(2, h.codec ? CODEC_BYTE[h.codec] : 0)
  v.setUint8(3, (h.keyframe ? FLAG_KEYFRAME : 0) | (h.discontinuity ? FLAG_DISCONTINUITY : 0) | (h.selfView ? FLAG_SELF_VIEW : 0))
  v.setUint32(4, h.sinkId ?? 0, true)
  v.setBigUint64(8, BigInt(Math.max(0, Math.floor(h.timestampUs ?? 0))), true)
  v.setUint16(16, h.width ?? 0, true)
  v.setUint16(18, h.height ?? 0, true)
  v.setUint32(20, h.seq ?? 0, true)
  v.setUint32(24, payload.byteLength, true)
  v.setUint32(28, 0, true)
  out.set(payload, FRAME_HEADER_BYTES)
  return out.buffer
}

/** The H.264 codec string WebCodecs wants for an Annex B keyframe: `avc1.` and
 *  the SPS's profile, constraint and level bytes. Null: no SPS in the frame. */
export function h264CodecOf(annexB: Uint8Array): string | null {
  for (let i = 0; i + 3 < annexB.length; i++) {
    // A start code: 00 00 01 (the 4-byte 00 00 00 01 ends the same way).
    if (annexB[i] !== 0 || annexB[i + 1] !== 0 || annexB[i + 2] !== 1) continue
    const nal = i + 3
    if ((annexB[nal] & 0x1f) === 7 && nal + 3 < annexB.length) {
      const hex = (b: number): string => b.toString(16).padStart(2, '0').toUpperCase()
      return `avc1.${hex(annexB[nal + 1])}${hex(annexB[nal + 2])}${hex(annexB[nal + 3])}`
    }
    i = nal - 1
  }
  return null
}

/** Constrained Baseline 3.1: what the draft's probe asks for. */
export const H264_PROBE_CODEC = 'avc1.42E01F'
