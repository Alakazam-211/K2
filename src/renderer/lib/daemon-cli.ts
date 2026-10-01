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

import { getLocalDaemonWs, invalidateDaemonWs, daemonHttpBase, type DaemonWsAvailable } from '@/kessel/daemon-ws'
import type { ServerScope } from '@/kessel/server-scope'
import { assertServerScope } from '@/kessel/assert-scope'
import type { ConnectHost } from '@/stores/connect-host'
import { useConnectHostStore } from '@/stores/connect-host'
import { forceSoftHealthProbe } from '@/lib/connection-gate-probe'
import {
  classifyRemoteFetchError,
  logRemotePath,
  redactRemoteUrl,
} from '@/lib/remote-path-log'
import { CLI_CONNECTED_RETRY_DELAYS_MS, isConnectionLevelError, withRemoteRetry } from '@/lib/remote-retry'
import {
  isPasswordChangeRequired,
  isPossibleAuthFailure,
  requirePasswordRotation,
  reviveRemoteSession,
} from '@/lib/remote-session'
import { cliSearchParams, withDaemonFetch } from '@/web/session-token'

/** A response plus its (already-consumed) body text. The body is read
 *  exactly once, up front, because BOTH the auth-failure classifier and the
 *  parse helpers need it. */
interface CliHttpResult {
  res: Response
  text: string
}

/**
 * 0.40.48 storm killer — thrown by `cliFetch` INSTEAD of issuing a network
 * request while the active remote host is in a non-'connected' recovery
 * state (reconnecting / reauthenticating / signin-required / wedged).
 *
 * Rationale: during the live wedge incident a dozen independent retry loops
 * (stores, panes, pollers) each kept firing `/cli/*` requests through the
 * same poisoned pooled connection — ~11 req/s of guaranteed failures that
 * also kept the pool warm. While ConnectionGate's recovery machinery owns
 * the host's fate, everything else fails fast here: no fetch, no retries,
 * no socket churn. Recovery traffic itself is UNAFFECTED because none of it
 * rides `cliFetch` — /boot-status polls and the whoami probe use their own
 * `fetch` in ConnectionGate, and `reviveRemoteSession`'s whoami + login use
 * their own `fetch` in lib/remote-session — so the gate can never starve
 * its own escape path (see `recoveryGateAllows`).
 *
 * Callers already treat any throw from daemonCli* as a failed call; this
 * error's message is deliberately NOT connection-level-shaped so
 * `withRemoteRetry` never burns its backoff on it. Use `instanceof
 * RecoveringError` to special-case it (e.g. suppress error toasts).
 */
export class RecoveringError extends Error {
  constructor(hostLabel: string, recoveryKind: string) {
    super(`host "${hostLabel}" is ${recoveryKind} — request skipped until recovery completes`)
    this.name = 'RecoveringError'
  }
}

/**
 * Thrown by `cliFetch` when the scope's `connectionKey` changed between
 * start and completion. For the primary scope that is `activeHostKey` — the
 * window switched servers — and the HTTP result (if any) is discarded so a
 * GET started on host A cannot land against, or be applied on, host B. A
 * pinned scope's key never changes, so it never throws this. Not a
 * connection-level error: no retry, no `forceSoftHealthProbe`.
 */
export class HostSwitchedError extends Error {
  readonly startedKey: string
  readonly currentKey: string
  constructor(startedKey: string, currentKey: string) {
    super(`host switched (${startedKey} → ${currentKey}) — result discarded`)
    this.name = 'HostSwitchedError'
    this.startedKey = startedKey
    this.currentKey = currentKey
  }
}

export function isHostSwitchedError(err: unknown): boolean {
  return err instanceof Error && err.name === 'HostSwitchedError'
}

/** The window's active remote (the host revive and rotation act on), or
 *  null while the window is on `local`. */
function windowActiveRemote(): ConnectHost | null {
  const active = useConnectHostStore.getState().activeHost
  return active === 'local' ? null : active
}

function throwIfHostSwitched(scope: ServerScope, startedKey: string): void {
  const now = scope.connectionKey
  if (now !== startedKey) throw new HostSwitchedError(startedKey, now)
}

/**
 * The per-scope gate: `false` when the scope points at the window's ACTIVE
 * host, that host is a remote, and its recovery state machine says it's not
 * 'connected'. Local is never gated (its recovery field is meaningless —
 * stays 'connected'), and a remote in the healthy baseline passes
 * untouched, so steady-state behavior is byte-identical to before.
 *
 * A pinned scope for a server that is NOT the window's host is not gated by
 * the window's recovery: A recovering must not block a room on B. (Per-host
 * recovery state for other servers is M2's connection pool.)
 *
 * NOTE this reads the store at CALL time (like getDaemonWs does), so a
 * recovery that completes between two calls immediately unblocks traffic —
 * nothing subscribes or spins.
 */
