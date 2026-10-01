// Home M4 — what a room on another server shows when it cannot be used
// (prd-home-multi-server-client MS27, MS30, MS45, MS83).
//
// The pool's entry for that server drives it. The room shows a state and
// NEVER switches the window, retries on the window's server, or reloads
// (MS5, MS45). Every state has exactly one action, and each action targets
// that server only:
//
//   | state           | copy                                         | action            |
//   |-----------------|----------------------------------------------|-------------------|
//   | connecting      | Connecting to B…                             | Retry             |
//   | offline         | B is offline. Retrying…                      | Retry             |
//   | restarting      | B is restarting…                             | Retry             |
//   | signing-in      | Signing in to B…                             | Sign in           |
//   | signin-required | Sign in to B.                                | Sign in           |
//   | rotate-required | B needs a new password.                      | Sign in           |
//   | kicked          | Removed from B. Sign in again.               | Sign in           |
//   | version-too-old | B is on vX. This room needs vY or newer.     | Open B's server   |
//   | no-access       | You don't have access to this agent on B.    | Open B's server   |
//   | removed         | This server was removed from this computer.  | none              |
//
// "Open B's server" is the explicit, user-chosen switch of this window to
// B (today's Home behaviour with the preview off); it is never automatic.

import { FEATURES, gte } from '@/lib/server-capabilities'
import { roleAllowsRoom, type HostEntry } from '@/lib/host-pool'

export type RoomFailureKind =
  | 'connecting'
  | 'offline'
  | 'restarting'
  | 'signing-in'
  | 'signin-required'
  | 'rotate-required'
  | 'kicked'
  | 'version-too-old'
  | 'no-access'
  | 'removed'

export type RoomFailureAction = 'retry' | 'sign-in' | 'open-server'

export interface RoomFailure {
  kind: RoomFailureKind
  /** The banner's headline. */
  title: string
  /** Optional second line (an auth note, the next retry). */
  detail: string | null
  /** The one button, or null (only `removed`). */
  action: RoomFailureAction | null
  actionLabel: string | null
  /** Panes stay as they were, dimmed, with input off (MS45). */
  inputOff: boolean
}

export interface RoomFailureInput {
  /** The server's label for copy ("dtl", "This computer"). */
  serverLabel: string
  /** The pool's entry for that server; undefined = never checked. */
  entry: HostEntry | undefined
  /** Is that server still saved on this computer (always true for `local`)? */
  saved: boolean
  /** When the pool will check again (ms epoch), for "Retrying in N s". */
  nextCheckAt?: number | null
  now?: number
}

/** The version a room needs at least (MS30). */
export const HOME_ROOM_MIN_VERSION: string = FEATURES['home-room']

function retryDetail(nextCheckAt: number | null | undefined, now: number | undefined): string | null {
  if (nextCheckAt == null || now === undefined) return null
  const s = Math.max(0, Math.ceil((nextCheckAt - now) / 1000))
  return s === 0 ? 'Retrying now.' : `Next try in ${s} s.`
}

function failure(
  kind: RoomFailureKind,
  title: string,
  detail: string | null,
  action: RoomFailureAction | null,
  serverLabel: string,
): RoomFailure {
  const actionLabel =
    action === 'retry'
      ? 'Retry'
      : action === 'sign-in'
        ? `Sign in to ${serverLabel}`
        : action === 'open-server'
          ? `Open ${serverLabel}’s server`
          : null
  return { kind, title, detail, action, actionLabel, inputOff: true }
}

/**
 * Pure: the failure state a room shows, or null when the room is usable.
 * Order: removed, never checked, offline, restarting, too old, then the
 * login states, then access. A server that is down is "offline" before
 * anything about its login, because its login cannot be checked.
 */
export function roomFailure(input: RoomFailureInput): RoomFailure | null {
  const b = input.serverLabel
  if (!input.saved) {
    return failure('removed', 'This server was removed from this computer.', null, null, b)
  }
  const e = input.entry
  if (!e || e.reach === 'unknown') {
    return failure('connecting', `Connecting to ${b}…`, null, 'retry', b)
  }
  if (e.reach === 'offline') {
    return failure('offline', `${b} is offline. Retrying…`, retryDetail(input.nextCheckAt, input.now), 'retry', b)
  }
  if (e.reach === 'starting') {
    return failure('restarting', `${b} is restarting…`, null, 'retry', b)
  }
  const version = e.boot?.version ?? null
  if (version === null || !gte(version, HOME_ROOM_MIN_VERSION)) {
    const have = version === null ? 'an unknown version' : `v${version}`
    return failure(
      'version-too-old',
      `${b} is on ${have}. This room needs v${HOME_ROOM_MIN_VERSION} or newer.`,
      `Update ${b}, or open its server in this window.`,
      'open-server',
      b,
    )
  }
  switch (e.auth) {
    case 'kicked':
      return failure('kicked', `Removed from ${b}. Sign in again.`, e.authNote, 'sign-in', b)
    case 'rotate-required':
      return failure('rotate-required', `${b} needs a new password.`, e.authNote, 'sign-in', b)
    case 'signin-required':
      return failure('signin-required', `Sign in to ${b}.`, e.authNote, 'sign-in', b)
    case 'signing-in':
      return failure('signing-in', `Signing in to ${b}…`, null, 'sign-in', b)
    case 'ok':
      break
  }
  if (!roleAllowsRoom(e.role)) {
    return failure(
      'no-access',
      `You don’t have access to this agent on ${b}.`,
      `Your role on ${b} is ${e.role}. Members, Admins and Owners can open it here.`,
      'open-server',
      b,
    )
  }
  return null
}
