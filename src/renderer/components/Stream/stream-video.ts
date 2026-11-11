// The stream engine's video decode (E9): K2SV frames carry an H.264 Annex B
// access unit (SPS + PPS in front of every keyframe; codec byte 1) or a VP8
// frame (codec byte 2), decoded here with WebCodecs (`VideoDecoder`; H.264
// is configured with no `description`, which means Annex B).
//
//  - `probeDecoders()` / `decodeSupport()`: what this webview can decode.
//    H.264 is what VideoToolbox sends (WKWebView 16.4+, WebView2 and
//    Chromium say yes); VP8 is a Linux host's software encode. WebKitGTK
//    builds without WebCodecs say no to both and the viewer stays on JPEG.
//    A pane offers only what the probe found (`offerCodecs`, R13).
//  - `VideoSink`: one per socket. Configures on the first keyframe (H.264:
//    the codec string from its SPS; VP8: `vp8`), decodes every frame in
//    order (a delta can't be skipped), draws each output unless the view is
//    held, and acks each seq once the decoder takes it (or, while chunks
//    pile up for the codec, once it is drawn or dropped), so the daemon's
//    ack window keeps moving even when the decoder holds frames. An H.264
//    keyframe's SPS gets the no-reorder VUI first (`h264-vui.ts`). A delta with no keyframe before it is dropped
//    (never decoded into garbage) and a keyframe asked for (`needKeyframe`,
//    again after `KEYFRAME_REASK_MS` if none came: the daemon drops asks
//    inside its throttle); a decoder error does the same, and three in a
//    row give up on video so the caller falls back to JPEG.
//
// Pure apart from `browserWebCodecs()` (which reads the globals when
// called): the seams are injected, and tests use `testing.ts`'s fake.

import { withLowLatencyVui } from './h264-vui'
import type { DecodeSupport, VideoCodec } from './stream-wire'

/** The parts of a WebCodecs `VideoFrame` we use. */
export interface VideoFrameLike {
  readonly timestamp: number
  readonly displayWidth: number
  readonly displayHeight: number
  close(): void
}

export interface VideoDecoderConfigLike {
  codec: string
  optimizeForLatency?: boolean
}

export interface VideoDecoderLike {
  readonly state: string
  /** Chunks waiting for the codec (WebCodecs); absent in some fakes. */
  readonly decodeQueueSize?: number
  configure(config: VideoDecoderConfigLike): void
  decode(chunk: unknown): void
  close(): void
}

export interface VideoDecoderInitLike {
  output(frame: VideoFrameLike): void
  error(err: unknown): void
}

/** WebCodecs, or a fake of it (tests). */
export interface WebCodecsLike {
  createDecoder(init: VideoDecoderInitLike): VideoDecoderLike
  createChunk(init: { type: 'key' | 'delta'; timestamp: number; data: Uint8Array }): unknown
  isConfigSupported(config: VideoDecoderConfigLike): Promise<boolean>
}

interface WebCodecsGlobals {
  VideoDecoder?: {
    new (init: VideoDecoderInitLike): VideoDecoderLike
    isConfigSupported(config: VideoDecoderConfigLike): Promise<{ supported?: boolean }>
  }
  EncodedVideoChunk?: new (init: { type: 'key' | 'delta'; timestamp: number; data: Uint8Array }) => unknown
}

/** The webview's WebCodecs, or null when it has none. */
export function browserWebCodecs(): WebCodecsLike | null {
  const g = globalThis as unknown as WebCodecsGlobals
  const Decoder = g.VideoDecoder
  const Chunk = g.EncodedVideoChunk
  if (typeof Decoder !== 'function' || typeof Chunk !== 'function') return null
  return {
    createDecoder: (init) => new Decoder(init),
    createChunk: (init) => new Chunk(init),
    isConfigSupported: async (config) => {
      try {
        return (await Decoder.isConfigSupported(config))?.supported === true
      } catch {
        return false
      }
    },
  }
}

/** What VideoToolbox sends: Constrained High (low-latency mode), or Main. */
export const H264_PROBE_CODECS: readonly string[] = ['avc1.640C1F', 'avc1.4D401F']
/** VP8 has one codec string. */
export const VP8_PROBE_CODECS: readonly string[] = ['vp8']

/** Set to "jpeg" to keep this webview's streams on JPEG (a debugging switch). */
export const CODEC_PREF_KEY = 'k2.stream.codec'
/** The Mobile tab's switch from before the engine module; still read. */
export const LEGACY_CODEC_PREF_KEY = 'k2.mobile.streamCodec'

function jpegPinned(): boolean {
  try {
    const ls = globalThis.localStorage
    return ls?.getItem(CODEC_PREF_KEY) === 'jpeg' || ls?.getItem(LEGACY_CODEC_PREF_KEY) === 'jpeg'
  } catch {
    return false
  }
}

async function anySupported(codecs: WebCodecsLike, list: readonly string[]): Promise<boolean> {
  for (const codec of list) {
    if (await codecs.isConfigSupported({ codec, optimizeForLatency: true })) return true
  }
  return false
}

