// The device stream (prd-mobile-dev-v1 §7; vs-live M-A4, M-A19), mirroring
// Builder A's frozen wire, `k2-core::sim::stream`:
//
//   GET /cli/sim/stream?workspace=<id|handle|path>&token=…[&fps=<n>]
//       [&codecs=h264,jpeg&win=<n>]  (WebSocket)
//
//  - daemon → viewer, BINARY: a 16-byte header then a baseline JPEG:
//      0..4 b"K2SF" · 4..8 seq u32 · 8..10 width px u16 · 10..12 height px
//      u16 · 12..14 width pt u16 · 14..16 height pt u16 (all LE)
//  - daemon → viewer, BINARY (0.46.7, when `hello.codec` is "h264"): a
//    24-byte `K2SV` header then an Annex B H.264 access unit:
//      0..4 b"K2SV" · 4..8 seq u32 · 8..10 width px · 10..12 height px ·
//      12..14 width pt · 14..16 height pt · 16 codec u8 (1 = H.264) ·
//      17 flags u8 (bit 0 keyframe) · 18..20 age ms u16 · 20..24 ts ms u32
//    (all LE). Decoded by `sim-video.ts` (WebCodecs).
//  - daemon → viewer, TEXT: JSON tagged by "t": hello (with `codec` and
//    `window` since 0.46.7), state, hold, error, fps, codec (the daemon fell
//    back to JPEG mid-stream);
//  - viewer → daemon, TEXT: JSON tagged by "t": ack {seq} (up to `window`
//    frames in flight; 1 for an older daemon), fps, keyframe, touch, tap,
//    swipe, text, key (+shift/alt), paste, edit, button, rotate, appearance,
//    open-url, take-over, hand-back. Input is in points.
//  - negotiation: the viewer offers `codecs=h264,jpeg` (only when WebCodecs
//    here can decode H.264) and `win=3`; the daemon answers in `hello`. An
//    older daemon ignores both and sends K2SF frames one at a time.
//  - close codes: 4401 login revoked, 4403 forbidden, 4410 device stopped.
//
// The pane talks to a `SimStreamConnection`; `createStubSimStream` stands in
// for the daemon in tests and headless screenshots (a painted fake screen,
// no JPEG decode).

import type { ServerScope } from '@/kessel/server-scope'
import { daemonWsBase } from '@/kessel/daemon-ws'
import { openQueuedWebSocket } from '@/lib/grid-dial-queue'
import { parseDeviceProfile, parseSimHold, type SimDeviceProfile, type SimHold, type SimLaneId, type SimState } from './simApi'
import { videoDecodeSupport } from './sim-video'

export const FRAME_MAGIC = 'K2SF'
export const FRAME_HEADER_LEN = 16
export const VIDEO_MAGIC = 'K2SV'
export const VIDEO_HEADER_LEN = 24
export const VIDEO_CODEC_H264 = 1
export const VIDEO_FLAG_KEY = 1

/** Frames this tab asks to have in flight (`win=`; the daemon may give fewer). */
export const STREAM_WINDOW = 3

export type SimCodec = 'jpeg' | 'h264'

/** fps caps (stream::LOCAL_MAX_FPS, REMOTE_*). */
export const LOCAL_FPS = 30
export const REMOTE_DEFAULT_FPS = 8
export const REMOTE_FPS_CHOICES: readonly number[] = [2, 4, 8, 12, 15]
/** H.264 streams (stream::REMOTE_VIDEO_*): a remote tab may go to 30. */
export const REMOTE_VIDEO_DEFAULT_FPS = 24
export const REMOTE_VIDEO_FPS_CHOICES: readonly number[] = [2, 4, 8, 12, 15, 24, 30]

export const CLOSE_AUTH_REVOKED = 4401
export const CLOSE_FORBIDDEN = 4403
export const CLOSE_DEVICE_STOPPED = 4410

export function defaultFps(isRemote: boolean): number {
  return isRemote ? REMOTE_DEFAULT_FPS : LOCAL_FPS
}

/** `lane::Key`. */
export type SimKey =
  | 'enter'
  | 'backspace'
  | 'delete'
  | 'tab'
  | 'escape'
  | 'arrow-up'
  | 'arrow-down'
  | 'arrow-left'
  | 'arrow-right'
