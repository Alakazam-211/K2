// The words for the daemon's activity rows (prd-daemon-activity-and-thread-
// working-v1 RL14, DA31). The renderer only labels what the daemon decided;
// every DA31 reason has copy here, and `activity-copy.test.ts` fails on a
// reason in `crates/k2-core/src/activity/fixtures/reasons.json` that has
// none.

import type { ActivityDisplay, ActivityRow } from '@/stores/session-events'

/** The daemon's `STALE_AFTER` (DA29): `unverifiable` starts this long after
 *  the last evidence. */
const STALE_AFTER_MS = 30 * 60 * 1000

/** One short word per display (RL14). `unverifiable` gets its minutes from
 *  `activityLabel`. */
export const DISPLAY_LABEL: Record<ActivityDisplay, string> = {
  working: 'working',
  monitoring: 'monitoring background tasks',
  waiting: 'needs you',
  unverifiable: 'no update',
  idle: 'idle',
}

/** Why a row shows what it shows (DA31), for tooltips. */
export const REASON_COPY: Record<string, string> = {
  turn_running: 'Working on a turn',
  tool_running: 'Running a tool',
  thinking: 'Thinking',
  waiting_permission: 'Waiting for your permission',
  waiting_question: 'Waiting for your answer',
  subagents_running: 'Subagents are still working',
  background_shell: 'A background command is running',
  monitor_running: 'A monitor is running',
  crons_scheduled: 'Scheduled tasks are set',
  owed_notification: 'Waiting for a background task to report',
  turn_done: 'Finished the turn',
  turn_failed: 'The turn failed',
  interrupted: 'Stopped by you',
  prompt_dismissed: 'You dismissed the prompt',
  permission_done: 'Permission answered',
  compacted: 'Compacted the conversation',
  session_boundary: 'Started a new conversation',
  idle_prompt: 'Waiting for your next message',
  transcript_turn_end: 'Finished the turn',
  agent_exited: 'The agent exited',
  pty_exited: 'The session ended',
  owed_expired: 'Stopped waiting for a background task',
  crons_cleared: 'Scheduled tasks cleared',
  child_done: 'Background work finished',
  stale_no_evidence: 'Nothing heard from the agent for a while',
  unconfirmed: 'Not heard from since the server started',
  stale: 'Nothing heard from the agent for a while',
  unbound_turn_end: 'Finished without replying here',
}

/** Whole minutes since the row's last evidence (for `unverifiable`). */
export function minutesWithoutUpdate(
  row: Pick<ActivityRow, 'evidenceAt' | 'staleSince'>,
  now: number,
  skewMs = 0,
): number {
  const from = row.evidenceAt ?? (row.staleSince !== null ? row.staleSince - STALE_AFTER_MS : null)
  if (from === null) return 0
  return Math.max(0, Math.floor((now + skewMs - from) / 60_000))
}

/** "working", "monitoring background tasks", "needs you", "idle", or
 *  "no update in 34m" (RL14). `confirmed: false` adds nothing. */
export function activityLabel(
  row: Pick<ActivityRow, 'display' | 'evidenceAt' | 'staleSince'>,
  now: number,
  skewMs = 0,
): string {
  if (row.display === 'unverifiable') return `no update in ${minutesWithoutUpdate(row, now, skewMs)}m`
  return DISPLAY_LABEL[row.display]
}

/** A row's tooltip: its label, then why. */
export function activityTitle(
  row: Pick<ActivityRow, 'display' | 'evidenceAt' | 'staleSince' | 'reason'>,
  now: number,
  skewMs = 0,
): string {
  const label = activityLabel(row, now, skewMs)
  const why = REASON_COPY[row.reason]
  const head = label.charAt(0).toUpperCase() + label.slice(1)
  return why ? `${head} · ${why}` : head
}
