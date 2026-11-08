// `WS /cli/voice/events` (prd-voice-chat-only-members-v1 §6.3, D6): a
// server's `voice_rooms_changed {rev}` on a socket of its own. A Voice &
// Chat only login never gets the app socket (`/cli/sessions/events` is a
// Member route), so its headset section hears room changes here. The
// daemon sends only `{"kind":"voice_rooms_changed","rev":n}` and pings.
//
// Kept minimal: one socket per subscription, reconnect with a widening
// pause, and give up after a few failures in a row that never opened (the
// voice list's 60 s sweep still covers that server). Every close reaches the
// pool, so a kick (4001) or a revoked session reads as one.

import type { ServerScope } from '@/kessel/server-scope'
import { withCliTokenQuery } from '@/web/session-token'
import { notePoolSocketClose } from '@/lib/pool-hooks'
import { openQueuedWebSocket } from '@/lib/grid-dial-queue'

export const VOICE_EVENTS_PATH = '/cli/voice/events'

/** Pauses before each reconnect, then the last one repeats. */
export const VOICE_EVENTS_RETRY_MS = [2_000, 5_000, 15_000, 30_000] as const

/** Failures in a row (no frame ever opened) before the socket gives up. */
export const VOICE_EVENTS_MAX_DEAD_TRIES = 4

/** `{kind: 'voice_rooms_changed', rev}` → the rev (null when absent); any
 *  other frame → undefined. */
export function voiceRoomsRev(data: unknown): number | null | undefined {
  if (typeof data !== 'string') return undefined
  let v: unknown
  try {
    v = JSON.parse(data)
  } catch {
    return undefined
  }
  if (!v || typeof v !== 'object') return undefined
  const { kind, rev } = v as { kind?: unknown; rev?: unknown }
  if (kind !== 'voice_rooms_changed') return undefined
  return typeof rev === 'number' && Number.isFinite(rev) ? rev : null
}

export interface VoiceEventsDeps {
  /** MS70: real sockets dial through the server's queue. */
  open: (url: string, scope: ServerScope) => WebSocket | Promise<WebSocket>
  setTimer: (fn: () => void, ms: number) => unknown
  clearTimer: (t: unknown) => void
}

const realDeps: VoiceEventsDeps = {
  open: (url, scope) => openQueuedWebSocket(scope, url),
  setTimer: (fn, ms) => setTimeout(fn, ms),
  clearTimer: (t) => clearTimeout(t as ReturnType<typeof setTimeout>),
}

/** Listen to `scope`'s voice events. Returns the unsubscribe. */
export function subscribeVoiceEvents(
  scope: ServerScope,
  fn: (rev: number | null) => void,
  deps: VoiceEventsDeps = realDeps,
): () => void {
  let stopped = false
  let ws: WebSocket | null = null
  let timer: unknown = null
  let tries = 0
  let deadTries = 0

  const schedule = (): void => {
    if (stopped) return
    if (deadTries >= VOICE_EVENTS_MAX_DEAD_TRIES) return
    const ms = VOICE_EVENTS_RETRY_MS[Math.min(tries, VOICE_EVENTS_RETRY_MS.length - 1)]
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
      url = withCliTokenQuery(`${base}${VOICE_EVENTS_PATH}`, creds.token)
    } catch (err) {
      console.warn(`[voice-events] ${scope.hostKey}: no address yet:`, err)
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
      console.warn(`[voice-events] ${scope.hostKey}: dial failed:`, err)
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
    }
    sock.onmessage = (ev: MessageEvent) => {
      const rev = voiceRoomsRev(ev.data)
      if (rev !== undefined) fn(rev)
    }
    sock.onclose = (ev: CloseEvent) => {
      if (ws === sock) ws = null
      notePoolSocketClose(scope.hostKey, ev.code)
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
