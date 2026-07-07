// Feedback F2 — single-item view: two tabs along the top,
// Thread (default) | Agent (the asking session's terminal).
//
// Thread: the ask + structured-option one-tap buttons + the comment
// thread + a reply box. There is NO Answer mode — it's just a comment
// thread. Every reply (and every tapped option) posts a plain comment;
// the daemon injects human comments into the asking session, and the
// first human comment on a waiting question/approval becomes the
// answer behind the scenes (so `ask --wait` unblocks). Resolve/dismiss
// actions ride the resolve route.
//
// Agent: the ASKING session's terminal embedded IN PLACE via the
// kessel TerminalPane attach machinery (attachAgentName → the idempotent
// /cli/sessions/v2/spawn reuses the existing daemon PTY — the same
// mechanism as AgentChatPane and the orange-tab sandbox adoption). A
// live session attaches immediately; a dormant one shows "Wake session"
// (canonical → ensure-pinned-chat, sandbox → sandbox/reopen) and then
// attaches; no session_id shows a plain empty state.

import React, { useCallback, useEffect, useRef, useState } from 'react'
import { daemonCliGet, daemonCliPost } from '@/lib/daemon-cli'
import { TerminalPane } from '@/kessel-term/TerminalPane'
import { formatRelativeTime } from '@/lib/format-relative-time'
import { KeyCombo } from '@/components/KeySymbol'
import {
  commentFeedback,
  fetchFeedbackShow,
  optionsActionable,
  resolveFeedback,
  type FeedbackListRow,
  type FeedbackShow,
} from './feedback-api'
import { KindBadge, PriorityBadge, StatusBadge } from './badges'

interface FeedbackItemViewProps {
  id: string
  /** The selected list row — carries projectPath/projectName so the
   *  header renders instantly while the full thread loads. */
  listRow: FeedbackListRow
  nowSec: number
  /** Store revision — bumped by feedback events; refetches the thread. */
  revision: number
  /** Fired after any successful mutation so the parent list + badge
   *  update instantly (the daemon events also arrive, slightly later). */
  onMutated: () => void
}

type ItemTab = 'thread' | 'terminal'

export function FeedbackItemView({
  id,
  listRow,
  nowSec,
  revision,
  onMutated,
}: FeedbackItemViewProps): React.JSX.Element {
  const [tab, setTab] = useState<ItemTab>('thread')
  const [item, setItem] = useState<FeedbackShow | null>(null)
  const [error, setError] = useState<string | null>(null)

  const load = useCallback(async (): Promise<void> => {
    try {
      const data = await fetchFeedbackShow(id)
      setItem(data)
      setError(null)
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    }
  }, [id])

  // Item switch (the parent keys this view by id) → fetch immediately.
  useEffect(() => {
    void load()
  }, [load])

  // Event-driven refresh: any feedback event (commented / answered /
  // status-changed / created) bumps the store revision, and the OPEN
  // thread refetches so a new reply appears without reselecting the
  // item. Bursts coalesce on a trailing 300ms window — N rapid
  // comments fire ONE fetch (each bump resets the timer via the
  // cleanup). Only `item` is replaced by the refetch; the composer's
  // draft lives in ThreadTab state, so mid-typed text survives.
  const seenRevision = useRef(revision)
  useEffect(() => {
    if (revision === seenRevision.current) return
    seenRevision.current = revision
    const timer = setTimeout(() => {
      void load()
    }, 300)
    return () => clearTimeout(timer)
  }, [revision, load])

  const projectPath = item?.projectPath ?? listRow.projectPath
  const workspaceName = item?.workspace ?? listRow.projectName
  const view = item ?? listRow

  return (
    <div className="flex-1 flex flex-col min-h-0">
      {/* Item header: title + badges + the two tabs along the top. */}
      <div className="px-4 pt-3 border-b border-[var(--color-border)] flex-shrink-0">
        <div className="flex items-center gap-3 min-w-0">
          <span className="text-sm font-medium text-[var(--color-text-primary)] truncate flex-1" title={view.title}>
            {view.title}
          </span>
          <PriorityBadge priority={view.priority} />
          <KindBadge kind={view.kind} />
          <StatusBadge status={view.status} />
        </div>
        <div className="mt-1 text-[10px] text-[var(--color-text-muted)] truncate">
          {view.agentName}
          <span className="opacity-60"> · {workspaceName} · asked {formatRelativeTime(view.createdAt, nowSec)}</span>
        </div>
        <div className="flex items-center gap-1 mt-2">
          {(['thread', 'terminal'] as const).map((t) => (
            <button
              key={t}
              type="button"
              onClick={() => setTab(t)}
              className={`px-3 py-1.5 text-[11px] font-medium border-b-2 -mb-px transition-colors cursor-pointer ${
                tab === t
                  ? 'border-[var(--color-accent)] text-[var(--color-text-primary)]'
                  : 'border-transparent text-[var(--color-text-muted)] hover:text-[var(--color-text-secondary)]'
              }`}
            >
              {t === 'thread' ? 'Thread' : 'Agent'}
            </button>
          ))}
        </div>
      </div>

      {tab === 'thread' ? (
        <ThreadTab
          item={item}
          error={error}
          nowSec={nowSec}
          onChanged={() => {
            void load()
            onMutated()
          }}
        />
      ) : (
        <TerminalTab
          sessionId={view.sessionId}
          sessionKind={view.sessionKind}
          projectId={view.projectId}
          projectPath={projectPath}
          feedbackId={id}
        />
      )}
    </div>
  )
}