/** `lane::EditOp`: a ⌘ / Ctrl editing chord inside the device. */
export type SimEditOp = 'select-all' | 'undo' | 'redo' | 'cut' | 'copy'
/** `lane::Button`. */
export type SimButton = 'home' | 'back' | 'lock' | 'volume-up' | 'volume-down' | 'app-switch' | 'dev-menu'
/** `lane::Orientation`. */
export type SimOrientation = 'portrait' | 'landscape-left' | 'landscape-right' | 'portrait-upside-down'
export type SimAppearance = 'light' | 'dark'

/** `stream::ViewerMsg` the tab sends (ack and fps go through their own calls). */
export type SimInput =
  | { t: 'tap'; x: number; y: number }
  | { t: 'swipe'; x1: number; y1: number; x2: number; y2: number; durationMs?: number }
  | {
      t: 'touch'
      phase: 'down' | 'move' | 'up'
      x: number
      y: number
      /** A second finger (pinch); only where `hello.input.multiTouch`. */
      x2?: number
      y2?: number
      /** On `up`: the release velocity in points per second (a flick). */
      vx?: number
      vy?: number
    }
  | { t: 'text'; text: string }
  | { t: 'key'; key: SimKey; shift?: boolean; alt?: boolean }
  | { t: 'paste'; text: string }
  | { t: 'edit'; op: SimEditOp }
  | { t: 'button'; button: SimButton }
  | { t: 'rotate'; orientation: SimOrientation }
  | { t: 'appearance'; appearance: SimAppearance }
  | { t: 'open-url'; url: string }
  | { t: 'take-over' }
  | { t: 'hand-back' }

/** A point on the device screen, in points. */
export interface SimPoint {
  x: number
  y: number
}

/** What a frame holds: a JPEG or an H.264 access unit off the wire, or
 *  (stub) a painter. */
export type SimFrameSource =
  | { kind: 'jpeg'; data: Blob }
  | { kind: 'h264'; key: boolean; data: Uint8Array; widthPx: number; heightPx: number }
  | { kind: 'paint'; paint: (ctx: CanvasRenderingContext2D, width: number, height: number) => void }

export interface SimFrame {
  /** Per device; the ack names it. */
  seq: number
  /** The screen in points for the current orientation. */
  widthPt: number
  heightPt: number
  source: SimFrameSource
}

/** `stream::InputCaps` from `hello` (absent from older daemons). */
export interface SimInputCaps {
  /** Two fingers at once: pinch. */
  multiTouch: boolean
  /** `touch` up takes `vx`/`vy`. */
  fling: boolean
}

/** `stream::ServerMsg`. */
export type SimServerMsg =
  | {
      t: 'hello'
      sim: string
      lane: SimLaneId
      state: SimState
      profile?: SimDeviceProfile
      fps: number
      remote: boolean
      hold: SimHold
      isolation: string
      input?: SimInputCaps
      /** "jpeg" from an older daemon (it has no codec field). */
      codec: SimCodec
      /** Frames in flight this socket may have (1 from an older daemon). */
      window: number
    }
  | { t: 'state'; state: SimState; message?: string }
  | { t: 'hold'; hold: SimHold }
  | { t: 'error'; code: string; message: string }
  | { t: 'fps'; fps: number }
  | { t: 'codec'; codec: SimCodec; message?: string }

export type SimConnState = 'connecting' | 'open' | 'closed' | 'error'

export interface SimStreamHandlers {
  onFrame(frame: SimFrame): void
  onServer(msg: SimServerMsg): void
  onConn(state: SimConnState, reason?: string): void
}

export interface SimStreamConnection {
  send(input: SimInput): void
  /** Frame `seq` is drawn (or skipped while held); send the next. */
  ack(seq: number): void
  /** Ask for another rate (the daemon clamps and answers `fps`). */
  setFps(fps: number): void
  /** The video decoder lost its place: send a keyframe next. */
  requestKeyframe(): void
  close(): void
}

/** What this tab offers in the query. */
export interface SimStreamOffer {
  /** Offer H.264 when this webview can decode it. */
  video: boolean
  /** `win=`. */
  window: number
}

