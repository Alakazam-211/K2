// Renderer-side helper for talking to `k2so-daemon`'s `/cli/*` HTTP
// surface. Phase 2 Unit 6 added the file tree / chat history sidebar /
// theme manager / skill layers / review checklist routes; before that
// only Unit 1's CompanionSection used this pattern (hand-rolled fetch).
// This helper extracts the boilerplate so every renderer call site can
// be a one-liner.
//
// The daemon already exposes its loopback port + per-boot auth token
// via `getDaemonWs()` (cached after first call). Every helper here
// resolves those creds, fires the request, and surfaces the response.
//
// Error policy: any non-2xx response throws with a clean message
// (prefers JSON `{"error":"..."}` over raw status). The renderer's
// existing `try/catch` blocks around the old `invoke(...)` calls keep
// working unchanged.

import { getDaemonWs, invalidateDaemonWs, daemonHttpBase } from '@/kessel/daemon-ws'
import { useConnectHostStore } from '@/stores/connect-host'

/**
 * connect-users (#617) session expiry: a 401 from a REMOTE host means its
 * cached session token expired. Drop it + re-trigger the full-screen
 * sign-in (the store no-ops for 'local' or a non-active host). Surfacing
 * the sign-in is enough — we deliberately do NOT auto-retry the request.
 */
function handleRemoteUnauthorized(res: Response): void {
  if (res.status !== 401) return
  const active = useConnectHostStore.getState().activeHost
  if (active === 'local') return
  useConnectHostStore.getState().expireSession(active.id)
}

/**
 * GET /cli/<route>?<params>&token=<token>.
 * `route` should be the part AFTER `/cli/`, e.g. `fs/read-dir`.
 * Returns the parsed JSON body. Throws on non-2xx.
 *
 * Phase 2.5 fix (finding #547): on a network-level failure
 * (`fetch` throws — ECONNREFUSED, ENOTFOUND, …) we invalidate the
 * cached creds and retry once. The renderer caches `daemon_ws_url`
 * for the lifetime of the app process, so a daemon restart that
 * mints a new port would otherwise be silently routed to a dead
 * socket for the duration of the session. Invalidation forces the
 * next call to re-read `~/.k2so/daemon.{port,token}` from disk.
 */
export async function daemonCliGet<T = unknown>(
  route: string,
  params?: Record<string, string | number | boolean | undefined | null>,
): Promise<T> {
  return withConnRetry(async () => {
    const creds = await getDaemonWs()
    const search = new URLSearchParams()
    if (params) {
      for (const [k, v] of Object.entries(params)) {
        if (v !== undefined && v !== null) search.set(k, String(v))
      }
    }
    search.set('token', creds.token)
    const url = `${daemonHttpBase(creds)}/cli/${route}?${search.toString()}`
    const res = await fetch(url, { method: 'GET' })
    handleRemoteUnauthorized(res)
    return parseDaemonResponse<T>(res)
  })
}

/**
 * GET /cli/<route>?<params>&token=<token>, returning the RAW response
 * body as text — never JSON-parsed. Use this for routes whose body is
 * plain text or whose JSON-shaped payload must be handled verbatim by the
 * caller (e.g. `timer/entries-export`, where a `format=json` body is a
 * JSON array string that the caller blobs/downloads as-is — parsing it to
 * an array would break `new Blob([data])`).
 *
 * Same creds resolution + connection-retry + remote-401 handling as
 * `daemonCliGet`; only the success-body handling differs (text, not JSON).
 * Throws on non-2xx with the same `{"error":"..."}`-aware message.
 */
export async function daemonCliGetText(
  route: string,
  params?: Record<string, string | number | boolean | undefined | null>,
): Promise<string> {
  return withConnRetry(async () => {
    const creds = await getDaemonWs()
    const search = new URLSearchParams()
    if (params) {
      for (const [k, v] of Object.entries(params)) {
        if (v !== undefined && v !== null) search.set(k, String(v))
      }
    }
    search.set('token', creds.token)
    const url = `${daemonHttpBase(creds)}/cli/${route}?${search.toString()}`
    const res = await fetch(url, { method: 'GET' })
    handleRemoteUnauthorized(res)
    return parseDaemonText(res)
  })
}

