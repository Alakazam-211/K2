// The device stream (prd-mobile-dev-v1 §7; vs-live M-A4, M-A19).
//
// `GET /cli/sim/stream?sim=<id>&token=…&fps=<n>` is a WebSocket:
//  - outbound (daemon → tab): BINARY messages are JPEG frames at dp/points
//    resolution; TEXT messages are JSON (`meta`, `control`, `error`);
//  - inbound (tab → daemon): JSON TEXT — one `ack` per drawn frame (one
//    frame in flight per viewer, the k1 ack idea), and the person's input
//    (`tap`, `swipe`, `type`, `key`, `button`, `rotate`, `take`, `release`).
//
// ADAPTER: Builder A owns the socket. Until the shapes are frozen this file
// is the only place that knows the wire; the pane talks to a
// `SimStreamConnection`. `createStubSimStream` stands in for the daemon in
// tests and headless screenshots (no JPEG decode, a painted fake screen).

import type { ServerScope } from '@/kessel/server-scope'
import { daemonWsBase } from '@/kessel/daemon-ws'
import { openQueuedWebSocket } from '@/lib/grid-dial-queue'
import type { SimControl, SimOrientation } from './simApi'

/** A point on the device screen, in dp / points. */
export interface SimPoint {
  x: number
  y: number
}

export type SimButton = 'home' | 'back' | 'lock' | 'menu'
export type SimNamedKey =
  | 'Enter'
  | 'Backspace'
  | 'Delete'
  | 'Tab'
  | 'Escape'
  | 'ArrowUp'
  | 'ArrowDown'
  | 'ArrowLeft'
  | 'ArrowRight'

/** The person's input, sent on the stream socket (M-A4). */
export type SimInput =
  | { action: 'tap'; x: number; y: number }
  | { action: 'swipe'; from: SimPoint; to: SimPoint; durationMs: number }
  | { action: 'type'; text: string }
  | { action: 'key'; key: SimNamedKey }
  | { action: 'button'; button: SimButton }
  | { action: 'rotate'; direction: 'left' | 'right' }
  | { action: 'take' }
  | { action: 'release' }

/** What a frame holds: a JPEG off the wire, or (stub) a painter. */
export type SimFrameSource =
  | { kind: 'jpeg'; data: Blob }
  | { kind: 'paint'; paint: (ctx: CanvasRenderingContext2D, width: number, height: number) => void }

export interface SimFrame {
  /** Per-connection counter; the ack names it. */
  seq: number
  source: SimFrameSource
}

/** The stream's view of the device (sent on connect and on change). */
export interface SimStreamMeta {
  /** Screen size in dp / points for the CURRENT orientation. */
  width: number
  height: number
  orientation: SimOrientation
}

export type SimStreamState = 'connecting' | 'open' | 'closed' | 'error'

export interface SimStreamHandlers {
  onFrame(frame: SimFrame): void
  onMeta(meta: SimStreamMeta): void
  onControl(control: SimControl): void
  onState(state: SimStreamState, reason?: string): void
}

export interface SimStreamConnection {
  send(input: SimInput): void
  /** The frame `seq` is drawn (or skipped while held); send the next. */
  ack(seq: number): void
  close(): void
}

export interface SimStreamOptions {
  scope: ServerScope
  simId: string
  /** Frame cap for this viewer: local up to 30, remote 8 by default (2–15). */
  fps: number
  handlers: SimStreamHandlers
}

export type SimStreamConnector = (opts: SimStreamOptions) => SimStreamConnection

/** Local viewers: up to 30 fps. Remote (Connect): 8 by default (M-A19). */
export const LOCAL_FPS = 30
export const REMOTE_DEFAULT_FPS = 8
export const REMOTE_FPS_CHOICES: readonly number[] = [2, 4, 8, 12, 15]

export function defaultFps(isRemote: boolean): number {
  return isRemote ? REMOTE_DEFAULT_FPS : LOCAL_FPS
}

/** The stream URL. `token` rides the query for WebSockets (same as the grid
 *  socket); the daemon re-checks it every 5 s (M-A4). */
export function simStreamUrl(wsBase: string, simId: string, token: string | null | undefined, fps: number): string {
  const q = new URLSearchParams({ sim: simId, fps: String(Math.round(fps)) })
  if (token) q.set('token', token)
  return `${wsBase}/cli/sim/stream?${q.toString()}`
}

function parseText(
  raw: string,
): { type: 'meta'; meta: SimStreamMeta } | { type: 'control'; control: SimControl } | { type: 'error'; message: string } | null {
  let msg: unknown
  try {
    msg = JSON.parse(raw)
  } catch {
    console.warn('[mobile] stream: text frame is not JSON:', raw.slice(0, 120))
    return null
  }
  if (!msg || typeof msg !== 'object') return null
  const m = msg as Record<string, unknown>
  if (m.type === 'meta' && typeof m.width === 'number' && typeof m.height === 'number') {
    return {
      type: 'meta',
      meta: { width: m.width, height: m.height, orientation: m.orientation === 'landscape' ? 'landscape' : 'portrait' },
    }
  }
  if (m.type === 'control') {
    const heldBy = m.heldBy === 'person' || m.heldBy === 'takeover' ? m.heldBy : null
    return { type: 'control', control: { heldBy, ...(typeof m.by === 'string' && m.by ? { by: m.by } : {}) } }
  }
  if (m.type === 'error') return { type: 'error', message: typeof m.message === 'string' ? m.message : 'stream error' }
  console.warn('[mobile] stream: unknown text frame:', raw.slice(0, 120))
  return null
}

