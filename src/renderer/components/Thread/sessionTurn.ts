// The Thread's activity drawer for a chat that lands in one agent session
// (Rosson 2026-10-18): a ticket's reply goes to the ticket's agent session,
// the project chat to the PoC's. Those posts are not Thread injects, so the
// daemon opens no Thread turn for them (`thread_activity.rs`: a turn starts
// on a human Thread inject only). What the daemon does say about that
// session is its activity row (`activity_changed` frames and
// `GET /cli/activity/snapshot`, `stores/activity`): the lead working,
// waiting or idle, since when, its subagents, background tasks and command
// count. This module reads that row as the drawer's turn. Pure: the row in,
// a turn out; nothing is kept or decided in the renderer.
//
//   - Lead working: "Working" (or "Thinking…" when the row says so), the
//     clock from the row's turn start, subagents, background tasks and
//     commands as the drawer's counters.
//   - Lead waiting: "Needs you", stuck on a permission prompt or a question
//     (the row's reason).
//   - No update for a while (`unverifiable`): "No update in Nm".
//   - Lead idle with subagents still running: the children line (a reply
//     must not hide live work, as in the Thread).
//   - Otherwise (idle, or no row, or no single target): no turn, so the
//     drawer slides away / never shows.
//
// One turn per lead turn: `turnId` is the session and the row's turn start,
// so a message sent while the agent works keeps the same drawer (the
// Thread's one-turn behaviour) and a new lead turn is a new one.

import type { ThreadTurn } from '@/components/SessionView/overlayThread'
import { rowForAgentName, useActivity, type ActivityRow, type ScopeActivity } from '@/stores/activity'
import type { ServerScope } from '@/kessel/server-scope'
import { useMemo } from 'react'

/** The one session a chat's posts land in: its v2 session id (preferred
 *  when the store has it), else its v2 agent name (a pinned Chat's agent
 *  name is its project id). Null: no single target, no drawer. */
export interface ChatSessionTarget {
  sessionId?: string | null
  agentName?: string | null
}

/** The activity row for `target`, or null. */
export function rowForChatTarget(
  state: Pick<ScopeActivity, 'rows'>,
  target: ChatSessionTarget | null,
): ActivityRow | null {
  if (!target) return null
  if (target.sessionId) {
    const row = state.rows.get(target.sessionId)
    if (row) return row
  }
  return target.agentName ? rowForAgentName(state, target.agentName) : null
}

/** The drawer's turn for a session's activity row (see the header), or
 *  null when there is nothing to show. `skewMs` is the store's
 *  `serverNow − Date.now()`. */
export function sessionTurnFromRow(row: ActivityRow | null, skewMs: number): ThreadTurn | null {
  if (!row) return null
  const subagents = Math.max(0, row.counts?.subagents ?? row.children.subagents)
  const background = Math.max(0, row.children.shells + row.children.monitors)
  const startedAt = row.turnStartedAt ?? row.lead.since
  const base = {
    turnId: `${row.sessionId}:${startedAt}`,
    startedAt,
    messages: 1,
    lastComposeAt: startedAt,
    phaseSince: row.lead.since,
    line: null,
    subagents,
    subagentsDone: 0,
    background,
    // The row counts commands, and tools without kinds: only commands
    // have a counter of their own.
    tally: { read: 0, search: 0, cmd: Math.max(0, row.counts?.commands ?? 0), edit: 0 },
    end: null,
    rev: row.rev,
    skewMs,
  }
  if (row.display === 'unverifiable') {
    return { ...base, state: 'unverifiable', phase: 'stale', phaseSince: row.staleSince ?? row.lead.since, waitingOn: null }
  }
  if (row.lead.state === 'waiting' || row.display === 'waiting') {
    return { ...base, state: 'needs-you', phase: 'waiting', waitingOn: row.reason === 'waiting_question' ? 'question' : 'permission' }
  }
  if (row.lead.state === 'working') {
    return { ...base, state: 'working', phase: row.reason === 'thinking' ? 'thinking' : 'working', waitingOn: null }
  }
  if (subagents > 0) {
    return { ...base, state: 'working', phase: 'children', waitingOn: null }
  }
  return null
}

/** The drawer's turn for the session a chat posts to, on `scope`'s
 *  daemon activity view. */
export function useChatSessionTurn(scope: Pick<ServerScope, 'id'>, target: ChatSessionTarget | null): ThreadTurn | null {
  const sessionId = target?.sessionId ?? null
  const agentName = target?.agentName ?? null
  const row = useActivity(scope, (s) => rowForChatTarget(s, sessionId || agentName ? { sessionId, agentName } : null))
  const skewMs = useActivity(scope, (s) => s.skewMs)
  return useMemo(() => sessionTurnFromRow(row, skewMs), [row, skewMs])
}
