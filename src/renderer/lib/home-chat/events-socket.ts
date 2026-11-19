// `WS /cli/home-chat/events` (prd-home-people-channels-v1 §14.4): one
// person's Home chat frames from one server. The daemon follows Sew for the
// person and sends `hello` (with the DM calls already ringing), `message`,
// `message_updated` (a call's record), `deleted`, `read`, `reaction`,
// `typing`, `member_removed`, `summary_changed`, `status`, and the DM call
// frames `call_ring` and `call_updated` (§11a.3), and `notify` (the
// person's push level for a conversation changed, on any device;
// prd-home-chat-pushes-v1 §7). Nothing from any other person or server.
//
// The client sends one kind of frame up: the viewing lease
// (`{"t":"viewing"}`, `lib/home-chat/viewing-lease.ts`), through the `send`
// that `onLink(true, send)` hands over while the socket is open.
//
// A Home chat socket of its own (not the app socket): a Voice & Chat only
// login never gets `/cli/sessions/events`. One per server per window
// (`stores/home-chat.ts` shares it); the daemon allows 3 per login.
//
// Kept like `voice-events-socket.ts`: reconnect with a widening pause; after
// a few failures in a row that never opened (the rows keep their last
// badges, greyed) it slows to one try a minute but never stops: this socket
// is how a DM call rings in this window, so a server back after a sleep or
// an outage rings again without a reload. Every close reaches the pool.

import type { ServerScope } from '@/kessel/server-scope'
import { withCliTokenQuery } from '@/web/session-token'
import { notePoolSocketClose } from '@/lib/pool-hooks'
import { openQueuedWebSocket } from '@/lib/grid-dial-queue'
import { parseCall, parseRing, type HomeChatCall, type HomeChatRing } from './calls'
import type { HomeChatNotifyLevel } from './api'

export const HOME_CHAT_EVENTS_PATH = '/cli/home-chat/events'

/** Pauses before each reconnect, then the last one repeats. */
export const HOME_CHAT_EVENTS_RETRY_MS = [2_000, 5_000, 15_000, 30_000] as const

/** Failures in a row (never opened) before the socket slows down. */
export const HOME_CHAT_EVENTS_MAX_DEAD_TRIES = 4

/** The pause between tries once it has slowed down. */
export const HOME_CHAT_EVENTS_SLOW_RETRY_MS = 60_000

/** One frame, parsed. Unknown frames are dropped. */
export type HomeChatFrame =
  | { type: 'hello'; chat: string | null; rings?: HomeChatRing[] }
  | { type: 'message'; conv: string; message: unknown }
  | { type: 'message_updated'; conv: string; message: unknown }
  | { type: 'call_ring'; conv: string; ring: HomeChatRing }
  | { type: 'call_updated'; conv: string; call: HomeChatCall }
  | { type: 'deleted'; conv: string; id: string }
  | { type: 'read'; conv: string }
  | { type: 'reaction'; conv: string; msg: string; emoji: string; on: boolean; count: number; member: string | null; mine: boolean }
  | { type: 'typing'; conv: string; member: string }
  | { type: 'member_removed'; conv: string }
  | { type: 'summary_changed'; conv: string | null }
  | { type: 'status'; chat: string | null }
  | { type: 'notify'; conv: string; level: HomeChatNotifyLevel | null }

function idOf(v: unknown): string | null {
  if (typeof v === 'string' && v) return v
  if (typeof v === 'number' && Number.isFinite(v)) return String(v)
  return null
}

/** A reaction frame's emoji: Sew's `{kind:'unicode', value}`, or a string. */
function emojiOf(v: unknown): string | null {
  if (typeof v === 'string' && v) return v
  const e = v as { kind?: unknown; value?: unknown } | null
  return e && e.kind === 'unicode' && typeof e.value === 'string' && e.value ? e.value : null
}