/** The real socket (M-A4). */
export const wsSimStream: SimStreamConnector = ({ scope, simId, fps, handlers }) => {
  let ws: WebSocket | null = null
  let closed = false
  let seq = 0
  const pending: string[] = []
  handlers.onState('connecting')
  const sendRaw = (text: string): void => {
    if (ws && ws.readyState === WebSocket.OPEN) ws.send(text)
    else if (!closed) pending.push(text)
  }
  void (async () => {
    try {
      const creds = await scope.creds()
      if (closed) return
      // MS70: every daemon socket dials through the per-server queue.
      const sock = await openQueuedWebSocket(scope, simStreamUrl(daemonWsBase(creds), simId, creds.token, fps))
      if (closed) {
        sock.close()
        return
      }
      sock.binaryType = 'blob'
      ws = sock
      const opened = (): void => {
        if (closed) return
        handlers.onState('open')
        // Input typed before the socket opened goes out in order. Acks are
        // never queued (a new socket starts with a key frame).
        for (const p of pending.splice(0)) sock.send(p)
      }
      if (sock.readyState === WebSocket.OPEN) opened()
      else sock.onopen = opened
      sock.onmessage = (ev: MessageEvent) => {
        if (closed) return
        if (typeof ev.data === 'string') {
          const m = parseText(ev.data)
          if (!m) return
          if (m.type === 'meta') handlers.onMeta(m.meta)
          else if (m.type === 'control') handlers.onControl(m.control)
          else handlers.onState('error', m.message)
          return
        }
        seq += 1
        const data = ev.data instanceof Blob ? ev.data : new Blob([ev.data as ArrayBuffer], { type: 'image/jpeg' })
        handlers.onFrame({ seq, source: { kind: 'jpeg', data } })
      }
      sock.onerror = () => {
        if (!closed) handlers.onState('error', 'The device stream failed.')
      }
      sock.onclose = (ev: CloseEvent) => {
        if (closed) return
        handlers.onState('closed', ev.reason || undefined)
      }
    } catch (err) {
      if (!closed) handlers.onState('error', err instanceof Error ? err.message : String(err))
    }
  })()
  return {
    send(input) {
      sendRaw(JSON.stringify(input))
    },
    ack(n) {
      if (ws && ws.readyState === WebSocket.OPEN) ws.send(JSON.stringify({ action: 'ack', seq: n }))
    },
    close() {
      closed = true
      pending.length = 0
      ws?.close()
      ws = null
    },
  }
}

// ── Stub (tests, headless screenshots, building before the daemon) ──────

export interface StubSimStream {
  connector: SimStreamConnector
  /** Every input the tab sent, in order. */
  inputs: SimInput[]
  /** Acked seqs, in order. */
  acks: number[]
  /** Push one frame now (a painted fake screen). */
  pushFrame(label?: string): void
  pushMeta(meta: SimStreamMeta): void
  pushControl(control: SimControl): void
  /** How many connections are open now. */
  openCount(): number
}

/** A fake home screen: status bar, a title, a grid of app tiles. */
function paintFakeScreen(label: string) {
  return (ctx: CanvasRenderingContext2D, w: number, h: number): void => {
    ctx.fillStyle = '#1d2433'
    ctx.fillRect(0, 0, w, h)
    ctx.fillStyle = '#e8ecf4'
    ctx.font = `${Math.max(10, Math.round(w / 28))}px sans-serif`
    ctx.fillText('9:41', w * 0.06, h * 0.035)
    ctx.font = `${Math.max(14, Math.round(w / 16))}px sans-serif`
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
  initial: SimStreamMeta = { width: 412, height: 915, orientation: 'portrait' },
): StubSimStream {
  const inputs: SimInput[] = []
  const acks: number[] = []
  const live = new Set<SimStreamHandlers>()
  let seq = 0
  let meta = initial
  const stub: StubSimStream = {
    inputs,
    acks,
    connector: ({ handlers }) => {
      live.add(handlers)
      handlers.onState('connecting')
      queueMicrotask(() => {
        if (!live.has(handlers)) return
        handlers.onState('open')
        handlers.onMeta(meta)
      })
      return {
        send(input) {
          inputs.push(input)
          if (input.action === 'rotate') {
            meta = {
              width: meta.height,
              height: meta.width,
              orientation: meta.orientation === 'portrait' ? 'landscape' : 'portrait',
            }
            for (const h of live) h.onMeta(meta)
          }
        },
        ack(n) {
          acks.push(n)
        },
        close() {
          live.delete(handlers)
        },
      }
    },
    pushFrame(label = 'Stub device') {
      seq += 1
      for (const h of live) h.onFrame({ seq, source: { kind: 'paint', paint: paintFakeScreen(label) } })
    },
    pushMeta(next) {
      meta = next
      for (const h of live) h.onMeta(next)
    },
    pushControl(control) {
      for (const h of live) h.onControl(control)
    },
    openCount: () => live.size,
  }
  return stub
}
