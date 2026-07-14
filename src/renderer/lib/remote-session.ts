// Runtime re-auth for stale remote-host sessions.
//
// THE BUG THIS FIXES
// Connect-user sessions are IN-MEMORY in the daemon (connect_users.rs), so a
// remote restart/update WIPES them while the client still holds the old
// session token. `/boot-status` is PUBLIC, so ConnectionGate's health-poll
// keeps reporting 'connected' — but every authed `/cli/*` call now returns
// **403** `{"error":"Invalid or missing auth token"}` (cli_response.rs;
// NOT a 401), and a WS upgrade rejection surfaces to the renderer as a
// generic close, indistinguishable from a network drop. Nothing at runtime
// re-ran the login flow, so every retry loop spun forever on the dead token
// and only a full relaunch (whose first-connect whoami probe + RemoteSignIn
// auto-login re-mints the session) recovered.
//
// THE FIX
// `reviveRemoteSession` re-runs, at runtime, the SAME session-mint flow boot
// uses: confirm the session is dead via `GET /cli/auth/whoami` (the daemon's
// token-gate 403 body is byte-identical to a role-denial 403, so only whoami
// can tell "stale token" from "not permitted"), then re-login with the
// keychain-remembered password via `loginToHost` — which commits the fresh
// token into the connect-host store, so `getDaemonWs()` consumers (fetch
// helpers, WS reconnect loops) pick it up on their next attempt.
//
// Guarantees:
//   - SINGLE-FLIGHT per host: concurrent 401/403s from many panes share one
//     re-login; everyone awaits the same promise.
//   - BACKOFF between failed attempts (immediate first, then widening to a
//     30s cap) so a plaintext login is never hammered in a loop.
//   - If the stored password itself is rejected (or none is remembered), the
//     session is expired the classic way — token dropped + RemoteSignIn
//     raised — surfacing "sign in again" instead of retrying forever.

import {
  useConnectHostStore,
  loginToHost,
  resolvePassword,
  type ConnectHost,
} from '@/stores/connect-host'
import { invalidateDaemonWs } from '@/kessel/daemon-ws'
import { deriveRecovery, authSignalFromRevive, type RemoteRecoveryState } from '@/lib/remote-recovery'
import { jittered } from '@/lib/backoff'

/**
 * Classify an HTTP response as a POSSIBLE stale-session rejection — distinct
 * from network errors (which `isConnectionLevelError` owns) and from ordinary
 * app errors (4xx/5xx with other bodies).
 *
 *   - 401 → always auth (the daemon's connect-user routes speak 401).
 *   - 403 → auth ONLY when the body carries the daemon's token-gate message
 *     ("Invalid or missing auth token" from the /cli/* dispatch gate,
 *     "invalid or missing token" from require_owner_or_admin / WS upgrades).
 *     A role-denial 403 unfortunately reuses the same copy, which is why a
 *     positive match still goes through the whoami probe in
 *     {@link reviveRemoteSession} before anything is dropped.
 *
 * `body` may be the raw response text or an already-extracted error message.
 */
export function isPossibleAuthFailure(status: number, body: string): boolean {
  if (status === 401) return true
  if (status !== 403) return false
  return /invalid or missing (?:auth )?token/i.test(body)
}

/** Outcome of a revival attempt. Callers retry their request ONLY on
 *  'revived' (the token actually changed). */
export type ReviveOutcome =
  /** A fresh session was minted and committed to the store — retry now. */
  | 'revived'
  /** whoami says the current session is ALIVE — the 403 that triggered this
   *  was a role denial (or a race with an earlier revival), not staleness. */
  | 'still-valid'
  /** No remembered password, or it was rejected — the token was dropped and
   *  RemoteSignIn raised (when this is the active host). The user must act. */
  | 'signin-required'
  /** Network-level failure probing/logging in — the token was NOT touched;
   *  the caller's own reconnect backoff keeps going. */
  | 'unreachable'
  /** A recent attempt failed and the backoff window hasn't elapsed. */
  | 'cooldown'
  /** 'local' or an unknown host id — nothing to revive. */
  | 'not-applicable'

