// The remote-host recovery state machine (owner contract, 0.40.x):
//
//   "If we're going to tell users it auto-reconnects after a remote reboot,
//    it must actually auto-reconnect. If it fails, send them to the log-in
//    screen. And we need some simple way to know the computer is still
//    booting back up."
//
// A disrupted ACTIVE remote connection resolves into exactly THREE
// user-visible states (plus the healthy baseline), and the UI always shows
// which one it's in:
//
//   1. 'reconnecting'      — host unreachable OR reachable-but-not-ready
//                            (the daemon's /boot-status readiness handshake
//                            reports a pre-'ready' phase — the "still
//                            booting back up" affordance). NO login prompt
//                            here: auth is unknowable until the host is up.
//   2. 'reauthenticating'  — host is up + ready but our session token was
//                            rejected (the daemon restart wiped its
//                            in-memory connect-sessions). The single-flight
//                            silent re-login (lib/remote-session.ts) is
//                            doing its thing; usually resolves in one
//                            round-trip with no user action.
//   3. 'signin-required'   — the re-login itself was rejected (stored
//                            credentials no longer valid) → route the user
//                            to the RemoteSignIn screen. The ONLY state
//                            that asks the user for anything.
//
// This module is the PURE core: inputs (boot-status result / auth signal) →
// state, and state → poll cadence. ConnectionGate + lib/remote-session feed
// it and commit the result to the connect-host store (`recovery`), which the
// K2 Connect panel, the top-bar host indicator, and the in-app banner render.

import type { ReviveOutcome } from '@/lib/remote-session'

/** The healthy baseline + the three recovery states. `bootPhase` carries the
 *  host's /boot-status phase when it answered but isn't 'ready' (e.g.
 *  'migrating' during an update) so the UI can say WHY it's waiting. */
export type RemoteRecoveryState =
  | { kind: 'connected' }
  | { kind: 'reconnecting'; bootPhase: string | null }
  | { kind: 'reauthenticating' }
  | { kind: 'signin-required' }

/** What the last /boot-status poll said. `reachable: false` = network-level
 *  failure (down / mid-restart / tunnel gap); otherwise the daemon's
 *  readiness `phase` ('ready' | 'starting' | 'migrating' | …). */
export type BootStatusInput =
  | { reachable: false }
  | { reachable: true; phase: string }

/** The auth signal for the current session token:
 *    - 'ok'              → session verified alive (or nothing suggests otherwise)
 *    - 'unknown'         → probe blipped — NOT evidence of a dead session
 *    - 'rejected'        → 401/403-classified; silent re-login pending/underway
 *    - 'reauth-failed'   → the re-login itself was auth-rejected (or there is
 *                          no stored credential to retry with) — user needed */
export type AuthSignal = 'ok' | 'unknown' | 'rejected' | 'reauth-failed'

/**
 * The reducer. Boot state DOMINATES auth state: while the host is down or
 * still booting, the session can't be evaluated and the user must never be
 * bounced to a login screen for a machine that's mid-reboot.
 */
export function deriveRecovery(inputs: {
  bootStatus: BootStatusInput
  auth: AuthSignal
}): RemoteRecoveryState {
  const { bootStatus, auth } = inputs
  if (!bootStatus.reachable) return { kind: 'reconnecting', bootPhase: null }
  if (bootStatus.phase !== 'ready') {
    return { kind: 'reconnecting', bootPhase: bootStatus.phase }
  }
  switch (auth) {
    case 'ok':
    case 'unknown': // a blip never tears down / alarms a working session
      return { kind: 'connected' }
    case 'rejected':
      return { kind: 'reauthenticating' }
    case 'reauth-failed':
      return { kind: 'signin-required' }
  }
}

/** Fold a revival attempt's outcome into an {@link AuthSignal}:
 *    - a fresh token ('revived') or a confirmed-alive session ('still-valid')
 *      → 'ok'
 *    - 'signin-required' (no/rejected stored password) → 'reauth-failed'
 *    - 'unreachable' / 'cooldown' → 'rejected' (re-auth still pending; the
 *      next health tick re-evaluates — and flips to 'reconnecting' if the
 *      host itself went down)
 *    - 'not-applicable' → 'ok' (local / unknown host: nothing to recover) */
export function authSignalFromRevive(outcome: ReviveOutcome): AuthSignal {
  switch (outcome) {
    case 'revived':
    case 'still-valid':
    case 'not-applicable':
      return 'ok'
    case 'signin-required':
      return 'reauth-failed'
    case 'unreachable':
    case 'cooldown':
      return 'rejected'
  }
}

/** Steady-state health cadence for a connected / re-authenticating remote
 *  (matches ConnectionGate's REMOTE_HEALTH_POLL_MS). */
export const RECOVERY_HEALTH_POLL_MS = 4000
/** Backoff cap while a remote is confirmed down/booting (matches
 *  ConnectionGate's REMOTE_RECONNECT_MAX_MS). */
export const RECOVERY_RECONNECT_MAX_MS = 8000

/** The user-facing one-liner for each recovery state — shared by the gate's
 *  RecoveryBanner and the Connections panel's active-host tile so the two
 *  surfaces can never disagree about what state the connection is in.
 *    - reconnecting     → "restarting/reconnecting" (+ boot phase when the
 *                         host answered /boot-status pre-'ready' — the
 *                         "still booting back up" affordance)
 *    - reauthenticating → the silent re-login is in flight
 *    - signin-required  → the ONLY state that asks the user */
export function recoveryStatusText(label: string, recovery: RemoteRecoveryState): string {
  switch (recovery.kind) {
    case 'connected':
      return ''
    case 'reconnecting':
      return recovery.bootPhase !== null
        ? `${label} is restarting (${recovery.bootPhase})… reconnecting automatically`
        : `Reconnecting to ${label}… retrying automatically`
    case 'reauthenticating':
      return `Re-authenticating with ${label}…`
    case 'signin-required':
      return `${label} needs you to sign in again.`
  }
}

/**
 * The retry schedule, per state: while 'reconnecting' ease off exponentially
 * (500ms → 8s cap over `attempt` failed polls) so a down host isn't
 * hammered; otherwise keep the normal 4s health cadence so recovery — and
 * staleness — are noticed promptly. 'signin-required' polling is governed by
 * the gate parking on the tokenless host, not by this schedule.
 */
export function recoveryPollMs(state: RemoteRecoveryState, attempt: number): number {
  if (state.kind === 'reconnecting') {
    return Math.min(RECOVERY_RECONNECT_MAX_MS, 500 * 2 ** Math.min(Math.max(attempt, 0), 5))
  }
  return RECOVERY_HEALTH_POLL_MS
}
