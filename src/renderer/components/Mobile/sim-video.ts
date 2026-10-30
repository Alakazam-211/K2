// The device stream as H.264 (0.46.7): K2SV frames carry Annex B access
// units (SPS + PPS in front of every keyframe), decoded here with WebCodecs
// (`VideoDecoder`, configured with no `description`, which means Annex B).
//
//  - `videoDecodeSupport()`: can this webview decode what VideoToolbox
//    sends? WKWebView (Safari 16.4+), WebView2 and Chromium say yes;
//    WebKitGTK builds without WebCodecs say no and the tab stays on JPEG.
//  - `H264Sink`: one per socket. Configures on the first keyframe (codec
//    string from its SPS), decodes every frame in order (a delta can't be
//    skipped), draws each output unless the view is held, and acks each seq
//    once it is drawn or dropped, so the daemon's ack window keeps moving.
//    A delta with no keyframe before it is skipped and a keyframe asked for;
//    a decoder error does the same; three in a row give up on video and the
//    caller falls back to JPEG.

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
export const PROBE_CODECS: readonly string[] = ['avc1.640C1F', 'avc1.4D401F']

/** Set to "jpeg" to keep this device's tab on JPEG (a debugging switch). */
export const CODEC_PREF_KEY = 'k2.mobile.streamCodec'

function jpegPinned(): boolean {
  try {
    return globalThis.localStorage?.getItem(CODEC_PREF_KEY) === 'jpeg'
  } catch {
    return false
  }
}

let cachedSupport: Promise<boolean> | null = null

/** Can this webview decode the daemon's H.264? (Probed once.) */
export function videoDecodeSupport(codecs: WebCodecsLike | null = browserWebCodecs()): Promise<boolean> {
  if (jpegPinned() || !codecs) return Promise.resolve(false)
  if (!cachedSupport) {
    cachedSupport = (async () => {
      for (const codec of PROBE_CODECS) {
        if (await codecs.isConfigSupported({ codec, optimizeForLatency: true })) return true
      }
      return false
    })()
  }
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

export interface H264Packet {
  seq: number
  key: boolean
  data: Uint8Array
  /** The picture size to show (the coded picture may be padded to even). */
  widthPx: number
  heightPx: number
}

export interface H264SinkOptions {
  codecs: WebCodecsLike
  draw(frame: VideoFrameLike, widthPx: number, heightPx: number): void
  ack(seq: number): void
  needKeyframe(): void
  /** Video can't go on here; switch the stream to JPEG. */
  failed(reason: string): void
  /** The view is held (frozen): decode, ack, don't draw. */
  held(): boolean
}

/** Decoder errors in a row before the sink gives up on video. */
export const MAX_DECODE_ERRORS = 3

interface Queued {
  seq: number
  widthPx: number
  heightPx: number
}

export class H264Sink {
  private decoder: VideoDecoderLike | null = null
  private codec: string | null = null
  private needKey = true
  private askedKey = false
  private queue: Queued[] = []
  private errors = 0
  private closed = false

  constructor(private readonly opts: H264SinkOptions) {}

  /** Frames waiting in the decoder (tests). */
  get queued(): number {
    return this.queue.length
  }

  get configuredCodec(): string | null {
    return this.codec
  }

  push(p: H264Packet): void {
    if (this.closed) {
      this.opts.ack(p.seq)
      return
    }
    if (p.key) {
      const codec = codecFromAnnexB(p.data)
      if (!codec) {
        this.opts.ack(p.seq)
        this.onError(new Error('a keyframe without an SPS'))
        return
      }
      if (!this.decoder || this.decoder.state === 'closed' || codec !== this.codec) {
        this.openDecoder(codec)
      }
      this.needKey = false
      this.askedKey = false
    } else if (this.needKey || !this.decoder) {
      // No reference to decode against: skip it, ask once for a keyframe.
      this.opts.ack(p.seq)
      this.askKeyframe()
      return
    }
    const dec = this.decoder
    if (!dec) return
    this.queue.push({ seq: p.seq, widthPx: p.widthPx, heightPx: p.heightPx })
    try {
      dec.decode(this.opts.codecs.createChunk({ type: p.key ? 'key' : 'delta', timestamp: p.seq, data: p.data }))
    } catch (err) {
      this.onError(err)
    }
  }

  /** Stop decoding. `ackQueued`: the socket stays open (the daemon switched
   *  to JPEG), so free the window for what the decoder still held. */
  close(ackQueued = false): void {
    this.closed = true
    const queued = this.queue.splice(0)
    if (ackQueued) for (const q of queued) this.opts.ack(q.seq)
    this.closeDecoder()
  }

  private openDecoder(codec: string): void {
    // Whatever the old decoder still held is lost with it.
    for (const q of this.queue.splice(0)) this.opts.ack(q.seq)
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
        console.warn('[mobile] video decoder close:', err)
      }
    }
  }

  private onOutput(f: VideoFrameLike): void {
    try {
      // Outputs come in decode order; anything older was dropped.
      while (this.queue.length > 0 && this.queue[0].seq !== f.timestamp) {
        const lost = this.queue.shift()
        if (lost) this.opts.ack(lost.seq)
      }
      const q = this.queue.shift()
      if (!q) {
        console.warn('[mobile] video: a decoded frame nobody queued', f.timestamp)
        return
      }
      this.errors = 0
      if (!this.closed && !this.opts.held()) this.opts.draw(f, q.widthPx, q.heightPx)
      this.opts.ack(q.seq)
    } finally {
      f.close()
    }
  }

  private onError(err: unknown): void {
    this.errors += 1
    console.warn('[mobile] video decode error:', err)
    // What the decoder held is lost: ack it so the window frees.
    for (const q of this.queue.splice(0)) this.opts.ack(q.seq)
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
    if (this.askedKey) return
    this.askedKey = true
    this.opts.needKeyframe()
  }
}
