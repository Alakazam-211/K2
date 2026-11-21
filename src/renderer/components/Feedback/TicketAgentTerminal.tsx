// The ticket's agent terminal, in place: find the session a ticket's
// replies land in (live by id, or the workspace's pinned chat via D6),
// attach with `attachAgentName` (the idempotent v2/spawn reuses the existing
// PTY), and mark the workspace Active so the reaper spares it. A dormant
// session offers Wake.
//
// Drawn by the ticket column's Agent tab (an older server), and as the
// terminal drawer of the ticket's Thread pane (TicketThreadPane).

import React, { createContext, useCallback, useEffect, useState } from 'react'
import { PrimaryRoom } from '@/components/Room/PrimaryRoom'
import { daemonCliGet, daemonCliPost } from '@/lib/daemon-cli'
import { TerminalPane } from '@/kessel-term/TerminalPane'
import { PageLiveContext } from '@/contexts/TabVisibilityContext'
import { activateProject } from '@/stores/projects'
import {
  activateOnLiveSessionAttach,
  wakeCanonicalMemberSession,
} from '@/components/Projects/wake-member-session'
import { primaryScope } from '@/kessel/server-scope'
import { Button, TILE_FOOT_CLIP } from '@/components/ui'
import { askingSessionWakeAction, type FeedbackSessionKind } from './feedback-api'

/** May the ticket column offer the ticket agent's terminal? True in the
 *  app; Zen's Tickets view sets false (chat only). */
export const TicketRailTerminalContext = createContext(true)

interface LiveSessionRow {
  sessionId: string
  agentName: string
  command: string | null
  args: string[]
  cwd: string
  isV2: boolean
}

export type TermPhase =
  | { kind: 'checking' }
  | { kind: 'none' }
  | { kind: 'live'; agentName: string; cwd: string; sessionId?: string }
  | { kind: 'dormant'; wakeable: boolean }
  | { kind: 'waking' }
  | { kind: 'error'; message: string }

/** Every live daemon session (path=/ prefix-matches every absolute cwd —
 *  sandbox cells live under ~/.k2/sandbox-sessions/, outside any
 *  registered workspace, so a workspace-scoped list would miss them). */
async function fetchLiveSessions(): Promise<LiveSessionRow[]> {
  const rows = await daemonCliGet<LiveSessionRow[]>(primaryScope(), 'sessions/list-for-workspace', { path: '/' })
  return Array.isArray(rows) ? rows : []
}

/** Find (and wake on demand) the asking session. */
function useAskingSession(args: {
  sessionId: string | null
  sessionKind: FeedbackSessionKind
  canonicalSessionId: string | null | undefined
  projectId: string
  projectPath: string | null
}): { phase: TermPhase; wake: () => Promise<void>; retry: () => void } {
  const { sessionId, sessionKind, canonicalSessionId, projectId, projectPath } = args
  const [phase, setPhase] = useState<TermPhase>(sessionId ? { kind: 'checking' } : { kind: 'none' })

  const resolve = useCallback(async (): Promise<TermPhase> => {
    if (!sessionId) return { kind: 'none' }
    try {
      const live = await fetchLiveSessions()
      const match = live.find((r) => r.sessionId === sessionId)
      const action = askingSessionWakeAction({
        sessionId,
        sessionKind,
        canonicalSessionId,
        liveById: Boolean(match),
      })
      if (action === 'attach-live' && match) {
        return { kind: 'live', agentName: match.agentName, cwd: match.cwd, sessionId: match.sessionId }
      }
      if (action === 'ensure-pinned-chat' && projectPath) {
        const lookup = await daemonCliGet<{ sessionAlive: boolean; sessionId: string | null }>(
          primaryScope(),
          'sessions/lookup-by-agent',
          { agent: projectId },
        )
        if (lookup.sessionAlive) {
          return { kind: 'live', agentName: projectId, cwd: projectPath, sessionId: lookup.sessionId ?? undefined }
        }
      }
      if (action === 'checking') return { kind: 'checking' }
      const wakeable = projectPath !== null && (action === 'ensure-pinned-chat' || action === 'sandbox/reopen')
      return { kind: 'dormant', wakeable }
    } catch (e) {
      return { kind: 'error', message: e instanceof Error ? e.message : String(e) }
    }
  }, [sessionId, sessionKind, canonicalSessionId, projectId, projectPath])

  useEffect(() => {
    let cancelled = false
    if (!sessionId) {
      setPhase({ kind: 'none' })
      return
    }
    setPhase({ kind: 'checking' })
    void resolve().then((p) => {
      if (!cancelled) setPhase(p)
    })
    return () => {
      cancelled = true
    }
  }, [sessionId, resolve])

  // Client watching ⇒ Active so active_reaper spares the chat.
  useEffect(() => {
    if (phase.kind !== 'live') return
    activateOnLiveSessionAttach(projectId, activateProject)
  }, [phase.kind, projectId])

  const wake = useCallback(async (): Promise<void> => {
    if (!projectPath || !sessionId) return
    const action = askingSessionWakeAction({ sessionId, sessionKind, canonicalSessionId, liveById: false })
    if (action !== 'ensure-pinned-chat' && action !== 'sandbox/reopen') return
    setPhase({ kind: 'waking' })
    try {
      if (action === 'ensure-pinned-chat') {
        await wakeCanonicalMemberSession(projectId, projectPath, {
          activateProject,
          ensurePinnedChat: (project) => daemonCliPost(primaryScope(), 'workspace/ensure-pinned-chat', { project }),
        })
        setPhase({ kind: 'live', agentName: projectId, cwd: projectPath })
        return
      }
      activateOnLiveSessionAttach(projectId, activateProject)
      await daemonCliPost(primaryScope(), 'sandbox/reopen', { project_path: projectPath, session_id: sessionId })
      for (let attempt = 0; attempt < 10; attempt++) {
        const live = await fetchLiveSessions()
        const match = live.find((r) => r.sessionId === sessionId)
        if (match) {
          setPhase({ kind: 'live', agentName: match.agentName, cwd: match.cwd, sessionId: match.sessionId })
          return
        }
        await new Promise((r) => setTimeout(r, 500))
      }
      setPhase({ kind: 'error', message: 'Session woke but never registered — try again.' })
    } catch (e) {
      setPhase({ kind: 'error', message: e instanceof Error ? e.message : String(e) })
    }
  }, [projectPath, sessionId, sessionKind, canonicalSessionId, projectId])

  const retry = useCallback(() => {
    setPhase({ kind: 'checking' })
    void resolve().then(setPhase)
  }, [resolve])

  return { phase, wake, retry }
}

