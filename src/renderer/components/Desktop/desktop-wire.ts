// The Desktop tab's stream wire (prd-remote-desktop-tab-v1 §6), mirrored
// from `crates/k2-core/src/desktop/wire.rs` field for field.
//
// Pure (R14): no React, no DOM, no `window`, so the phone (P2) can lift it.
// Text messages are JSON tagged by `t` in kebab-case with camelCase fields;
// a message whose `t` this client doesn't know is refused, never guessed at.
// Binary frames are the sim's K2SF / K2SV headers; the desktop adds K2SV
// codec byte 2 = VP8.

/** K2SV codec bytes. */
export const VIDEO_CODEC_H264 = 1
export const VIDEO_CODEC_VP8 = 2

/** Close codes (the sim's numbers). */
export const CLOSE_AUTH_REVOKED = 4401
export const CLOSE_FORBIDDEN = 4403
export const CLOSE_DESKTOP_OFF = 4410

/** Close reasons (the WebSocket close frame's text). */
export const CLOSE_REASON = {
  authRevoked: 'auth_revoked',
  grantGone: 'grant_gone',
  forbidden: 'forbidden',
  kicked: 'kicked',
  sameMachine: 'same_machine',
  auditUnavailable: 'audit_unavailable',
  desktopOff: 'desktop_off',
  deskdEnded: 'deskd_ended',
  /** 4403: no input and no pane shown for 30 min (S8). */
  idle: 'idle',
  /** 4403: someone clicked Disconnect on the banner at the screen. */
  disconnectedAtScreen: 'disconnected_at_screen',
  /** 4403: the 8 h limit of one viewing (S8). */
  sessionLimit: 'session_limit',
  /** 4410: K2 Screen couldn't run the picture and had nothing to fall back
   *  to (a Mac's K2 Screen sends H.264 only). A `state` message with why
   *  comes just before. */
  pictureFailed: 'picture_failed',
} as const

export type Codec = 'jpeg' | 'h264' | 'vp8'
export type ViewerPath = 'local' | 'lan' | 'tunnel'
export type Lane = 'main' | 'media'
export type Encoder = 'hardware' | 'software'
export type Mode = 'view' | 'control'
export type HostOs = 'macos' | 'linux' | 'windows'
export type Button = 'left' | 'right' | 'middle' | 'back' | 'forward'
export type WheelMode = 'pixel' | 'line'

export const BUTTONS: readonly Button[] = ['left', 'right', 'middle', 'back', 'forward']

export interface Display {
  id: string
  name: string
  widthPt: number
  heightPt: number
  widthPx: number
  heightPx: number
  scale: number
  main: boolean
}

export interface ViewerName {
  name: string
  mode: Mode
}

/** Daemon → viewer. */
export type ServerMsg =
  | {
      t: 'hello'
      desktop: string
      display: string
      displays: Display[]
      codec: Codec
      window: number
      fps: number
      path: ViewerPath
      lane: Lane
      encoder: Encoder
      mode: Mode
      controller: string | null
      os: HostOs
      localWins: boolean
      sameMachine: boolean
    }
  | { t: 'state'; state: string; message: string }
  | { t: 'control'; controller: string | null; you: boolean; localHold: boolean }
  | { t: 'viewers'; viewers: ViewerName[] }
  | { t: 'cursor'; id: number; hotX: number; hotY: number; png: string }
  | { t: 'cursor-pos'; x: number; y: number; visible: boolean }
  | { t: 'displays'; displays: Display[]; /** The display shown now. */ display?: string }
  | { t: 'clipboard'; text: string | null; reason?: string }
  | { t: 'fps'; fps: number }
  | { t: 'codec'; codec: Codec; message?: string }
  | { t: 'error'; code: string; message: string }
  | { t: 'stats'; stats: StreamStats }

/** The engine's `stats` readout (`stats=1`, the sim's shape): the daemon's
 *  stage times over the last second. */
export interface StreamStats {
  frames: number
  stages: { stage: string; n: number; p50: number; p95: number; max: number }[]
  inputs: { id: number; recvMs: number; laneMs: number }[]
  hubCpuMs: number
  encoderCpuMs: number
}

/** Viewer → daemon. */
export type ViewerMsg =
  | { t: 'ack'; seq: number }
  | { t: 'fps'; fps: number }
  | { t: 'keyframe' }
  | { t: 'pointer'; x: number; y: number }
  | { t: 'button'; button: Button; down: boolean }
  | { t: 'wheel'; dx: number; dy: number; mode: WheelMode }
  | { t: 'key'; code: string; down: boolean }
  | { t: 'text'; text: string }
  | { t: 'clipboard-set'; text: string }
  | { t: 'clipboard-get' }
  | { t: 'take-control' }
  | { t: 'hand-back' }
  | { t: 'display'; id: string }
  | { t: 'view'; on: boolean }
  /** This viewer has the tab in view (every 30 s while shown; LD-24). */
  | { t: 'present' }

const SERVER_TAGS: ReadonlySet<string> = new Set([
  'hello', 'state', 'control', 'viewers', 'cursor', 'cursor-pos', 'displays', 'clipboard', 'fps', 'codec', 'error', 'stats',
])
const VIEWER_TAGS: ReadonlySet<string> = new Set([
  'ack', 'fps', 'keyframe', 'pointer', 'button', 'wheel', 'key', 'text', 'clipboard-set', 'clipboard-get',
  'take-control', 'hand-back', 'display', 'view', 'present',
])

