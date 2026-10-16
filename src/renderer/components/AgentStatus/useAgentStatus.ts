// The one selector over the daemon's activity rollup (plan-agent-status-
// unify-v1 §3.4). Daemon-first: every input but "not viewed" (RL11, per
// client by design) comes from the daemon. On a daemon before 0.46.0 (no
// D1 `live` / `busy` on its rollups) live falls back to the renderer's
// `liveSessionCwds` (primary server only) and working to `counts.working`.

import { useMemo } from 'react'
import type { ServerScope } from '@/kessel/server-scope'
import {
  daemonReportsLive,
  projectUnseenOutcome,
  useActivity,
  workspaceRollup,
  workspaceRowWith,
  type ScopeActivity,
} from '@/stores/activity'
import { projectHasLiveSession, useActiveAgentsStore } from '@/stores/active-agents'
import { agentStatus, type AgentStatusView } from '@/lib/agent-status'
import type { ActivityDisplay } from '@/stores/session-events'

export interface AgentStatusRef {
  projectId?: string | null
  path?: string | null
  /** `projects/list` `heartbeatEnabled` (any enabled heartbeat). */
  heartbeatEnabled?: number | boolean | null
}

function statusRowFor(s: ScopeActivity, ref: AgentStatusRef, display: ActivityDisplay) {
  const r = workspaceRollup(s, ref)
  if (!r || !(r.counts?.[display] > 0)) return null
  return workspaceRowWith(s, ref, display)
}

/** A workspace's status view on `scope`'s server. */
export function useAgentStatus(scope: Pick<ServerScope, 'id' | 'isPrimary'>, ref: AgentStatusRef): AgentStatusView {
  const projectId = ref.projectId ?? null
  const path = ref.path ?? null
  const rollup = useActivity(scope, (s) => workspaceRollup(s, { projectId, path }))
  const unseen = useActivity(scope, (s) => (projectId ? projectUnseenOutcome(s, projectId) : null))
  const daemonLive = useActivity(scope, daemonReportsLive)
  const waitingRow = useActivity(scope, (s) => statusRowFor(s, { projectId, path }, 'waiting'))
  const monitoringRow = useActivity(scope, (s) => statusRowFor(s, { projectId, path }, 'monitoring'))
  const staleRow = useActivity(scope, (s) => statusRowFor(s, { projectId, path }, 'unverifiable'))
  const workingRow = useActivity(scope, (s) => statusRowFor(s, { projectId, path }, 'working'))
  // Older daemon only: the renderer's live-session set (primary server).
  const liveFallback = useActiveAgentsStore((s) =>
    !daemonLive && scope.isPrimary && path ? projectHasLiveSession(s.liveSessionCwds, path) : null,
  )
  const heartbeat = !!ref.heartbeatEnabled
  return useMemo(
    () =>
      agentStatus({
        rollup,
        unseen,
        heartbeatEnabled: heartbeat,
        liveFallback,
        daemonLive,
        titleRow: waitingRow ?? monitoringRow ?? staleRow,
        workingRow,
      }),
    [rollup, unseen, heartbeat, liveFallback, daemonLive, waitingRow, monitoringRow, staleRow, workingRow],
  )
}
