// Keep a shown remote room's connection warm (research-remote-thread-load-v1
// Fix 1, item I).
//
// The daemon closes an idle keep-alive connection after 60 s
// (`dispatcher.rs`). Through the tunnel a new connection costs 0.5–1.25 s
// against 0.15–0.4 s warm, so the first Thread send after a minute of
// reading paid a fresh TLS handshake. While a remote room is on screen and
// the window is visible, a cheap unauthenticated `GET /boot-status` goes to
// its server when nothing else went there for {@link WARM_PING_INTERVAL_MS}.
// Hidden room, hidden window, or a local room: nothing is sent.

import { useEffect } from 'react'
import type { ServerScope } from '@/kessel/server-scope'
import { lastCliActivityAt, noteHostActivity } from '@/lib/cli-timing'

/** Under the daemon's 60 s idle close, with room for a slow answer. */
export const WARM_PING_INTERVAL_MS = 40_000

export interface WarmPingDeps {
  /** Send the ping (never throws to the caller; failures are ignored). */
  ping(): Promise<void>
  /** When anything last went to that server (wall clock ms), or null. */
  lastActivity(): number | null
  now(): number
  setInterval(fn: () => void, ms: number): unknown
  clearInterval(handle: unknown): void
}

export interface WarmPing {
  /** On while the room is shown and the window visible; off otherwise. */
  setActive(on: boolean): void
  dispose(): void
}

export function createWarmPing(deps: WarmPingDeps): WarmPing {
  let timer: unknown = null
  let inFlight = false
  let disposed = false

  const tick = (): void => {
    if (inFlight) return
    const last = deps.lastActivity()
    // Something else used the connection recently: it is warm.
    if (last !== null && deps.now() - last < WARM_PING_INTERVAL_MS) return
    inFlight = true
    void deps
      .ping()
      .catch(() => {
        /* the connection gate owns health; a missed ping changes nothing */
      })
      .finally(() => {
        inFlight = false
      })
  }

  const stop = (): void => {
    if (timer !== null) deps.clearInterval(timer)
    timer = null
  }

  return {
    setActive(on) {
      if (disposed) return
      if (!on) {
        stop()
        return
      }
      if (timer !== null) return
      timer = deps.setInterval(tick, WARM_PING_INTERVAL_MS)
    },
    dispose() {
      disposed = true
      stop()
    },
  }
}

function documentVisible(): boolean {
  return typeof document === 'undefined' || document.visibilityState !== 'hidden'
}

/** Keep `scope`'s connection warm while `shown` and the window is visible
 *  (a remote scope only). */
export function useRemoteWarmPing(scope: ServerScope, shown: boolean): void {
  useEffect(() => {
    if (!scope.isRemote || !shown) return
    const host = scope.hostKey
    const warm = createWarmPing({
      ping: async () => {
        const base = await scope.httpBase()
        const res = await fetch(`${base}/boot-status`, { method: 'GET', cache: 'no-store' })
        // Drain the body so the connection goes back to the pool.
        await res.text()
        noteHostActivity(host)
      },
      lastActivity: () => lastCliActivityAt(host),
      now: Date.now,
      setInterval: (fn, ms) => setInterval(fn, ms),
      clearInterval: (h) => clearInterval(h as ReturnType<typeof setInterval>),
    })
    const sync = (): void => warm.setActive(documentVisible())
    sync()
    if (typeof document !== 'undefined') document.addEventListener('visibilitychange', sync)
    return () => {
      if (typeof document !== 'undefined') document.removeEventListener('visibilitychange', sync)
      warm.dispose()
    }
  }, [scope, shown])
}
