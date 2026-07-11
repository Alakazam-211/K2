// Heartbeat delivery targeting — where a heartbeat's wakeup goes.
//
// Replaces the 0.37.8 "Send into pinned chat" checkbox with a
// three-way target, mirrored in the daemon's
// `/cli/heartbeat/set-session` route:
//
//   - `pinned`  : deliver into the workspace's pinned chat session via
//                 `workspace_msg::deliver_live` (the checkbox's old
//                 on-state). The heartbeat's own saved session stays in
//                 the DB untouched.
//   - `auto`    : the heartbeat's own session, minted fresh on the next
//                 fire (the default state — `last_session_id` cleared).
//   - `session` : an explicit saved session the user picked from the
//                 workspace's chat history; fires resume THAT
//                 conversation (`session_id` + `provider` persisted).
//
// Pure derivation/apply helpers live here (node-testable, no component
// imports); the drop-down UI is
// `components/common/HeartbeatSessionPicker.tsx`.

import { invoke } from '@tauri-apps/api/core'

export type HeartbeatDeliveryMode = 'pinned' | 'auto' | 'session'

export interface HeartbeatDeliveryTarget {
  mode: HeartbeatDeliveryMode
  /** mode='session' only — the saved conversation to resume on fire. */
  sessionId?: string | null
  /** mode='session' only — the harness that owns `sessionId`. */
  provider?: string | null
}

/** The delivery-relevant slice of a heartbeat row (both the
 *  per-workspace `HeartbeatRow` and the system-wide
 *  `SystemHeartbeatRow` carry these fields). */
export interface HeartbeatDeliveryFields {
  useWorkspaceSession: boolean
  lastSessionId: string | null
  /** Only stamped by an explicit mode='session' pick — auto fires never
   *  set it, and rows from older daemons omit the key entirely
   *  (`undefined` reads as absent). */
  sessionProvider?: string | null
}

/** A saved-session candidate row from the daemon's `chat/list`
 *  aggregator (multi-provider, workspace-scoped). Mirrors the shape
 *  AgentChatPane's pinned-tab dropdown consumes. */
export interface HeartbeatSessionCandidate {
  sessionId: string
  title: string
  timestamp: number
  messageCount: number
  provider: string
}

/** Derive the drop-down's current value from a heartbeat row.
 *  `useWorkspaceSession` wins (pinned mode leaves the saved-session
 *  columns untouched, matching the old checkbox semantics); an
 *  explicitly-trained session is recognized by `sessionProvider`
 *  riding alongside `lastSessionId` (auto fires stamp only the id). */
export function deriveDeliveryTarget(row: HeartbeatDeliveryFields): HeartbeatDeliveryTarget {
  if (row.useWorkspaceSession) return { mode: 'pinned' }
  if (row.lastSessionId && row.sessionProvider) {
    return { mode: 'session', sessionId: row.lastSessionId, provider: row.sessionProvider }
  }
  return { mode: 'auto' }
}

/** Optimistic local mirror of what the daemon's set-session write does
 *  to a row: pinned flips the flag and keeps the saved session; auto
 *  clears it; an explicit session pins id + provider. */
export function applyDeliveryTarget<T extends HeartbeatDeliveryFields>(
  row: T,
  next: HeartbeatDeliveryTarget,
): T {
  switch (next.mode) {
    case 'pinned':
      return { ...row, useWorkspaceSession: true }
    case 'auto':
      return { ...row, useWorkspaceSession: false, lastSessionId: null, sessionProvider: null }
    case 'session':
      return {
        ...row,
        useWorkspaceSession: false,
        lastSessionId: next.sessionId ?? null,
        sessionProvider: next.provider ?? null,
      }
  }
}

/** The saved-session rows the drop-down may offer, newest first.
 *  INVARIANT: the session currently bound to the workspace's pinned
 *  chat never appears as a normal row — it's reachable only through
 *  the "Pinned chat" entry. */
export function selectableSessions(
  rows: HeartbeatSessionCandidate[],
  pinnedSessionId: string | null,
): HeartbeatSessionCandidate[] {
  return rows
    .filter((r) => pinnedSessionId === null || r.sessionId !== pinnedSessionId)
    .sort((a, b) => b.timestamp - a.timestamp)
}

/** Persist a heartbeat's delivery target through the thin Tauri bridge
 *  (`k2so_heartbeat_set_session` → daemon `/cli/heartbeat/set-session`).
 *  The daemon answers `{"success":true,…}` or `{"error":"…"}` — a
 *  non-2xx rejects the invoke, and a 2xx body carrying `error` is
 *  raised too so callers have exactly one failure path to revert on. */
export async function setHeartbeatSession(
  projectPath: string,
  name: string,
  target: HeartbeatDeliveryTarget,
): Promise<void> {
  const resp = await invoke<string>('k2so_heartbeat_set_session', {
    projectPath,
    name,
    mode: target.mode,
    sessionId: target.mode === 'session' ? target.sessionId ?? null : null,
    provider: target.mode === 'session' ? target.provider ?? null : null,
  })
  let parsed: unknown = null
  try {
    parsed = JSON.parse(resp)
  } catch {
    /* non-JSON success body — nothing to inspect */
  }
  if (parsed && typeof parsed === 'object') {
    const err = (parsed as { error?: unknown }).error
    if (typeof err === 'string' && err) throw new Error(err)
  }
}
