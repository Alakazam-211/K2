// A Home row on another server, in the one status language
// (plan-agent-status-unify-v1 §3.1 "Reach", S4).
//
// A remote row has two things to say:
//   - reach: is its server there? The machine chip's server icon LEDs, like
//     the top-bar server switcher (green reachable, amber checking /
//     starting / signing in, red offline; the row dims when offline). Only
//     states that need an action keep a word: Sign in, No access, Not found.
//   - the agent's status: the same cluster as a connected row. An open room
//     reads its own server's rows live (with "not viewed", which needs a live
//     socket); a closed row reads that server's `presence/summary`
//     `agentActivity` (D2 adds `heartbeat` / `live` / `busy` / `counts`;
//     before D2 live is unknown, never guessed).
//
// Pure: no stores. `OtherHomeRow` feeds it.

import { agentStatus, EMPTY_AGENT_STATUS, type AgentRollupInput, type AgentStatusView, type UnseenOutcome } from '@/lib/agent-status'
import type { PresenceActivity, RowStatus, RowStatusKind } from '@/lib/home-status'
import type { HomeRowRef } from '@/lib/home-address'
import type { ActivityDisplay } from '@/stores/session-events'
import { pathUnderRoot, unseenOutcomeUnderRoot, type ScopeActivity } from '@/stores/activity'

export type RowReach = 'ok' | 'wait' | 'off'

/** The server-icon LED colour per reach (tokens, not hex). */
export const REACH_LED: Record<RowReach, string> = {
  ok: 'var(--color-status-ok)',
  wait: 'var(--color-status-warn-amber)',
  off: 'var(--color-status-error)',
}

export const REACH_COPY: Record<RowReach, string> = {
  ok: 'reachable',
  wait: 'checking',
  off: 'offline',
}

/** Kinds that mean "the server answered and the agent is there". */
const ACTIVITY_KINDS: ReadonlySet<RowStatusKind> = new Set([
  'working',
  'permission',
  'monitoring',
  'unverifiable',
  'review',
  'idle',
  'live',
])

/** A row's server reach, from its Home status. */
export function rowReach(kind: RowStatusKind): RowReach {
  if (kind === 'offline') return 'off'
  if (kind === 'checking' || kind === 'starting' || kind === 'signing-in' || kind === 'sign-in') return 'wait'
  return 'ok'
}

/** The word a row keeps (an action), or null: glyphs everywhere else (Q2). */
export function rowActionWord(status: RowStatus): string | null {
  if (status.kind === 'sign-in') return 'Sign in'
  if (status.kind === 'no-access') return 'No access'
  if (status.kind === 'not-found') return status.label || 'Not found'
  return null
}

/** What an open room's server says about the row's workspace (its rows
 *  under the room's root): display counts, D1 `busy` / `live` when that
 *  daemon reports them, and this client's "not viewed". */
export interface RoomFacts {
  counts: Record<ActivityDisplay, number>
  busy: number | null
  live: number | null
  unseen: UnseenOutcome | null
}

function displayOfCounts(c: Partial<Record<ActivityDisplay, number>>): ActivityDisplay {
  const order: ActivityDisplay[] = ['waiting', 'working', 'monitoring', 'unverifiable', 'idle']
  return order.find((d) => (c[d] ?? 0) > 0) ?? 'idle'
}

/** The summary entry for this row's workspace (by its id on that server). */
export function summaryHit(activity: PresenceActivity[] | null | undefined, row: HomeRowRef): PresenceActivity | null {
  if (!activity || !row.workspaceId) return null
  return activity.find((a) => a.workspaceId === row.workspaceId) ?? null
}

/** A summary entry as a rollup (D2 fields when present; before D2 its one
 *  display stands for one session). */
