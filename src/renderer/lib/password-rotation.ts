// Forced password rotation for connect-users (PRD connect-login-edge-only
// §4.2 W1–W4): the daemon-facing plumbing behind the "Set a new password"
// step, kept React-free so it is unit-testable and shared by every sign-in
// entry point (RemoteSignIn re-auth, the switcher's pickHost auto-login,
// Settings → Connections add-server).
//
// Contract with the daemon (connect_users_routes.rs, read-only here):
//   - `POST /cli/auth/login` → `{token, username, expiresAt, mustChangePassword}`.
//     A `mustChangePassword: true` session is RESTRICTED: only whoami,
//     change-password, logout and GET users/policy answer; every other
//     session-authed route 403s `{"error":"password_change_required"}` (W2).
//   - `GET /cli/users/policy` → `{minLength, requireSpecial, requireNumber,
//     requireUppercase}` (authorised: the restricted session may read it).
//   - `POST /cli/auth/change-password` `{currentPassword, newPassword}` →
//     200 `{"success":true}` and EVERY session of that user is revoked
//     (including the one that made the call) — the caller must log in again
//     with the new password; 400 `{"error":"<first unmet policy rule>"}`;
//     401 `{"error":"unauthorized"}` for a wrong current password OR a
//     lockout (deliberately indistinguishable).
//
// All three calls go to the host's OWN base URL (`https://<label>.k2.dev`,
// LAN, self-host…) — they are session-token authed and are NOT part of the
// edge-only login gate. Only the login POST itself moves host (login-url.ts).

import { withCliTokenQuery, withDaemonFetch } from '@/web/session-token'
import { hostBaseUrl, type ConnectHost } from '@/stores/connect-host'

/** Wire shape of `GET /cli/users/policy` (camelCase, all present). */
export interface PasswordPolicy {
  minLength: number
  requireSpecial: boolean
  requireNumber: boolean
  requireUppercase: boolean
}

/** The daemon's built-in default; used only when the policy fetch fails
 *  (the daemon re-validates on change-password regardless). */
export const DEFAULT_PASSWORD_POLICY: PasswordPolicy = {
  minLength: 8,
  requireSpecial: false,
  requireNumber: false,
  requireUppercase: false,
}

/** W3 copy — one place, used by every entry point's rotation step. */
export const ROTATION_COPY = {
  title: 'Set a new password',
  lead: 'Your server admin gave you a temporary password. Choose a new one to continue.',
  currentLabel: 'Temporary password',
  newLabel: 'New password',
  confirmLabel: 'Confirm new password',
  submit: 'Set password and continue',
  mismatch: 'New passwords do not match.',
  missingCurrent: 'Enter the temporary password you were given.',
  badCurrent: 'Temporary password incorrect, or too many attempts. Try again later.',
} as const

/** Per-request timeout for the policy read + change POST. */
const ROTATION_TIMEOUT_MS = 8000

/** W2 classifier: the daemon's restricted-session refusal on the data plane.
 *  `body` may be the raw response text or an already-extracted error field
 *  (daemon-cli surfaces `error` verbatim), so match the bare token too. */
export function isPasswordChangeRequired(status: number, body: string): boolean {
  if (status !== 403) return false
  return /password_change_required/.test(body)
}

/** Human hint line for the active policy ("At least 8 chars · number"). */
export function policyHint(policy: PasswordPolicy): string {
  const parts = [`At least ${policy.minLength} characters`]
  if (policy.requireUppercase) parts.push('an uppercase letter')
  if (policy.requireNumber) parts.push('a number')
  if (policy.requireSpecial) parts.push('a special character')
  return parts.join(' · ')
}

/** Client-side mirror of `connect_users::validate_policy` — the first unmet
 *  rule as a message, or null when the password satisfies the policy. The
 *  daemon is the authority; this only saves a round trip. */
export function validateNewPassword(pw: string, policy: PasswordPolicy): string | null {
  if (pw.length < policy.minLength) {
    return `Password must be at least ${policy.minLength} characters.`
  }
  if (policy.requireUppercase && !/[A-Z]/.test(pw)) {
    return 'Password must include an uppercase letter.'
  }
  if (policy.requireNumber && !/[0-9]/.test(pw)) {
    return 'Password must include a number.'
  }
  if (policy.requireSpecial && !/[^A-Za-z0-9]/.test(pw)) {
    return 'Password must include a special character.'
  }
  return null
}