// ── Agent tab (the asking session's terminal, in place) ──────────────────

export function AgentTab({
  feedbackId,
  projectId,
  projectPath,
  sessionId,
  sessionKind,
  canonicalSessionId,
}: {
  feedbackId: string
  projectId: string
  projectPath: string | null
  sessionId: string | null
  sessionKind: FeedbackSessionKind
  canonicalSessionId: string | null | undefined
}): React.JSX.Element {
  // Pinned-chat tickets (canonical / D6) attach to the workspace agent even
  // without a stamped session id; anything else needs the asking session.
  const effectiveSessionId = sessionId ?? canonicalSessionId ?? null
  const { phase, wake, retry } = useAskingSession({
    sessionId: effectiveSessionId,
    sessionKind: sessionId ? sessionKind : 'canonical',
    canonicalSessionId,
    projectId,
    projectPath,
  })

  if (phase.kind === 'none') {
    return (
      <EmptyTermState
        title="No session attached"
        detail="This ticket was filed outside a known session, so there is no terminal to show."
      />
    )
  }
  if (phase.kind === 'checking') return <EmptyTermState title="Looking for the agent…" />
  if (phase.kind === 'waking') return <EmptyTermState title="Waking the agent…" />
  if (phase.kind === 'error') {
    return (
      <div className="flex-1 flex flex-col items-center justify-center gap-3 px-6 text-center">
        <p className="type-row text-[var(--color-status-error-text)] max-w-[48ch]">{phase.message}</p>
        <Button variant="quiet" onClick={retry}>
          Retry
        </Button>
      </div>
    )
  }
  if (phase.kind === 'dormant') {
    return (
      <div
        data-testid="ticket-agent-dormant"
        className="flex-1 flex flex-col items-center justify-center gap-3 px-6 text-center"
      >
        <p className="type-name text-[var(--color-text-primary)]">The agent is asleep</p>
        <p className="type-hint text-[var(--color-text-muted)] max-w-[44ch]">
          {phase.wakeable ? 'Wake it to see its terminal here.' : 'Its session isn’t running right now.'}
        </p>
        {phase.wakeable && (
          <Button variant="accent" onClick={() => void wake()}>
            Wake the agent
          </Button>
        )}
      </div>
    )
  }

  // Live — attach in place. attachAgentName keys the idempotent v2/spawn
  // to the EXISTING daemon session (reused:true), so this never mints a
  // duplicate PTY.
  return (
    <PageLiveContext.Provider value={true}>
      <div className={`flex-1 min-h-0 ${TILE_FOOT_CLIP}`} data-testid="ticket-agent-terminal">
        <PrimaryRoom>
          <TerminalPane
            terminalId={`feedback-term:${feedbackId}`}
            cwd={phase.cwd}
            attachAgentName={phase.agentName}
            sessionId={phase.sessionId}
          />
        </PrimaryRoom>
      </div>
    </PageLiveContext.Provider>
  )
}

function EmptyTermState({ title, detail }: { title: string; detail?: string }): React.JSX.Element {
  return (
    <div
      data-testid="ticket-agent-empty"
      className="flex-1 flex flex-col items-center justify-center gap-2 px-6 text-center"
    >
      <p className="type-row text-[var(--color-text-secondary)]">{title}</p>
      {detail && <p className="type-hint text-[var(--color-text-muted)] max-w-[44ch]">{detail}</p>}
    </div>
  )
}
