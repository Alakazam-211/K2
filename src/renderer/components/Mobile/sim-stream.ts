// The device stream (prd-mobile-dev-v1 §7; vs-live M-A4, M-A19), mirroring
// Builder A's frozen wire, `k2-core::sim::stream`:
//
//   GET /cli/sim/stream?workspace=<id|handle|path>&token=…[&fps=<n>]
//       [&codecs=h264,jpeg&win=<n>][&stats=1]  (WebSocket)
//
//  - daemon → viewer, BINARY: K2SF (a 16-byte header then a baseline
//    JPEG) or, 0.46.7, when `hello.codec` is "h264", K2SV (a 24-byte header
//    then an Annex B H.264 access unit). The headers, their parse and the
//    decode are the stream engine's (`Stream/stream-wire.ts`,
//    `Stream/stream-video.ts`), shared with the remote desktop.
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
//  - `stats=1` (M0, measurement; only while the latency readout is on): a
//    `stats` text message about once a second with the daemon's stage
//    times, and the daemon-side time of each `touch` that carries an `id`.
//  - close codes: 4401 login revoked, 4403 forbidden, 4410 device stopped.
//
// The pane talks to a `SimStreamConnection`; `createStubSimStream` stands in
// for the daemon in tests and headless screenshots (a painted fake screen,
// no JPEG decode).

import type { ServerScope } from '@/kessel/server-scope'
import { daemonWsBase } from '@/kessel/daemon-ws'
import { openQueuedWebSocket } from '@/lib/grid-dial-queue'
import { decodeSupport } from '@/components/Stream/stream-video'
import { readBinaryFrame, STREAM_WINDOW, type StreamFrame, type StreamFrameSource } from '@/components/Stream/stream-wire'
import { parseDeviceProfile, parseSimHold, type SimDeviceProfile, type SimHold, type SimLaneId, type SimState } from './simApi'
import type { SimStreamStats } from './sim-latency'

export type SimCodec = 'jpeg' | 'h264'

/** What a local tab asks for (fix 2): H.264 here goes to 60
 *  (stream::LOCAL_VIDEO_MAX_FPS); the daemon clamps JPEG to 30
 *  (stream::LOCAL_MAX_FPS) and says so in `hello.fps`. */
export const LOCAL_FPS = 60
export const REMOTE_DEFAULT_FPS = 8
export const REMOTE_FPS_CHOICES: readonly number[] = [2, 4, 8, 12, 15]
/** H.264 streams (stream::REMOTE_VIDEO_*): a remote tab may go to 30. */
export const REMOTE_VIDEO_DEFAULT_FPS = 24
export const REMOTE_VIDEO_FPS_CHOICES: readonly number[] = [2, 4, 8, 12, 15, 24, 30]
/** H.264 on the LAN (stream::LAN_VIDEO_MAX_FPS): 2–60, default 60. */
export const LAN_VIDEO_DEFAULT_FPS = 60
export const LAN_VIDEO_FPS_CHOICES: readonly number[] = [2, 4, 8, 12, 15, 24, 30, 60]

/** How the viewer reaches the daemon (`hello.path`, stream::ViewerPath). */
export type SimViewerPath = 'local' | 'lan' | 'tunnel'

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
      /** A probe id (M0, `stats=1` only): the daemon reports this touch's time. */
      id?: number
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

/** What a frame holds (the engine's `StreamFrameSource`): a JPEG or a video
 *  access unit off the wire, or (stub) a painter. */
export type SimFrameSource = StreamFrameSource

/** A device frame (the engine's `StreamFrame`); its size in points is the
 *  screen's for the current orientation. */
export type SimFrame = StreamFrame

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
      /** Absent from an older daemon: unknown (the tab treats it as before). */
      path?: SimViewerPath
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
  | { t: 'stats'; stats: SimStreamStats }

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
  /** Ask for the daemon's `stats` (M0; the latency readout). */
  stats?: boolean
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
  extra?: { stats?: boolean },
): string {
  const q = new URLSearchParams({ workspace, fps: String(Math.round(fps)) })
  if (offer) {
    q.set('codecs', offer.codecs.join(','))
    q.set('win', String(Math.max(1, Math.round(offer.window))))
  }
  if (extra?.stats) q.set('stats', '1')
  if (token) q.set('token', token)
  return `${wsBase}/cli/sim/stream?${q.toString()}`
}

function parseInputCaps(o: Record<string, unknown>): SimInputCaps {
  return { multiTouch: o.multiTouch === true, fling: o.fling === true }
}
const CODECS: ReadonlySet<string> = new Set(['jpeg', 'h264'])
const PATHS: ReadonlySet<string> = new Set(['local', 'lan', 'tunnel'])

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
          ...(typeof o.path === 'string' && PATHS.has(o.path) ? { path: o.path as SimViewerPath } : {}),
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
      case 'stats': {
        const st = o.stats as Record<string, unknown> | undefined
        if (!st || typeof st !== 'object' || !Array.isArray(st.stages) || !Array.isArray(st.inputs)) break
        const num = (v: unknown): number => (typeof v === 'number' ? v : 0)
        return {
          t: 'stats',
          stats: {
            frames: num(st.frames),
            stages: (st.stages as Record<string, unknown>[]).map((x) => ({ stage: String(x.stage), n: num(x.n), p50: num(x.p50), p95: num(x.p95), max: num(x.max) })),
            inputs: (st.inputs as Record<string, unknown>[]).map((x) => ({ id: num(x.id), recvMs: num(x.recvMs), laneMs: num(x.laneMs) })),
            hubCpuMs: num(st.hubCpuMs),
            encoderCpuMs: num(st.encoderCpuMs),
          },
        }
      }
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
export const wsSimStream: SimStreamConnector = ({ scope, workspace, fps, handlers, offer, stats }) => {
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
      const video = offer?.video ? (await decodeSupport()).h264 : false
      if (closed) return
      const codecs: SimCodec[] = video ? ['h264', 'jpeg'] : ['jpeg']
      const url = simStreamUrl(daemonWsBase(creds), workspace, creds.token, fps, offer ? { codecs, window: offer.window } : undefined, { stats })
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
        const r: ReturnType<typeof readBinaryFrame> = ev.data instanceof ArrayBuffer ? readBinaryFrame(ev.data) : { error: 'header' }
        if ('error' in r) {
          console.warn(r.error === 'video' ? '[mobile] stream: a K2SV message this tab cannot read' : '[mobile] stream: a binary message without the K2SF header')
          return
        }
        handlers.onFrame(r.frame)
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
