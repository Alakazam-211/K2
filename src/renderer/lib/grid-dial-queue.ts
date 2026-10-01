/**
 * Cap concurrent daemon WS *handshakes* (CONNECTING only).
 * Burst-fail backs off every pane so heal timers cannot thrash.
 *
 * Remote WSS through the tunnel used to die if we fan out (WKWebView
 * "Insufficient resources", 0.40.97). Max 1 made two Projects panes
 * paint in series (~800ms + ~800ms). Two at a time is enough for a
 * dashboard and still far under the old 4+ storm. Local loopback is
 * cheap — allow more so a split layout looks simultaneous.
 *
 * Home M2 (MS25, MS70): the queue is per SERVER, keyed by host key (the
 * primary scope and a pinned scope for the same server share one queue).
 * Each server has its own in-flight count, failure burst and backoff, and
 * its own max (2 remote / 4 local). One window-wide cap of 4 bounds every
 * REMOTE handshake across all servers, so a dead or busy B never takes
 * more than its share and never stalls A or C. A window that only dials
 * its own server behaves exactly as before (2 on a remote, 4 on local).
 *
 * Every daemon socket kind dials through here, not only grids:
 * `openQueuedGridWebSocket` waits for OPEN and hands back an open socket;
 * `openQueuedWebSocket` (events, overlay, transcript, chatter) holds the
 * slot only while the socket is CONNECTING and hands the socket back at
 * once, so callers keep their own onopen/onclose handling.
 */

import type { ServerScope } from '@/kessel/server-scope'

export const MAX_CONCURRENT_DIALS = 2
export const MAX_CONCURRENT_DIALS_LOCAL = 4
/** MS25: remote handshakes in flight across every server in this window. */
export const MAX_CONCURRENT_REMOTE_DIALS_WINDOW = 4
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

/** Remote handshakes in flight across all servers (the window cap). */
let remoteInflight = 0
const waiters: Waiter[] = []
const scopeStates = new Map<string, ScopeDialState>()
let maxOverrideForTests: number | null = null

/** The queue key: the server, not the scope object. */
function queueKey(scope: ServerScope): string {
  return scope.hostKey
}

function stateForKey(key: string): ScopeDialState {
  let st = scopeStates.get(key)
  if (!st) {
    st = { inflight: 0, backoffUntil: 0, recentFails: [] }
    scopeStates.set(key, st)
  }
  return st
}

function stateFor(scope: ServerScope): ScopeDialState {
  return stateForKey(queueKey(scope))
}

export function resetGridDialQueueForTests(): void {
  remoteInflight = 0
  waiters.length = 0
  scopeStates.clear()
  maxOverrideForTests = null
}

/** Test seam: override the per-server cap (both remote and local). */
export function setGridDialMaxForTests(n: number | null): void {
  maxOverrideForTests = n
}

/** Test seam: handshakes in flight for one server, or (null) every remote
 *  handshake in this window plus every local one. */
export function gridDialInflightForTests(scope: ServerScope | null): number {
  if (scope === null) {
    let total = 0
    for (const st of scopeStates.values()) total += st.inflight
    return total
  }
  return stateFor(scope).inflight
}

/** Test seam: remote handshakes in flight across all servers. */
export function remoteDialInflightForTests(): number {
  return remoteInflight
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

/** One server's own cap, from that server's remoteness. */
function scopeMaxDials(scope: ServerScope): number {
  if (maxOverrideForTests != null) return maxOverrideForTests
  return scope.isRemote ? MAX_CONCURRENT_DIALS : MAX_CONCURRENT_DIALS_LOCAL
}

function hasRoom(scope: ServerScope): boolean {
  if (stateFor(scope).inflight >= scopeMaxDials(scope)) return false
  return !scope.isRemote || remoteInflight < MAX_CONCURRENT_REMOTE_DIALS_WINDOW
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

/** A held handshake slot. `release` is idempotent. */
interface DialSlot {
  release: () => void
}

async function acquireDialSlot(scope: ServerScope, signal?: AbortSignal): Promise<DialSlot> {
  for (;;) {
    if (isAborted(signal)) throw new Error('grid-dial-aborted')
    const wait = gridDialBackoffRemainingMs(scope)
    if (wait > 0) {
      await sleepMs(wait, signal)
      continue
    }
    if (isAborted(signal)) throw new Error('grid-dial-aborted')
    if (hasRoom(scope)) {
      const key = queueKey(scope)
      const remote = scope.isRemote
      if (remote) remoteInflight += 1
      stateForKey(key).inflight += 1
      let released = false
      return {
        release: () => {
          if (released) return
          released = true
          releaseDialSlot(key, remote)
        },
      }
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

function releaseDialSlot(key: string, remote: boolean): void {
  if (remote) remoteInflight = Math.max(0, remoteInflight - 1)
  const st = stateForKey(key)
  st.inflight = Math.max(0, st.inflight - 1)
  // Wake every waiter that can now proceed (its own server has room under
  // the window cap), in FIFO order. With one server this is exactly the
  // old `shift()`. Each waiter re-enters acquireDialSlot and re-checks;
  // increment-on-wake + re-check deadlocks the next pane.
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
  const slot = await acquireDialSlot(scope, opts?.signal)
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
    slot.release()
  }
}

/**
 * MS70: dial any other daemon socket (events, overlay, transcript,
 * chatter) through `scope`'s queue. Waits for a slot, constructs the
 * socket, and returns it right away while it is still CONNECTING; the slot
 * is released on the socket's first open / error / close, or after
 * HANDSHAKE_TIMEOUT_MS (the socket is NOT closed then — the caller's own
 * handlers stay in charge). A handshake that errors or closes before
 * opening counts toward this server's burst backoff, as a grid's does.
 *
 * Throws `grid-dial-aborted` when `signal` aborts while waiting, and
 * `grid-dial-failed` when the constructor throws.
 */
export async function openQueuedWebSocket(
  scope: ServerScope,
  url: string,
  opts?: { signal?: AbortSignal },
): Promise<WebSocket> {
  const slot = await acquireDialSlot(scope, opts?.signal)
  let ws: WebSocket
  try {
    ws = new WebSocket(url)
  } catch {
    noteGridDialFailure(scope)
    slot.release()
    throw new Error('grid-dial-failed')
  }
  if (typeof ws.addEventListener !== 'function') {
    // A socket object without events (a test double): the slot covers
    // construction only.
    slot.release()
    return ws
  }
  let settled = false
  const settle = (failed: boolean): void => {
    if (settled) return
    settled = true
    clearTimeout(timer)
    ws.removeEventListener('open', onOpen)
    ws.removeEventListener('error', onFail)
    ws.removeEventListener('close', onFail)
    if (failed) noteGridDialFailure(scope)
    slot.release()
  }
  const onOpen = (): void => settle(false)
  const onFail = (): void => settle(true)
  const timer = setTimeout(() => settle(false), HANDSHAKE_TIMEOUT_MS)
  ws.addEventListener('open', onOpen)
  ws.addEventListener('error', onFail)
  ws.addEventListener('close', onFail)
  return ws
}
