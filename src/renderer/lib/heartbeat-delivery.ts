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
import { daemonCliGet } from '@/lib/daemon-cli'

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
  /** User-archived sessions are never offered as resume targets. */
  archived?: boolean
  customName?: string | null
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
/** Session ids that at least one heartbeat currently delivers into.
 *  Pinned-mode heartbeats map to `pinnedWorkspaceSessionId` (the
 *  workspace's canonical chat). Auto / --set map to `lastSessionId`. */
export function sessionIdsTargetedByHeartbeats(
  rows: Array<Partial<HeartbeatDeliveryFields> & { lastSessionId?: string | null }>,
  pinnedWorkspaceSessionId: string | null,
): Set<string> {
  const ids = new Set<string>()
  for (const row of rows) {
    if (row.useWorkspaceSession) {
      if (pinnedWorkspaceSessionId) ids.add(pinnedWorkspaceSessionId)
      continue
    }
    const sid = row.lastSessionId?.trim()
    if (sid) ids.add(sid)
  }
  return ids
}

export function selectableSessions(
  rows: HeartbeatSessionCandidate[],
  pinnedSessionId: string | null,
): HeartbeatSessionCandidate[] {
  return rows
    .filter((r) => !r.archived)
    .filter((r) => pinnedSessionId === null || r.sessionId !== pinnedSessionId)
    .sort((a, b) => b.timestamp - a.timestamp)
}

/** Persist a heartbeat's delivery target.
 *
 *  0.40.48 host-aware fix: this used to ride the Tauri bridge
 *  (`k2so_heartbeat_set_session` → the LOCAL daemon), so with a remote
 *  host active the write targeted the wrong machine and 400'd with
 *  "Project not found: <remote path>" — the local daemon has never heard
 *  of the remote workspace. Default scope is now the ACTIVE host via
 *  `/cli/heartbeat/set-session` (the exact route the bridge proxied).
 *
 *  `scope: 'local'` preserves the old behavior for surfaces whose ROSTER
 *  is still local-machine-scoped (WakeSchedulerSection reads via local
 *  list_all invokes) — a host-aware write against local rows would just
 *  invert the mismatch.
 *
 *  The daemon answers `{"success":true,…}` or `{"error":"…"}` — a non-2xx
 *  rejects, and a 2xx body carrying `error` is raised too so callers have
 *  exactly one failure path to revert on. */
export async function setHeartbeatSession(
  projectPath: string,
  name: string,
  target: HeartbeatDeliveryTarget,
  opts?: { scope?: 'active-host' | 'local' },
): Promise<void> {
  if (opts?.scope === 'local') {
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
    return
  }
  const resp = await daemonCliGet<{ success?: boolean; error?: string }>(
    'heartbeat/set-session',
    {
      project: projectPath,
      name,
      mode: target.mode,
      // getUrl omits null params, matching the route's empty-filtered
      // `session_id`/`provider` extraction.
      session_id: target.mode === 'session' ? target.sessionId ?? null : null,
      provider: target.mode === 'session' ? target.provider ?? null : null,
    },
  )
  if (resp && typeof resp === 'object' && typeof resp.error === 'string' && resp.error) {
    throw new Error(resp.error)
  }
}