function recoveryGateAllows(
  scope: ServerScope,
): { ok: true } | { ok: false; label: string; kind: string } {
  if (!scope.isWindowHost()) return { ok: true }
  const s = useConnectHostStore.getState()
  if (s.activeHost === 'local') return { ok: true }
  if (s.recovery.kind === 'connected') return { ok: true }
  return { ok: false, label: s.activeHost.label, kind: s.recovery.kind }
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
/** Cap concurrent `/cli/*` fetches against a REMOTE host. Reorder/move
 *  in Settings used to fire N workspaces/list + N feedback/list at once
 *  and collapse E2E (WKWebView CORS / handshake eof). Local is uncapped.
 *  The cap is per server (keyed by host key): a busy room on B never eats
 *  A's slots. */
const REMOTE_CLI_MAX_INFLIGHT = 4

interface CliSlotPool {
  inflight: number
  waiters: Array<() => void>
}

const remoteCliPools = new Map<string, CliSlotPool>()

function cliPoolFor(hostKey: string): CliSlotPool {
  let pool = remoteCliPools.get(hostKey)
  if (!pool) {
    pool = { inflight: 0, waiters: [] }
    remoteCliPools.set(hostKey, pool)
  }
  return pool
}

/** Returns the pool the slot was taken from (release it there), or null
 *  when the scope is local (uncapped). */
async function acquireRemoteCliSlot(scope: ServerScope): Promise<CliSlotPool | null> {
  if (!scope.isRemote) return null
  const pool = cliPoolFor(scope.hostKey)
  for (;;) {
    if (pool.inflight < REMOTE_CLI_MAX_INFLIGHT) {
      pool.inflight += 1
      return pool
    }
    await new Promise<void>((resolve) => {
      pool.waiters.push(resolve)
    })
  }
}

function releaseRemoteCliSlot(pool: CliSlotPool): void {
  pool.inflight = Math.max(0, pool.inflight - 1)
  const next = pool.waiters.shift()
  if (next) next()
}

/** Test seam: in-flight `/cli/*` count for one host key. */
export function remoteCliInflightForTests(hostKey: string): number {
  const pool = remoteCliPools.get(hostKey)
  return pool ? pool.inflight : 0
}

async function cliFetch(
  scope: ServerScope,
  build: (creds: DaemonWsAvailable) => { url: string; init?: RequestInit },
): Promise<CliHttpResult> {
  assertServerScope(scope, 'daemonCli')
  // Tag the host this call belongs to. A switch mid-flight (selectHost
  // already flipped `activeHost`) must drop the result — including
  // fs/write-file unmount autosave and layout save — so leftover panes
  // cannot complete against the NEW daemon. (Primary scope only: a pinned
  // scope's connection key is fixed.)
  const startedKey = scope.connectionKey
  // 0.40.48: while the active REMOTE host is recovering, fail fast instead
  // of feeding the retry storm (and, in the wedged case, a poisoned pool).
  // Checked once at entry — the post-revival replay below is exempt by
  // construction (revival just proved the host reachable + re-authed).
  const gate = recoveryGateAllows(scope)
  if (!gate.ok) throw new RecoveringError(gate.label, gate.kind)
  const heldRemoteSlot = await acquireRemoteCliSlot(scope)
  let lastUrl = ''
  try {
    throwIfHostSwitched(scope, startedKey)
    const out = await withConnRetry(async () => {
      throwIfHostSwitched(scope, startedKey)
      const attempt = async (): Promise<CliHttpResult> => {
        throwIfHostSwitched(scope, startedKey)
        const creds = await scope.creds()
        throwIfHostSwitched(scope, startedKey)
        const { url, init } = build(creds)
        lastUrl = url
        // Hosted web: credentials:include (send/store k2_session) + X-K2-Client.
        // Desktop: withDaemonFetch is a no-op — init is unchanged.
        const res = await fetch(url, withDaemonFetch(init ?? {}))
        throwIfHostSwitched(scope, startedKey)
        return { res, text: await res.text() }
      }
      let result = await attempt()
      throwIfHostSwitched(scope, startedKey)
      // Re-login and the rotation prompt run ONLY for the window's own
      // server (Home M1 / MS77). A pinned scope for another server gets the
      // original 401/403 back with no auth side effects; its revive lands
      // with M2's connection pool.
      if (isPossibleAuthFailure(result.res.status, result.text)) {
        const host = scope.isWindowHost() ? windowActiveRemote() : null
        if (host) {
          const outcome = await reviveRemoteSession(host.id)
          // 'revived' means the store now carries a NEW token — replay once so
          // the caller never sees the transient stale-session rejection. Any
          // other outcome (still-valid role denial, sign-in required, network,
          // cooldown) keeps the original response.
          if (outcome === 'revived') {
            throwIfHostSwitched(scope, startedKey)
            result = await attempt()
          }
        }
      } else if (isPasswordChangeRequired(result.res.status, result.text)) {
        // W2: a restricted (temporary-password) session — the token is
        // valid, so revival would only say 'still-valid'. Route the user to
        // the rotation step; the original 403 still surfaces to the caller.
        const host = scope.isWindowHost() ? windowActiveRemote() : null
        if (host) requirePasswordRotation(host.id)
      }
      throwIfHostSwitched(scope, startedKey)
      return result
    })
    throwIfHostSwitched(scope, startedKey)
    return out
  } catch (err) {
    if (isHostSwitchedError(err)) throw err
    // Compose send and other /cli/* rides this path. Edge 404/CORS throws
    // here; kick a health tick so the arbiter can reload the poisoned pool
    // instead of waiting 25s (or for Local → remote). HTTP 400 is an
    // application error from parseDaemonResponse — not this catch, and
    // not a probe.
    if (isConnectionLevelError(err) && scope.isRemote && scope.isWindowHost()) {
      logRemotePath('cli-fail', {
        url: lastUrl ? redactRemoteUrl(lastUrl) : undefined,
        class: classifyRemoteFetchError(err),
        err: err instanceof Error ? `${err.name}: ${err.message}` : String(err),
      })
      forceSoftHealthProbe()
    }
    throw err
  } finally {
    if (heldRemoteSlot) releaseRemoteCliSlot(heldRemoteSlot)
  }
}

/**
 * GET /cli/<route>?<params>[&token=<token>] on `scope`'s server.
 * `scope` is required (Home M1): pass `primaryScope()` for the window's
 * server, or the scope already in hand.
 * `route` should be the part AFTER `/cli/`, e.g. `fs/read-dir`.
 * Returns the parsed JSON body. Throws on non-2xx.
 *
 * Desktop always appends `?token=`. Hosted web (`VITE_WEB`) omits the
 * query token and authenticates via the `k2_session` cookie
 * (`credentials: 'include'` + `X-K2-Client: web` from `withDaemonFetch`).
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
  scope: ServerScope,
  route: string,
  params?: Record<string, string | number | boolean | undefined | null>,
): Promise<T> {
  const { res, text } = await cliFetch(scope, (creds) => ({
    url: getUrl(creds, route, params),
    init: { method: 'GET' },
  }))
  return parseDaemonResponse<T>(res, text)
}

/** Build `GET/POST /cli/<route>?<params>[&token=]` for resolved creds —
 *  shared by daemonCliGet/Post/GetText so all rebuild the URL with FRESH
 *  creds on the post-revival replay. Hosted web omits `token`. */
function getUrl(
  creds: DaemonWsAvailable,
  route: string,
  params?: Record<string, string | number | boolean | undefined | null>,
): string {
  const search = cliSearchParams(creds.token, params)
  const q = search.toString()
  return `${daemonHttpBase(creds)}/cli/${route}${q ? `?${q}` : ''}`
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
  scope: ServerScope,
  route: string,
  params?: Record<string, string | number | boolean | undefined | null>,
): Promise<string> {
  const { res, text } = await cliFetch(scope, (creds) => ({
    url: getUrl(creds, route, params),
    init: { method: 'GET' },
  }))
  return parseDaemonText(res, text)
}

/**
 * POST /cli/<route>[?token=<token>] with `body` JSON-encoded. `body`
 * fields go in the body, NOT the query string — passwords and large
 * payloads (file contents) belong out of URL-logging path.
 *
 * Desktop: `?token=` as always. Hosted web: cookie auth (no query token)
 * + CSRF header via `withDaemonFetch`.
 *
 * Same connection-retry semantics as `daemonCliGet`.
 */
export async function daemonCliPost<T = unknown>(
  scope: ServerScope,
  route: string,
  body?: unknown,
): Promise<T> {
  const { res, text } = await cliFetch(scope, (creds) => ({
    url: getUrl(creds, route),
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
  // Always the LOCAL loopback daemon (desktop clone-to pull). Keep the
  // classic `?token=` path — this is never the hosted-web same-origin
  // surface (web never activates the local Tauri daemon).
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
 * distinct from a non-2xx response). Connected `/cli/*` uses delay-0 only
 * (original + immediate eviction retry); CORS/ACAO rethrows immediately
 * inside {@link withRemoteRetry}. `invalidateDaemonWs` fires before each retry
 * so the daemon's (possibly rotated) port/token is re-read.
 *
 * Non-2xx responses are NOT retried here — those are application errors the
 * route handler explicitly returned. The one exception lives in `cliFetch`:
 * an auth-classified 401/403 from a remote host goes through session revival
 * and is replayed once IFF a fresh token was minted.
 */
function withConnRetry<T>(op: () => Promise<T>): Promise<T> {
  return withRemoteRetry(op, { onRetry: invalidateDaemonWs, delaysMs: CLI_CONNECTED_RETRY_DELAYS_MS })
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