export interface SimStreamOptions {
  scope: ServerScope
  /** The workspace whose device to watch (path, handle or id). */
  workspace: string
  fps: number
  handlers: SimStreamHandlers
  /** Absent: the pre-0.46.7 query (JPEG, one frame in flight). */
  offer?: SimStreamOffer
}

export type SimStreamConnector = (opts: SimStreamOptions) => SimStreamConnection

/** The stream URL. `token` rides the query for WebSockets (same as the grid
 *  socket); the daemon re-checks it every 5 s. */
export function simStreamUrl(
  wsBase: string,
  workspace: string,
  token: string | null | undefined,
  fps: number,
  offer?: { codecs: readonly SimCodec[]; window: number },
): string {
  const q = new URLSearchParams({ workspace, fps: String(Math.round(fps)) })
  if (offer) {
    q.set('codecs', offer.codecs.join(','))
    q.set('win', String(Math.max(1, Math.round(offer.window))))
  }
  if (token) q.set('token', token)
  return `${wsBase}/cli/sim/stream?${q.toString()}`
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

/** Split a binary message into its header fields and the JPEG bytes. */
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

/** Encode a frame the way the daemon does (tests, the stub). */
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

function parseInputCaps(o: Record<string, unknown>): SimInputCaps {
  return { multiTouch: o.multiTouch === true, fling: o.fling === true }
}
const CODECS: ReadonlySet<string> = new Set(['jpeg', 'h264'])

const STATES: ReadonlySet<string> = new Set(['installing', 'booting', 'running', 'stopping', 'stopped', 'failed'])

/** One text frame → a server message, or null (logged). */
export function parseServerMsg(raw: string): SimServerMsg | null {
  let m: unknown
  try {
    m = JSON.parse(raw)
  } catch {
    console.warn('[mobile] stream: text frame is not JSON:', raw.slice(0, 120))
    return null
  }
  if (!m || typeof m !== 'object') return null
  const o = m as Record<string, unknown>
  try {
    switch (o.t) {
      case 'hello':
        if (typeof o.sim !== 'string' || typeof o.lane !== 'string' || typeof o.state !== 'string' || !STATES.has(o.state)) break
        if (o.codec !== undefined && (typeof o.codec !== 'string' || !CODECS.has(o.codec))) break
        return {
          t: 'hello',
          sim: o.sim,
          lane: o.lane as SimLaneId,
          state: o.state as SimState,
          ...(o.profile ? { profile: parseDeviceProfile(o.profile) } : {}),
          fps: typeof o.fps === 'number' ? o.fps : 0,
          remote: o.remote === true,
          hold: parseSimHold(o.hold),
          isolation: typeof o.isolation === 'string' ? o.isolation : '',
          ...(o.input && typeof o.input === 'object' ? { input: parseInputCaps(o.input as Record<string, unknown>) } : {}),
          codec: o.codec === undefined ? 'jpeg' : (o.codec as SimCodec),
          window: typeof o.window === 'number' && o.window >= 1 ? Math.floor(o.window) : 1,
        }
      case 'state':
        if (typeof o.state !== 'string' || !STATES.has(o.state)) break
        return { t: 'state', state: o.state as SimState, ...(typeof o.message === 'string' ? { message: o.message } : {}) }
      case 'hold':
        return { t: 'hold', hold: parseSimHold(o.hold) }
      case 'error':
        return { t: 'error', code: String(o.code ?? 'error'), message: String(o.message ?? 'stream error') }
      case 'fps':
        if (typeof o.fps !== 'number') break
        return { t: 'fps', fps: o.fps }
      case 'codec':
        if (typeof o.codec !== 'string' || !CODECS.has(o.codec)) break
        return { t: 'codec', codec: o.codec as SimCodec, ...(typeof o.message === 'string' ? { message: o.message } : {}) }
    }
  } catch (err) {
    console.warn('[mobile] stream: bad text frame:', err)
    return null
  }
  console.warn('[mobile] stream: unknown text frame:', raw.slice(0, 120))
  return null
}

/** What a close code means for the person watching. */
export function closeReason(code: number, reason: string): string {
  if (code === CLOSE_AUTH_REVOKED) return 'Your login was signed out of this server.'
  if (code === CLOSE_FORBIDDEN) return 'This login may not watch this device.'
  if (code === CLOSE_DEVICE_STOPPED) return 'The device stopped.'
  return reason || 'The device stream closed.'
}

/** The real socket. */
export const wsSimStream: SimStreamConnector = ({ scope, workspace, fps, handlers, offer }) => {
  let ws: WebSocket | null = null
  let closed = false
  const pending: string[] = []
  handlers.onConn('connecting')
  const sendRaw = (text: string): void => {
    if (ws && ws.readyState === WebSocket.OPEN) ws.send(text)
    else if (!closed) pending.push(text)
  }
  void (async () => {
    try {
      const creds = await scope.creds()
      if (closed) return
      const video = offer?.video ? await videoDecodeSupport() : false
      if (closed) return
      const codecs: SimCodec[] = video ? ['h264', 'jpeg'] : ['jpeg']
      const url = simStreamUrl(daemonWsBase(creds), workspace, creds.token, fps, offer ? { codecs, window: offer.window } : undefined)
      // MS70: every daemon socket dials through the per-server queue.
      const sock = await openQueuedWebSocket(scope, url)
      if (closed) {
        sock.close()
        return
      }
      sock.binaryType = 'arraybuffer'
      ws = sock
      const opened = (): void => {
        if (closed) return
        handlers.onConn('open')
        // Input sent before the socket opened goes out in order. Acks are
        // never queued (a new socket starts with a fresh frame).
        for (const p of pending.splice(0)) sock.send(p)
      }
      if (sock.readyState === WebSocket.OPEN) opened()
      else sock.onopen = opened
      sock.onmessage = (ev: MessageEvent) => {
        if (closed) return
        if (typeof ev.data === 'string') {
          const m = parseServerMsg(ev.data)
          if (m) handlers.onServer(m)
          return
        }
        if (ev.data instanceof ArrayBuffer && frameMagic(ev.data) === VIDEO_MAGIC) {
          const v = decodeVideoFrame(ev.data)
          if (!v || v.codec !== VIDEO_CODEC_H264) {
            console.warn('[mobile] stream: a K2SV message this tab cannot read')
            return
          }
          handlers.onFrame({
            seq: v.seq,
            widthPt: v.widthPt,
            heightPt: v.heightPt,
            source: { kind: 'h264', key: v.key, data: v.data, widthPx: v.widthPx, heightPx: v.heightPx },
          })
          return
        }
        const f = ev.data instanceof ArrayBuffer ? decodeFrame(ev.data) : null
        if (!f) {
          console.warn('[mobile] stream: a binary message without the K2SF header')
          return
        }
        handlers.onFrame({
          seq: f.seq,
          widthPt: f.widthPt,
          heightPt: f.heightPt,
          source: { kind: 'jpeg', data: new Blob([f.jpeg], { type: 'image/jpeg' }) },
        })
      }
      sock.onerror = () => {
        if (!closed) handlers.onConn('error', 'The device stream failed.')
      }
      sock.onclose = (ev: CloseEvent) => {
        if (closed) return
        handlers.onConn('closed', closeReason(ev.code, ev.reason))
      }
    } catch (err) {
      if (!closed) handlers.onConn('error', err instanceof Error ? err.message : String(err))
    }
  })()
  return {
    send(input) {
      sendRaw(JSON.stringify(input))
    },
    ack(seq) {
      if (ws && ws.readyState === WebSocket.OPEN) ws.send(JSON.stringify({ t: 'ack', seq }))
    },
    setFps(n) {
      sendRaw(JSON.stringify({ t: 'fps', fps: Math.round(n) }))
    },
    requestKeyframe() {
      if (ws && ws.readyState === WebSocket.OPEN) ws.send(JSON.stringify({ t: 'keyframe' }))
    },
    close() {
      closed = true
      pending.length = 0
      ws?.close()
      ws = null
    },
  }
}

// ── Stub (tests, headless screenshots) ──────────────────────────────────

export interface StubSimStream {
  connector: SimStreamConnector
  /** Every input the tab sent, in order. */
  inputs: SimInput[]
  /** Acked seqs, in order. */
  acks: number[]
  /** fps requests, in order. */
  fpsAsks: number[]
  /** Keyframe asks. */
  keyframeAsks: number
  /** The offer each connection made, in order. */
  offers: (SimStreamOffer | undefined)[]
  /** Push one frame now (a painted fake screen). */
  pushFrame(label?: string): void
  pushServer(msg: SimServerMsg): void
  /** How many connections are open now. */
  openCount(): number
  /** The workspace the last connection asked for. */
  lastWorkspace(): string | null
}

/** A fake home screen: status bar, a title, a grid of app tiles. */
function paintFakeScreen(label: string) {
  return (ctx: CanvasRenderingContext2D, w: number, h: number): void => {
    ctx.fillStyle = '#1d2433'
    ctx.fillRect(0, 0, w, h)
    ctx.fillStyle = '#e8ecf4'
    ctx.font = `${Math.max(10, Math.round(Math.min(w, h) / 28))}px sans-serif`
    ctx.fillText('9:41', w * 0.06, h * 0.035)
    ctx.font = `${Math.max(14, Math.round(Math.min(w, h) / 16))}px sans-serif`
    ctx.fillText(label, w * 0.08, h * 0.14)
    const cols = 4
    const size = (w * 0.84) / cols - w * 0.04
    const colors = ['#4f8cff', '#38c172', '#f6993f', '#e3342f', '#9561e2', '#f66d9b', '#4dc0b5', '#ffed4a']
    for (let i = 0; i < 12; i++) {
      const cx = w * 0.08 + (i % cols) * (size + w * 0.04)
      const cy = h * 0.22 + Math.floor(i / cols) * (size + w * 0.07)
      ctx.fillStyle = colors[i % colors.length]
      ctx.fillRect(cx, cy, size, size)
    }
  }
}

export function createStubSimStream(
  screen: { widthPt: number; heightPt: number } = { widthPt: 412, heightPt: 915 },
  caps: { input?: SimInputCaps } = {},
): StubSimStream {
  const inputs: SimInput[] = []
  const acks: number[] = []
  const fpsAsks: number[] = []
  const offers: (SimStreamOffer | undefined)[] = []
  const live = new Set<SimStreamHandlers>()
  let seq = 0
  let size = { ...screen }
  let workspace: string | null = null
  const stub: StubSimStream = {
    inputs,
    acks,
    fpsAsks,
    keyframeAsks: 0,
    offers,
    connector: (opts) => {
      const { handlers } = opts
      workspace = opts.workspace
      offers.push(opts.offer)
      live.add(handlers)
      handlers.onConn('connecting')
      queueMicrotask(() => {
        if (!live.has(handlers)) return
        handlers.onConn('open')
        handlers.onServer({
          t: 'hello',
          sim: 's1',
          lane: 'android',
          state: 'running',
          fps: opts.fps,
          remote: false,
          hold: { kind: 'free' },
          isolation: 'Separate, not sealed: the device runs as the same system user as this server’s agents.',
          ...(caps.input ? { input: caps.input } : {}),
          codec: 'jpeg',
          window: opts.offer ? Math.min(STREAM_WINDOW, opts.offer.window) : 1,
        })
      })
      return {
        send(input) {
          inputs.push(input)
          if (input.t === 'rotate') {
            const landscape = input.orientation === 'landscape-left' || input.orientation === 'landscape-right'
            const long = Math.max(size.widthPt, size.heightPt)
            const short = Math.min(size.widthPt, size.heightPt)
            size = landscape ? { widthPt: long, heightPt: short } : { widthPt: short, heightPt: long }
          }
        },
        ack(n) {
          acks.push(n)
        },
        setFps(n) {
          fpsAsks.push(n)
        },
        requestKeyframe() {
          stub.keyframeAsks += 1
        },
        close() {
          live.delete(handlers)
        },
      }
    },
    pushFrame(label = 'Stub device') {
      seq += 1
      for (const h of live) {
        h.onFrame({ seq, widthPt: size.widthPt, heightPt: size.heightPt, source: { kind: 'paint', paint: paintFakeScreen(label) } })
      }
    },
    pushServer(msg) {
      for (const h of live) h.onServer(msg)
    },
    openCount: () => live.size,
    lastWorkspace: () => workspace,
  }
  return stub
}
