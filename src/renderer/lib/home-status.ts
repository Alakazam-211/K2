// Home P1 (vs-live H14 + plan decisions 1 and 5) — what a Home row says.
//
// The status is answered by THIS client, not by the connected daemon, so a
// server switch never changes it:
//   - a row on the connected server: working / monitoring / needs you /
//     no update / idle / review from the daemon's activity rows
//     (prd-daemon-activity-and-thread-working-v1 S5); presence from the
//     live roster;
//   - a row on another saved server: the connection pool's entry for that
//     server (Home M2, `lib/host-pool.ts`): live / starting / offline from
//     its PUBLIC `/boot-status` (`phase:"ready"` or `ready:true` — the
//     dark-tunnel draft trims the body to `{version, ready, connectLogin}`),
//     the login state, the role from whoami, and who is on it from
//     `GET /cli/presence/summary` read with that server's own saved login;
//   - a saved server with no login in hand: "Sign in", not offline;
//   - a server that rejects the saved login, removed this login, or wants
//     a new password: "Sign in", with the reason as the row's detail;
//   - a login whose role there is below Member, or unknown: "No access".
// `/boot-status` never carries who is online; the summary is the separate
// signed-in read.

import { displayUnderRoot, type ActivityDisplay, type ScopeActivity } from '@/stores/activity'
import type { HostEntry } from '@/lib/host-pool'
import { roleAllowsRoom } from '@/lib/host-pool'
import { usersForWorkspace, type RosterUser } from '@/stores/presence'
import { parseHomeAddress, workspaceHandle, type HomeRowRef } from '@/lib/home-address'

/** How often a visible Home re-checks other servers. */
export const HOME_POLL_MS = 30_000

export interface PresencePerson {
  user: string
  name: string
  role: string
}

/** One row of `GET /cli/presence/summary` → `workspaces[]`. */
export interface PresenceWorkspace {
  workspaceId: string | null
  handle: string | null
  name: string | null
  path: string
  count: number
  people: PresencePerson[]
}

/** One row of `GET /cli/presence/summary` → `agentActivity[]` (Home 0.43.2,
 *  Z23): a registered workspace on that server with a non-idle agent. The
 *  server folds it; this client folds nothing. `display` is the daemon's
 *  real rollup (prd-daemon-activity-and-thread-working-v1 A31); a server
 *  before it sends only `status`. */
export interface PresenceActivity {
  workspaceId: string
  status: 'working' | 'permission'
  display?: ActivityDisplay
}

/** What a Home row says an agent is doing: the daemon's display, with
 *  `waiting` named `permission` (Home's "Needs you" kind). */
export type RoomRowActivity = 'working' | 'permission' | 'monitoring' | 'unverifiable' | 'idle'

/** The daemon's display as a Home row word. */
export function homeActivityWord(display: ActivityDisplay): RoomRowActivity {
  return display === 'waiting' ? 'permission' : display
}

export type RowStatusKind =
  | 'working'
  | 'permission'
  | 'monitoring'
  | 'unverifiable'
  | 'review'
  | 'idle'
  | 'live'
  | 'starting'
  | 'offline'
  | 'signing-in'
  | 'sign-in'
  | 'no-access'
  | 'checking'
  | 'not-found'

export interface RowStatus {
  kind: RowStatusKind
  label: string
  /** Other people on this agent right now (never includes you). */
  people: PresencePerson[]
  /** Why, for the row's tooltip ("Removed from B. Sign in again."). */
  detail?: string | null
  /** "Same server as …" (MS81): this server answers with the same
   *  `instanceId` as an earlier row's server. */
  note?: string | null
}

const LABELS: Record<RowStatusKind, string> = {
  working: 'Working',
  permission: 'Needs you',
  monitoring: 'Monitoring',
  unverifiable: 'No update',
  review: 'Done',
  idle: 'Idle',
  live: 'Live',
  starting: 'Starting',
  offline: 'Offline',
  'signing-in': 'Signing in…',
  'sign-in': 'Sign in',
  'no-access': 'No access',
  checking: 'Checking…',
  'not-found': 'Not found',
}

function status(
  kind: RowStatusKind,
  people: PresencePerson[] = [],
  extra: { detail?: string | null; note?: string | null } = {},
): RowStatus {
  const out: RowStatus = { kind, label: LABELS[kind], people }
  if (extra.detail) out.detail = extra.detail
  if (extra.note) out.note = extra.note
  return out
}

/** `/boot-status` says ready: `phase === 'ready'` or `ready === true`. */
export function bootIsReady(b: { phase?: unknown; ready?: unknown } | null): boolean {
  if (!b) return false
  return b.phase === 'ready' || b.ready === true
}