/** Ask `isConfigSupported` for each video codec (H.264 first, then VP8).
 *  `jpeg` is the caller's: whether it has a JPEG decode path. */
export async function probeDecoders(codecs: WebCodecsLike | null, jpeg: boolean): Promise<DecodeSupport> {
  if (!codecs) return { h264: false, vp8: false, jpeg }
  const h264 = await anySupported(codecs, H264_PROBE_CODECS)
  const vp8 = await anySupported(codecs, VP8_PROBE_CODECS)
  return { h264, vp8, jpeg }
}

let cachedSupport: Promise<DecodeSupport> | null = null

/** What this thread can decode (probed once; the JPEG pin turns video off). */
export function decodeSupport(codecs: WebCodecsLike | null = browserWebCodecs()): Promise<DecodeSupport> {
  const jpeg = typeof (globalThis as { createImageBitmap?: unknown }).createImageBitmap === 'function'
  if (jpegPinned() || !codecs) return Promise.resolve({ h264: false, vp8: false, jpeg })
  if (!cachedSupport) cachedSupport = probeDecoders(codecs, jpeg)
  return cachedSupport
}

export function resetVideoSupportForTests(): void {
  cachedSupport = null
}

/** Split an Annex B stream into NAL units (3- or 4-byte start codes). */
export function splitNals(data: Uint8Array): Uint8Array[] {
  const starts: number[] = []
  for (let i = 0; i + 3 <= data.length; ) {
    if (data[i] === 0 && data[i + 1] === 0 && data[i + 2] === 1) {
      starts.push(i + 3)
      i += 3
    } else {
      i += 1
    }
  }
  const out: Uint8Array[] = []
  starts.forEach((s, k) => {
    let end = k + 1 < starts.length ? starts[k + 1] - 3 : data.length
    while (k + 1 < starts.length && end > s && data[end - 1] === 0) end -= 1
    if (end > s) out.push(data.subarray(s, end))
  })
  return out
}

const NAL_SPS = 7

/** `avc1.PPCCLL` from the first SPS (RFC 6381), or null without one. */
export function codecFromAnnexB(data: Uint8Array): string | null {
  const sps = splitNals(data).find((n) => (n[0] & 0x1f) === NAL_SPS)
  if (!sps || sps.length < 4) return null
  const hex = (b: number): string => b.toString(16).toUpperCase().padStart(2, '0')
  return `avc1.${hex(sps[1])}${hex(sps[2])}${hex(sps[3])}`
}

/** A VP8 key frame (RFC 6386 §9.1): frame tag bit 0 clear, then the start
 *  code 9d 01 2a and the 14-bit width and height. */
export function isVp8Keyframe(data: Uint8Array): boolean {
  return data.length >= 10 && (data[0] & 1) === 0 && data[3] === 0x9d && data[4] === 0x01 && data[5] === 0x2a
}

/** The decoder's codec string for a keyframe, or null when it isn't one. */
export function codecForKeyframe(codec: VideoCodec, data: Uint8Array): string | null {
  if (codec === 'vp8') return isVp8Keyframe(data) ? 'vp8' : null
  return codecFromAnnexB(data)
}

export interface VideoPacket {
  seq: number
  codec: VideoCodec
  key: boolean
  data: Uint8Array
  /** The picture size to show (the coded picture may be padded to even). */
  widthPx: number
  heightPx: number
}

export interface VideoSinkOptions {
  codecs: WebCodecsLike
  draw(frame: VideoFrameLike, widthPx: number, heightPx: number): void
  ack(seq: number): void
  /** The decoder lost its place: the pane sends `{"t":"keyframe"}`. */
  needKeyframe(): void
  /** Video can't go on here; switch the stream to JPEG. */
  failed(reason: string): void
  /** The view is held (frozen): decode, ack, don't draw. */
  held(): boolean
  /** ms, for the re-ask (default `performance.now`). */
  now?(): number
}

/** Decoder errors in a row before the sink gives up on video. */
export const MAX_DECODE_ERRORS = 3

/** Ask again for a keyframe this long after an unanswered ask (the daemon
 *  drops asks within 250 ms of the last, `KEYFRAME_MIN_GAP`). */
export const KEYFRAME_REASK_MS = 1000

interface Queued {
  seq: number
  widthPx: number
  heightPx: number
  /** Acked when handed to the decoder (it wasn't backed up). */
  acked: boolean
}

/** A frame is acked when the decoder takes it, unless this many chunks
 *  already wait for the codec; then on output. A decoder may hold decoded
 *  frames before output (no VUI reorder limit); acking only on output then
 *  starves the daemon's window of 3 (z3mbpZ 11-10: 2–4 fps of 55). */
export const DECODE_BACKLOG = 2

export class VideoSink {
  private decoder: VideoDecoderLike | null = null
  private codec: string | null = null
  private needKey = true
  private askedAt: number | null = null
  private queue: Queued[] = []
  private errors = 0
  private closed = false
  private readonly now: () => number

