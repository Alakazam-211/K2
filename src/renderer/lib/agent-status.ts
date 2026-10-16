// One agent status language (plan-agent-status-unify-v1 §3, approved by
// Rosson Long 2026-10-16). Every agent row (Agents nav, Home nav, Active
// drawer; tabs reuse it) draws four slots at its right edge:
//
//   working | heartbeat | status-or-live | number
//
// This module is the pure fold: the daemon's workspace rollup (RL1) plus
// this client's "not viewed" mark (RL11) plus the heartbeat flag in, one
// `AgentStatusView` out. Nothing here reads a store; `useAgentStatus`
// (components/AgentStatus) feeds it.
//
// Status slot (one per row, ranked, the highest wins):
//   1 needs-you   counts.waiting > 0                 filled square, orange
//   2 failed      a turn failed, not viewed (client)  ×, red
//   3 done        a turn finished, not viewed         ✓, green
//   4 monitoring  counts.monitoring > 0              hollow square, neutral
//   5 no-update   counts.unverifiable > 0            dashed square, muted
// With no status the slot shows the live-session square:
//   6 live        rollup.live > 0                    small filled green square
//   7 reaped      rollup.live == 0                   small filled grey square
//   8 idle        live unknown (older daemon, Grok)  empty, reserved
//
// Working is NOT a status (it has its own slot): rollup.busy > 0 (D1: rows
// whose lead is working or that still run subagents), or on a daemon before
// D1 counts.working > 0.

import type { ActivityDisplay, ActivityRow } from '@/stores/session-events'
import { activityTitle } from '@/lib/activity-copy'

export type AgentStatus = 'needs-you' | 'failed' | 'done' | 'monitoring' | 'no-update' | 'live' | 'reaped' | 'idle'

/** The five real statuses, highest first (§3.1). */
export const STATUS_RANK: readonly AgentStatus[] = ['needs-you', 'failed', 'done', 'monitoring', 'no-update']

/** A turn this client hasn't looked at (RL11): how it ended. */
export type UnseenOutcome = 'done' | 'failed'

/** What the fold reads from a workspace rollup. `live` / `busy` are the
 *  daemon's D1 fields (absent from a daemon before 0.46.0). */
export interface AgentRollupInput {
  display: ActivityDisplay
  counts?: Partial<Record<ActivityDisplay, number>> | null
  live?: number | null
  busy?: number | null
}

export interface AgentStatusView {
  /** At least one enabled heartbeat (it can run on its own). */
  heartbeat: boolean
  /** A live terminal session holds RAM; null = unknown (never guessed). */
  live: boolean | null
  /** At least one session is mid-turn or still runs subagents. */
  working: boolean
  /** The status slot: a ranked status, else live / reaped, else idle. */
  status: AgentStatus
  /** The status slot's tooltip / aria-label ('' when idle). */
  title: string
  /** The working slot's tooltip / aria-label ('' when not working). */
  workingTitle: string
}

export interface AgentStatusInput {
  /** The daemon's rollup for the workspace (null = no rows: idle). */
  rollup: AgentRollupInput | null
  /** This client's unseen finished turn, if any (RL11). */
  unseen: UnseenOutcome | null
  heartbeatEnabled: boolean
  /** Live session when the daemon doesn't report `live` (the renderer's
   *  older `liveSessionCwds` signal, or a summary's flag); null = unknown. */
  liveFallback?: boolean | null
  /** The daemon reports `live` on its rollups (D1): a workspace with no
   *  rollup has no live session (reaped), not "unknown". */
  daemonLive?: boolean
  /** The row whose display the status names, for its reason in the
   *  tooltip (`activityTitle`). */
  titleRow?: Pick<ActivityRow, 'display' | 'evidenceAt' | 'staleSince' | 'reason'> | null
  /** The row that is working, for the working tooltip's reason. */
  workingRow?: Pick<ActivityRow, 'display' | 'evidenceAt' | 'staleSince' | 'reason'> | null
  now?: number
  skewMs?: number
}

export const STATUS_COPY: Record<AgentStatus, string> = {
  'needs-you': 'Needs you',
  failed: 'Failed · The turn failed (not viewed yet)',
  done: 'Done · Finished the turn (not viewed yet)',
  monitoring: 'Monitoring · Only background work is left',
  'no-update': 'No update',
  live: 'Live session (using RAM)',
  reaped: 'No live session (reaped to save RAM)',
  idle: '',
}

export const WORKING_COPY = 'Working'
export const HEARTBEAT_COPY = 'Has an enabled heartbeat (it can run on its own)'

function count(r: AgentRollupInput | null, d: ActivityDisplay): number {
  if (!r) return 0
  const c = r.counts?.[d]
  if (typeof c === 'number') return c
  // A rollup without counts (shouldn't happen on the wire): its display
  // stands for one session.
  return r.display === d ? 1 : 0
}

/** Does the rollup carry the daemon's D1 `live` field? */
export function rollupHasLive(r: AgentRollupInput | null | undefined): boolean {
  return !!r && typeof r.live === 'number'
}

/** Pure: fold the inputs into one view (§3.1). */
export function agentStatus(input: AgentStatusInput): AgentStatusView {
  const r = input.rollup
  const now = input.now ?? Date.now()
  const skew = input.skewMs ?? 0

  const working = typeof r?.busy === 'number' ? r.busy > 0 : count(r, 'working') > 0

  let live: boolean | null
  if (rollupHasLive(r)) live = (r!.live as number) > 0
  else if (input.daemonLive) live = false
  else live = input.liveFallback ?? null

  let status: AgentStatus
  if (count(r, 'waiting') > 0) status = 'needs-you'
  else if (input.unseen === 'failed') status = 'failed'
  else if (input.unseen === 'done') status = 'done'
  else if (count(r, 'monitoring') > 0) status = 'monitoring'
  else if (count(r, 'unverifiable') > 0) status = 'no-update'
  else if (live === true) status = 'live'
  else if (live === false) status = 'reaped'
  else status = 'idle'

  const statusDisplay: ActivityDisplay | null =
    status === 'needs-you' ? 'waiting' : status === 'monitoring' ? 'monitoring' : status === 'no-update' ? 'unverifiable' : null
  const row = input.titleRow
  const title =
    statusDisplay && row && row.display === statusDisplay
      ? activityTitle(row, now, skew)
      : status === 'needs-you'
        ? 'Needs you'
        : STATUS_COPY[status]

  const wr = input.workingRow
  const workingTitle = !working ? '' : wr && wr.display === 'working' ? activityTitle(wr, now, skew) : WORKING_COPY

  return { heartbeat: input.heartbeatEnabled, live, working, status, title, workingTitle }
}

/** A real status (not live / reaped / idle)? */
export function isRankedStatus(s: AgentStatus): boolean {
  return STATUS_RANK.includes(s)
}

/** The number column's label for a 0-based row: `1`–`9`, `0` for the
 *  tenth (the Cmd+0 chord), then `-` so the column never collapses. */
export function shortcutLabel(index: number): string {
  if (!Number.isInteger(index) || index < 0) return '-'
  if (index < 9) return String(index + 1)
  if (index === 9) return '0'
  return '-'
}

/** The view for a row with nothing to say (no rollup, nothing known). */
export const EMPTY_AGENT_STATUS: AgentStatusView = Object.freeze({
  heartbeat: false,
  live: null,
  working: false,
  status: 'idle',
  title: '',
  workingTitle: '',
}) as AgentStatusView
