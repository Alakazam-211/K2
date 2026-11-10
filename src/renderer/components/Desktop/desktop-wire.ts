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
  | { t: 'displays'; displays: Display[] }
  | { t: 'clipboard'; text: string | null; reason?: string }
  | { t: 'fps'; fps: number }
  | { t: 'codec'; codec: Codec }
  | { t: 'error'; code: string; message: string }

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

const SERVER_TAGS: ReadonlySet<string> = new Set([
  'hello', 'state', 'control', 'viewers', 'cursor', 'cursor-pos', 'displays', 'clipboard', 'fps', 'codec', 'error',
])
const VIEWER_TAGS: ReadonlySet<string> = new Set([
  'ack', 'fps', 'keyframe', 'pointer', 'button', 'wheel', 'key', 'text', 'clipboard-set', 'clipboard-get',
  'take-control', 'hand-back', 'display', 'view',
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
      default:
        return "You don't have access to this computer's screen."
    }
  }
  if (code === CLOSE_DESKTOP_OFF) {
    return reason === CLOSE_REASON.deskdEnded
      ? 'K2 Screen stopped on this computer.'
      : 'Remote desktop was turned off on this computer.'
  }
  return 'The connection to this computer closed.'
}
