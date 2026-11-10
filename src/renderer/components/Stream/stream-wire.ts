// The stream engine's binary frames (E9; prd-remote-desktop-tab-v1 §6),
// shared by every source the player draws: the Mobile tab's device
// (`/cli/sim/stream`) and the remote desktop (`/cli/desktop/stream`). The
// wire is `k2-core::sim::stream`'s:
//
//  - K2SF: a 16-byte header then a baseline JPEG:
//      0..4 b"K2SF" · 4..8 seq u32 · 8..10 width px u16 · 10..12 height px
//      u16 · 12..14 width pt u16 · 14..16 height pt u16 (all LE)
//  - K2SV: a 24-byte header then one encoded access unit:
//      0..4 b"K2SV" · 4..8 seq u32 · 8..10 width px · 10..12 height px ·
//      12..14 width pt · 14..16 height pt · 16 codec u8 (1 = H.264 Annex B,
//      2 = VP8) · 17 flags u8 (bit 0 keyframe) · 18..20 age ms u16 ·
//      20..24 ts ms u32 (all LE)
//  - the viewer offers `codecs=<list>&win=<n>` and acks each seq; text
//    messages are each source's own (`sim-stream.ts`, the desktop's wire).
//
// Pure: no DOM, no React, no `window` (R14). `Blob` and `DataView` are in
// every JS runtime this runs in (webview, worker, node).

export const FRAME_MAGIC = 'K2SF'
export const FRAME_HEADER_LEN = 16
export const VIDEO_MAGIC = 'K2SV'
export const VIDEO_HEADER_LEN = 24
/** K2SV codec byte: H.264 (Annex B; SPS + PPS in front of every keyframe). */
export const VIDEO_CODEC_H264 = 1
/** K2SV codec byte: VP8 (one frame per message). */
export const VIDEO_CODEC_VP8 = 2
export const VIDEO_FLAG_KEY = 1

/** Frames a viewer asks to have in flight (`win=`; the daemon may give fewer). */
export const STREAM_WINDOW = 3

export type VideoCodec = 'h264' | 'vp8'
/** What a viewer may offer in `codecs=`. */
export type StreamCodec = 'jpeg' | VideoCodec

/** The K2SV codec byte's name, or null for one this player can't read. */
export function videoCodecName(byte: number): VideoCodec | null {
  if (byte === VIDEO_CODEC_H264) return 'h264'
  if (byte === VIDEO_CODEC_VP8) return 'vp8'
  return null
}

export function videoCodecByte(codec: VideoCodec): number {
  return codec === 'vp8' ? VIDEO_CODEC_VP8 : VIDEO_CODEC_H264
}

/** A 2d context, as far as a painter needs (type only: no DOM at runtime). */
export type PaintContext = CanvasRenderingContext2D

/** What a frame holds: a JPEG or an encoded access unit off the wire, or
 *  (stubs) a painter. */
export type StreamFrameSource =
  | { kind: 'jpeg'; data: Blob }
  | { kind: VideoCodec; key: boolean; data: Uint8Array; widthPx: number; heightPx: number }
  | { kind: 'paint'; paint: (ctx: PaintContext, width: number, height: number) => void }

export type VideoFrameSource = Extract<StreamFrameSource, { kind: VideoCodec }>

export function isVideoSource(s: StreamFrameSource): s is VideoFrameSource {
  return s.kind === 'h264' || s.kind === 'vp8'
}

export interface StreamFrame {
  /** Per source; the ack names it. */
  seq: number
  /** The source's size in points (a device's orientation, a display). */
  widthPt: number
  heightPt: number
  source: StreamFrameSource
  /** The whole binary message, for the worker player to take over (fix 4;
   *  absent for a stub's painted frames). */
  raw?: ArrayBuffer
}

/** The four magic bytes of a binary message, or ''. */
export function frameMagic(buf: ArrayBuffer): string {
  if (buf.byteLength < 4) return ''
  const b = new Uint8Array(buf, 0, 4)
  return String.fromCharCode(b[0], b[1], b[2], b[3])
}

export interface VideoFrameHeader {
  seq: number
  widthPx: number
  heightPx: number
  widthPt: number
  heightPt: number
  /** The raw codec byte (`videoCodecName` reads it). */
  codec: number
  key: boolean
  ageMs: number
  tsMs: number
}

/** Split a K2SV message into its header and the access unit. */
export function decodeVideoFrame(buf: ArrayBuffer): (VideoFrameHeader & { data: Uint8Array<ArrayBuffer> }) | null {
  if (buf.byteLength < VIDEO_HEADER_LEN || frameMagic(buf) !== VIDEO_MAGIC) return null
  const v = new DataView(buf)
  return {
    seq: v.getUint32(4, true),
    widthPx: v.getUint16(8, true),
    heightPx: v.getUint16(10, true),
    widthPt: v.getUint16(12, true),
    heightPt: v.getUint16(14, true),
    codec: v.getUint8(16),
    key: (v.getUint8(17) & VIDEO_FLAG_KEY) !== 0,
    ageMs: v.getUint16(18, true),
    tsMs: v.getUint32(20, true),
    data: new Uint8Array(buf, VIDEO_HEADER_LEN),
  }
}