export type RowStatusInput =
  | {
      /** The row's server is the one this window is connected to. */
      where: 'connected'
      row: HomeRowRef
      /** The matched workspace on the connected server (null = gone). */
      workspace: { id: string; path: string } | null
      activity: RoomRowActivity | 'review'
      roster: RosterUser[]
      rosterSupported: boolean
      /** Your identity on that server (`owner` or your username). */
      self: string
    }
  | {
      where: 'other'
      row: HomeRowRef
      /** False when the row's server is not in the saved list. */
      saved: boolean
      /** You hold a login (token) for that server. `local` always does. */
      hasLogin: boolean
      /** The connection pool's entry for that server (undefined = never
       *  checked). */
      entry: HostEntry | undefined
      self: string
      /** The label of an earlier row's server that answers with the same
       *  `instanceId` (MS81), or null. */
      sameServerAs?: string | null
      /** The server's label, for copy. */
      serverLabel?: string
      /** Home 0.43.2 (Z23): set when a room for this row is open (hot or
       *  warm). Its live slice wins over the pool's summary, which can be
       *  up to `HOME_POLL_MS` old. */
      roomActivity?: RoomRowActivity | null
    }

/** Pure: fold everything we know into what the row shows. */
export function resolveRowStatus(input: RowStatusInput): RowStatus {
  if (input.where === 'connected') {
    if (!input.workspace) return status('not-found')
    const people = input.rosterSupported
      ? usersForWorkspace(input.roster, input.workspace.path)
          .filter((u) => u.user !== input.self)
          .map((u) => ({ user: u.user, name: u.user, role: u.role }))
      : []
    return status(input.activity, people)
  }
  const note = input.sameServerAs ? `Same server as ${input.sameServerAs}` : null
  if (!input.saved) return status('offline', [], { note })
  const e = input.entry
  if (!input.hasLogin) {
    // No login held. The pool may be signing in on its own (another window
    // holds the lease), or know why the login is gone.
    if (e?.auth === 'signing-in') return status('signing-in', [], { note })
    return status('sign-in', [], { detail: e?.authNote ?? null, note })
  }
  if (!e || e.reach === 'unknown') return status('checking', [], { note })
  if (e.reach === 'offline') return status('offline', [], { note })
  if (e.reach === 'starting') return status('starting', [], { note })
  if (e.auth === 'signing-in') return status('signing-in', [], { note })
  if (e.auth !== 'ok') {
    const server = input.serverLabel ?? 'this server'
    const detail =
      e.authNote ??
      (e.auth === 'rotate-required'
        ? `${server} needs a new password.`
        : e.auth === 'kicked'
          ? `Removed from ${server}. Sign in again.`
          : null)
    return status('sign-in', [], { detail, note })
  }
  // MS83: a login below Member — or a role this app does not know — opens
  // nothing there.
  if (!roleAllowsRoom(e.role)) {
    return status('no-access', [], {
      detail: `Your login on ${input.serverLabel ?? 'this server'} is ${e.role ?? 'unknown'}. Ask its owner for Member access.`,
      note,
    })
  }
  const people = presenceForRow(e.presence, input.row, input.self)
  // Home 0.43.2 (Z23): what the agent is doing. An open room's server rows
  // are live; otherwise the server's folded summary (a closed row lags by
  // at most HOME_POLL_MS). An older server sends no activity: "Live".
  const busy =
    input.roomActivity != null ? input.roomActivity : activityForRow(e.activity ?? null, input.row)
  if (busy !== null && busy !== 'idle') return status(busy, people, { note })
  return status('live', people, { note })
}

/** The summary's activity for this row's workspace (by its id on that
 *  server), or null: its real `display` when the server sends one
 *  (S5), else the older `status` word. */
export function activityForRow(
  activity: PresenceActivity[] | null,
  row: HomeRowRef,
): RoomRowActivity | null {
  if (!activity || !row.workspaceId) return null
  const hit = activity.find((a) => a.workspaceId === row.workspaceId)
  if (!hit) return null
  return hit.display ? homeActivityWord(hit.display) : hit.status
}

/** An open room's word for its Home row: the highest display among its
 *  server's rows under the room's workspace (RL1's rank). */
export function roomRowActivity(view: Pick<ScopeActivity, 'rows'>, root: string): RoomRowActivity {
  return homeActivityWord(displayUnderRoot(view, root))
}

/** The people a summary lists on this row's workspace, minus you. Match
 *  by workspace id first (exact on that server), then by handle. */
export function presenceForRow(
  summary: PresenceWorkspace[] | null,
  row: HomeRowRef,
  self: string,
): PresencePerson[] {
  if (!summary) return []
  const parsed = parseHomeAddress(row.address)
  const hit =
    (row.workspaceId ? summary.find((w) => w.workspaceId === row.workspaceId) : undefined) ??
    (parsed
      ? summary.find(
          (w) =>
            w.handle === parsed.handle ||
            (w.handle === null && w.name !== null && workspaceHandle({ name: w.name }) === parsed.handle),
        )
      : undefined)
  if (!hit) return []
  return hit.people.filter((p) => p.user !== self)
}

/** Initials for a presence chip (`anna` → `A`, `Rosson Long` → `RL`). */
export function personInitials(name: string): string {
  const parts = name.trim().split(/[\s._-]+/).filter(Boolean)
  if (parts.length === 0) return '?'
  if (parts.length === 1) return parts[0].slice(0, 1).toUpperCase()
  return (parts[0][0] + parts[1][0]).toUpperCase()
}
