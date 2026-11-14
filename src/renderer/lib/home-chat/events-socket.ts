// `WS /cli/home-chat/events` (prd-home-people-channels-v1 §14.4): one
// person's Home chat frames from one server. The daemon follows Sew for the
// person and sends `hello`, `message`, `deleted`, `read`, `reaction`,
// `typing`, `member_removed`, `summary_changed`, `status` and forwarded
// `event`s. Nothing from any other person or server.
//
// A Home chat socket of its own (not the app socket): a Voice & Chat only
// login never gets `/cli/sessions/events`. One per server per window
// (`stores/home-chat.ts` shares it); the daemon allows 3 per login.
//
// Kept like `voice-events-socket.ts`: reconnect with a widening pause; give
// up after a few failures in a row that never opened (the rows keep their
// last badges, greyed). Every close reaches the pool.

import type { ServerScope } from '@/kessel/server-scope'
import { withCliTokenQuery } from '@/web/session-token'
import { notePoolSocketClose } from '@/lib/pool-hooks'
import { openQueuedWebSocket } from '@/lib/grid-dial-queue'

export const HOME_CHAT_EVENTS_PATH = '/cli/home-chat/events'

/** Pauses before each reconnect, then the last one repeats. */
export const HOME_CHAT_EVENTS_RETRY_MS = [2_000, 5_000, 15_000, 30_000] as const

/** Failures in a row (never opened) before the socket gives up. */
export const HOME_CHAT_EVENTS_MAX_DEAD_TRIES = 4

/** One frame, parsed. Unknown frames are dropped. */
export type HomeChatFrame =
  | { type: 'hello'; chat: string | null }
  | { type: 'message'; conv: string; message: unknown }
  | { type: 'deleted'; conv: string; id: string }
  | { type: 'read'; conv: string }
  | { type: 'reaction'; conv: string; msg: string; emoji: string; on: boolean; count: number; member: string | null; mine: boolean }
  | { type: 'typing'; conv: string; member: string }
  | { type: 'member_removed'; conv: string }
  | { type: 'summary_changed'; conv: string | null }
  | { type: 'status'; chat: string | null }

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
    case 'hello':
    case 'status':
      return { type: v.type, chat: typeof v.chat === 'string' ? v.chat : null }
    case 'message':
      return conv ? { type: 'message', conv, message: v.message } : null
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

/** Listen to `scope`'s Home chat frames. `onLink(true)` when a socket opens,
 *  `onLink(false)` when it closes. Returns the unsubscribe. */
export function subscribeHomeChatEvents(
  scope: ServerScope,
  fn: (frame: HomeChatFrame) => void,
  onLink: (up: boolean) => void = () => {},
  deps: HomeChatEventsDeps = realDeps,
): () => void {
  let stopped = false
  let ws: WebSocket | null = null
  let timer: unknown = null
  let tries = 0
  let deadTries = 0

  const schedule = (): void => {
    if (stopped || deadTries >= HOME_CHAT_EVENTS_MAX_DEAD_TRIES) return
    const ms = HOME_CHAT_EVENTS_RETRY_MS[Math.min(tries, HOME_CHAT_EVENTS_RETRY_MS.length - 1)]
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
      onLink(true)
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
