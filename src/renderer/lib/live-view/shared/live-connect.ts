// Keeping a live view's real source connected to K2's docs socket
// (`WS /cli/docs/live`, docs-live-daemon.ts), for any view whose source
// takes the socket's welcomes: one attempt at a time; the first welcome
// attaches the source, a later one is a reconnect (the kit's watch asks for
// a fresh snapshot, presence sends your place again). A drop the daemon
// says can be retried (`cut_off`, `network`) reconnects with backoff while
// the tab says "Reconnecting"; Retry now dials at once. A refusal at open
// (`declined`) is the source's to word; any other end says the connection
// failed. The timeline's and the scene's real sources both use it.

import type { ErrorReport } from '@k2/mpc-client'
import { LiveConnectError, type LiveClose, type LiveTransport, type LiveWelcome } from '@/lib/docs-live-transport'

/** Waits between attempts, ms: 1 s, doubling, at most 30 s. */
export function backoffMs(attempt: number): number {
  return Math.min(30_000, 1000 * 2 ** Math.max(0, attempt))
}

/** What the driver needs of a real source. */
export interface ConnectableSource<W> {
  attach(welcome: W): void
  reconnected(welcome: W): void
  setConnection(c: 'reconnecting' | 'failed' | null): void
  refusedAtOpen(e: ErrorReport): void
  onRetryAsked(cb: () => void): void
  close(): void
}

export interface KeepConnectedOptions {
  /** Tests: the timer (default `setTimeout`). */
  schedule?: (fn: () => void, ms: number) => () => void
}

/** The socket's close (or a refused connect) as the source's states. */
export function closeOf(e: unknown): LiveClose {
  if (e instanceof LiveConnectError) return e.close
  const c = e as Partial<LiveClose> | null
  if (c && typeof c.reason === 'string') return { reason: c.reason, retry: !!c.retry, ...(c.error ? { error: c.error } : {}), ...(c.code ? { code: c.code } : {}), ...(c.message ? { message: c.message } : {}) }
  return { reason: 'network', retry: true }
}

/** Dials `t` for `source` and keeps it connected; `stop` closes the socket and stops retrying. */
export function keepConnected<W>(t: LiveTransport, source: ConnectableSource<W>, o: KeepConnectedOptions = {}): { stop: () => void } {
  const schedule =
    o.schedule ??
    ((fn: () => void, ms: number) => {
      const id = setTimeout(fn, ms)
      return () => clearTimeout(id)
    })
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
      source.refusedAtOpen((c.error as ErrorReport | undefined) ?? { id: c.code ?? 'declined', message: c.message ?? '' } as ErrorReport)
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
      source.attach(w as unknown as W)
    } else source.reconnected(w as unknown as W)
  }

  const offClose = t.onClose((c) => ended(c))
  source.onRetryAsked(() => {
    attempt = 0
    void dial()
  })
  void dial()

  return {
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
