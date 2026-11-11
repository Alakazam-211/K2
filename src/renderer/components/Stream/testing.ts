// The stream engine's test kit (E9): fakes a pane's tests share, so the
// Mobile tab and the remote desktop (D7b) test the same way. No vitest
// import here; tests install the globals themselves:
//
//   vi.stubGlobal('WebSocket', FakeWebSocket)
//   const codecs = fakeWebCodecs({ h264: true, vp8: false })
//   for (const [k, v] of Object.entries(webCodecsGlobals(codecs))) vi.stubGlobal(k, v)
//
//  - `FakeWebSocket`: a socket the adapter dials (it has the events the
//    per-server dial queue waits on); the test opens it, pushes text and
//    binary messages, closes it, and reads what was sent.
//  - `fakeWebCodecs`: a `WebCodecsLike` whose `isConfigSupported` answers
//    per codec and whose decoders record configure / decode and emit or
//    fail on demand; `webCodecsGlobals` wraps it as `VideoDecoder` /
//    `EncodedVideoChunk` for code that reads the globals.
//  - `fakePlayerWorker`: a `WorkerLike` that answers the player's probe.
//  - sample access units: an H.264 key (SPS + PPS + IDR) and delta, a VP8
//    key (frame tag + start code) and delta.

import type { WorkerLike } from './stream-player'
import type { FromWorker } from './stream-player.worker'
import type { VideoDecoderConfigLike, VideoDecoderInitLike, VideoDecoderLike, VideoFrameLike, WebCodecsLike } from './stream-video'
import type { DecodeSupport } from './stream-wire'

// ── Sample access units ─────────────────────────────────────────────────

const SC = [0, 0, 0, 1]
/** SPS (Constrained High 3.1, as VideoToolbox's low-latency mode sends) + PPS + IDR. */
export const H264_KEY = new Uint8Array([...SC, 0x67, 0x64, 0x0c, 0x1f, 0xac, ...SC, 0x68, 0xee, 0x3c, ...SC, 0x65, 0x88, 0x84])
export const H264_DELTA = new Uint8Array([...SC, 0x41, 0x9a, 0x02])
/** A VP8 key frame's first bytes: frame tag (key, shown), 9d 01 2a, 640×360. */
export const VP8_KEY = new Uint8Array([0x50, 0x2a, 0x00, 0x9d, 0x01, 0x2a, 0x80, 0x02, 0x68, 0x01, 0x00])
/** A VP8 inter frame's tag (bit 0 set). */
export const VP8_DELTA = new Uint8Array([0x31, 0x05, 0x00, 0x11, 0x22])

// ── A fake WebSocket ────────────────────────────────────────────────────

export class FakeWebSocket {
  static instances: FakeWebSocket[] = []
  static CONNECTING = 0
  static OPEN = 1
  static CLOSING = 2
  static CLOSED = 3
  /** Forget every socket (call in `beforeEach`). */
  static reset(): void {
    FakeWebSocket.instances = []
  }
  /** The newest socket, or throw. */
  static last(): FakeWebSocket {
    const s = FakeWebSocket.instances[FakeWebSocket.instances.length - 1]
    if (!s) throw new Error('no socket was dialed')
    return s
  }
  url: string
  readyState = 0
  binaryType = 'blob'
  /** Text the viewer sent, in order. */
  sent: string[] = []
  onopen: (() => void) | null = null
  onmessage: ((ev: { data: unknown }) => void) | null = null
  onerror: (() => void) | null = null
  onclose: ((ev: { code: number; reason?: string }) => void) | null = null
  private listeners = new Map<string, Set<() => void>>()
  constructor(url: string) {
    this.url = url
    FakeWebSocket.instances.push(this)
  }
  addEventListener(type: string, fn: () => void): void {
    const set = this.listeners.get(type) ?? new Set()
    set.add(fn)
    this.listeners.set(type, set)
  }
  removeEventListener(type: string, fn: () => void): void {
    this.listeners.get(type)?.delete(fn)
  }
  /** The query of the URL dialed. */
  get query(): URLSearchParams {
    return new URL(this.url).searchParams
  }
  open(): void {
    this.readyState = 1
    for (const fn of [...(this.listeners.get('open') ?? [])]) fn()
    this.onopen?.()
  }
  /** The daemon sends a text message (an object is JSON-encoded). */
  serverText(msg: string | object): void {
    this.onmessage?.({ data: typeof msg === 'string' ? msg : JSON.stringify(msg) })
  }
  /** The daemon sends a binary message. */
  serverBinary(buf: ArrayBuffer): void {
    this.onmessage?.({ data: buf })
  }
  /** The daemon closes the socket. */
  serverClose(code = 1000, reason = ''): void {
    this.readyState = 3
    for (const fn of [...(this.listeners.get('close') ?? [])]) fn()
    this.onclose?.({ code, reason })
  }
  send(text: string): void {
    if (this.readyState !== 1) throw new Error('send before open')
    this.sent.push(text)
  }
  /** What was sent, parsed. */
  sentJson(): { t: string; [k: string]: unknown }[] {
    return this.sent.map((t) => JSON.parse(t) as { t: string; [k: string]: unknown })
  }
  close(): void {
    this.readyState = 3
  }
}