export function parseHomeChatFrame(data: unknown): HomeChatFrame | null {
  if (typeof data !== 'string') return null
  let v: Record<string, unknown>
  try {
    v = JSON.parse(data) as Record<string, unknown>
  } catch {
    return null
  }
  if (!v || typeof v !== 'object') return null
  const conv = idOf(v.conv)
  switch (v.type) {
    case 'hello': {
      // The calls already ringing for this person when the socket opened.
      const rings: HomeChatRing[] = []
      for (const c of Array.isArray(v.calls) ? (v.calls as Array<Record<string, unknown>>) : []) {
        const r = parseRing({ call_id: c.callId, channel_id: c.conv, started_by: c.startedBy, ringing: c.ringing, silent: c.silent, expires_at: c.expiresAt })
        if (r?.ringing) rings.push(r)
      }
      return { type: 'hello', chat: typeof v.chat === 'string' ? v.chat : null, rings }
    }
    case 'status':
      return { type: 'status', chat: typeof v.chat === 'string' ? v.chat : null }
    case 'message':
      return conv ? { type: 'message', conv, message: v.message } : null
    case 'message_updated':
      return conv ? { type: 'message_updated', conv, message: v.message } : null
    case 'call_ring': {
      const ring = parseRing(v.d)
      return ring ? { type: 'call_ring', conv: ring.conv, ring } : null
    }
    case 'call_updated': {
      const call = parseCall(v.d)
      return call ? { type: 'call_updated', conv: call.conv, call } : null
    }
    case 'deleted': {
      const id = idOf(v.id)
      return conv && id ? { type: 'deleted', conv, id } : null
    }
    case 'read':
      return conv ? { type: 'read', conv } : null
    case 'member_removed':
      return conv ? { type: 'member_removed', conv } : null
    case 'summary_changed':
      return { type: 'summary_changed', conv }
    case 'notify': {
      // A level the client doesn't know (or none): the store reads it again.
      const level = v.level === 'all' || v.level === 'mentions' || v.level === 'none' ? v.level : null
      return conv ? { type: 'notify', conv, level } : null
    }
    case 'typing': {
      const member = idOf(v.member)
      return conv && member ? { type: 'typing', conv, member } : null
    }
    case 'reaction': {
      const msg = idOf(v.msg)
      const emoji = emojiOf(v.emoji)
      if (!conv || !msg || !emoji) return null
      return {
        type: 'reaction',
        conv,
        msg,
        emoji,
        on: v.on === true,
        count: typeof v.count === 'number' ? v.count : 0,
        member: idOf(v.member),
        mine: v.mine === true,
      }
    }
    default:
      return null
  }
}

export interface HomeChatEventsDeps {
  open: (url: string, scope: ServerScope) => WebSocket | Promise<WebSocket>
  setTimer: (fn: () => void, ms: number) => unknown
  clearTimer: (t: unknown) => void
}

const realDeps: HomeChatEventsDeps = {
  open: (url, scope) => openQueuedWebSocket(scope, url),
  setTimer: (fn, ms) => setTimeout(fn, ms),
  clearTimer: (t) => clearTimeout(t as ReturnType<typeof setTimeout>),
}

/** A WebSocket's OPEN readyState (no global needed under test). */
const WS_OPEN = 1

/** Sends a text frame on the open socket; false when there is none. */
export type HomeChatEventsSend = (text: string) => boolean

/** Listen to `scope`'s Home chat frames. `onLink(true, send)` when a socket
 *  opens (`send` writes on it, and on each later socket), `onLink(false)`
 *  when it closes. Returns the unsubscribe. */
export function subscribeHomeChatEvents(
  scope: ServerScope,
  fn: (frame: HomeChatFrame) => void,
  onLink: (up: boolean, send?: HomeChatEventsSend) => void = () => {},
  deps: HomeChatEventsDeps = realDeps,
): () => void {
  let stopped = false
  let ws: WebSocket | null = null
  let timer: unknown = null
  let tries = 0
  let deadTries = 0

  const send: HomeChatEventsSend = (text) => {
    const s = ws
    if (stopped || !s || s.readyState !== WS_OPEN) return false
    try {
      s.send(text)
      return true
    } catch (err) {
      console.warn(`[home-chat] ${scope.hostKey}: send failed:`, err)
      return false
    }
  }

  const schedule = (): void => {
    if (stopped) return
    const ms =
      deadTries >= HOME_CHAT_EVENTS_MAX_DEAD_TRIES
        ? HOME_CHAT_EVENTS_SLOW_RETRY_MS
        : HOME_CHAT_EVENTS_RETRY_MS[Math.min(tries, HOME_CHAT_EVENTS_RETRY_MS.length - 1)]
    tries += 1
    timer = deps.setTimer(() => {
      timer = null
      void connect()
    }, ms)
  }

  const connect = async (): Promise<void> => {
    if (stopped) return
    let url: string
    try {
      const [base, creds] = await Promise.all([scope.wsBase(), scope.creds()])
      url = withCliTokenQuery(`${base}${HOME_CHAT_EVENTS_PATH}`, creds.token)
    } catch (err) {
      console.warn(`[home-chat] ${scope.hostKey}: no address yet:`, err)
      deadTries += 1
      schedule()
      return
    }
    if (stopped) return
    let opened = false
    let sock: WebSocket
    try {
      sock = await deps.open(url, scope)
    } catch (err) {
      console.warn(`[home-chat] ${scope.hostKey}: dial failed:`, err)
      deadTries += 1
      schedule()
      return
    }
    if (stopped) {
      sock.close()
      return
    }
    ws = sock
    sock.onopen = () => {
      opened = true
      tries = 0
      deadTries = 0
      onLink(true, send)
    }
    sock.onmessage = (ev: MessageEvent) => {
      const f = parseHomeChatFrame(ev.data)
      if (f) fn(f)
    }
    sock.onclose = (ev: CloseEvent) => {
      if (ws === sock) ws = null
      notePoolSocketClose(scope.hostKey, ev.code)
      if (opened) onLink(false)
      if (!opened) deadTries += 1
      schedule()
    }
  }

  void connect()
  return () => {
    stopped = true
    if (timer !== null) deps.clearTimer(timer)
    if (ws) {
      ws.onclose = null
      ws.close()
      ws = null
    }
  }
}
