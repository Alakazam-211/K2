/**
 * Cap concurrent grid WS *handshakes* (CONNECTING only).
 * Burst-fail backs off every pane so heal timers cannot thrash.
 */

export const MAX_CONCURRENT_DIALS = 1
export const FAIL_BURST_WINDOW_MS = 3_000
export const FAIL_BURST_COUNT = 3
export const BACKOFF_MS = 8_000
/** Bound limbo CONNECTING so two hung dials cannot freeze the app. */
export const HANDSHAKE_TIMEOUT_MS = 4_000

let inflight = 0
const waiters: Array<() => void> = []
let backoffUntil = 0
const recentFails: number[] = []

export function resetGridDialQueueForTests(): void {
  inflight = 0
  waiters.length = 0
  backoffUntil = 0
  recentFails.length = 0
}

export function gridDialBackoffRemainingMs(now = Date.now()): number {
  return Math.max(0, backoffUntil - now)
}

export function noteGridDialFailure(now = Date.now()): void {
  recentFails.push(now)
  while (recentFails.length > 0 && now - recentFails[0]! > FAIL_BURST_WINDOW_MS) {
    recentFails.shift()
  }
  if (recentFails.length >= FAIL_BURST_COUNT) {
    backoffUntil = now + BACKOFF_MS
    recentFails.length = 0
  }
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

async function acquireDialSlot(signal?: AbortSignal): Promise<void> {
  for (;;) {
    if (isAborted(signal)) throw new Error('grid-dial-aborted')
    const wait = gridDialBackoffRemainingMs()
    if (wait > 0) {
      await sleepMs(wait, signal)
      continue
    }
    if (isAborted(signal)) throw new Error('grid-dial-aborted')
    if (inflight < MAX_CONCURRENT_DIALS) {
      inflight += 1
      return
    }
    await new Promise<void>((resolve) => {
      const wake = () => {
        signal?.removeEventListener('abort', onAbort)
        resolve()
      }
      const onAbort = () => {
        const i = waiters.indexOf(wake)
        if (i >= 0) waiters.splice(i, 1)
        resolve()
      }
      waiters.push(wake)
      if (isAborted(signal)) {
        onAbort()
        return
      }
      signal?.addEventListener('abort', onAbort)
    })
  }
}

function releaseDialSlot(): void {
  inflight = Math.max(0, inflight - 1)
  const next = waiters.shift()
  // Waiter re-enters acquireDialSlot. Increment-on-wake + re-check
  // deadlocks the next pane (inflight already at MAX when it loops).
  if (next) next()
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
 * Dial a grid WS through the global queue. Caller owns the socket after
 * OPEN (or after reject). Failed handshake counts toward resource backoff.
 */
export async function openQueuedGridWebSocket(
  url: string,
  opts?: GridDialOpts,
): Promise<WebSocket> {
  await acquireDialSlot(opts?.signal)
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
      noteGridDialFailure()
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
      noteGridDialFailure()
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
    releaseDialSlot()
  }
}
