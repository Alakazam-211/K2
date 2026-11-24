// Renderer-side helper for talking to `k2-daemon`'s `/cli/*` HTTP
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

import {
  getLocalDaemonWs,
  invalidateDaemonWs,
  refreshLocalDaemonWs,
  daemonHttpBase,
  LOCAL_DAEMON_HOST,
  type DaemonWsAvailable,
} from '@/kessel/daemon-ws'
import { isWebClient } from '@/lib/is-web'
import { assertScopeMayPost, type ServerScope } from '@/kessel/server-scope'
import { assertServerScope } from '@/kessel/assert-scope'
import type { ConnectHost } from '@/stores/connect-host'
import { useConnectHostStore } from '@/stores/connect-host'
import { reportRemoteFailure } from '@/lib/connection-gate-probe'
import { noteLocalTrouble } from '@/lib/local-health'
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
import { getPinnedAuthRecovery, notePoolRoleRefused } from '@/lib/pool-hooks'
import { isRoleRequired } from '@/lib/voice-chat-only'
import { DaemonHttpError } from '@/lib/daemon-http-error'
import { recordCliTiming, type CliLane } from '@/lib/cli-timing'
import { noteUserPost } from '@/lib/user-send-bus'

/** A response plus its (already-consumed) body text. The body is read
 *  exactly once, up front, because BOTH the auth-failure classifier and the
 *  parse helpers need it. */