/** Widening pauses REQUIRED between successive failed revival attempts for
 *  one host: first retry after 1s, then 4s / 15s, capped at 30s. Exported
 *  for the unit tests. */
export const REVIVE_BACKOFF_MS = [1000, 4000, 15000, 30000]

/** Per-attempt timeout for the whoami staleness probe (matches
 *  ConnectionGate's REMOTE_WHOAMI_TIMEOUT_MS). */
const WHOAMI_TIMEOUT_MS = 4000

// Single-flight + backoff bookkeeping, keyed by host id. `cooldownMs` is the
// JITTERED gate for the next attempt, computed once when the failure is
// recorded (0.40.48: fixed backoffs across many clients/loops re-align into
// a synchronized retry storm after a remote reboot; jittering at record time
// keeps the pure `reviveBackoffMs` schedule deterministic for tests while
// decorrelating live retries).
const inflight = new Map<string, Promise<ReviveOutcome>>()
const failures = new Map<string, { count: number; lastAt: number; cooldownMs: number }>()

/** `<scheme>://<host>[:<port>]` — same rule as connect-host's hostBaseUrl /
 *  host-ops' remoteCreds (secure+443 omits the port). Inlined so this lib
 *  pulls in no Settings-section code. */
function hostBase(host: Pick<ConnectHost, 'hostname' | 'port' | 'secure'>): string {
  const scheme = host.secure ? 'https' : 'http'
  const authority =
    host.secure && host.port === 443 ? host.hostname : `${host.hostname}:${host.port}`
  return `${scheme}://${authority}`
}

/** Probe a SPECIFIC host's session via `GET /cli/auth/whoami?token=…`.
 *  Same verdict rule as ConnectionGate's first-connect probe: 401/403 is an
 *  authoritative 'dead'; 2xx 'alive'; anything else (timeout, 5xx) is a
 *  blip — 'unknown', never grounds to drop a token. */
async function probeSession(host: ConnectHost): Promise<'alive' | 'dead' | 'unknown'> {
  try {
    const res = await fetch(
      `${hostBase(host)}/cli/auth/whoami?token=${encodeURIComponent(host.token)}`,
      { method: 'GET', signal: AbortSignal.timeout(WHOAMI_TIMEOUT_MS) },
    )
    if (res.status === 401 || res.status === 403) return 'dead'
    if (res.ok) return 'alive'
    return 'unknown'
  } catch {
    return 'unknown'
  }
}

/** The pure backoff rule: given how many attempts have FAILED in a row, how
 *  long after the last one is the next allowed? Exported for tests. */
export function reviveBackoffMs(failedCount: number): number {
  if (failedCount <= 0) return 0
  return REVIVE_BACKOFF_MS[Math.min(failedCount, REVIVE_BACKOFF_MS.length) - 1]
}

/**
 * Revive a remote host's connect-session after an auth-classified failure.
 *
 * Flow: whoami-confirm the session is dead → resolve the keychain-remembered
 * password → `loginToHost` (which commits the fresh token via setHostToken +
 * rememberToken, so every `getDaemonWs()` consumer sees it immediately) →
 * `invalidateDaemonWs()` for good measure. If no password is remembered or
 * the login is auth-rejected, the session is expired (token dropped,
 * RemoteSignIn raised for the active host) — the one case that needs the user.
 *
 * `force` bypasses the failure backoff (a deliberate user gesture — the
 * switcher/tile reconnect) but still joins an in-flight attempt.
 */
