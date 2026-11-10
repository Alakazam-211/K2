// The viewer's role on the window's REMOTE server, for Settings → General's
// Restart and Update rows. Pure: no React, no stores (node-testable); the
// hook that runs it is `useActiveHostRole.ts`.
//
// Rosson 2026-11-05: "it often leaves the button greyed out and says you
// need to be an owner or an admin, even though I am logged in as an owner
// or an admin. It's a bit chronic." The rows read `auth/whoami` ONCE per
// host id, at mount. Any failure of that one read — the request gate
// refusing it while the connection was still recovering (RecoveringError),
// a token not in memory yet after a boot or a server switch, a blip — left
// the role `null` for as long as the page stayed open, and a `null` role
// rendered exactly like a Member: a disabled button titled "Only the host
// owner or an admin can update this host". Here an unread role is never a
// refusal: it is "checking" (and read again), and only a role the host
// actually reported refuses.

export type HostRole = 'owner' | 'admin' | 'member'

/** What the row knows about the viewer's role on the active remote. */
export type HostRoleState =
  /** Not read yet (first read in flight, or waiting for the connection). */
  | { kind: 'checking' }
  /** The host said so. */
  | { kind: 'known'; role: HostRole }
  /** The host refused the login itself (whoami 401/403 after the request
   *  layer's own re-login): the person must sign in again. */
  | { kind: 'signed-out'; reason: string }
  /** Whoami failed for another reason (network, 5xx, an odd reply). The
   *  row keeps reading it; the action stays allowed (the host decides). */
  | { kind: 'unreadable'; reason: string }

/** Read whoami's answer. `role` wins; an older host without one says
 *  `owner: true` for the owner. Null = not a whoami answer. */
export function roleFromWhoami(body: unknown): HostRole | null {
  if (typeof body !== 'object' || body === null) return null
  const b = body as { role?: unknown; owner?: unknown }
  if (b.role === 'owner' || b.role === 'admin' || b.role === 'member') return b.role
  if (b.owner === true) return 'owner'
  // A role this app doesn't know is below the update/restart tier.
  if (typeof b.role === 'string' && b.role.trim() !== '') return 'member'
  return null
}

/** True for an error the request gate raised without sending anything,
 *  because the window's connection is recovering (daemon-cli
 *  `RecoveringError`) or switched hosts mid-call (`HostSwitchedError`).
 *  Neither says anything about the role: wait and read again. */
export function isNotSentError(err: unknown): boolean {
  if (!(err instanceof Error)) return false
  return err.name === 'RecoveringError' || err.name === 'HostSwitchedError'
}

function statusOf(err: unknown): number | null {
  const s = (err as { status?: unknown } | null)?.status
  return typeof s === 'number' ? s : null
}

function messageOf(err: unknown): string {
  return err instanceof Error ? err.message : String(err)
}

/** Fold one whoami attempt into a state. `body` when it answered, `err`
 *  when it threw. A not-sent error stays `checking`. */
export function roleStateFrom(result: { body: unknown } | { err: unknown }): HostRoleState {
  if ('body' in result) {
    const role = roleFromWhoami(result.body)
    if (role) return { kind: 'known', role }
    return { kind: 'unreadable', reason: 'its reply did not name a role' }
  }
  const { err } = result
  if (isNotSentError(err)) return { kind: 'checking' }
  const status = statusOf(err)
  if (status === 401 || status === 403) return { kind: 'signed-out', reason: messageOf(err) }
  return { kind: 'unreadable', reason: messageOf(err) }
}

/** Pause before reading whoami again after attempt `n` (0-based) failed. */
export const ROLE_RETRY_MS = [1000, 3000, 10000, 30000] as const
export function roleRetryDelayMs(attempt: number): number {
  return ROLE_RETRY_MS[Math.min(Math.max(attempt, 0), ROLE_RETRY_MS.length - 1)]
}

/** Read again later? Everything but a known role is read again. */
export function shouldRecheckRole(state: HostRoleState): boolean {
  return state.kind !== 'known'
}

export type HostAction = 'update' | 'restart'

function roleName(role: HostRole): string {
  return role === 'owner' ? 'the owner' : role === 'admin' ? 'an Admin' : 'a Member'
}

/** Copy for a Member (a role the host reported). */
export function memberRefusalCopy(hostLabel: string, action: HostAction): string {
  return `You're signed in to ${hostLabel} as a Member. Only the server's owner or an admin can ${action} it.`
}

/** Copy for a login the host no longer accepts. */
export function signedOutCopy(hostLabel: string, action: HostAction): string {
  return `Your sign-in to ${hostLabel} has expired. Sign in to ${hostLabel} again to ${action} it.`
}

/** The row's gate for one admin action on the active host. */
export interface HostActionGate {
  show: boolean
  enabled: boolean
  /** A line under the row, or null. */
  note: string | null
  /** The disabled button's tooltip, or undefined. */
  title: string | undefined
}

/**
 * Show / enable / explain an admin action (update, restart) on the window's
 * remote server. Only a role the host REPORTED below Admin refuses; an unread
 * role is "checking" (disabled for the moment, never "owner or admin"
 * copy), and an unreadable one leaves the button on: the host's own route
 * gate decides, and its answer is explained by `refusalCopy`.
 */
export function hostActionGate(args: {
  isRemote: boolean
  supported: boolean
  role: HostRoleState
  hostLabel: string
  action: HostAction
}): HostActionGate {
  const { isRemote, supported, role, hostLabel, action } = args
  if (!isRemote || !supported) return { show: false, enabled: false, note: null, title: undefined }
  switch (role.kind) {
    case 'known':
      if (role.role === 'owner' || role.role === 'admin') {
        return { show: true, enabled: true, note: null, title: undefined }
      }
      return {
        show: true,
        enabled: false,
        note: memberRefusalCopy(hostLabel, action),
        title: memberRefusalCopy(hostLabel, action),
      }
    case 'checking':
      return {
        show: true,
        enabled: false,
        note: null,
        title: `Checking your access on ${hostLabel}…`,
      }
    case 'signed-out':
      return {
        show: true,
        enabled: false,
        note: signedOutCopy(hostLabel, action),
        title: signedOutCopy(hostLabel, action),
      }
    case 'unreadable':
      return {
        show: true,
        enabled: true,
        note: `Couldn't read your access on ${hostLabel} (${role.reason}). Still checking…`,
        title: undefined,
      }
  }
}

/**
 * The host turned an action down with its token-gate 403 ("invalid or
 * missing token"). That body is the same for a Member and for a login the
 * host no longer knows, so the row reads whoami again (`recheck`) and says
 * which it was, instead of always "Only the host owner or an admin…".
 */
export function refusalCopy(
  hostLabel: string,
  action: HostAction,
  message: string,
  recheck: HostRoleState,
): string {
  switch (recheck.kind) {
    case 'known':
      if (recheck.role === 'member') return memberRefusalCopy(hostLabel, action)
      return `${hostLabel} turned down the ${action} (${message}) although you're ${roleName(recheck.role)} there. Try again; if it keeps happening, sign in to ${hostLabel} again.`
    case 'signed-out':
      return signedOutCopy(hostLabel, action)
    case 'checking':
    case 'unreadable':
      return `${hostLabel} turned down the ${action}: ${message}. Couldn't confirm your access there right now.`
  }
}
