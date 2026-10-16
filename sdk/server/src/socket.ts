// A subscribe row over its socket (runtime S23): one WebSocket, frames
// projected onto the row's schema, reconnects after a drop.
//
// K2's App sockets send no hello and no replay. After any gap (a drop, or
// the 4008 "lagged" close), onError gets a `resync` K2Error naming the verb
// to refetch with, and the socket reconnects (backoff 0.5 s → 10 s). A
// socket refused before it ever opened (no room, no cap, a dead pass)
// ends with one `failed` error and no retry.

import { K2Error } from './errors.ts'
import type { AppVerbRow } from './generated/verbs.ts'
import { project } from './wire.ts'

export type WebSocketCtor = new (url: string) => WebSocket

const BACKOFF_MS = [500, 1000, 2000, 5000, 10000]

export interface SocketOpts {
  verb: string
  row: AppVerbRow
  /** The URL to dial now (the pass is read again on every reconnect). */
  url: () => string
  onFrame: (frame: unknown) => void
  onError: (error: K2Error) => void
  WebSocket: WebSocketCtor
}

export function openSocket(o: SocketOpts): () => void {
  const refetch = o.row.socket?.refetch ?? null
  let stopped = false
  let everOpened = false
  let attempt = 0
  let ws: WebSocket | null = null
  let timer: ReturnType<typeof setTimeout> | null = null

  const fail = (e: K2Error): void => {
    try {
      o.onError(e)
    } catch (err) {
      // A throwing onError must not kill the socket loop; surface it.
      queueMicrotask(() => {
        throw err
      })
    }
  }

  const connect = (): void => {
    let url: string
    try {
      url = o.url()
    } catch (e) {
      stopped = true
      fail(e instanceof K2Error ? e : new K2Error({ code: 'failed', hint: String(e), verb: o.verb }))
      return
    }
    const sock = new o.WebSocket(url)
    ws = sock
    sock.addEventListener('open', () => {
      everOpened = true
      attempt = 0
    })
    sock.addEventListener('message', (ev: MessageEvent) => {
      if (typeof ev.data !== 'string') return
      let v: unknown
      try {
        v = JSON.parse(ev.data)
      } catch {
        return // not a frame
      }
      o.onFrame(project(o.row.response, v, o.row.responseNames))
    })
    sock.addEventListener('close', (ev: CloseEvent) => {
      if (stopped) return
      if (!everOpened) {
        stopped = true
        fail(
          new K2Error({
            code: 'failed',
            hint: `${o.verb}: the socket was refused before it opened (check the room, its cap and the pass)`,
            verb: o.verb,
          }),
        )
        return
      }
      const why = ev.code === 4008 ? 'K2 dropped it for lagging (4008)' : `it closed (${ev.code})`
      fail(
        new K2Error({
          code: 'resync',
          hint: `${o.verb}: ${why}; reconnecting${refetch ? `, refetch with ${refetch}` : ''}`,
          verb: o.verb,
        }),
      )
      const wait = BACKOFF_MS[Math.min(attempt, BACKOFF_MS.length - 1)] ?? 10000
      attempt += 1
      timer = setTimeout(() => {
        timer = null
        if (!stopped) connect()
      }, wait)
    })
  }

  connect()
  return () => {
    stopped = true
    if (timer) clearTimeout(timer)
    if (ws && ws.readyState <= 1) ws.close(1000, 'unsubscribed')
  }
}