export function reviveRemoteSession(
  hostId: string,
  opts?: { force?: boolean },
): Promise<ReviveOutcome> {
  const existing = inflight.get(hostId)
  if (existing) return existing
  const failed = failures.get(hostId)
  if (!opts?.force && failed && Date.now() - failed.lastAt < failed.cooldownMs) {
    return Promise.resolve('cooldown')
  }
  // Recovery surface (owner contract, lib/remote-recovery.ts): while THIS
  // host is the active remote, a real revival attempt is the visible
  // 'reauthenticating' state, and its outcome folds back through the pure
  // reducer ('revived' → connected, 'signin-required' → signin-required,
  // 'unreachable'/'cooldown' → still reauthenticating; the gate's next
  // health tick flips to 'reconnecting' if the host itself went down).
  // A hostId not in the address book skips the paint (nothing to revive).
  if (useConnectHostStore.getState().hosts.some((h) => h.id === hostId)) {
    setRecoveryIfActive(hostId, { kind: 'reauthenticating' })
  }
  const attempt = doRevive(hostId)
    .then((outcome) => {
      if (outcome === 'revived' || outcome === 'still-valid') {
        failures.delete(hostId)
      } else if (outcome !== 'not-applicable') {
        const prev = failures.get(hostId)
        const count = (prev?.count ?? 0) + 1
        failures.set(hostId, {
          count,
          lastAt: Date.now(),
          // Jitter the cooldown once, at record time (never above the pure
          // schedule's value, so the 30s cap holds).
          cooldownMs: jittered(reviveBackoffMs(count)),
        })
      }
      if (outcome !== 'not-applicable') {
        setRecoveryIfActive(
          hostId,
          deriveRecovery({
            // The rejection that triggered revival came FROM the host, so it
            // is reachable+ready as far as this attempt knows; the gate's
            // boot-status poll owns the down/booting transitions.
            bootStatus: { reachable: true, phase: 'ready' },
            auth: authSignalFromRevive(outcome),
          }),
        )
      }
      return outcome
    })
    .finally(() => {
      inflight.delete(hostId)
    })
  inflight.set(hostId, attempt)
  return attempt
}

/** Commit a recovery state ONLY when `hostId` is the active remote — a
 *  background tile's revival must never repaint the active connection's
 *  indicator. */
function setRecoveryIfActive(hostId: string, recovery: RemoteRecoveryState): void {
  const store = useConnectHostStore.getState()
  const active = store.activeHost
  if (active !== 'local' && active.id === hostId) store.setRecovery(recovery)
}

async function doRevive(hostId: string): Promise<ReviveOutcome> {
  const store = useConnectHostStore.getState()
  const host = store.hosts.find((h) => h.id === hostId)
  if (!host) return 'not-applicable'

  // Confirm staleness before touching anything: the daemon's token-gate 403
  // body is identical to a role-denial 403, and racing callers may arrive
  // after an earlier revival already fixed the token.
  if (host.token && host.token.length > 0) {
    const probe = await probeSession(host)
    if (probe === 'alive') return 'still-valid'
    if (probe === 'unknown') return 'unreachable'
  }

  // Legacy raw-token host (no username): there is no login flow to re-run —
  // drop the dead token and surface sign-in.
  if (!host.username || host.username.length === 0) {
    expireAndClear(hostId)
    return 'signin-required'
  }

  const password = await resolvePassword(host.id)
  if (!password) {
    expireAndClear(hostId)
    return 'signin-required'
  }

  // The SAME mint flow boot uses (loginToHost already rides withRemoteRetry
  // for the dead-pooled-socket eviction and commits the token + keychain
  // cache on success). The password never leaves this scope.
  const result = await loginToHost(host, password)
  if (result.ok) {
    invalidateDaemonWs()
    return 'revived'
  }
  if (result.kind === 'auth') {
    // The remembered password itself is rejected — only the user can fix
    // this. Expire so RemoteSignIn surfaces instead of retrying forever.
    expireAndClear(hostId)
    return 'signin-required'
  }
  return 'unreachable'
}

/** Drop the dead session token everywhere: `expireSession` raises the
 *  full-screen RemoteSignIn when `hostId` is the ACTIVE remote (and no-ops
 *  otherwise); `clearHostToken` flips a non-active host's Connections tile
 *  to signed-out. The remembered password is kept in both paths. */
function expireAndClear(hostId: string): void {
  const store = useConnectHostStore.getState()
  store.expireSession(hostId)
  store.clearHostToken(hostId)
}

/** Test-only: clear the single-flight + backoff maps. */
export function __resetRemoteSessionForTests(): void {
  inflight.clear()
  failures.clear()
}
