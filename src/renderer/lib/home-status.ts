// Home P1 (vs-live H14 + plan decisions 1 and 5) — what a Home row says.
//
// The status is answered by THIS client, not by the connected daemon, so a
// server switch never changes it:
//   - a row on the connected server: working / idle / permission / review
//     from the existing activity store; presence from the live roster;
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

import type { PaneStatus } from '@/stores/active-agents'
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
 *  Z23): a registered workspace on that server with a busy agent. The
 *  server folds it; this client folds nothing. */
export interface PresenceActivity {
  workspaceId: string
  status: 'working' | 'permission'
}

/** What an OPEN room for the row says right now, from its own activity
 *  slice (Z23): `idle` when nothing in it is busy. */
export type RoomRowActivity = 'working' | 'permission' | 'idle'

export type RowStatusKind =
  | 'working'
  | 'permission'
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
      activity: PaneStatus
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
  // Home 0.43.2 (Z23): what the agent is doing. An open room's own slice
  // is live; otherwise the server's folded summary (a closed row lags by
  // at most HOME_POLL_MS). An older server sends no activity: "Live".
  const busy =
    input.roomActivity != null ? input.roomActivity : activityForRow(e.activity ?? null, input.row)
  if (busy === 'working' || busy === 'permission') return status(busy, people, { note })
  return status('live', people, { note })
}

/** The summary's activity for this row's workspace (by its id on that
 *  server), or null. */
export function activityForRow(
  activity: PresenceActivity[] | null,
  row: HomeRowRef,
): PresenceActivity['status'] | null {
  if (!activity || !row.workspaceId) return null
  return activity.find((a) => a.workspaceId === row.workspaceId)?.status ?? null
}

/** Fold an open room's activity slice into one word for its Home row: any
 *  pane waiting on you → `permission`, else any working → `working`, else
 *  `idle`. Daemon truth and the client feed merge per pane the way the tab
 *  dots do (`mergePaneStatus`). */
export function roomRowActivity(
  view: { paneStatuses: Map<string, PaneStatus>; daemonPaneStatuses: Map<string, PaneStatus> },
  merge: (client: PaneStatus | undefined, daemon: PaneStatus | undefined) => PaneStatus,
): RoomRowActivity {
  let working = false
  const keys = new Set([...view.paneStatuses.keys(), ...view.daemonPaneStatuses.keys()])
  for (const k of keys) {
    const s = merge(view.paneStatuses.get(k), view.daemonPaneStatuses.get(k))
    if (s === 'permission') return 'permission'
    if (s === 'working') working = true
  }
  return working ? 'working' : 'idle'
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
