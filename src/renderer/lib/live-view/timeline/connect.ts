// The real timeline's connection (T7): K2's docs socket (`WS /cli/docs/live`,
// docs-live-daemon.ts) under the real source. One attempt at a time; the
// first welcome attaches the source, a later one is a reconnect (the kit's
// watch asks for a fresh snapshot, presence sends your place again). A drop
// the daemon says can be retried (`cut_off`, `network`) reconnects with
// backoff while the tab says "Reconnecting"; Retry now dials at once. A
// refusal at open (`declined`) is the can't-be-live card, with the pack's
// words; any other end says the connection failed.

import type { ErrorReport, Transport } from '@k2/mpc-client'
import { LiveConnectError, type LiveClose, type LiveTransport, type LiveWelcome } from '@/lib/docs-live-transport'
import type { KeyedSourceOptions } from './keyed-source'
import { KeyedWatchTimelineSource } from './keyed-source'

/** Waits between attempts, ms: 1 s, doubling, at most 30 s. */
export function backoffMs(attempt: number): number {
  return Math.min(30_000, 1000 * 2 ** Math.max(0, attempt))
}

export interface TimelineConnection {
  source: KeyedWatchTimelineSource
  /** Close the socket and stop retrying (the tab closed or switched to File). */
  stop: () => void
}

export interface ConnectOptions extends KeyedSourceOptions {
  /** Tests: the timer (default `setTimeout`). */
  schedule?: (fn: () => void, ms: number) => () => void
}

/** The socket's close (or a refused connect) as the source's states. */
function closeOf(e: unknown): LiveClose {
  if (e instanceof LiveConnectError) return e.close
  const c = e as Partial<LiveClose> | null
  if (c && typeof c.reason === 'string') return { reason: c.reason, retry: !!c.retry, ...(c.error ? { error: c.error } : {}), ...(c.code ? { code: c.code } : {}), ...(c.message ? { message: c.message } : {}) }
  return { reason: 'network', retry: true }
}

/** Opens the real source on `t` and keeps it connected. */
export function connectTimeline(t: LiveTransport, o: ConnectOptions): TimelineConnection {
  const schedule =
    o.schedule ??
    ((fn: () => void, ms: number) => {
      const id = setTimeout(fn, ms)
      return () => clearTimeout(id)
    })
  const source = new KeyedWatchTimelineSource(t as unknown as Transport, o)
  let attached = false
  let stopped = false
  let attempt = 0
  let prev: number | undefined
  let ctl: AbortController | null = null
  let cancelTimer: (() => void) | null = null

  const ended = (c: LiveClose): void => {
    if (stopped) return
    if (c.reason === 'declined') {
      // The kit refused to open the doc live: the pack's code is in the error's details.
      const error: ErrorReport = c.error ?? { id: c.code ?? 'declined', message: c.message ?? '' }
      source.refusedAtOpen(error)
      return
    }
    if (c.retry) {
      source.setConnection('reconnecting')
      cancelTimer?.()
      cancelTimer = schedule(() => void dial(), backoffMs(attempt))
      attempt += 1
      return
    }
    source.setConnection('failed')
  }

  const dial = async (): Promise<void> => {
    if (stopped) return
    cancelTimer?.()
    cancelTimer = null
    ctl?.abort()
    const me = new AbortController()
    ctl = me
    let w: LiveWelcome
    try {
      w = await t.connect({ ...(prev !== undefined ? { prev } : {}), signal: me.signal })
    } catch (e) {
      if (me.signal.aborted || stopped) return
      return ended(closeOf(e))
    }
    if (stopped || ctl !== me) return
    attempt = 0
    prev = w.client
    if (!attached) {
      attached = true
      source.attach(w)
    } else source.reconnected(w)
  }

  const offClose = t.onClose((c) => ended(c))
  source.onRetryAsked(() => {
    attempt = 0
    void dial()
  })
  void dial()

  return {
    source,
    stop: () => {
      if (stopped) return
      stopped = true
      cancelTimer?.()
      ctl?.abort()
      offClose()
      source.close()
      t.close()
    },
  }
}
