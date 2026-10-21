// app-room-stream (K2 0.46.3) — the Garden side of `thread.inbox`.
//
// One daemon socket, `WS /cli/thread/stream?rooms=all`, on the window's
// server, with this window's own login: every new Thread message (and card
// change) on that server's agents, as it lands, instead of one overlay
// socket per conversation. The daemon decides what this login may read
// (a login reads every Thread, as `GET /cli/thread` does); Projects are not
// on it (Rosson 2026-10-20: no Projects on the bridge for now).
//
// Widgets never see a room id, a conversation id or a token: each frame is
// turned into `{address, item}` here (the row address the Agents widget
// shows, `handle::host`), and the custom-widget bridge projects the item.
// One socket serves every subscribed widget; it opens with the first and
// closes with the last, and reconnects with jittered backoff.
//
// Contract: `.k2/prds/contract-app-room-stream-0463.md`.

import { getDaemonWs, daemonWsBase } from '@/kessel/daemon-ws'
import { jittered } from '@/lib/backoff'
import { openQueuedWebSocket } from '@/lib/grid-dial-queue'
import { primaryRoom } from '@/stores/room'
import { useProjectsStore } from '@/stores/projects'
import { useConnectHostStore } from '@/stores/connect-host'
import { activeHomeHostKey, homeAddress, workspaceHandle } from '@/lib/home-address'
import type { OverlayThreadItem } from '@/components/SessionView/overlayThread'

export const THREAD_STREAM_PATH = '/cli/thread/stream'

/** One message for a `thread.inbox` listener. */
export interface ZenInboxMessage {
  /** The agent's row address (`handle::host`). */
  address: string
  item: OverlayThreadItem
}

/** A stream frame → a message, or null (another kind, no doc, a room this
 *  window has no row for). Pure, for tests. */
export function zenInboxMessage(frame: unknown, addressOf: (room: string) => string | null): ZenInboxMessage | null {
  if (!frame || typeof frame !== 'object' || Array.isArray(frame)) return null
  const f = frame as Record<string, unknown>
  if (f.kind !== 'thread_message') return null
  if (typeof f.room !== 'string' || typeof f.id !== 'string' || typeof f.seq !== 'number') return null
  // Over the daemon's 32 KiB frame cap the doc (or its body) is left off;
  // a widget re-reads that item with `thread.read`. Not pushed here.
  if (f.truncated === true || !f.doc || typeof f.doc !== 'object') return null
  const address = addressOf(f.room)
  if (!address) return null
  return {
    address,
    item: { collection: 'thread', seq: f.seq, id: f.id, doc: f.doc as OverlayThreadItem['doc'] },
  }
}

/** The window's server's rows, by workspace id. */
function addressOfRoom(room: string): string | null {
  const p = useProjectsStore.getState().projects.find((x) => x.id === room)
  if (!p) return null
  const handle = workspaceHandle(p)
  if (!handle) return null
  return homeAddress(handle, activeHomeHostKey(useConnectHostStore.getState().activeHost))
}

const listeners = new Set<(m: ZenInboxMessage) => void>()
let stop: (() => void) | null = null

const BACKOFF_MS = [500, 1000, 2000, 5000, 10000]

function start(): () => void {
  let cancelled = false
  let ws: WebSocket | null = null
  let timer: ReturnType<typeof setTimeout> | null = null
  let attempt = 0
  const scope = primaryRoom().scope

  const schedule = (): void => {
    if (cancelled) return
    const base = BACKOFF_MS[Math.min(attempt, BACKOFF_MS.length - 1)] ?? 10000
    attempt += 1
    timer = setTimeout(() => {
      timer = null
      void connect()
    }, jittered(base))
  }

  const connect = async (): Promise<void> => {
    if (cancelled) return
    try {
      const creds = await getDaemonWs(scope)
      if (cancelled) return
      // MS70: every daemon socket dials through the per-server queue.
      const sock = await openQueuedWebSocket(
        scope,
        `${daemonWsBase(creds)}${THREAD_STREAM_PATH}?rooms=all&token=${encodeURIComponent(creds.token)}`,
      )
      if (cancelled) {
        sock.close(1000, 'unsubscribed')
        return
      }
      ws = sock
      sock.onopen = () => {
        attempt = 0
      }
      sock.onmessage = (ev) => {
        if (typeof ev.data !== 'string') return
        let frame: unknown
        try {
          frame = JSON.parse(ev.data)
        } catch {
          return
        }
        const m = zenInboxMessage(frame, addressOfRoom)
        if (!m) return
        for (const l of [...listeners]) {
          try {
            l(m)
          } catch (err) {
            console.warn('[zen] thread.inbox listener failed:', err)
          }
        }
      }
      sock.onclose = () => {
        if (ws === sock) ws = null
        schedule()
      }
    } catch (err) {
      console.warn('[zen] thread.inbox socket:', err)
      schedule()
    }
  }

  void connect()
  return () => {
    cancelled = true
    if (timer) clearTimeout(timer)
    if (ws && ws.readyState <= 1) ws.close(1000, 'unsubscribed')
    ws = null
  }
}

/** `thread.inbox(cb)`: every new message on this server's agents. */
export function threadInboxSubscribe(cb: (m: ZenInboxMessage) => void): () => void {
  listeners.add(cb)
  if (!stop) stop = start()
  return () => {
    listeners.delete(cb)
    if (listeners.size === 0 && stop) {
      stop()
      stop = null
    }
  }
}

/** Tests only. */
export function __resetZenThreadInboxForTests(): void {
  listeners.clear()
  stop?.()
  stop = null
}
