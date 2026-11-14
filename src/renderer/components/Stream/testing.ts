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
//
// For players of media files (the seams in `stream-media.ts`):
//
//  - `fakeVideoFrame`: a decoded frame whose I420 planes `copyTo` can read
//    (or, with `format: null`, one that can't expose them), for tests that
//    hash frames; `FakeVideoDecoder.emitFrame` outputs one.
//  - `fakeRangeFetch`: a `FetchLike` over in-memory files that answers
//    `Range: bytes=a-b` with 206, records each request, and can stall.
//  - `fakeClock`: a `ClockLike` whose time moves only when the test says.
//  - `fakeAudioCodecs`: an `AudioCodecsLike` like `fakeWebCodecs`, whose
//    decoders emit planar float audio on demand.

import type {
  AudioCodecsLike,
  AudioDataLike,
  AudioDecoderConfigLike,
  AudioDecoderInitLike,
  AudioDecoderLike,
  ClockLike,
  FetchInitLike,
  FetchLike,
  FetchResponseLike,
} from './stream-media'
import type { WorkerLike } from './stream-player'
import type { FromWorker } from './stream-player.worker'
import type {
  PlaneLayoutLike,
  VideoDecoderConfigLike,
  VideoDecoderInitLike,
  VideoDecoderLike,
  VideoFrameCopyToOptionsLike,
  VideoFrameLike,
  WebCodecsLike,
} from './stream-video'
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
  /** Output a frame the test built (e.g. `fakeVideoFrame`, to hash it). */
  emitFrame<F extends VideoFrameLike>(frame: F): F {
    this.init.output(frame)
    return frame
  }
  /** The decoder fails (as WebCodecs does: it closes, then calls error). */
  fail(err: unknown = new Error('decode failed')): void {
    this.state = 'closed'
    this.init.error(err)
  }
}

export interface FakeVideoFrameInit {
  /** The frame's timestamp (µs for a file player; the seq for the stream). */
  timestamp: number
  width: number
  height: number
  /** `I420` (the default): `copyTo` gives Y, then U, then V, each tightly
   *  packed. `null`: a frame whose memory can't be read (`copyTo` rejects). */
  format?: 'I420' | null
  /** The three planes; default: Y = (timestamp + i) mod 256, U = V = 128. */
  planes?: readonly [Uint8Array, Uint8Array, Uint8Array]
}

export type FakeVideoFrame = VideoFrameLike & {
  readonly format: 'I420' | null
  closed: boolean
  /** The planes `copyTo` writes (Y, U, V). */
  readonly planes: readonly [Uint8Array, Uint8Array, Uint8Array]
}

/** A decoded frame for tests that read its planes. It doesn't convert:
 *  `copyTo` with a `format` other than its own rejects, as does any read
 *  after `close`. */
export function fakeVideoFrame(init: FakeVideoFrameInit): FakeVideoFrame {
  const { timestamp, width, height } = init
  const format = init.format === undefined ? 'I420' : init.format
  const cw = (width + 1) >> 1
  const ch = (height + 1) >> 1
  const planes: readonly [Uint8Array, Uint8Array, Uint8Array] = init.planes ?? [
    Uint8Array.from({ length: width * height }, (_, i) => (timestamp + i) & 0xff),
    new Uint8Array(cw * ch).fill(128),
    new Uint8Array(cw * ch).fill(128),
  ]
  const strides = [width, cw, cw]
  const size = planes[0].length + planes[1].length + planes[2].length
  const readable = (options?: VideoFrameCopyToOptionsLike): void => {
    if (f.closed) throw new Error('InvalidStateError: the frame is closed')
    if (format === null) throw new Error('NotSupportedError: this frame cannot be read')
    if (options?.format !== undefined && options.format !== format) throw new Error(`NotSupportedError: the fake does not convert to ${options.format}`)
  }
  const f: FakeVideoFrame = {
    timestamp,
    displayWidth: width,
    displayHeight: height,
    format,
    planes,
    closed: false,
    allocationSize(options) {
      readable(options)
      return size
    },
    async copyTo(destination, options) {
      readable(options)
      const out = ArrayBuffer.isView(destination)
        ? new Uint8Array(destination.buffer, destination.byteOffset, destination.byteLength)
        : new Uint8Array(destination)
      if (out.byteLength < size) throw new Error('TypeError: the destination is too small')
      const layout: PlaneLayoutLike[] = []
      let offset = 0
      planes.forEach((p, k) => {
        out.set(p, offset)
        layout.push({ offset, stride: strides[k] })
        offset += p.length
      })
      return layout
    },
    close() {
      f.closed = true
    },
  }
  return f
}