/**
 * POST /cli/<route>?token=<token> with `body` JSON-encoded. `body`
 * fields go in the body, NOT the query string — passwords and large
 * payloads (file contents) belong out of URL-logging path.
 *
 * Same connection-retry semantics as `daemonCliGet`.
 */
export async function daemonCliPost<T = unknown>(
  route: string,
  body?: unknown,
): Promise<T> {
  return withConnRetry(async () => {
    const creds = await getDaemonWs()
    const url = `${daemonHttpBase(creds)}/cli/${route}?token=${creds.token}`
    const res = await fetch(url, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: body !== undefined ? JSON.stringify(body) : undefined,
    })
    handleRemoteUnauthorized(res)
    return parseDaemonResponse<T>(res)
  })
}

/**
 * Run `op` once. If it throws a connection-level error (caught
 * `fetch` failure — distinct from a non-2xx response), invalidate
 * the cached daemon creds and try a second time. A successful
 * second attempt returns the value; a second failure throws.
 *
 * Non-2xx responses are NOT retried — those are application errors
 * the route handler explicitly returned, not stale-creds issues.
 *
 * Kept distinct from `parseDaemonResponse` so HTTP errors continue
 * to throw verbatim — the retry only protects against the kernel
 * refusing the connection (the symptom of finding #547).
 */
async function withConnRetry<T>(op: () => Promise<T>): Promise<T> {
  try {
    return await op()
  } catch (err) {
    if (!isConnectionLevelError(err)) throw err
    // Daemon may have rebooted and rotated its port; force a re-read.
    invalidateDaemonWs()
    return await op()
  }
}

/** Detect connection-level errors (kernel refused the connection,
 *  DNS failure, network down). These are the failure modes a
 *  port-mismatch produces — distinct from HTTP-level errors which
 *  `parseDaemonResponse` already throws. Browser `fetch` throws a
 *  `TypeError` with message starting "Failed to fetch" / "Load
 *  failed" / "NetworkError" depending on engine; Tauri's webview
 *  uses WKWebView (Safari) and surfaces "Load failed". */
function isConnectionLevelError(err: unknown): boolean {
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

async function parseDaemonResponse<T>(res: Response): Promise<T> {
  const text = await res.text()
  if (!res.ok) {
    // Daemon's bad_request shape: `{"error":"<message>"}`. Surface
    // the message verbatim so existing renderer code that does
    // `e instanceof Error ? e.message : String(e)` shows a useful
    // string rather than `[object Response]`.
    let msg = text
    try {
      const parsed = JSON.parse(text)
      if (parsed && typeof parsed.error === 'string') msg = parsed.error
    } catch {
      /* fall through with raw text */
    }
    throw new Error(msg || `daemon ${route(res.url)} ${res.status}`)
  }
  if (text.length === 0) return undefined as unknown as T
  try {
    return JSON.parse(text) as T
  } catch {
    // Some routes return plain text (e.g. an error string from a
    // historical Tauri command). Cast through unknown so the caller's
    // type assertion still applies — pre-Phase-2 behavior was the
    // same.
    return text as unknown as T
  }
}

/** Like `parseDaemonResponse` but returns the raw body text on success
 *  (no JSON parse). Non-2xx still throws the `{"error":"..."}`-aware
 *  message so callers see a useful string, not `[object Response]`. */
async function parseDaemonText(res: Response): Promise<string> {
  const text = await res.text()
  if (!res.ok) {
    let msg = text
    try {
      const parsed = JSON.parse(text)
      if (parsed && typeof parsed.error === 'string') msg = parsed.error
    } catch {
      /* fall through with raw text */
    }
    throw new Error(msg || `daemon ${route(res.url)} ${res.status}`)
  }
  return text
}

function route(url: string): string {
  try {
    const u = new URL(url)
    return u.pathname
  } catch {
    return url
  }
}