// ── Thread tab ────────────────────────────────────────────────────────────

function ThreadTab({
  item,
  error,
  nowSec,
  onChanged,
}: {
  item: FeedbackShow | null
  error: string | null
  nowSec: number
  onChanged: () => void
}): React.JSX.Element {
  const [reply, setReply] = useState('')
  const [busy, setBusy] = useState(false)
  const [actionError, setActionError] = useState<string | null>(null)
  const scrollRef = useRef<HTMLDivElement>(null)

  // Keep the newest message in view whenever the rendered thread changes
  // (initial load, item switch, or a fresh comment). Runs after paint so
  // scrollHeight reflects the freshly-rendered list.
  const commentCount = item?.comments.length ?? 0
  useEffect(() => {
    if (!item) return
    requestAnimationFrame(() => {
      const el = scrollRef.current
      if (el) el.scrollTop = el.scrollHeight
    })
  }, [item, commentCount])

  const submit = useCallback(
    async (op: () => Promise<void>): Promise<void> => {
      if (busy) return
      setBusy(true)
      setActionError(null)
      try {
        await op()
        onChanged()
      } catch (e) {
        setActionError(e instanceof Error ? e.message : String(e))
      } finally {
        setBusy(false)
      }
    },
    [busy, onChanged],
  )

  if (error) {
    return <div className="flex-1 px-4 py-3 text-[11px] text-[var(--color-status-error-soft)]">Failed to load item: {error}</div>
  }
  if (!item) {
    return (
      <div className="flex-1 flex items-center justify-center text-xs text-[var(--color-text-muted)]">
        Loading thread…
      </div>
    )
  }

  const canTapOptions = optionsActionable(item)
  const openItem = item.status === 'waiting' || item.status === 'answered'

  // Always a plain comment — the daemon lands human comments in the
  // asking session, and the first one on a waiting ask answers it.
  const sendReply = (): void => {
    const text = reply.trim()
    if (!text) return
    void submit(async () => {
      await commentFeedback(item.id, text)
      setReply('')
    })
  }

  return (
    <div className="flex-1 flex flex-col min-h-0">
      <div ref={scrollRef} className="flex-1 overflow-y-auto min-h-0 px-4 py-3">
        {/* The ask body (title is in the header; body adds the context). */}
        {item.body && (
          <div className="mb-3 px-3 py-2 bg-white/[0.03] border border-[var(--color-border)] text-xs text-[var(--color-text-secondary)] whitespace-pre-wrap break-words">
            {item.body}
          </div>
        )}

        {/* Structured options — one-tap buttons while waiting: tapping
            posts the label as a comment (which answers a waiting ask
            daemon-side); the accepted choice stays highlighted after. */}
        {item.options && item.options.length > 0 && (
          <div className="mb-3 flex flex-wrap gap-2">
            {item.options.map((opt) => {
              const accepted = item.answer === opt
              return (
                <button
                  key={opt}
                  type="button"
                  disabled={!canTapOptions || busy}
                  onClick={() => void submit(() => commentFeedback(item.id, opt))}
                  className={`px-3 py-1.5 text-[11px] font-medium border transition-colors ${
                    accepted
                      ? 'border-[var(--color-accent)] bg-[var(--color-accent)]/15 text-[var(--color-text-primary)]'
                      : canTapOptions
                        ? 'border-[var(--color-border)] text-[var(--color-text-secondary)] hover:border-[var(--color-accent)] hover:text-[var(--color-text-primary)] cursor-pointer'
                        : 'border-[var(--color-border)] text-[var(--color-text-muted)] opacity-50'
                  } disabled:cursor-not-allowed`}
                >
                  {opt}
                </button>
              )
            })}
          </div>
        )}

        {/* Thread. The first comment is the ask itself (seeded by the
            daemon in the agent's voice) — rendered like any message. */}
        <div className="flex flex-col gap-2">
          {item.comments.map((c, i) => {
            const isOwner = c.author === 'owner'
            return (
              <div key={`${c.at}-${i}`} className="flex flex-col">
                <div className="flex items-baseline gap-2">
                  <span className={`text-[10px] font-semibold ${isOwner ? 'text-[var(--color-accent)]' : 'text-[var(--color-text-secondary)]'}`}>
                    {isOwner ? 'You' : c.author}
                  </span>
                  <span className="text-[9px] text-[var(--color-text-muted)] tabular-nums">
                    {formatRelativeTime(c.at, nowSec)}
                  </span>
                </div>
                <div className="text-xs text-[var(--color-text-primary)] whitespace-pre-wrap break-words mt-0.5">
                  {c.body}
                </div>
              </div>
            )
          })}
        </div>
      </div>

      {/* Reply box + status actions. */}
      <div className="border-t border-[var(--color-border)] px-4 py-3 flex-shrink-0">
        {actionError && <div className="mb-2 text-[11px] text-[var(--color-status-error-soft)]">{actionError}</div>}
        <textarea
          value={reply}
          onChange={(e) => setReply(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === 'Enter' && (e.metaKey || e.ctrlKey)) {
              e.preventDefault()
              sendReply()
            }
          }}
          placeholder="Add a comment — it lands in the agent's session"
          rows={2}
          className="w-full px-2.5 py-2 text-xs bg-[var(--color-bg-elevated)] text-[var(--color-text-primary)] border border-[var(--color-border)] outline-none focus:border-[var(--color-accent)] resize-none placeholder:text-[var(--color-text-muted)]"
        />
        <div className="flex items-center gap-2 mt-2">
          <button
            type="button"
            disabled={busy || reply.trim().length === 0}
            onClick={sendReply}
            className="px-3 py-1.5 text-[11px] font-medium bg-[var(--color-accent)]/15 text-[var(--color-text-primary)] hover:bg-[var(--color-accent)]/25 disabled:opacity-50 disabled:cursor-not-allowed transition-colors cursor-pointer flex items-center gap-1.5"
          >
            Comment
            <span className="text-[9px] font-mono text-[var(--color-text-muted)]">
              <KeyCombo combo="⌘⏎" />
            </span>
          </button>
          <div className="flex-1" />
          {openItem && (
            <>
              <button
                type="button"
                disabled={busy}
                onClick={() => void submit(() => resolveFeedback(item.id, 'resolved'))}
                className="px-3 py-1.5 text-[11px] text-[var(--color-text-secondary)] hover:text-[var(--color-text-primary)] hover:bg-white/[0.06] disabled:opacity-50 transition-colors cursor-pointer"
                title="Mark resolved — you got what you needed"
              >
                Resolve
              </button>
              <button
                type="button"
                disabled={busy}
                onClick={() => void submit(() => resolveFeedback(item.id, 'dismissed'))}
                className="px-3 py-1.5 text-[11px] text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)] hover:bg-white/[0.06] disabled:opacity-50 transition-colors cursor-pointer"
                title="Dismiss without an answer"
              >
                Dismiss
              </button>
            </>
          )}
        </div>
      </div>
    </div>
  )
}

