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

import { getDaemonWs, getLocalDaemonWs, invalidateDaemonWs, daemonHttpBase, type DaemonWsAvailable } from '@/kessel/daemon-ws'
import { useConnectHostStore } from '@/stores/connect-host'
import { withRemoteRetry } from '@/lib/remote-retry'
import { isPossibleAuthFailure, reviveRemoteSession } from '@/lib/remote-session'

/** A response plus its (already-consumed) body text. The body is read
 *  exactly once, up front, because BOTH the auth-failure classifier and the
 *  parse helpers need it. */
interface CliHttpResult {
  res: Response
  text: string
}

/**
 * Resolve creds → fire ONE request built by `build` → read the body.
 *
 * Stale-session recovery (the runtime half of connect-users #617): a remote
 * daemon restart wipes its in-memory sessions, after which every authed
 * `/cli/*` call returns 403 "Invalid or missing auth token" (NOT 401 — see
 * remote-session.ts). On an auth-classified rejection from a REMOTE host we
 * run `reviveRemoteSession` (single-flight whoami-confirm + re-login with
 * the remembered password — the same mint flow boot uses) and, ONLY if a
 * fresh token was actually minted, retry the request once with the new
 * creds. Every other failure surfaces unchanged; a 401/403 rejected a
 * request before doing work, so the single replay is side-effect-safe.
 */
async function cliFetch(
  build: (creds: DaemonWsAvailable) => { url: string; init?: RequestInit },
): Promise<CliHttpResult> {
  return withConnRetry(async () => {
    const attempt = async (): Promise<CliHttpResult> => {
      const creds = await getDaemonWs()
      const { url, init } = build(creds)
      const res = await fetch(url, init)
      return { res, text: await res.text() }
    }
    let out = await attempt()
    if (isPossibleAuthFailure(out.res.status, out.text)) {
      const active = useConnectHostStore.getState().activeHost
      if (active !== 'local') {
        const outcome = await reviveRemoteSession(active.id)
        // 'revived' means the store now carries a NEW token — replay once so
        // the caller never sees the transient stale-session rejection. Any
        // other outcome (still-valid role denial, sign-in required, network,
        // cooldown) keeps the original response.
        if (outcome === 'revived') out = await attempt()
      }
    }
    return out
  })
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
  const { res, text } = await cliFetch((creds) => ({
    url: getUrl(creds, route, params),
    init: { method: 'GET' },
  }))
  return parseDaemonResponse<T>(res, text)
}

/** Build `GET /cli/<route>?<params>&token=` for resolved creds — shared by
 *  daemonCliGet/daemonCliGetText so both rebuild the URL with FRESH creds on
 *  the post-revival replay. */
function getUrl(
  creds: DaemonWsAvailable,
  route: string,
  params?: Record<string, string | number | boolean | undefined | null>,
): string {
  const search = new URLSearchParams()
  if (params) {
    for (const [k, v] of Object.entries(params)) {
      if (v !== undefined && v !== null) search.set(k, String(v))
    }
  }
  search.set('token', creds.token)
  return `${daemonHttpBase(creds)}/cli/${route}?${search.toString()}`
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
  const { res, text } = await cliFetch((creds) => ({
    url: getUrl(creds, route, params),
    init: { method: 'GET' },
  }))
  return parseDaemonText(res, text)
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
  const { res, text } = await cliFetch((creds) => ({
    url: `${daemonHttpBase(creds)}/cli/${route}?token=${creds.token}`,
    init: {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: body !== undefined ? JSON.stringify(body) : undefined,
    },
  }))
  return parseDaemonResponse<T>(res, text)
}

/**
 * POST /cli/<route> against the LOCAL daemon, regardless of the active
 * host. The pull half of clone-to ("Clone to this computer") runs its
 * unpack + cleanup on the local daemon WHILE the remote host stays
 * active, so it can't go through the active-host `daemonCliPost`.
 * Same connection-retry + response parsing as `daemonCliPost`; the
 * remote-session revival step is intentionally absent (revival is a
 * remote-host recovery — the local daemon's token never goes stale
 * within an app session, and a restart invalidates via `withConnRetry`).
 */
export async function localDaemonCliPost<T = unknown>(
  route: string,
  body?: unknown,
): Promise<T> {
  const { res, text } = await withConnRetry(async () => {
    const creds = await getLocalDaemonWs()
    const res = await fetch(`${daemonHttpBase(creds)}/cli/${route}?token=${creds.token}`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: body !== undefined ? JSON.stringify(body) : undefined,
    })
    return { res, text: await res.text() }
  })
  return parseDaemonResponse<T>(res, text)
}

/**
 * Run `op`, retrying on a connection-level error (a caught `fetch` failure —
 * distinct from a non-2xx response). The shared {@link withRemoteRetry}
 * backoff evicts a dead pooled WKWebView socket on the first (immediate) retry
 * and rides out a remote restart over the rest of the schedule; the
 * `invalidateDaemonWs` hook fires before each retry so the daemon's
 * (possibly rotated) port/token is re-read.
 *
 * Non-2xx responses are NOT retried here — those are application errors the
 * route handler explicitly returned. The one exception lives in `cliFetch`:
 * an auth-classified 401/403 from a remote host goes through session revival
 * and is replayed once IFF a fresh token was minted.
 */
function withConnRetry<T>(op: () => Promise<T>): Promise<T> {
  return withRemoteRetry(op, { onRetry: invalidateDaemonWs })
}

async function parseDaemonResponse<T>(res: Response, text: string): Promise<T> {
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
function parseDaemonText(res: Response, text: string): string {
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