interface CliHttpResult {
  res: Response
  text: string
  /** A binary call's body on a 2xx (`CliCallMeta.binary`). */
  blob?: Blob
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
 * Stale-session recovery (the runtime half of connect-users #617): a
 * remote session goes stale when it expires (7-day TTL) or is revoked.
 * Sessions are persisted on the daemon, so a daemon restart does NOT wipe
 * them (`crates/k2-core/src/connect_users.rs`). A stale session makes every
 * authed `/cli/*` call return 403 "Invalid or missing auth token" (NOT 401
 * — see remote-session.ts). On an auth-classified rejection from a REMOTE host we
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

/** The Thread lane (research-remote-thread-load-v1 Fix 1): a Thread read or
 *  a Thread / project chat post never waits behind a room's mount reads.
 *  It takes a normal slot when one is free, else one of these extra slots,
 *  so a server sees at most 4 + 2 requests from this window at once, and a
 *  burst of Thread calls can't flood the tunnel either. When a slot frees,
 *  a waiting Thread call goes first. */
const REMOTE_CLI_PRIORITY_EXTRA = 2

/** Routes on the Thread lane: what paints a Thread and what sends to one. */
export const PRIORITY_CLI_ROUTES: ReadonlySet<string> = new Set([
  'thread',
  'thread/activity',
  'thread/post',
  'project-group/msg',
])

export function isPriorityCliRoute(route: string): boolean {
  return PRIORITY_CLI_ROUTES.has(route)
}

interface CliSlotPool {
  inflight: number
  waiters: Array<() => void>
  priorityWaiters: Array<() => void>
}

const remoteCliPools = new Map<string, CliSlotPool>()

function cliPoolFor(hostKey: string): CliSlotPool {
  let pool = remoteCliPools.get(hostKey)
  if (!pool) {
    pool = { inflight: 0, waiters: [], priorityWaiters: [] }
    remoteCliPools.set(hostKey, pool)
  }
  return pool
}

/** Returns the pool the slot was taken from (release it there), or null
 *  when the scope is local (uncapped). A freed slot is handed straight to
 *  the next waiter (Thread lane first), so nothing can cut in between. */
async function acquireRemoteCliSlot(scope: ServerScope, priority = false): Promise<CliSlotPool | null> {
  if (!scope.isRemote) return null
  const pool = cliPoolFor(scope.hostKey)
  const cap = priority ? REMOTE_CLI_MAX_INFLIGHT + REMOTE_CLI_PRIORITY_EXTRA : REMOTE_CLI_MAX_INFLIGHT
  if (pool.inflight < cap) {
    pool.inflight += 1
    return pool
  }
  await new Promise<void>((resolve) => {
    if (priority) pool.priorityWaiters.push(resolve)
    else pool.waiters.push(resolve)
  })
  return pool
}

function releaseRemoteCliSlot(pool: CliSlotPool): void {
  // Hand the slot over without freeing it: a Thread call waiting may take
  // it at any count; a normal call only while the count after this
  // release leaves room under the normal cap.
  const nextPriority = pool.priorityWaiters.shift()
  if (nextPriority) {
    nextPriority()
    return
  }
  if (pool.inflight <= REMOTE_CLI_MAX_INFLIGHT) {
    const next = pool.waiters.shift()
    if (next) {
      next()
      return
    }
  }
  pool.inflight = Math.max(0, pool.inflight - 1)
}

/** Run `fn` holding one of `scope`'s `/cli` slots (Home M2, MS25): the
 *  connection pool's status checks share the same per-server cap of 4 as
 *  every `daemonCli*` call. Local is uncapped. */
export async function withHostCliSlot<T>(scope: ServerScope, fn: () => Promise<T>): Promise<T> {
  const held = await acquireRemoteCliSlot(scope)
  try {
    return await fn()
  } finally {
    if (held) releaseRemoteCliSlot(held)
  }
}

/** Test seam: in-flight `/cli/*` count for one host key. */
export function remoteCliInflightForTests(hostKey: string): number {
  const pool = remoteCliPools.get(hostKey)
  return pool ? pool.inflight : 0
}

/** What a `cliFetch` call is, for its timing line (and its lane). */
interface CliCallMeta {
  /** The part after `/cli/`, e.g. `thread`. */
  route: string
  method: string
  /** Give up on one try after this long (`CliTimeoutError`). */
  timeoutMs?: number
  /** The body is bytes: a 2xx is read as a Blob (`text` stays empty); any
   *  other answer is read as text, as always. */
  binary?: boolean
}

/**
 * thread-send-resilience (2026-11-11): a Thread post that carries its own
 * `clientMsgId` gives up after this long. After sleep a pooled socket can
 * sit half-open and a fetch on it may hang for minutes; the outbox tries
 * the SAME id again on reconnect, and the server answers a repeat with the
 * stored post (`duplicate: true`), so giving up is safe. A post without an
 * id (no outbox) never times out here: a retry of it could post twice.
 */
export const THREAD_POST_TIMEOUT_MS = 30_000
let threadPostTimeoutMs = THREAD_POST_TIMEOUT_MS

/** Tests: a short Thread post timeout (null: the default). */
export function setThreadPostTimeoutForTests(ms: number | null): void {
  threadPostTimeoutMs = ms ?? THREAD_POST_TIMEOUT_MS
}

/** A `/cli/*` try that got no answer in time (no status: the caller may try
 *  again; nothing says whether the server acted on it). */
export class CliTimeoutError extends Error {
  constructor(route: string, ms: number) {
    super(`no answer from the server in ${Math.round(ms / 1000)} s (${route})`)
    this.name = 'CliTimeoutError'
  }
}

function hasClientMsgId(body: unknown): boolean {
  return !!body && typeof body === 'object' && typeof (body as { clientMsgId?: unknown }).clientMsgId === 'string'
}

/** Codes that mean "this token is valid but may not do that": a permission
 *  card, an owner-only wall, a Voice & Chat only role, a password rotation.
 *  Never a reason to re-read the local token. */
const NOT_A_DEAD_TOKEN_CODES: ReadonlySet<string> = new Set([
  'owner_only',
  'role_required',
  'password_change_required',
  'permission_required',
])

/**
 * The LOCAL daemon refused the token itself (0.46.15): 401 or 403 with the
 * daemon's token-gate text — `Invalid or missing auth token` (the `/cli/*`
 * gate, `CliResponse::forbidden`) or `invalid or missing token` (the
 * `/events` gate, `require_owner`, per-route `Who::Invalid` arms). The
 * daemon mints a new owner token on every boot, so after a restart under a
 * running app every cached-token request gets this answer.
 *
 * NOT a token refusal: a body with a permission card (`permission`), or one
 * whose code says the token is fine but not allowed (`owner_only`,
 * `role_required`, `password_change_required`). The desktop app always
 * presents the owner token locally, so the owner-gate text alone means the
 * daemon no longer knows that token.
 */
export function isLocalTokenRefusal(status: number, text: string): boolean {
  if (status !== 401 && status !== 403) return false
  if (!/invalid or missing (?:auth )?token/i.test(text)) return false
  let parsed: unknown = null
  try {
    parsed = JSON.parse(text)
  } catch {
    return true
  }
  if (!parsed || typeof parsed !== 'object' || Array.isArray(parsed)) return true
  const obj = parsed as Record<string, unknown>
  if ('permission' in obj) return false
  const codes: unknown[] = [obj.code, obj.error]
  if (obj.error && typeof obj.error === 'object') codes.push((obj.error as Record<string, unknown>).code)
  return !codes.some((c) => typeof c === 'string' && NOT_A_DEAD_TOKEN_CODES.has(c))
}

/** True when `creds` are the local daemon's (desktop loopback), the only
 *  creds `refreshLocalDaemonWs` can re-read. */
function isLocalDesktopCreds(scope: ServerScope, creds: DaemonWsAvailable): boolean {
  return !scope.isRemote && !isWebClient() && creds.host === LOCAL_DAEMON_HOST
}

/**
 * One request to the LOCAL daemon refused at its token gate: re-read the
 * creds (single-flight, `refreshLocalDaemonWs`) and, only when they changed,
 * send it ONCE more through `again`. The token check is the first thing the
 * daemon does with a `/cli/*` request (dispatcher.rs, the `/cli/*` arms):
 * a refused request ran no handler, so sending it again (a POST too) cannot
 * apply it twice. A second refusal is returned as is: no loop.
 */
async function retryAfterLocalTokenRefusal(
  used: DaemonWsAvailable,
  result: CliHttpResult,
  again: () => Promise<CliHttpResult>,
): Promise<CliHttpResult> {
  let fresh: DaemonWsAvailable | null
  try {
    fresh = await refreshLocalDaemonWs(used)
  } catch {
    // The daemon isn't reachable to re-read: keep the refusal.
    return result
  }
  return fresh ? again() : result
}

async function cliFetch(
  scope: ServerScope,
  meta: CliCallMeta,
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
  // Fix 0 (research-remote-thread-load-v1): time the wait for a slot and
  // the network separately (lib/cli-timing, quiet unless asked).
  const t0 = performance.now()
  const at = Date.now()
  const priority = isPriorityCliRoute(meta.route)
  const lane: CliLane = !scope.isRemote ? 'local' : priority ? 'priority' : 'normal'
  const heldRemoteSlot = await acquireRemoteCliSlot(scope, priority)
  const tSlot = performance.now()
  let status: number | null = null
  let lastUrl = ''
  let lastCreds: DaemonWsAvailable | null = null
  try {
    throwIfHostSwitched(scope, startedKey)
    const out = await withConnRetry(async () => {
      throwIfHostSwitched(scope, startedKey)
      const attempt = async (): Promise<CliHttpResult> => {
        throwIfHostSwitched(scope, startedKey)
        const creds = await scope.creds()
        throwIfHostSwitched(scope, startedKey)
        lastCreds = creds
        const { url, init } = build(creds)
        lastUrl = url
        // Hosted web: credentials:include (send/store k2_session) + X-K2-Client.
        // Desktop: withDaemonFetch is a no-op — init is unchanged.
        const timeoutMs = meta.timeoutMs
        let res: Response
        if (timeoutMs) {
          const ac = new AbortController()
          const t = setTimeout(() => ac.abort(), timeoutMs)
          try {
            res = await fetch(url, withDaemonFetch({ ...(init ?? {}), signal: ac.signal }))
          } catch (err) {
            if (ac.signal.aborted) throw new CliTimeoutError(meta.route, timeoutMs)
            throw err
          } finally {
            clearTimeout(t)
          }
        } else {
          res = await fetch(url, withDaemonFetch(init ?? {}))
        }
        throwIfHostSwitched(scope, startedKey)
        if (meta.binary && res.ok) return { res, text: '', blob: await res.blob() }
        return { res, text: await res.text() }
      }
      let result = await attempt()
      throwIfHostSwitched(scope, startedKey)
      const usedCreds = lastCreds as DaemonWsAvailable | null
      if (usedCreds && isLocalDesktopCreds(scope, usedCreds) && isLocalTokenRefusal(result.res.status, result.text)) {
        // 0.46.15: the local daemon restarted under the app and minted a
        // new token. Re-read it and send this request once more.
        result = await retryAfterLocalTokenRefusal(usedCreds, result, attempt)
        throwIfHostSwitched(scope, startedKey)
        return result
      }
      // Re-login and the rotation prompt of the window's own server run
      // here as before (Home M1 / MS77). A pinned scope for another server
      // goes to the connection pool instead (Home M2): one revive through
      // the app-wide login lease, then one replay. No keychain delete and
      // no overlay on that path.
      if (isPossibleAuthFailure(result.res.status, result.text)) {
        const host = scope.isWindowHost() ? windowActiveRemote() : null
        const pinnedAuthRecovery = getPinnedAuthRecovery()
        if (!scope.isWindowHost() && scope.isRemote && pinnedAuthRecovery) {
          const outcome = await pinnedAuthRecovery(scope)
          if (outcome === 'revived') {
            throwIfHostSwitched(scope, startedKey)
            result = await attempt()
          }
        } else if (host) {
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
      } else if (scope.isRemote && isRoleRequired(result.res.status, result.text)) {
        // Voice & Chat only (§7): a role refusal is never a dead login. The
        // pool re-reads whoami for that server so the switcher, Home, the
        // headset and the gate follow a live role change. No replay.
        notePoolRoleRefused(scope.hostKey)
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
    status = out.res.status
    return out
  } catch (err) {
    if (isHostSwitchedError(err)) throw err
    // Compose send and other /cli/* rides this path. Edge 404/CORS throws
    // here. One failed request is EVIDENCE, not a verdict (0.46.10): it
    // asks the gate to probe the host now. A healthy probe drops it (no
    // recovery, no reload, no socket teardown); a failed probe counts it as
    // a strike, so a dead host still goes to recovery at once. HTTP 400 is
    // an application error from parseDaemonResponse — not this catch, and
    // not a probe.
    if (isConnectionLevelError(err) && scope.isRemote && scope.isWindowHost()) {
      logRemotePath('cli-fail', {
        url: lastUrl ? redactRemoteUrl(lastUrl) : undefined,
        class: classifyRemoteFetchError(err),
        err: err instanceof Error ? `${err.name}: ${err.message}` : String(err),
      })
      reportRemoteFailure()
    } else if (isConnectionLevelError(err) && !scope.isRemote && scope.isWindowHost()) {
      // 0.46.18: the window's own local server. Only starts the one probe
      // loop behind the light "busy" note (lib/local-health); never a
      // reload, a remount or a blocking overlay.
      noteLocalTrouble()
    }
    throw err
  } finally {
    if (heldRemoteSlot) releaseRemoteCliSlot(heldRemoteSlot)
    const tEnd = performance.now()
    recordCliTiming({
      at,
      host: safeHostKey(scope),
      route: meta.route,
      method: meta.method,
      lane,
      queuedMs: tSlot - t0,
      networkMs: tEnd - tSlot,
      totalMs: tEnd - t0,
      status,
    })
  }
}

/** The scope's host key for a timing line; never throws (a primary scope
 *  reads the store, a test double may not have one). */
function safeHostKey(scope: ServerScope): string {
  try {
    return scope.hostKey || 'unknown'
  } catch {
    return 'unknown'
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
 * next call to re-read `~/.k2/daemon.{port,token}` from disk.
 */
export async function daemonCliGet<T = unknown>(
  scope: ServerScope,
  route: string,
  params?: Record<string, string | number | boolean | undefined | null>,
  /** `signal`: give the request up (a caller's own deadline), so a request
   *  that never answers frees its slot on that server. */
  opts?: { signal?: AbortSignal },
): Promise<T> {
  const signal = opts?.signal
  const { res, text } = await cliFetch(scope, { route, method: 'GET' }, (creds) => ({
    url: getUrl(creds, route, params),
    init: signal ? { method: 'GET', signal } : { method: 'GET' },
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
  const { res, text } = await cliFetch(scope, { route, method: 'GET' }, (creds) => ({
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
 *
 * `opts.timeoutMs`: give one try up after that long (`CliTimeoutError`),
 * for a read whose caller must get an answer (a Garden widget's polled
 * read, `zen-custom-store`). Never for a write a retry could repeat.
 */
export async function daemonCliPost<T = unknown>(
  scope: ServerScope,
  route: string,
  body?: unknown,
  opts?: { timeoutMs?: number },
): Promise<T> {
  assertServerScope(scope, 'daemonCliPost')
  // Home M4: a view-only room never writes to its server.
  assertScopeMayPost(scope, route)
  const timeoutMs = route === 'thread/post' && hasClientMsgId(body) ? threadPostTimeoutMs : opts?.timeoutMs
  const { res, text } = await cliFetch(scope, { route, method: 'POST', timeoutMs }, (creds) => ({
    url: getUrl(creds, route),
    init: {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: body !== undefined ? JSON.stringify(body) : undefined,
    },
  }))
  const parsed = await parseDaemonResponse<T>(res, text)
  // Home groups: a Thread post or card answer the server took is the
  // person sending to that agent (`lib/user-send-bus.ts`).
  noteUserPost(route, scope.hostKey, body)
  return parsed
}

/**
 * POST /cli/<route>?<params>[&token=] with raw `bytes` as the body
 * (`application/octet-stream`), for a route that takes a file's bytes
 * (Home chat's `uploads/data`). The answer is JSON, as for
 * {@link daemonCliPost}; the view-only room check runs the same.
 */
export async function daemonCliPostBytes<T = unknown>(
  scope: ServerScope,
  route: string,
  params: Record<string, string | number | boolean | undefined | null>,
  bytes: Uint8Array,
): Promise<T> {
  assertServerScope(scope, 'daemonCliPostBytes')
  assertScopeMayPost(scope, route)
  const { res, text } = await cliFetch(scope, { route, method: 'POST' }, (creds) => ({
    url: getUrl(creds, route, params),
    init: { method: 'POST', headers: { 'Content-Type': 'application/octet-stream' }, body: bytes as unknown as BodyInit },
  }))
  return parseDaemonResponse<T>(res, text)
}

/**
 * GET /cli/<route>?<params>[&token=] whose body is bytes (a Home chat
 * file): the Blob on a 2xx; anything else throws as {@link daemonCliGet}.
 */
export async function daemonCliGetBlob(
  scope: ServerScope,
  route: string,
  params?: Record<string, string | number | boolean | undefined | null>,
  opts?: { signal?: AbortSignal },
): Promise<Blob> {
  const signal = opts?.signal
  const { res, text, blob } = await cliFetch(scope, { route, method: 'GET', binary: true }, (creds) => ({
    url: getUrl(creds, route, params),
    init: signal ? { method: 'GET', signal } : { method: 'GET' },
  }))
  if (!res.ok || !blob) throw httpError(res, text)
  return blob
}

/**
 * POST /cli/<route>?<params>[&token=] with an empty body — the 0.44.4
 * replacement for GET-shaped verbs (`terminal/write`, `heartbeat/enable`,
 * `agents/lock`, …). The daemon now answers GET with 405 on any route
 * that changes state; the params keep riding the query string exactly as
 * they did on the GET, so the handler sees the same map.
 *
 * Unlike {@link daemonCliPost} this does NOT run the view-only / room write
 * check: these calls used to be GETs, and call sites that write from a room
 * already run `assertScopeMayWrite` themselves (unchanged).
 *
 * Older servers (pre-0.44.4) only route these paths as GET and answer the
 * POST with their top-level `405 method not allowed for this route` before
 * any handler runs, so replaying the same request as GET is side-effect
 * safe; {@link isPreFourFourFourMethod405} detects exactly that answer.
 */
export async function daemonCliPostQuery<T = unknown>(
  scope: ServerScope,
  route: string,
  params?: Record<string, string | number | boolean | undefined | null>,
): Promise<T> {
  assertServerScope(scope, 'daemonCliPostQuery')
  const { res, text } = await postQueryWithOldServerFallback(scope, route, params)
  return parseDaemonResponse<T>(res, text)
}

/** Text-body twin of {@link daemonCliPostQuery} (mirrors `daemonCliGetText`). */
export async function daemonCliPostQueryText(
  scope: ServerScope,
  route: string,
  params?: Record<string, string | number | boolean | undefined | null>,
): Promise<string> {
  assertServerScope(scope, 'daemonCliPostQueryText')
  const { res, text } = await postQueryWithOldServerFallback(scope, route, params)
  return parseDaemonText(res, text)
}

/** True for the pre-0.44.4 daemon's top-level method guard (route not on
 *  its POST allowlist). A 0.44.4+ daemon never sends this for a POST to a
 *  route that takes POST. */
export function isPreFourFourFourMethod405(status: number, text: string): boolean {
  return status === 405 && text.includes('method not allowed for this route')
}

async function postQueryWithOldServerFallback(
  scope: ServerScope,
  route: string,
  params?: Record<string, string | number | boolean | undefined | null>,
): Promise<CliHttpResult> {
  const posted = await cliFetch(scope, { route, method: 'POST' }, (creds) => ({
    url: getUrl(creds, route, params),
    init: { method: 'POST' },
  }))
  if (!isPreFourFourFourMethod405(posted.res.status, posted.text)) return posted
  return cliFetch(scope, { route, method: 'GET' }, (creds) => ({
    url: getUrl(creds, route, params),
    init: { method: 'GET' },
  }))
}

/**
 * POST /cli/<route> against the LOCAL daemon, regardless of the active
 * host. The pull half of clone-to ("Clone to this computer") runs its
 * unpack + cleanup on the local daemon WHILE the remote host stays
 * active, so it can't go through the active-host `daemonCliPost`.
 * Same connection-retry + response parsing as `daemonCliPost`; the
 * remote-session revival step is intentionally absent (revival is a
 * remote-host recovery). A local daemon restart mints a new token: a token
 * refusal re-reads it and resends once (`localCliFetch`).
 */
export async function localDaemonCliPost<T = unknown>(
  route: string,
  body?: unknown,
): Promise<T> {
  // Always the LOCAL loopback daemon (desktop clone-to pull). Keep the
  // classic `?token=` path — this is never the hosted-web same-origin
  // surface (web never activates the local Tauri daemon).
  const { res, text } = await localCliFetch((creds) =>
    fetch(`${daemonHttpBase(creds)}/cli/${route}?token=${creds.token}`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: body !== undefined ? JSON.stringify(body) : undefined,
    }),
  )
  return parseDaemonResponse<T>(res, text)
}

/**
 * GET /cli/<route>?<params> against the LOCAL daemon, regardless of the
 * active host — the read twin of {@link localDaemonCliPost}. "Clone to this
 * computer" asks this computer's vault for its carry recipient while the
 * remote host stays active (0.45.2).
 */
export async function localDaemonCliGet<T = unknown>(
  route: string,
  params?: Record<string, string | number | boolean | undefined | null>,
): Promise<T> {
  const { res, text } = await localCliFetch((creds) => {
    const search = new URLSearchParams()
    for (const [k, v] of Object.entries(params ?? {})) {
      if (v !== undefined && v !== null) search.set(k, String(v))
    }
    search.set('token', creds.token)
    return fetch(`${daemonHttpBase(creds)}/cli/${route}?${search.toString()}`, { method: 'GET' })
  })
  return parseDaemonResponse<T>(res, text)
}

/** One request to the LOCAL daemon (`localDaemonCli*`): the connection
 *  retry, plus one re-read + resend after a token refusal (a daemon
 *  restart), as `cliFetch` does for a local scope. */
function localCliFetch(send: (creds: DaemonWsAvailable) => Promise<Response>): Promise<CliHttpResult> {
  return withConnRetry(async () => {
    let used: DaemonWsAvailable | null = null
    const once = async (): Promise<CliHttpResult> => {
      const creds = await getLocalDaemonWs()
      used = creds
      const res = await send(creds)
      return { res, text: await res.text() }
    }
    const result = await once()
    const usedCreds = used as DaemonWsAvailable | null
    if (usedCreds && !isWebClient() && isLocalTokenRefusal(result.res.status, result.text)) {
      return retryAfterLocalTokenRefusal(usedCreds, result, once)
    }
    return result
  })
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
    throw httpError(res, text)
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
    throw httpError(res, text)
  }
  return text
}

/** The `{"error":"...","code":"..."}`-aware error for a non-2xx answer,
 *  carrying the status (Usage v2 B25: branch on it, not on the text). */
function httpError(res: Response, text: string): DaemonHttpError {
  let msg = text
  let code: string | null = null
  let details: Record<string, unknown> | null = null
  let body: unknown = null
  try {
    const parsed = JSON.parse(text)
    body = parsed
    if (parsed && typeof parsed.error === 'string') msg = parsed.error
    if (parsed && typeof parsed.code === 'string' && parsed.code !== '') code = parsed.code
    // The `{ok:false, error:{code, hint}}` shape (notify, homes-sync routes).
    if (parsed && parsed.error && typeof parsed.error === 'object') {
      if (typeof parsed.error.hint === 'string' && parsed.error.hint !== '') msg = parsed.error.hint
      if (typeof parsed.error.code === 'string' && parsed.error.code !== '') code = code ?? parsed.error.code
      if (!Array.isArray(parsed.error)) details = parsed.error as Record<string, unknown>
    }
  } catch {
    /* fall through with raw text */
  }
  return new DaemonHttpError(msg || `daemon ${route(res.url)} ${res.status}`, res.status, code, details, body)
}

function route(url: string): string {
  try {
    const u = new URL(url)
    return u.pathname
  } catch {
    return url
  }
}
