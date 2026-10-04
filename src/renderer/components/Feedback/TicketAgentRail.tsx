// "Chat with agent" — the right-hand rail of the ticket detail: the asking
// agent's session in the same Thread | Terminal view the Agents page uses.
//
// Nothing new is built here. The rail mounts the Agents page's own session
// surface, `AgentSessionChrome` (the View menu — `SessionViewMenu` — with
// Terminal / Thread / Split view, the agent's name and the refresh button)
// around the kessel `TerminalPane`. One pane, one switcher; the chosen
// view is remembered by `useSessionViewTab` under the same key the Agents
// page uses for that conversation, so the rail and the Agents page agree.
//
// Attaching rides the ticket's old Agent tab (unchanged): find the asking
// session (live-by-id, or the workspace's pinned chat via D6), attach with
// `attachAgentName` (the idempotent v2/spawn reuses the existing PTY), and
// mark the workspace Active so the reaper spares it (PRD §4.3.1). A
// dormant session offers Wake. When the terminal cannot be shown (no
// session, dormant, an error), the rail shows the Thread only, read-only,
// with a note.
//
// `footer` (the ticket's quick-answer buttons) sits at the bottom of the
// rail, right under the session's compose area.

import React, { useCallback, useEffect, useState } from 'react'
import { PrimaryRoom } from '@/components/Room/PrimaryRoom'
import { daemonCliGet, daemonCliPost } from '@/lib/daemon-cli'
import { TerminalPane } from '@/kessel-term/TerminalPane'
import { PageLiveContext } from '@/contexts/TabVisibilityContext'
import { activateProject } from '@/stores/projects'
import {
  activateOnLiveSessionAttach,
  wakeCanonicalMemberSession,
} from '@/components/Projects/wake-member-session'
import { AgentSessionChrome } from '@/components/SessionView/AgentSessionChrome'
import { ThreadOverlayPane } from '@/components/SessionView/ThreadOverlayPane'
import { resolvePinnedChatCopyableAddress } from '@/lib/chat-session-tab'
import { primaryScope } from '@/kessel/server-scope'
import { askingSessionWakeAction, type FeedbackSessionKind } from './feedback-api'

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

/** The pinned-chat Thread address for the agent's workspace (the same
 *  lookup AgentChatPane uses). Empty until resolved. */
function useThreadAddr(projectPath: string | null, projectId: string): string {
  const [addr, setAddr] = useState('')
  useEffect(() => {
    let cancelled = false
    if (!projectPath) return
    resolvePinnedChatCopyableAddress(primaryScope(), projectPath, projectId)
      .then((a) => {
        if (!cancelled && a?.clipboard) setAddr(a.clipboard)
      })
      .catch(() => {})
    return () => {
      cancelled = true
    }
  }, [projectPath, projectId])
  return addr
}