/** Messages that act on the machine (refused for a watcher). Same list as
 *  `ViewerMsg::is_input`. */
export function isInputMsg(m: ViewerMsg): boolean {
  return (
    m.t === 'pointer' ||
    m.t === 'button' ||
    m.t === 'wheel' ||
    m.t === 'key' ||
    m.t === 'text' ||
    m.t === 'clipboard-set' ||
    m.t === 'clipboard-get'
  )
}

function isObj(v: unknown): v is Record<string, unknown> {
  return !!v && typeof v === 'object' && !Array.isArray(v)
}

/** Parse one daemon text message; `null` for bad JSON or an unknown `t`. */
export function parseServerMsg(text: string): ServerMsg | null {
  let v: unknown
  try {
    v = JSON.parse(text)
  } catch {
    return null
  }
  if (!isObj(v) || typeof v.t !== 'string' || !SERVER_TAGS.has(v.t)) return null
  return v as unknown as ServerMsg
}

/** Parse one viewer message (the daemon's rules: unknown `t`, or a button
 *  this wire doesn't name, is refused). */
export function parseViewerMsg(text: string): ViewerMsg | null {
  let v: unknown
  try {
    v = JSON.parse(text)
  } catch {
    return null
  }
  if (!isObj(v) || typeof v.t !== 'string' || !VIEWER_TAGS.has(v.t)) return null
  if (v.t === 'button' && !BUTTONS.includes(v.button as Button)) return null
  return v as unknown as ViewerMsg
}

/** One viewer message as the wire's JSON (field order as serde writes it). */
export function serializeViewerMsg(m: ViewerMsg): string {
  return JSON.stringify(m)
}

/** The tab's line for a close, by code then reason. */
export function closeReasonText(code: number, reason: string): string {
  if (code === CLOSE_AUTH_REVOKED) {
    return reason === CLOSE_REASON.grantGone
      ? 'Your access to this screen was taken away.'
      : 'Your sign-in on this computer ended. Sign in again to watch.'
  }
  if (code === CLOSE_FORBIDDEN) {
    switch (reason) {
      case CLOSE_REASON.kicked:
        return 'The owner disconnected you.'
      case CLOSE_REASON.sameMachine:
        return "You're on this computer."
      case CLOSE_REASON.auditUnavailable:
        return "K2 couldn't write the access log, so it didn't connect you."
      case CLOSE_REASON.idle:
        return 'Closed after 30 minutes with no input and the tab hidden.'
      case CLOSE_REASON.disconnectedAtScreen:
        return 'Someone at the screen disconnected you.'
      case CLOSE_REASON.sessionLimit:
        return 'Closed after 8 hours. Reconnect to keep watching.'
      default:
        return "You don't have access to this computer's screen."
    }
  }
  if (code === CLOSE_DESKTOP_OFF) {
    if (reason === CLOSE_REASON.pictureFailed) return 'The picture stopped. Try again.'
    return reason === CLOSE_REASON.deskdEnded
      ? 'K2 Screen stopped on this computer.'
      : 'Remote desktop was turned off on this computer.'
  }
  return 'The connection to this computer closed.'
}

/** What a viewer offers and asks for when it dials (`/cli/desktop/stream`). */
export interface DesktopStreamQuery {
  /** The single-use ticket from `POST /cli/desktop/ticket` (vs-live R20). */
  ticket: string
  /** What this viewer decodes, in preference order (JPEG by feature check). */
  codecs: readonly Codec[]
  window: number
  display?: string | null
  fps?: number | null
  view?: boolean
  stats?: boolean
  /** The pane's width in device pixels. */
  pw?: number
}

/** The stream URL. Only the ticket authorizes it: no `token=` ever rides
 *  this URL (vs-live R20: URLs end up in relay and proxy logs). */
export function desktopStreamUrl(wsBase: string, q: DesktopStreamQuery): string {
  const p = new URLSearchParams({ ticket: q.ticket })
  p.set('codecs', (q.codecs.length > 0 ? q.codecs : ['jpeg']).join(','))
  p.set('win', String(Math.max(1, Math.round(q.window))))
  if (q.display) p.set('display', q.display)
  if (q.fps && q.fps > 0) p.set('fps', String(Math.round(q.fps)))
  if (q.view) p.set('view', '1')
  if (q.stats) p.set('stats', '1')
  const pw = Math.round(q.pw ?? 0)
  if (pw > 0) p.set('pw', String(pw))
  return `${wsBase}/cli/desktop/stream?${p.toString()}`
}

/** Dial again on its own after this close? Only after a drop (the network,
 *  the daemon restarting): never after 4401 or 4403 (access is gone or was
 *  refused: asking again would only be refused again), and never after 4410
 *  (the desktop is off; its card says so). */
export function reconnectsAfter(code: number): boolean {
  return code !== CLOSE_AUTH_REVOKED && code !== CLOSE_FORBIDDEN && code !== CLOSE_DESKTOP_OFF && code !== 1000
}