// ── Agent tab (the asking session's terminal, in place) ──────────────────────────────────────────────────────────

interface LiveSessionRow {
  sessionId: string
  agentName: string
  command: string | null
  args: string[]
  cwd: string
  isV2: boolean
}

type TermPhase =
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
  const rows = await daemonCliGet<LiveSessionRow[]>('sessions/list-for-workspace', { path: '/' })
  return Array.isArray(rows) ? rows : []
}

function TerminalTab({
  sessionId,
  sessionKind,
  projectId,
  projectPath,
  feedbackId,
}: {
  sessionId: string | null
  sessionKind: 'canonical' | 'sandbox' | null
  projectId: string
  projectPath: string | null
  feedbackId: string
}): React.JSX.Element {
  const [phase, setPhase] = useState<TermPhase>(
    sessionId ? { kind: 'checking' } : { kind: 'none' },
  )

  // Resolve whether the asking session is live and which daemon
  // agent_name key to attach to:
  //  - sandbox cells register under a host-minted `api-<...>` key whose
  //    daemon sessionId IS the feedback row's session id → match by id.
  //  - canonical sessions run under the workspace's bare project-id key
  //    (the AgentChatPane attach key); the feedback row carries the
  //    CONVERSATION id, so liveness is checked via lookup-by-agent.
  const resolve = useCallback(async (): Promise<TermPhase> => {
    if (!sessionId) return { kind: 'none' }
    try {
      const live = await fetchLiveSessions()
      const match = live.find((r) => r.sessionId === sessionId)
      if (match) {
        return { kind: 'live', agentName: match.agentName, cwd: match.cwd, sessionId: match.sessionId }
      }
      if (sessionKind === 'canonical' && projectPath) {
        const lookup = await daemonCliGet<{ sessionAlive: boolean; sessionId: string | null }>(
          'sessions/lookup-by-agent',
          { agent: projectId },
        )
        if (lookup.sessionAlive) {
          return {
            kind: 'live',
            agentName: projectId,
            cwd: projectPath,
            sessionId: lookup.sessionId ?? undefined,
          }
        }
      }
      const wakeable =
        projectPath !== null && (sessionKind === 'canonical' || sessionKind === 'sandbox')
      return { kind: 'dormant', wakeable }
    } catch (e) {
      return { kind: 'error', message: e instanceof Error ? e.message : String(e) }
    }
  }, [sessionId, sessionKind, projectId, projectPath])

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

  // Wake a dormant session via the existing wake paths, then attach.
  const wake = useCallback(async (): Promise<void> => {
    if (!projectPath || !sessionId) return
    setPhase({ kind: 'waking' })
    try {
      if (sessionKind === 'canonical') {
        // Daemon-owned find-or-spawn under the canonical project-id key —
        // the same wake AgentChatPane rides.
        await daemonCliPost('workspace/ensure-pinned-chat', { project: projectPath })
        setPhase({ kind: 'live', agentName: projectId, cwd: projectPath })
        return
      }
      // Sandbox: re-mount the cell's persistent layer + resume, then poll
      // the live list until the session registers (the reopen returns as
      // soon as the relaunch is accepted).
      await daemonCliPost('sandbox/reopen', {
        project_path: projectPath,
        session_id: sessionId,
      })
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
  }, [projectPath, sessionId, sessionKind, projectId])

  if (phase.kind === 'none') {
    return (
      <EmptyTermState
        title="No session attached"
        detail="This ask was filed outside a known session, so there is no terminal to show."
      />
    )
  }
  if (phase.kind === 'checking') {
    return <EmptyTermState title="Checking session…" />
  }
  if (phase.kind === 'waking') {
    return <EmptyTermState title="Waking session…" />
  }
  if (phase.kind === 'error') {
    return (
      <div className="flex-1 flex flex-col items-center justify-center gap-3 px-6 text-center">
        <p className="text-xs text-[var(--color-status-error-soft)] max-w-[48ch]">{phase.message}</p>
        <button
          type="button"
          onClick={() => {
            setPhase({ kind: 'checking' })
            void resolve().then(setPhase)
          }}
          className="px-3 py-1.5 text-[11px] font-medium bg-white/[0.06] text-[var(--color-text-primary)] hover:bg-[var(--color-wash-2)] transition-colors cursor-pointer"
        >
          Retry
        </button>
      </div>
    )
  }
  if (phase.kind === 'dormant') {
    return (
      <div className="flex-1 flex flex-col items-center justify-center gap-3 px-6 text-center">
        <p className="text-xs font-semibold text-[var(--color-text-primary)]">Session is dormant</p>
        <p className="text-[11px] text-[var(--color-text-muted)] max-w-[44ch]">
          The asking session isn&apos;t running right now.
          {phase.wakeable ? ' Wake it to attach its terminal here.' : ''}
        </p>
        {phase.wakeable && (
          <button
            type="button"
            onClick={() => void wake()}
            className="px-3 py-1.5 text-[11px] font-medium bg-[var(--color-accent)]/15 text-[var(--color-text-primary)] hover:bg-[var(--color-accent)]/25 transition-colors cursor-pointer"
          >
            Wake session
          </button>
        )}
      </div>
    )
  }

  // phase.kind === 'live' — attach in place. attachAgentName keys the
  // idempotent v2/spawn to the EXISTING daemon session (reused:true), so
  // this never mints a duplicate PTY.
  return (
    <div className="flex-1 min-h-0">
      <TerminalPane
        terminalId={`feedback-term:${feedbackId}`}
        cwd={phase.cwd}
        attachAgentName={phase.agentName}
        sessionId={phase.sessionId}
      />
    </div>
  )
}

function EmptyTermState({ title, detail }: { title: string; detail?: string }): React.JSX.Element {
  return (
    <div className="flex-1 flex flex-col items-center justify-center gap-2 px-6 text-center">
      <p className="text-xs text-[var(--color-text-secondary)]">{title}</p>
      {detail && <p className="text-[11px] text-[var(--color-text-muted)] max-w-[44ch]">{detail}</p>}
    </div>
  )
}
