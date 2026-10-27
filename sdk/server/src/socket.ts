// A subscribe row over its socket (runtime S23): one WebSocket, frames
// projected onto the row's schema, reconnects after a drop.
//
// K2's App sockets send no hello and no replay. After any gap (a drop, or
// the 4008 "lagged" close), onError gets a `resync` K2Error naming the verb
// to refetch with, and the socket reconnects (backoff 0.5 s → 10 s); catch
// up in `onOpen` (every open, 0.46.5), which comes after the gap. A
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
  /** 0.46.5 (review D5): the socket is open, first time and after every
   *  reconnect. Catch up HERE (read the snapshot after the socket is
   *  open), not on the close-time `resync`, which comes before the gap. */
  onOpen?: () => void
  WebSocket: WebSocketCtor
}

/** What to refetch with after a gap: the row's verb, plus `thread.activity`
 *  for the Thread turn on a socket opened with the ear (`ear=1`). */
export function refetchHint(row: AppVerbRow): string | null {
  const verbs = [row.socket?.refetch, row.socket?.query?.ear === '1' ? 'thread.activity' : undefined].filter(
    (v): v is string => typeof v === 'string' && v.length > 0,
  )
  return verbs.length ? verbs.join(' and ') : null
}

export function openSocket(o: SocketOpts): () => void {
  const refetch = refetchHint(o.row)
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
      if (stopped || !o.onOpen) return
      try {
        o.onOpen()
      } catch (err) {
        queueMicrotask(() => {
          throw err
        })
      }
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
    const refusedBeforeOpen = (): void => {
      stopped = true
      fail(
        new K2Error({
          code: 'failed',
          hint: `${o.verb}: the socket was refused before it opened (check the room, its cap and the pass)`,
          verb: o.verb,
        }),
      )
    }
    // A refused handshake (a non-101 answer): some runtimes (Node 22's
    // WebSocket) report it with `error` and never fire `close`, so a
    // refusal must not wait for `close`. `stopped` keeps it to one error.
    sock.addEventListener('error', () => {
      if (stopped || everOpened) return
      refusedBeforeOpen()
    })
    sock.addEventListener('close', (ev: CloseEvent) => {
      if (stopped) return
      if (!everOpened) {
        refusedBeforeOpen()
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
