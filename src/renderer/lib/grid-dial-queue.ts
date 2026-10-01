/**
 * Cap concurrent grid WS *handshakes* (CONNECTING only).
 * Burst-fail backs off every pane so heal timers cannot thrash.
 *
 * Remote WSS through the tunnel used to die if we fan out (WKWebView
 * "Insufficient resources", 0.40.97). Max 1 made two Projects panes
 * paint in series (~800ms + ~800ms). Two at a time is enough for a
 * dashboard and still far under the old 4+ storm. Local loopback is
 * cheap — allow more so a split layout looks simultaneous.
 *
 * Home M1: the queue state is per server (keyed by `ServerScope.id`):
 * each scope has its own in-flight count, failure burst and backoff, and
 * its own max (2 remote / 4 local, from that scope's remoteness). One
 * global cap still bounds every handshake in this webview; it is the same
 * number as before (2 when the window is on a remote, 4 on local), so a
 * window that only dials its primary server behaves exactly as before.
 */

import type { ServerScope } from '@/kessel/server-scope'
import { useConnectHostStore } from '@/stores/connect-host'

export const MAX_CONCURRENT_DIALS = 2
export const MAX_CONCURRENT_DIALS_LOCAL = 4
export const FAIL_BURST_WINDOW_MS = 3_000
export const FAIL_BURST_COUNT = 3
export const BACKOFF_MS = 8_000
/** Bound limbo CONNECTING so two hung dials cannot freeze the app. */
export const HANDSHAKE_TIMEOUT_MS = 4_000

interface ScopeDialState {
  inflight: number
  backoffUntil: number
  recentFails: number[]
}

interface Waiter {
  scope: ServerScope
  wake: () => void
}

/** Every handshake in flight, across all scopes (the global cap). */
let globalInflight = 0
const waiters: Waiter[] = []
const scopeStates = new Map<string, ScopeDialState>()
let maxOverrideForTests: number | null = null

function stateFor(scope: ServerScope): ScopeDialState {
  let st = scopeStates.get(scope.id)
  if (!st) {
    st = { inflight: 0, backoffUntil: 0, recentFails: [] }
    scopeStates.set(scope.id, st)
  }
  return st
}

export function resetGridDialQueueForTests(): void {
  globalInflight = 0
  waiters.length = 0
  scopeStates.clear()
  maxOverrideForTests = null
}

export function setGridDialMaxForTests(n: number | null): void {
  maxOverrideForTests = n
}

/** Test seam: handshakes in flight for one scope / for the whole webview. */
export function gridDialInflightForTests(scope: ServerScope | null): number {
  if (scope === null) return globalInflight
  return stateFor(scope).inflight
}

export function gridDialBackoffRemainingMs(scope: ServerScope, now = Date.now()): number {
  return Math.max(0, stateFor(scope).backoffUntil - now)
}

export function noteGridDialFailure(scope: ServerScope, now = Date.now()): void {
  const st = stateFor(scope)
  st.recentFails.push(now)
  while (st.recentFails.length > 0 && now - st.recentFails[0]! > FAIL_BURST_WINDOW_MS) {
    st.recentFails.shift()
  }
  if (st.recentFails.length >= FAIL_BURST_COUNT) {
    st.backoffUntil = now + BACKOFF_MS
    st.recentFails.length = 0
  }
}

/** The global cap — unchanged from before M1: picked from the window's
 *  active host. */
function globalMaxDials(): number {
  if (maxOverrideForTests != null) return maxOverrideForTests
  return useConnectHostStore.getState().activeHost === 'local'
    ? MAX_CONCURRENT_DIALS_LOCAL
    : MAX_CONCURRENT_DIALS
}

/** One scope's own cap, from that server's remoteness. For the primary
 *  scope this equals the global cap. */
function scopeMaxDials(scope: ServerScope): number {
  if (maxOverrideForTests != null) return maxOverrideForTests
  return scope.isRemote ? MAX_CONCURRENT_DIALS : MAX_CONCURRENT_DIALS_LOCAL
}

function hasRoom(scope: ServerScope): boolean {
  return globalInflight < globalMaxDials() && stateFor(scope).inflight < scopeMaxDials(scope)
}

function isAborted(signal?: AbortSignal): boolean {
  return signal?.aborted === true
}

function sleepMs(ms: number, signal?: AbortSignal): Promise<void> {
  return new Promise((resolve) => {
    if (isAborted(signal)) {
      resolve()
      return
    }
    const timer = setTimeout(() => {
      signal?.removeEventListener('abort', onAbort)
      resolve()
    }, ms)
    const onAbort = () => {
      clearTimeout(timer)
      resolve()
    }
    signal?.addEventListener('abort', onAbort)
  })
}