// ── A fake WebCodecs ────────────────────────────────────────────────────

export class FakeVideoDecoder implements VideoDecoderLike {
  state = 'unconfigured'
  configs: VideoDecoderConfigLike[] = []
  chunks: { type: string; timestamp: number; bytes: number }[] = []
  /** WebCodecs' queue of chunks not yet at the codec. Unset (undefined): the
   *  sink acks on output, as with a decoder that doesn't report it. */
  decodeQueueSize?: number
  constructor(readonly init: VideoDecoderInitLike) {}
  configure(c: VideoDecoderConfigLike): void {
    this.state = 'configured'
    this.configs.push(c)
  }
  decode(chunk: unknown): void {
    if (this.state !== 'configured') throw new Error('decode before configure')
    this.chunks.push(chunk as { type: string; timestamp: number; bytes: number })
  }
  close(): void {
    this.state = 'closed'
  }
  /** Emit the decoded frame for `timestamp` (the sink uses the seq). */
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
  /** The decoder fails (as WebCodecs does: it closes, then calls error). */
  fail(err: unknown = new Error('decode failed')): void {
    this.state = 'closed'
    this.init.error(err)
  }
}

export interface FakeWebCodecsSupport {
  /** true: every `avc1.*` string; a list: only those. */
  h264?: boolean | readonly string[]
  vp8?: boolean
}

export interface FakeWebCodecs extends WebCodecsLike {
  decoders: FakeVideoDecoder[]
  /** Every codec string `isConfigSupported` was asked about, in order. */
  asked: string[]
  /** The newest decoder, or throw. */
  last(): FakeVideoDecoder
}

export function fakeWebCodecs(support: FakeWebCodecsSupport = { h264: true, vp8: true }): FakeWebCodecs {
  const decoders: FakeVideoDecoder[] = []
  const asked: string[] = []
  const yes = (codec: string): boolean => {
    if (codec === 'vp8') return support.vp8 === true
    if (codec.startsWith('avc1.')) return Array.isArray(support.h264) ? support.h264.includes(codec) : support.h264 === true
    return false
  }
  return {
    decoders,
    asked,
    last() {
      const d = decoders[decoders.length - 1]
      if (!d) throw new Error('no decoder was made')
      return d
    },
    createDecoder: (init) => {
      const d = new FakeVideoDecoder(init)
      decoders.push(d)
      return d
    },
    createChunk: (init) => ({ type: init.type, timestamp: init.timestamp, bytes: init.data.length }),
    isConfigSupported: async (c) => {
      asked.push(c.codec)
      return yes(c.codec)
    },
  }
}

/** `VideoDecoder` and `EncodedVideoChunk` globals backed by `codecs`. */
export function webCodecsGlobals(codecs: WebCodecsLike): { VideoDecoder: unknown; EncodedVideoChunk: unknown } {
  class VideoDecoder {
    static isConfigSupported = async (c: VideoDecoderConfigLike) => ({ supported: await codecs.isConfigSupported(c) })
    private readonly inner: VideoDecoderLike
    constructor(init: VideoDecoderInitLike) {
      this.inner = codecs.createDecoder(init)
    }
    get state(): string {
      return this.inner.state
    }
    configure(c: VideoDecoderConfigLike): void {
      this.inner.configure(c)
    }
    decode(chunk: unknown): void {
      this.inner.decode(chunk)
    }
    close(): void {
      this.inner.close()
    }
  }
  class EncodedVideoChunk {
    readonly type: 'key' | 'delta'
    readonly timestamp: number
    readonly bytes: number
    constructor(init: { type: 'key' | 'delta'; timestamp: number; data: Uint8Array }) {
      this.type = init.type
      this.timestamp = init.timestamp
      this.bytes = init.data.length
    }
  }
  return { VideoDecoder, EncodedVideoChunk }
}

// ── A fake player worker ────────────────────────────────────────────────

export interface FakePlayerWorker extends WorkerLike {
  /** Everything the player posted, with what it transferred. */
  posted: { msg: { t: string; [k: string]: unknown }; transfer: Transferable[] }[]
  /** The worker answers. */
  reply(msg: FromWorker): void
}

/** A worker that answers the probe with `probe` (a microtask later). */
export function fakePlayerWorker(probe: { canvas2d: boolean; decode: DecodeSupport }): FakePlayerWorker {
  const w: FakePlayerWorker = {
    posted: [],
    onmessage: null,
    onerror: null,
    postMessage(msg, transfer) {
      const m = msg as { t: string }
      w.posted.push({ msg: m, transfer: transfer ?? [] })
      if (m.t === 'probe') queueMicrotask(() => w.reply({ t: 'probe', ...probe }))
    },
    reply(msg) {
      w.onmessage?.({ data: msg })
    },
    terminate() {},
  }
  return w
}
