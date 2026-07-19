// Shared retry-on-network-error helper for REMOTE daemon fetches.
//
// THE BUG IT FIXES
// When a remote K2 server restarts/updates, the desktop app's WKWebView keeps
// DEAD pooled keep-alive sockets for that origin. Every *.k2.dev host shares
// one relay IP, so the webview pools a single connection per origin; after the
// remote churns (a restart / E2E cert re-provision) a cached GET may limp
// through, but the next `fetch()` reuses the dead socket and throws at the
// NETWORK layer ("Load failed" / "Couldn't establish a connection"). Until the
// app is fully relaunched, every subsequent call fails the same way.
//
// THE FIX (proven by the prior "issue #5" login retry)
// The throw itself EVICTS the dead socket from WKWebView's pool, so an
// immediate retry opens a FRESH socket and recovers — no app relaunch. Header
// tricks (`Connection: close`, `keepalive:false`, `cache:'no-store'`) do NOT
// work in WKWebView, so retry-on-network-error is the only reliable lever.
//
// This module is the SINGLE SOURCE OF TRUTH for (a) detecting a connection-
// level error and (b) the backoff schedule. Every bare remote `fetch` call
// site routes through `withRemoteRetry` so a remote restart self-heals.

import { jittered } from '@/lib/backoff'

/**
 * Detect connection-level errors (kernel refused the connection, DNS failure,
 * network down, dead pooled socket). These are the failure modes a daemon
 * restart / relay churn produces — distinct from HTTP-level errors which the
 * fetch wrappers already throw with the daemon's `{"error":...}` message.
 *
 * Browser `fetch` throws a `TypeError` whose message starts "Failed to fetch"
 * / "NetworkError" depending on the engine; Tauri's webview is WKWebView
 * (Safari) and surfaces "Load failed". `getDaemonWs` rejects with a
 * `daemon_ws_url`/`daemon not reachable` prefix when the daemon hasn't
 * published creds yet — same recovery path.
 *
 * IMPORTANT: a 401/403 or any other non-2xx is NOT a connection error — those
 * are authoritative application errors the daemon explicitly returned and must
 * surface immediately (never retried).
 */
export function isConnectionLevelError(err: unknown): boolean {
  if (!(err instanceof Error)) return false
  const m = err.message.toLowerCase()
  return (
    m.includes('failed to fetch') ||
    m.includes('load failed') ||
    m.includes('networkerror') ||
    m.includes('connection refused') ||
    m.includes('ecconnrefused') ||
    m.includes('econnrefused') ||
    // `getDaemonWs` rejects with this prefix when the daemon hasn't
    // published creds yet — same recovery path.
    m.includes('daemon_ws_url invoke failed') ||
    m.includes('daemon not reachable')
  )
}

/** Tuning knobs for {@link withRemoteRetry}. */
export interface RemoteRetryOptions {
  /**
   * The pause (ms) BEFORE each retry attempt that follows the first try.
   * `delaysMs.length` therefore equals the number of RETRIES; total attempts
   * = `delaysMs.length + 1`. The default `[0, 400, 1200, 2000]` is 4 attempts
   * over ~3.6s: the first retry is immediate (to evict the dead socket and
   * reopen a fresh one), later retries ride out the restart window.
   */
  delaysMs?: number[]
  /**
   * Called once between attempts (after the failed try, before the next).
   * Lets a caller invalidate cached creds (e.g. `invalidateDaemonWs`) so the
   * retry re-reads the daemon's possibly-rotated port/token.
   */
  onRetry?: () => void | Promise<void>
}

/** The default backoff: immediate first retry to evict+reopen the dead socket,
 *  then widening pauses to ride out the remote restart/update window. */
export const DEFAULT_REMOTE_RETRY_DELAYS_MS = [0, 400, 1200, 2000]

/**
 * Run `op`, retrying ONLY on a connection-level error (a dead pooled socket /
 * refused connection — see {@link isConnectionLevelError}). A non-2xx app
 * error or a 401 is authoritative and rethrown IMMEDIATELY (never retried, so
 * the caller's auth-expiry handling still fires on the first failure).
 *
 * The first retry fires after `delaysMs[0]` (0 by default — immediate: the
 * throw already evicted the dead socket, so reopening recovers at once); each
 * subsequent retry waits the next `delaysMs` entry. After the final attempt
 * the original error is rethrown verbatim (message preserved).
 */
export async function withRemoteRetry<T>(
  op: () => Promise<T>,
  opts?: RemoteRetryOptions,
): Promise<T> {
  const delays = opts?.delaysMs ?? DEFAULT_REMOTE_RETRY_DELAYS_MS
  // Attempt 0 is the initial try; attempts 1..N are the retries gated by
  // `delays[i-1]`.
  let lastErr: unknown
  for (let attempt = 0; attempt <= delays.length; attempt++) {
    if (attempt > 0) {
      // Let the caller invalidate cached creds before re-reading them.
      await opts?.onRetry?.()
      // 0.40.48: jitter each positive pause so many call sites retrying
      // after the same remote reboot don't fire in lockstep (the ~11 req/s
      // storm). A 0 delay stays 0 — the immediate first retry is what
      // evicts the dead pooled socket.
      const pause = delays[attempt - 1]
      if (pause > 0) await new Promise((r) => setTimeout(r, jittered(pause)))
    }
    try {
      return await op()
    } catch (err) {
      // Non-connection errors (non-2xx, 401, bad-request) are authoritative —
      // surface them immediately without burning the retry budget.
      if (!isConnectionLevelError(err)) throw err
      lastErr = err
    }
  }
  // Exhausted every attempt against a persistent connection failure — rethrow
  // the last (original-shaped) error so the caller sees a real message.
  throw lastErr
}