async function acquireDialSlot(scope: ServerScope, signal?: AbortSignal): Promise<void> {
  for (;;) {
    if (isAborted(signal)) throw new Error('grid-dial-aborted')
    const wait = gridDialBackoffRemainingMs(scope)
    if (wait > 0) {
      await sleepMs(wait, signal)
      continue
    }
    if (isAborted(signal)) throw new Error('grid-dial-aborted')
    if (hasRoom(scope)) {
      globalInflight += 1
      stateFor(scope).inflight += 1
      return
    }
    await new Promise<void>((resolve) => {
      const waiter: Waiter = {
        scope,
        wake: () => {
          signal?.removeEventListener('abort', onAbort)
          resolve()
        },
      }
      const onAbort = () => {
        const i = waiters.indexOf(waiter)
        if (i >= 0) waiters.splice(i, 1)
        resolve()
      }
      waiters.push(waiter)
      if (isAborted(signal)) {
        onAbort()
        return
      }
      signal?.addEventListener('abort', onAbort)
    })
  }
}

function releaseDialSlot(scope: ServerScope): void {
  globalInflight = Math.max(0, globalInflight - 1)
  const st = stateFor(scope)
  st.inflight = Math.max(0, st.inflight - 1)
  // Wake the first waiter that can now proceed (its own scope has room
  // under the global cap). With one scope this is exactly the old FIFO
  // `shift()`. The waiter re-enters acquireDialSlot; increment-on-wake +
  // re-check deadlocks the next pane (inflight already at MAX when it
  // loops).
  const i = waiters.findIndex((w) => hasRoom(w.scope))
  if (i < 0) return
  const [next] = waiters.splice(i, 1)
  next!.wake()
}

export type GridDialOpts = {
  /** Rechecked after the slot is granted, before `new WebSocket`. */
  isCancelled?: () => boolean
  /** Unblocks queue/backoff wait without counting as a failed dial. */
  signal?: AbortSignal
  /**
   * Runs after the slot is granted and before construct. Close the
   * prior grid socket here so N panes do not all CLOSING at once.
   */
  beforeDial?: () => void
}

/**
 * Dial a grid WS for `scope`'s server through the queue. Caller owns the
 * socket after OPEN (or after reject). Failed handshake counts toward that
 * scope's resource backoff.
 */
export async function openQueuedGridWebSocket(
  scope: ServerScope,
  url: string,
  opts?: GridDialOpts,
): Promise<WebSocket> {
  await acquireDialSlot(scope, opts?.signal)
  try {
    if (opts?.isCancelled?.() || isAborted(opts?.signal)) {
      throw new Error('grid-dial-aborted')
    }
    try {
      opts?.beforeDial?.()
    } catch {
      /* teardown must not keep the slot */
    }
    if (opts?.isCancelled?.() || isAborted(opts?.signal)) {
      throw new Error('grid-dial-aborted')
    }
    let ws: WebSocket
    try {
      ws = new WebSocket(url)
    } catch {
      // WKWebView "Insufficient resources" often throws at construct.
      noteGridDialFailure(scope)
      throw new Error('grid-dial-failed')
    }
    ws.binaryType = 'arraybuffer'
    const opened = await new Promise<boolean>((resolve) => {
      const timer = setTimeout(() => {
        cleanup()
        resolve(false)
      }, HANDSHAKE_TIMEOUT_MS)
      const onAbort = () => {
        cleanup()
        resolve(false)
      }
      const cleanup = () => {
        clearTimeout(timer)
        opts?.signal?.removeEventListener('abort', onAbort)
        ws.onopen = null
        ws.onerror = null
        ws.onclose = null
      }
      ws.onopen = () => {
        cleanup()
        resolve(true)
      }
      ws.onerror = () => {
        cleanup()
        resolve(false)
      }
      ws.onclose = () => {
        cleanup()
        resolve(false)
      }
      if (isAborted(opts?.signal)) {
        onAbort()
        return
      }
      opts?.signal?.addEventListener('abort', onAbort)
    })
    if (opts?.isCancelled?.() || isAborted(opts?.signal)) {
      if (ws.readyState !== WebSocket.CLOSED) {
        try {
          ws.close()
        } catch {
          /* ignore */
        }
      }
      throw new Error('grid-dial-aborted')
    }
    if (!opened) {
      noteGridDialFailure(scope)
      if (ws.readyState !== WebSocket.CLOSED) {
        try {
          ws.close()
        } catch {
          /* ignore */
        }
      }
      throw new Error('grid-dial-failed')
    }
    return ws
  } finally {
    releaseDialSlot(scope)
  }
}