function parsePolicy(raw: unknown): PasswordPolicy | null {
  if (!raw || typeof raw !== 'object') return null
  const r = raw as Record<string, unknown>
  if (typeof r.minLength !== 'number') return null
  return {
    minLength: r.minLength,
    requireSpecial: r.requireSpecial === true,
    requireNumber: r.requireNumber === true,
    requireUppercase: r.requireUppercase === true,
  }
}

/**
 * `GET <host>/cli/users/policy` with the host's (restricted) session token.
 * Desktop appends `?token=`; hosted web also sends the `k2_session` cookie
 * via `withDaemonFetch`. Falls back to {@link DEFAULT_PASSWORD_POLICY} when
 * the read fails — the daemon re-validates on submit, so a stale hint is
 * the worst case.
 */
export async function fetchPasswordPolicy(host: ConnectHost): Promise<PasswordPolicy> {
  try {
    const url = withCliTokenQuery(`${hostBaseUrl(host)}/cli/users/policy`, host.token)
    const res = await fetch(
      url,
      withDaemonFetch({ method: 'GET', signal: AbortSignal.timeout(ROTATION_TIMEOUT_MS) }),
    )
    if (!res.ok) {
      console.warn(`[password-rotation] policy read ${res.status}; using default policy`)
      return DEFAULT_PASSWORD_POLICY
    }
    const parsed = parsePolicy(await res.json())
    if (!parsed) {
      console.warn('[password-rotation] policy body malformed; using default policy')
      return DEFAULT_PASSWORD_POLICY
    }
    return parsed
  } catch (err) {
    console.warn('[password-rotation] policy read failed; using default policy', err)
    return DEFAULT_PASSWORD_POLICY
  }
}

/** Outcome of {@link changeHostPassword}. `kind` lets the step branch on
 *  copy without parsing messages:
 *    - 'weak'        → 400: the daemon's first unmet policy rule (message verbatim)
 *    - 'bad-current' → 401: wrong temporary password OR lockout
 *    - 'unreachable' → network-level failure; nothing was changed
 *    - 'server'      → any other non-2xx */
export type ChangePasswordResult =
  | { ok: true }
  | { ok: false; kind: 'weak' | 'bad-current' | 'unreachable' | 'server'; reason: string }

/**
 * `POST <host>/cli/auth/change-password` `{currentPassword, newPassword}`
 * as the host's current (restricted) session. On success the daemon has
 * revoked every session of this user — including `host.token` — so the
 * caller MUST re-login with `newPassword` before the host is usable.
 */
export async function changeHostPassword(
  host: ConnectHost,
  currentPassword: string,
  newPassword: string,
): Promise<ChangePasswordResult> {
  const url = withCliTokenQuery(`${hostBaseUrl(host)}/cli/auth/change-password`, host.token)
  let res: Response
  try {
    res = await fetch(
      url,
      withDaemonFetch({
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ currentPassword, newPassword }),
        signal: AbortSignal.timeout(ROTATION_TIMEOUT_MS),
      }),
    )
  } catch {
    return {
      ok: false,
      kind: 'unreachable',
      reason: `Couldn't reach ${host.hostname} to change the password. Try again.`,
    }
  }
  if (res.ok) return { ok: true }
  if (res.status === 400) {
    // The daemon returns the active policy's first unmet requirement.
    let reason = 'New password does not meet the requirements.'
    try {
      const body = (await res.json()) as { error?: unknown }
      if (typeof body.error === 'string' && body.error.length > 0) reason = body.error
    } catch {
      /* non-JSON 400 — keep the generic copy */
    }
    return { ok: false, kind: 'weak', reason }
  }
  if (res.status === 401) {
    return { ok: false, kind: 'bad-current', reason: ROTATION_COPY.badCurrent }
  }
  return { ok: false, kind: 'server', reason: `Server returned ${res.status} while changing the password.` }
}