export function TicketAgentRail({
  feedbackId,
  agentName,
  projectId,
  projectPath,
  sessionId,
  sessionKind,
  canonicalSessionId,
  onClose,
  footer,
}: {
  feedbackId: string
  /** The agent that filed the ticket (display only). */
  agentName: string
  projectId: string
  projectPath: string | null
  sessionId: string | null
  sessionKind: FeedbackSessionKind
  canonicalSessionId: string | null | undefined
  onClose: () => void
  /** Rendered at the bottom of the rail, under the compose area (the
   *  ticket's quick-answer buttons). */
  footer?: React.ReactNode
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
  const addr = useThreadAddr(projectPath, projectId)

  return (
    <aside
      data-testid="ticket-agent-rail"
      className="flex flex-col min-h-0 border-l border-[var(--color-border)] flex-shrink-0 bg-[var(--color-bg)]"
      style={{ width: 'min(640px, 48vw)' }}
    >
      <div className="flex items-center gap-2 px-3 h-9 border-b border-[var(--color-border)] flex-shrink-0">
        {/* Live: the session chrome below names the agent; don't repeat it. */}
        {phase.kind === 'live' ? (
          <span className="text-[10px] font-semibold uppercase tracking-wider text-[var(--color-text-muted)]">
            Chat with agent
          </span>
        ) : (
          <span className="text-xs font-semibold text-[var(--color-text-primary)] truncate" title={agentName}>
            {agentName}
          </span>
        )}
        <span className="flex-1" />
        <button
          type="button"
          data-testid="ticket-agent-rail-close"
          onClick={onClose}
          title="Hide the chat"
          aria-label="Hide the chat"
          className="flex h-6 w-6 items-center justify-center text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)] hover:bg-white/[0.06] cursor-pointer"
        >
          <svg width="11" height="11" viewBox="0 0 12 12" fill="none" stroke="currentColor" strokeWidth="1.5">
            <line x1="2" y1="2" x2="10" y2="10" />
            <line x1="10" y1="2" x2="2" y2="10" />
          </svg>
        </button>
      </div>

      <PageLiveContext.Provider value={true}>
        <PrimaryRoom>
          {phase.kind === 'live' ? (
            <div className="flex-1 min-h-0 flex flex-col" data-testid="ticket-agent-rail-live">
              <AgentSessionChrome
                title={agentName}
                addr={addr}
                conversationId={canonicalSessionId ?? null}
                agentName={phase.agentName}
                cwd={phase.cwd}
              >
                <TerminalPane
                  terminalId={`feedback-term:${feedbackId}`}
                  cwd={phase.cwd}
                  attachAgentName={phase.agentName}
                  sessionId={phase.sessionId}
                />
              </AgentSessionChrome>
            </div>
          ) : (
            <ThreadOnly
              addr={addr}
              conversationId={canonicalSessionId ?? null}
              agentName={agentName}
              phase={phase}
              onWake={() => void wake()}
              onRetry={retry}
            />
          )}
        </PrimaryRoom>
      </PageLiveContext.Provider>

      {footer}
    </aside>
  )
}

/** The terminal can't be shown: the Thread alone (read-only) plus why. */
function ThreadOnly({
  addr,
  conversationId,
  agentName,
  phase,
  onWake,
  onRetry,
}: {
  addr: string
  conversationId: string | null
  agentName: string
  phase: Exclude<TermPhase, { kind: 'live' }>
  onWake: () => void
  onRetry: () => void
}): React.JSX.Element {
  const note =
    phase.kind === 'none'
      ? 'This ticket was filed outside a known session, so there is no terminal to show.'
      : phase.kind === 'checking'
        ? 'Looking for the agent’s session…'
        : phase.kind === 'waking'
          ? 'Waking the agent’s session…'
          : phase.kind === 'error'
            ? `The terminal can’t be shown: ${phase.message}`
            : 'The agent’s session isn’t running, so only its Thread is shown.'
  return (
    <div className="flex-1 min-h-0 flex flex-col" data-testid="ticket-agent-rail-thread-only">
      <div
        data-testid="ticket-agent-rail-note"
        className="flex items-center gap-2 px-3 py-2 text-[11px] text-[var(--color-text-muted)] border-b border-[var(--color-border)] flex-shrink-0"
      >
        <span className="flex-1 min-w-0">{note}</span>
        {phase.kind === 'dormant' && phase.wakeable && (
          <button
            type="button"
            onClick={onWake}
            className="px-2 py-1 text-[11px] font-medium bg-[var(--color-accent)]/15 text-[var(--color-text-primary)] hover:bg-[var(--color-accent)]/25 cursor-pointer"
          >
            Wake session
          </button>
        )}
        {phase.kind === 'error' && (
          <button
            type="button"
            onClick={onRetry}
            className="px-2 py-1 text-[11px] bg-white/[0.06] text-[var(--color-text-primary)] cursor-pointer"
          >
            Retry
          </button>
        )}
      </div>
      <div className="flex-1 min-h-0 flex flex-col overflow-hidden">
        {addr ? (
          <ThreadOverlayPane addr={addr} conversationId={conversationId} agentName={agentName} />
        ) : (
          <div className="flex-1 flex items-center justify-center text-[11px] text-[var(--color-text-muted)] px-6 text-center">
            No Thread address for this agent yet.
          </div>
        )}
      </div>
    </div>
  )
}
