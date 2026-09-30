// Home P1 (vs-live H14 + plan decisions 1 and 5) — what a Home row says.
//
// The status is answered by THIS client, not by the connected daemon, so a
// server switch never changes it:
//   - a row on the connected server: working / idle / permission / review
//     from the existing activity store; presence from the live roster;
//   - a row on another saved server: live / offline from that server's
//     PUBLIC `/boot-status` (`phase:"ready"` or `ready:true` — the dark-
//     tunnel draft trims the body to `{version, ready, connectLogin}`),
//     plus who is on it from `GET /cli/presence/summary` read with that
//     server's own saved login;
//   - a saved server with no login in hand: "Sign in", not offline;
//   - a server that rejects the saved login: "Sign in" too.
// `/boot-status` never carries who is online; the summary is the separate
// signed-in read.

import { create } from 'zustand'
import { hostBootStatus, type HostCreds } from '@/lib/host-ops'
import type { PaneStatus } from '@/stores/active-agents'
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

export interface HostProbe {
  reach: 'live' | 'offline'
  /** 'none' = no login held; 'rejected' = the server refused it. */
  auth: 'ok' | 'none' | 'rejected'
  /** Null when not read (offline, no login, older server, network miss). */
  presence: PresenceWorkspace[] | null
  at: number
}

export type RowStatusKind =
  | 'working'
  | 'permission'
  | 'review'
  | 'idle'
  | 'live'
  | 'offline'
  | 'sign-in'
  | 'checking'
  | 'not-found'

export interface RowStatus {
  kind: RowStatusKind
  label: string
  /** Other people on this agent right now (never includes you). */
  people: PresencePerson[]
}

const LABELS: Record<RowStatusKind, string> = {
  working: 'Working',
  permission: 'Needs you',
  review: 'Done',
  idle: 'Idle',
  live: 'Live',
  offline: 'Offline',
  'sign-in': 'Sign in',
  checking: 'Checking…',
  'not-found': 'Not found',
}

function status(kind: RowStatusKind, people: PresencePerson[] = []): RowStatus {
  return { kind, label: LABELS[kind], people }
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
      probe: HostProbe | undefined
      self: string
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
  if (!input.saved) return status('offline')
  if (!input.hasLogin) return status('sign-in')
  const p = input.probe
  if (!p) return status('checking')
  if (p.reach === 'offline') return status('offline')
  if (p.auth === 'rejected' || p.auth === 'none') return status('sign-in')
  return status('live', presenceForRow(p.presence, input.row, input.self))
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

// ── Probe (one server) ────────────────────────────────────────────────────

export interface ProbeDeps {
  bootStatus: (creds: HostCreds) => Promise<{ phase?: unknown; ready?: unknown } | null>
  fetch: typeof fetch
  now: () => number
}

const defaultDeps: ProbeDeps = {
  bootStatus: (creds) => hostBootStatus(creds),
  fetch: (...args) => fetch(...args),
  now: () => Date.now(),
}

/** Check one server: public readiness, then (with a login) who is on it.
 *  Never throws — an unreachable server is `offline`. */
export async function probeHost(creds: HostCreds, deps: ProbeDeps = defaultDeps): Promise<HostProbe> {
  const boot = await deps.bootStatus(creds)
  const at = deps.now()
  if (!bootIsReady(boot)) {
    return { reach: 'offline', auth: creds.token ? 'ok' : 'none', presence: null, at }
  }
  if (!creds.token) return { reach: 'live', auth: 'none', presence: null, at }
  let res: Response
  try {
    res = await deps.fetch(
      `${creds.base}/cli/presence/summary?token=${encodeURIComponent(creds.token)}`,
      { method: 'GET', signal: AbortSignal.timeout(5000) },
    )
  } catch (err) {
    console.debug('[home] presence summary unreachable:', creds.base, err)
    return { reach: 'live', auth: 'ok', presence: null, at }
  }
  if (res.status === 401 || res.status === 403) {
    return { reach: 'live', auth: 'rejected', presence: null, at }
  }
  if (!res.ok) {
    // 404 = a server older than the summary route. Live, no presence.
    return { reach: 'live', auth: 'ok', presence: null, at }
  }
  let body: { workspaces?: unknown }
  try {
    body = (await res.json()) as { workspaces?: unknown }
  } catch (err) {
    console.debug('[home] presence summary was not JSON:', creds.base, err)
    return { reach: 'live', auth: 'ok', presence: null, at }
  }
  const list = Array.isArray(body?.workspaces) ? (body.workspaces as PresenceWorkspace[]) : null
  return { reach: 'live', auth: 'ok', presence: list, at }
}

// ── Probe results (module store; never cleared on a server switch) ─────────

interface HomeProbeState {
  probes: Record<string, HostProbe>
  setProbe: (hostKey: string, probe: HostProbe) => void
}

export const useHomeProbeStore = create<HomeProbeState>((set) => ({
  probes: {},
  setProbe: (hostKey, probe) => set((s) => ({ probes: { ...s.probes, [hostKey]: probe } })),
}))