  constructor(private readonly opts: VideoSinkOptions) {
    this.now = opts.now ?? (() => performance.now())
  }

  /** Frames waiting in the decoder (tests). */
  get queued(): number {
    return this.queue.length
  }

  get configuredCodec(): string | null {
    return this.codec
  }

  push(p: VideoPacket): void {
    if (this.closed) {
      this.opts.ack(p.seq)
      return
    }
    if (p.key) {
      const codec = codecForKeyframe(p.codec, p.data)
      if (!codec) {
        this.opts.ack(p.seq)
        this.onError(new Error(p.codec === 'vp8' ? 'a VP8 keyframe without its start code' : 'a keyframe without an SPS'))
        return
      }
      if (!this.decoder || this.decoder.state === 'closed' || codec !== this.codec) {
        this.openDecoder(codec)
      }
      this.needKey = false
      this.askedAt = null
    } else if (this.needKey || !this.decoder) {
      // No reference to decode against: drop it, ask for a keyframe.
      this.opts.ack(p.seq)
      this.askKeyframe()
      return
    }
    const dec = this.decoder
    if (!dec) return
    const backlog = dec.decodeQueueSize
    const q: Queued = { seq: p.seq, widthPx: p.widthPx, heightPx: p.heightPx, acked: false }
    this.queue.push(q)
    // An H.264 keyframe's SPS gets the no-reorder VUI, so the decoder outputs
    // each frame at once (h264-vui.ts).
    const data = p.key && p.codec === 'h264' ? withLowLatencyVui(p.data) : p.data
    try {
      dec.decode(this.opts.codecs.createChunk({ type: p.key ? 'key' : 'delta', timestamp: p.seq, data }))
    } catch (err) {
      this.onError(err)
      return
    }
    // The decoder has it: free the daemon's window now, unless chunks are
    // piling up for the codec (then the ack waits for the output, so a slow
    // decoder still slows the stream). A decoder that doesn't report its
    // queue (a fake) keeps acking on output.
    if (backlog !== undefined && backlog <= DECODE_BACKLOG && !q.acked) {
      q.acked = true
      this.opts.ack(q.seq)
    }
  }

  private ackOnce(q: Queued): void {
    if (q.acked) return
    q.acked = true
    this.opts.ack(q.seq)
  }

  /** Stop decoding. `ackQueued`: the socket stays open (the daemon switched
   *  to JPEG), so free the window for what the decoder still held. */
  close(ackQueued = false): void {
    this.closed = true
    const queued = this.queue.splice(0)
    if (ackQueued) for (const q of queued) this.ackOnce(q)
    this.closeDecoder()
  }

  private openDecoder(codec: string): void {
    // Whatever the old decoder still held is lost with it.
    for (const q of this.queue.splice(0)) this.ackOnce(q)
    this.closeDecoder()
    let dec: VideoDecoderLike | null = null
    dec = this.opts.codecs.createDecoder({
      output: (f) => {
        if (dec !== this.decoder) {
          f.close()
          return
        }
        this.onOutput(f)
      },
      error: (e) => {
        if (dec === this.decoder) this.onError(e)
      },
    })
    this.decoder = dec
    this.codec = codec
    dec.configure({ codec, optimizeForLatency: true })
  }

  private closeDecoder(): void {
    const d = this.decoder
    this.decoder = null
    this.codec = null
    if (d && d.state !== 'closed') {
      try {
        d.close()
      } catch (err) {
        console.warn('[stream] video decoder close:', err)
      }
    }
  }

  private onOutput(f: VideoFrameLike): void {
    try {
      // Outputs come in decode order; anything older was dropped.
      while (this.queue.length > 0 && this.queue[0].seq !== f.timestamp) {
        const lost = this.queue.shift()
        if (lost) this.ackOnce(lost)
      }
      const q = this.queue.shift()
      if (!q) {
        console.warn('[stream] video: a decoded frame nobody queued', f.timestamp)
        return
      }
      this.errors = 0
      if (!this.closed && !this.opts.held()) this.opts.draw(f, q.widthPx, q.heightPx)
      this.ackOnce(q)
    } finally {
      f.close()
    }
  }

  private onError(err: unknown): void {
    this.errors += 1
    console.warn('[stream] video decode error:', err)
    // What the decoder held is lost: ack it so the window frees.
    for (const q of this.queue.splice(0)) this.ackOnce(q)
    this.closeDecoder()
    this.needKey = true
    if (this.errors >= MAX_DECODE_ERRORS) {
      this.closed = true
      this.opts.failed(`The video decoder failed ${this.errors} times in a row.`)
      return
    }
    this.askKeyframe()
  }

  private askKeyframe(): void {
    const t = this.now()
    if (this.askedAt !== null && t - this.askedAt < KEYFRAME_REASK_MS) return
    this.askedAt = t
    this.opts.needKeyframe()
  }
}