export function summaryRollup(hit: PresenceActivity | null): AgentRollupInput | null {
  if (!hit) return null
  const display: ActivityDisplay = hit.display ?? (hit.status === 'permission' ? 'waiting' : 'working')
  const counts = hit.counts ?? { [display]: 1 }
  return {
    display,
    counts,
    ...(typeof hit.busy === 'boolean' ? { busy: hit.busy ? 1 : 0 } : {}),
    ...(typeof hit.live === 'boolean' ? { live: hit.live ? 1 : 0 } : {}),
  }
}

/** The status view of a row on another server. `activity` is that
 *  server's summary list (null = none); `room` an open room's facts. */
export function remoteAgentStatus(input: {
  status: RowStatus
  row: HomeRowRef
  activity: PresenceActivity[] | null | undefined
  room: RoomFacts | null
  /** The server's `/boot-status` lists `activity-live-busy` (D2). */
  serverLiveBusy?: boolean
}): AgentStatusView {
  if (!ACTIVITY_KINDS.has(input.status.kind)) return EMPTY_AGENT_STATUS
  const list = input.activity ?? null
  const hit = summaryHit(list, input.row)
  // D2: the server lists idle workspaces that hold a live session or a
  // heartbeat, with `live`: a row it doesn't list holds no session. Without
  // the feature a missing `live` is unknown (empty slot), never 0.
  const daemonLive = !!list && (input.serverLiveBusy === true || list.some((a) => typeof a.live === 'boolean'))
  const heartbeat = hit?.heartbeat === true
  if (input.room) {
    const r = input.room
    return agentStatus({
      rollup: {
        display: displayOfCounts(r.counts),
        counts: r.counts,
        ...(r.busy !== null ? { busy: r.busy } : {}),
        ...(r.live !== null ? { live: r.live } : {}),
      },
      unseen: r.unseen,
      heartbeatEnabled: heartbeat,
      daemonLive: r.live !== null || daemonLive,
    })
  }
  return agentStatus({ rollup: summaryRollup(hit), unseen: null, heartbeatEnabled: heartbeat, daemonLive })
}

/** A Grok Bot row (0.45.7) in the same language: working in its slot,
 *  needs-you as the status, live unknown. Not running is reach "offline". */
export function grokAgentStatus(state: 'working' | 'needs-you' | 'idle' | 'not-running' | null): AgentStatusView {
  if (state === 'working') return { ...EMPTY_AGENT_STATUS, working: true, workingTitle: 'Working' }
  if (state === 'needs-you') return { ...EMPTY_AGENT_STATUS, status: 'needs-you', title: 'Needs you' }
  return EMPTY_AGENT_STATUS
}

/** An open room's facts for its row's status cluster (plan-agent-status-
 *  unify-v1 S4): its server's rows under the room's workspace, the D1
 *  `busy` / `live` of its rollups there (null on a daemon without D1), and
 *  this client's "not viewed". Pure. */
export function roomFactsOf(view: Pick<ScopeActivity, 'rows' | 'workspaces' | 'unseenDone'>, root: string): RoomFacts {
  const counts = { working: 0, waiting: 0, monitoring: 0, unverifiable: 0, idle: 0 }
  for (const r of view.rows.values()) {
    if (r.workspacePath && pathUnderRoot(r.workspacePath, root)) counts[r.display] += 1
  }
  let busy: number | null = null
  let live: number | null = null
  for (const w of view.workspaces.values()) {
    if (!w.workspacePath || !pathUnderRoot(w.workspacePath, root)) continue
    if (typeof w.busy === 'number') busy = (busy ?? 0) + w.busy
    if (typeof w.live === 'number') live = (live ?? 0) + w.live
  }
  // A daemon with D1 that has no rollup here: nothing is live or busy.
  if (live === null) {
    for (const w of view.workspaces.values()) {
      if (typeof w.live === 'number') {
        live = 0
        busy ??= 0
        break
      }
    }
  }
  return { counts, busy, live, unseen: unseenOutcomeUnderRoot(view, root) }
}
