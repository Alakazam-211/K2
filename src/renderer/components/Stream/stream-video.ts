// The stream engine's video decode (E9): K2SV frames carry an H.264 Annex B
// access unit (SPS + PPS in front of every keyframe; codec byte 1) or a VP8
// frame (codec byte 2), decoded here with WebCodecs (`VideoDecoder`; H.264
// is configured with no `description`, which means Annex B).
//
//  - `probeDecoders()` / `decodeSupport()`: what this webview can decode,
//    asked with a `DecodeProbe` (the stream's by default; a file player
//    passes its own strings, AV1 among them). H.264 is what VideoToolbox sends (WKWebView 16.4+, WebView2 and
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

/** A WebCodecs `VideoPixelFormat` (`I420`, `NV12`, `BGRA`, ...). A string,
 *  so a format this file doesn't name still passes through. */
export type VideoPixelFormatLike = string

/** Where one plane landed in a `copyTo` buffer (WebCodecs `PlaneLayout`). */
export interface PlaneLayoutLike {
  offset: number
  stride: number
}

/** The `copyTo` / `allocationSize` options we use (WebCodecs
 *  `VideoFrameCopyToOptions`; `rect` and `layout` left out). */
export interface VideoFrameCopyToOptionsLike {
  /** Convert while copying (e.g. `RGBA`); omitted: the frame's own format. */
  format?: VideoPixelFormatLike
}

/** The parts of a WebCodecs `VideoFrame` we use. `format`, `allocationSize`
 *  and `copyTo` read the decoded planes (e.g. to hash a frame in YUV); they
 *  are optional because the stream only draws frames, and a decoder that
 *  can't expose its planes (a GPU-only frame) reports `format: null`. */
export interface VideoFrameLike {
  readonly timestamp: number
  readonly displayWidth: number
  readonly displayHeight: number
  /** The pixel layout `copyTo` gives by default, or null when the frame's
   *  memory can't be read. */
  readonly format?: VideoPixelFormatLike | null
  /** Bytes `copyTo` needs for these options. */
  allocationSize?(options?: VideoFrameCopyToOptionsLike): number
  /** Copy the planes into `destination`; resolves to each plane's layout. */
  copyTo?(destination: ArrayBuffer | ArrayBufferView, options?: VideoFrameCopyToOptionsLike): Promise<PlaneLayoutLike[]>
  close(): void
}

export interface VideoDecoderConfigLike {
  codec: string
  optimizeForLatency?: boolean
  /** Codec setup out of band: an `avcC` box body (H.264 in MP4, which also
   *  means the chunks are length-prefixed, not Annex B) or an `av1C` body.
   *  Omitted: H.264 is Annex B with SPS and PPS in band (the stream). */
  description?: ArrayBuffer | ArrayBufferView
  /** The coded picture size, when the container gives it. */
  codedWidth?: number
  codedHeight?: number
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

/** What a decode probe asks `isConfigSupported` about. Each family is
 *  supported when any of its strings is (asked in order, stopping at the
 *  first yes unless `everyString`). */
export interface DecodeProbe {
  h264: readonly string[]
  vp8: readonly string[]
  /** AV1 strings (e.g. `av01.0.08M.08`). Omitted: AV1 isn't asked about and
   *  the result has no `av01`. */
  av01?: readonly string[]
  /** Sent with every config asked about. Omitted: left out of the config. */
  optimizeForLatency?: boolean
  /** Ask every string, not just up to the first yes, and report each in the
   *  result's `byCodec` (for a capability readout). */
  everyString?: boolean
}

/** The live stream's probe: its two H.264 strings and VP8, low latency, first
 *  yes wins. The Mobile tab and the remote desktop use this. */
export const STREAM_DECODE_PROBE: DecodeProbe = {
  h264: H264_PROBE_CODECS,
  vp8: VP8_PROBE_CODECS,
  optimizeForLatency: true,
}

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

function probeConfig(codec: string, probe: DecodeProbe): VideoDecoderConfigLike {
  return probe.optimizeForLatency === undefined ? { codec } : { codec, optimizeForLatency: probe.optimizeForLatency }
}

async function anySupported(codecs: WebCodecsLike, list: readonly string[], probe: DecodeProbe, seen: Record<string, boolean> | null): Promise<boolean> {
  let any = false
  for (const codec of list) {
    const yes = await codecs.isConfigSupported(probeConfig(codec, probe))
    if (seen) seen[codec] = yes
    any ||= yes
    if (any && !seen) return true
  }
  return any
}

/** Every family the probe asks about reported unsupported. */
function noVideo(jpeg: boolean, probe: DecodeProbe): DecodeSupport {
  const out: DecodeSupport = { h264: false, vp8: false, jpeg }
  if (probe.av01) out.av01 = false
  if (probe.everyString) out.byCodec = {}
  return out
}

/** Ask `isConfigSupported` for each video codec (H.264 first, then VP8, then
 *  AV1 when the probe lists it). `jpeg` is the caller's: whether it has a
 *  JPEG decode path. The default probe is the live stream's. */
export async function probeDecoders(codecs: WebCodecsLike | null, jpeg: boolean, probe: DecodeProbe = STREAM_DECODE_PROBE): Promise<DecodeSupport> {
  if (!codecs) return noVideo(jpeg, probe)
  const seen: Record<string, boolean> | null = probe.everyString ? {} : null
  const h264 = await anySupported(codecs, probe.h264, probe, seen)
  const vp8 = await anySupported(codecs, probe.vp8, probe, seen)
  const out: DecodeSupport = { h264, vp8, jpeg }
  if (probe.av01) out.av01 = await anySupported(codecs, probe.av01, probe, seen)
  if (seen) out.byCodec = seen
  return out
}

let cachedSupport: Promise<DecodeSupport> | null = null
let cachedByProbe = new WeakMap<DecodeProbe, Promise<DecodeSupport>>()

/** What this thread can decode, probed once per probe. The default probe is
 *  the live stream's, and only it obeys the JPEG pin (a switch for streams,
 *  not for players of files). */
export function decodeSupport(codecs: WebCodecsLike | null = browserWebCodecs(), probe: DecodeProbe = STREAM_DECODE_PROBE): Promise<DecodeSupport> {
  const jpeg = typeof (globalThis as { createImageBitmap?: unknown }).createImageBitmap === 'function'
  if (probe === STREAM_DECODE_PROBE) {
    if (jpegPinned() || !codecs) return Promise.resolve(noVideo(jpeg, probe))
    if (!cachedSupport) cachedSupport = probeDecoders(codecs, jpeg)
    return cachedSupport
  }
  if (!codecs) return Promise.resolve(noVideo(jpeg, probe))
  let p = cachedByProbe.get(probe)
  if (!p) {
    p = probeDecoders(codecs, jpeg, probe)
    cachedByProbe.set(probe, p)
  }
  return p
}

export function resetVideoSupportForTests(): void {
  cachedSupport = null
  cachedByProbe = new WeakMap()
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
 *  starves the daemon's window of 3 (mac-b 11-10: 2–4 fps of 55). */
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