/** Encode a K2SV message the way the daemon does (tests). */
export function encodeVideoFrame(
  h: { seq: number; widthPx: number; heightPx: number; widthPt: number; heightPt: number; key: boolean; codec?: number; ageMs?: number; tsMs?: number },
  au: Uint8Array,
): ArrayBuffer {
  const buf = new ArrayBuffer(VIDEO_HEADER_LEN + au.byteLength)
  const v = new DataView(buf)
  for (let i = 0; i < 4; i++) v.setUint8(i, VIDEO_MAGIC.charCodeAt(i))
  v.setUint32(4, h.seq, true)
  v.setUint16(8, h.widthPx, true)
  v.setUint16(10, h.heightPx, true)
  v.setUint16(12, h.widthPt, true)
  v.setUint16(14, h.heightPt, true)
  v.setUint8(16, h.codec ?? VIDEO_CODEC_H264)
  v.setUint8(17, h.key ? VIDEO_FLAG_KEY : 0)
  v.setUint16(18, h.ageMs ?? 0, true)
  v.setUint32(20, h.tsMs ?? 0, true)
  new Uint8Array(buf, VIDEO_HEADER_LEN).set(au)
  return buf
}

/** Split a K2SF message into its header fields and the JPEG bytes. */
export function decodeFrame(buf: ArrayBuffer): { seq: number; widthPx: number; heightPx: number; widthPt: number; heightPt: number; jpeg: Uint8Array<ArrayBuffer> } | null {
  if (buf.byteLength < FRAME_HEADER_LEN) return null
  const v = new DataView(buf)
  const magic = String.fromCharCode(v.getUint8(0), v.getUint8(1), v.getUint8(2), v.getUint8(3))
  if (magic !== FRAME_MAGIC) return null
  return {
    seq: v.getUint32(4, true),
    widthPx: v.getUint16(8, true),
    heightPx: v.getUint16(10, true),
    widthPt: v.getUint16(12, true),
    heightPt: v.getUint16(14, true),
    jpeg: new Uint8Array(buf, FRAME_HEADER_LEN),
  }
}

/** Encode a K2SF frame the way the daemon does (tests, stubs). */
export function encodeFrame(h: { seq: number; widthPx: number; heightPx: number; widthPt: number; heightPt: number }, jpeg: Uint8Array): ArrayBuffer {
  const buf = new ArrayBuffer(FRAME_HEADER_LEN + jpeg.byteLength)
  const v = new DataView(buf)
  for (let i = 0; i < 4; i++) v.setUint8(i, FRAME_MAGIC.charCodeAt(i))
  v.setUint32(4, h.seq, true)
  v.setUint16(8, h.widthPx, true)
  v.setUint16(10, h.heightPx, true)
  v.setUint16(12, h.widthPt, true)
  v.setUint16(14, h.heightPt, true)
  new Uint8Array(buf, FRAME_HEADER_LEN).set(jpeg)
  return buf
}

/** One binary message read: a frame, or why it can't be shown
 *  (`video`: a K2SV codec this player can't read; `header`: neither magic). */
export type BinaryRead = { frame: StreamFrame } | { error: 'video' | 'header' }

/** A binary message → a frame, keeping the buffer for the worker player. */
export function readBinaryFrame(buf: ArrayBuffer): BinaryRead {
  if (frameMagic(buf) === VIDEO_MAGIC) {
    const v = decodeVideoFrame(buf)
    const kind = v ? videoCodecName(v.codec) : null
    if (!v || !kind) return { error: 'video' }
    return {
      frame: {
        seq: v.seq,
        widthPt: v.widthPt,
        heightPt: v.heightPt,
        source: { kind, key: v.key, data: v.data, widthPx: v.widthPx, heightPx: v.heightPx },
        raw: buf,
      },
    }
  }
  const f = decodeFrame(buf)
  if (!f) return { error: 'header' }
  return {
    frame: {
      seq: f.seq,
      widthPt: f.widthPt,
      heightPt: f.heightPt,
      source: { kind: 'jpeg', data: new Blob([f.jpeg], { type: 'image/jpeg' }) },
      raw: buf,
    },
  }
}

/** What this webview (or the player worker) can decode (`stream-video.ts`'s
 *  probe). */
export interface DecodeSupport {
  h264: boolean
  vp8: boolean
  jpeg: boolean
}

/** The `codecs=` list: what the source speaks (`want`, in preference
 *  order) cut to what the probe found. JPEG is the floor: an empty cut
 *  offers `jpeg` alone, so the daemon always has something to send. */
export function offerCodecs(support: DecodeSupport, want: readonly StreamCodec[] = ['h264', 'vp8', 'jpeg']): StreamCodec[] {
  const out = want.filter((c) => support[c])
  return out.length > 0 ? out : ['jpeg']
}
