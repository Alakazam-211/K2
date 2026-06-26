// Agent Ops live stream — long-lived WS to `GET /cli/ops/stream`.
//
// Phase D consumer of the Phase C fan-in multiplex
// (`crates/k2-daemon/src/ops_stream_ws.rs`): ONE socket carrying both the
// session-events plane and the awareness bus, source-tagged. The first frame
// is `{kind:'hello',...}`; every later frame is `{source,event}`.
//
// This mirrors `stores/session-events.ts`'s reconnect machinery verbatim
// (host-aware creds via getDaemonWs, token in the query so it's ciphertext
// over K2 Connect's TLS tunnel, exponential backoff, idempotent reconnect
// from BOTH onerror AND onclose per Issue #5). The server re-auths the token
// every 5s; we keep the socket open and the creds are stable for the session.

import {
  getDaemonWs,
  invalidateDaemonWs,
  daemonWsBase,
  type DaemonWsAvailable,
} from '@/kessel/daemon-ws'
import type { OpsHelloFrame, OpsStreamEnvelope } from './ops-api'

export interface OpsStreamHandlers {
  /** Fires on each successful (re)connect. Use it to re-pull the one-shot
   *  overview so any deltas missed during the drop window are backfilled. */
  onHello?: (frame: OpsHelloFrame) => void
  /** A source-tagged envelope frame (session event or awareness signal). */
  onEnvelope?: (env: OpsStreamEnvelope) => void
  /** Connection state transitions — drives the "live/connected" indicator. */
  onConnectionChange?: (connected: boolean) => void
}

export type UnsubscribeFn = () => void

const INITIAL_BACKOFF_MS = 500
const MAX_BACKOFF_MS = 5_000

/** Open the ops stream. Returns an unsubscribe fn that tears down the socket
 *  and stops the reconnect loop (idempotent). */
export function subscribeToOpsStream(handlers: OpsStreamHandlers): UnsubscribeFn {
  let socket: WebSocket | null = null
  let reconnectTimer: ReturnType<typeof setTimeout> | null = null
  let backoffMs = INITIAL_BACKOFF_MS
  let stopped = false

  const clearReconnect = (): void => {
    if (reconnectTimer !== null) {
      clearTimeout(reconnectTimer)
      reconnectTimer = null
    }
  }

  const scheduleReconnect = (): void => {
    if (stopped) return
    clearReconnect()
    const delay = backoffMs
    backoffMs = Math.min(backoffMs * 2, MAX_BACKOFF_MS)
    reconnectTimer = setTimeout(() => {
      reconnectTimer = null
      void openSocket()
    }, delay)
  }

  // Idempotent: fired from BOTH onerror AND onclose (Issue #5 — WebKit can
  // raise onerror without a follow-up onclose under throttling).
  const triggerReconnect = (): void => {
    if (stopped) return
    if (reconnectTimer !== null) return
    scheduleReconnect()
  }

  const openSocket = async (): Promise<void> => {
    if (stopped) return
    let creds: DaemonWsAvailable
    try {
      creds = await getDaemonWs()
    } catch (err) {
      invalidateDaemonWs()
      console.warn('[ops-stream] daemon credentials unavailable, retrying:', err)
      scheduleReconnect()
      return
    }
    if (stopped) return

    const url = `${daemonWsBase(creds)}/cli/ops/stream?token=${encodeURIComponent(creds.token)}`
    let ws: WebSocket
    try {
      ws = new WebSocket(url)
    } catch (err) {
      console.warn('[ops-stream] WS construction failed:', err)
      scheduleReconnect()
      return
    }
    socket = ws

    ws.onopen = () => {
      backoffMs = INITIAL_BACKOFF_MS
      handlers.onConnectionChange?.(true)
    }

    ws.onmessage = (ev) => {
      const raw = typeof ev.data === 'string' ? ev.data : null
      if (raw === null) return
      let msg: unknown
      try {
        msg = JSON.parse(raw)
      } catch (err) {
        console.warn('[ops-stream] failed to parse frame:', err, raw)
        return
      }
      if (!msg || typeof msg !== 'object') return
      const obj = msg as Record<string, unknown>
      if (obj.kind === 'hello') {
        handlers.onHello?.(obj as unknown as OpsHelloFrame)
        return
      }
      if (obj.source === 'session' || obj.source === 'awareness') {
        handlers.onEnvelope?.(obj as unknown as OpsStreamEnvelope)
      }
    }

    ws.onerror = () => {
      handlers.onConnectionChange?.(false)
      triggerReconnect()
    }

    ws.onclose = (ev) => {
      if (socket === ws) socket = null
      handlers.onConnectionChange?.(false)
      if (stopped) return
      console.debug(
        `[ops-stream] WS closed (code=${ev.code}, reason="${ev.reason ?? ''}") — scheduling reconnect`,
      )
      triggerReconnect()
    }
  }

  void openSocket()

  return () => {
    stopped = true
    clearReconnect()
    handlers.onConnectionChange?.(false)
    if (socket) {
      try {
        socket.close(1000, 'unsubscribe')
      } catch {
        // ignore
      }
      socket = null
    }
  }
}