export interface FakeWebCodecsSupport {
  /** true: every `avc1.*` string; a list: only those. */
  h264?: boolean | readonly string[]
  vp8?: boolean
  /** true: every `av01.*` string; a list: only those. Default: none. */
  av01?: boolean | readonly string[]
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
    if (codec.startsWith('av01.')) return Array.isArray(support.av01) ? support.av01.includes(codec) : support.av01 === true
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

// ── A fake fetch with Range ─────────────────────────────────────────────

export interface FakeFetchRequest {
  url: string
  /** The `Range` header sent, or null for a whole-file request. */
  range: string | null
}

export interface FakeRangeFetch {
  fetch: FetchLike
  /** Every request, in the order made. */
  requests: FakeFetchRequest[]
  /** Hold every answer (new and pending) until `resume`: a stalled link. */
  stall(): void
  resume(): void
  /** Answer `url` with this status from now on (e.g. 404, 500). */
  failWith(url: string, status: number): void
}

class FakeResponse implements FetchResponseLike {
  readonly ok: boolean
  readonly headers: { get(name: string): string | null }
  constructor(
    readonly status: number,
    private readonly body: Uint8Array,
    headers: Record<string, string>,
  ) {
    this.ok = status >= 200 && status < 300
    const lower = new Map(Object.entries(headers).map(([k, v]) => [k.toLowerCase(), v]))
    this.headers = { get: (name) => lower.get(name.toLowerCase()) ?? null }
  }
  async arrayBuffer(): Promise<ArrayBuffer> {
    return this.body.slice().buffer
  }
}

function abortError(): Error {
  const e = new Error('The operation was aborted.')
  e.name = 'AbortError'
  return e
}

/** A fetch over `files` (URL → bytes). `Range: bytes=a-b` (or `a-`, or
 *  `-n` for the last n) answers 206 with `Content-Range`; a range starting
 *  past the end answers 416; no range answers 200 with the whole file; an
 *  unknown URL answers 404. An aborted signal rejects with `AbortError`,
 *  also while stalled. */
export function fakeRangeFetch(files: Readonly<Record<string, Uint8Array>>): FakeRangeFetch {
  const requests: FakeFetchRequest[] = []
  const failures = new Map<string, number>()
  let stalled = false
  let held: (() => void)[] = []

  const answer = (url: string, range: string | null): FetchResponseLike => {
    const failed = failures.get(url)
    if (failed !== undefined) return new FakeResponse(failed, new Uint8Array(0), {})
    const body = files[url]
    if (!body) return new FakeResponse(404, new Uint8Array(0), {})
    const size = body.length
    if (range === null) return new FakeResponse(200, body, { 'Content-Length': String(size), 'Accept-Ranges': 'bytes' })
    const m = /^bytes=(\d*)-(\d*)$/.exec(range.trim())
    if (!m || (m[1] === '' && m[2] === '')) return new FakeResponse(416, new Uint8Array(0), { 'Content-Range': `bytes */${size}` })
    let first: number
    let last: number
    if (m[1] === '') {
      first = Math.max(0, size - Number(m[2]))
      last = size - 1
    } else {
      first = Number(m[1])
      last = m[2] === '' ? size - 1 : Math.min(Number(m[2]), size - 1)
    }
    if (first >= size || last < first) return new FakeResponse(416, new Uint8Array(0), { 'Content-Range': `bytes */${size}` })
    return new FakeResponse(206, body.subarray(first, last + 1), {
      'Content-Range': `bytes ${first}-${last}/${size}`,
      'Content-Length': String(last - first + 1),
      'Accept-Ranges': 'bytes',
    })
  }

  const fetch: FetchLike = (url: string, init?: FetchInitLike) => {
    const headers = init?.headers ?? {}
    const key = Object.keys(headers).find((k) => k.toLowerCase() === 'range')
    const range = key === undefined ? null : headers[key]
    requests.push({ url, range })
    const signal = init?.signal
    return new Promise<FetchResponseLike>((resolve, reject) => {
      if (signal?.aborted) {
        reject(abortError())
        return
      }
      let done = false
      const onAbort = (): void => {
        if (done) return
        done = true
        reject(abortError())
      }
      signal?.addEventListener('abort', onAbort, { once: true })
      const settle = (): void => {
        if (done) return
        done = true
        signal?.removeEventListener('abort', onAbort)
        resolve(answer(url, range))
      }
      if (stalled) held.push(settle)
      else queueMicrotask(settle)
    })
  }

  return {
    fetch,
    requests,
    stall() {
      stalled = true
    },
    resume() {
      stalled = false
      const go = held
      held = []
      for (const settle of go) settle()
    },
    failWith(url, status) {
      failures.set(url, status)
    },
  }
}

// ── A fake clock ────────────────────────────────────────────────────────

export interface FakeClock extends ClockLike {
  /** Move time on by `ms`, firing each timer due on the way, in due order
   *  (ties in the order set). A timer set by a timer fires in the same
   *  advance if it falls due within it. */
  advance(ms: number): void
  /** Timers still waiting. */
  readonly pending: number
}

export function fakeClock(start = 0): FakeClock {
  let t = start
  let nextId = 1
  const timers = new Map<number, { at: number; fn: () => void }>()
  /** The id of the earliest timer due by `until` (ids rise in set order,
   *  so the lower id wins a tie), or null. */
  const due = (until: number): number | null => {
    let best: { id: number; at: number } | null = null
    for (const [id, tm] of timers) {
      if (tm.at > until) continue
      if (!best || tm.at < best.at || (tm.at === best.at && id < best.id)) best = { id, at: tm.at }
    }
    return best ? best.id : null
  }
  return {
    now: () => t,
    setTimeout(fn, ms) {
      const id = nextId++
      timers.set(id, { at: t + Math.max(0, ms), fn })
      return id
    },
    clearTimeout(id) {
      timers.delete(id)
    },
    advance(ms) {
      const end = t + ms
      for (let id = due(end); id !== null; id = due(end)) {
        const tm = timers.get(id)
        timers.delete(id)
        if (!tm) continue
        t = Math.max(t, tm.at)
        tm.fn()
      }
      t = end
    },
    get pending() {
      return timers.size
    },
  }
}

// ── A fake AudioDecoder ─────────────────────────────────────────────────

/** Float32 planar audio for tests: each channel's samples set to `value`. */
export function fakeAudioData(init: { timestamp: number; numberOfFrames: number; sampleRate?: number; numberOfChannels?: number; value?: number }): AudioDataLike & { closed: boolean } {
  const sampleRate = init.sampleRate ?? 48000
  const channels = init.numberOfChannels ?? 2
  const frames = init.numberOfFrames
  const plane = new Float32Array(frames).fill(init.value ?? 0)
  const check = (options: { planeIndex: number; format?: string }): void => {
    if (d.closed) throw new Error('InvalidStateError: the audio data is closed')
    if (options.planeIndex < 0 || options.planeIndex >= channels) throw new Error('RangeError: no such plane')
    if (options.format !== undefined && options.format !== 'f32-planar') throw new Error(`NotSupportedError: the fake does not convert to ${options.format}`)
  }
  const d = {
    timestamp: init.timestamp,
    duration: Math.round((frames / sampleRate) * 1e6),
    sampleRate,
    numberOfFrames: frames,
    numberOfChannels: channels,
    format: 'f32-planar',
    closed: false,
    allocationSize(options: { planeIndex: number; format?: string }) {
      check(options)
      return plane.byteLength
    },
    copyTo(destination: ArrayBuffer | ArrayBufferView, options: { planeIndex: number; format?: string }) {
      check(options)
      const out = ArrayBuffer.isView(destination)
        ? new Uint8Array(destination.buffer, destination.byteOffset, destination.byteLength)
        : new Uint8Array(destination)
      if (out.byteLength < plane.byteLength) throw new Error('RangeError: the destination is too small')
      out.set(new Uint8Array(plane.buffer))
    },
    close() {
      d.closed = true
    },
  }
  return d
}

export class FakeAudioDecoder implements AudioDecoderLike {
  state = 'unconfigured'
  configs: AudioDecoderConfigLike[] = []
  chunks: { type: string; timestamp: number; bytes: number }[] = []
  decodeQueueSize?: number
  /** `flush` calls not yet resolved; `emit` what they wait on, then `settle`. */
  flushes: { resolve(): void; reject(err: unknown): void }[] = []
  resets = 0
  constructor(readonly init: AudioDecoderInitLike) {}
  configure(c: AudioDecoderConfigLike): void {
    this.state = 'configured'
    this.configs.push(c)
  }
  decode(chunk: unknown): void {
    if (this.state !== 'configured') throw new Error('decode before configure')
    this.chunks.push(chunk as { type: string; timestamp: number; bytes: number })
  }
  flush(): Promise<void> {
    if (this.state !== 'configured') return Promise.reject(new Error('flush before configure'))
    return new Promise((resolve, reject) => this.flushes.push({ resolve, reject }))
  }
  /** Resolve every pending `flush`. */
  settle(): void {
    for (const f of this.flushes.splice(0)) f.resolve()
  }
  /** As WebCodecs: pending flushes reject with `AbortError`, and the decoder
   *  needs `configure` again. */
  reset(): void {
    if (this.state === 'closed') throw new Error('InvalidStateError: reset after close')
    this.resets += 1
    this.chunks = []
    this.rejectFlushes()
    this.state = 'unconfigured'
  }
  close(): void {
    this.rejectFlushes()
    this.state = 'closed'
  }
  private rejectFlushes(): void {
    for (const f of this.flushes.splice(0)) f.reject(abortError())
  }
  /** Output decoded audio for `timestamp` (µs): `frames` samples per channel. */
  emit(timestamp: number, frames = 960, value = 0): AudioDataLike & { closed: boolean } {
    const d = fakeAudioData({ timestamp, numberOfFrames: frames, value, numberOfChannels: this.configs.at(-1)?.numberOfChannels, sampleRate: this.configs.at(-1)?.sampleRate })
    this.init.output(d)
    return d
  }
  /** The decoder fails (it closes, then calls error). */
  fail(err: unknown = new Error('decode failed')): void {
    this.state = 'closed'
    this.init.error(err)
  }
}

export interface FakeAudioCodecs extends AudioCodecsLike {
  decoders: FakeAudioDecoder[]
  /** Every codec string `isConfigSupported` was asked about, in order. */
  asked: string[]
  last(): FakeAudioDecoder
}

/** An `AudioCodecsLike` whose `isConfigSupported` says yes to `opus` only
 *  when `support.opus` is true (the default), and no to everything else. */
export function fakeAudioCodecs(support: { opus?: boolean } = { opus: true }): FakeAudioCodecs {
  const decoders: FakeAudioDecoder[] = []
  const asked: string[] = []
  return {
    decoders,
    asked,
    last() {
      const d = decoders[decoders.length - 1]
      if (!d) throw new Error('no audio decoder was made')
      return d
    },
    createDecoder: (init) => {
      const d = new FakeAudioDecoder(init)
      decoders.push(d)
      return d
    },
    createChunk: (init) => ({ type: init.type, timestamp: init.timestamp, bytes: init.data.length }),
    isConfigSupported: async (c) => {
      asked.push(c.codec)
      return c.codec === 'opus' && support.opus === true
    },
  }
}
