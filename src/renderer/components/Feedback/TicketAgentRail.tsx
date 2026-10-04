// The ticket's chat rail — the pre-redesign ticket chat surface, restored
// (Rosson, after the 0.43.2 board review: the rail had lost its compose
// box). It is the old FeedbackItemView's two tabs, moved to the right of
// the brief:
//
//   Thread — the ticket's comment thread (ThreadTab, from
//     `FeedbackItemView.tsx` before 93458b8f): the comments, then the
//     quick-answer buttons (the brief's Options, or the structured
//     `--options`) ABOVE the text area, then the text area and Send.
//     - An option pick posts `optionPick: true` → the daemon records it as
//       the answer (answered).
//     - Free text posts a plain comment → the ticket goes to
//       needs_discussion.
//     Both land in the agent's session (the daemon injects and wakes it),
//     exactly as the old Comment button did.
//   Agent — the asking session's terminal in place (the old TerminalTab):
//     find the session (live-by-id, or the workspace's pinned chat via D6),
//     attach with `attachAgentName` (the idempotent v2/spawn reuses the
//     existing PTY), and mark the workspace Active so the reaper spares it
//     (PRD §4.3.1). A dormant session offers Wake.
//
// Status, reassign and the rail's open/closed toggle live in the ticket
// header (FeedbackItemView); the rail has no action bar.

import React, { useCallback, useEffect, useRef, useState } from 'react'
import { PrimaryRoom } from '@/components/Room/PrimaryRoom'
import { daemonCliGet, daemonCliPost } from '@/lib/daemon-cli'
import { useSettingsStore } from '@/stores/settings'
import { TerminalPane } from '@/kessel-term/TerminalPane'
import { PageLiveContext } from '@/contexts/TabVisibilityContext'
import { formatRelativeTime } from '@/lib/format-relative-time'
import { KeyCombo } from '@/components/KeySymbol'
import { activateProject } from '@/stores/projects'
import {
  activateOnLiveSessionAttach,
  wakeCanonicalMemberSession,
} from '@/components/Projects/wake-member-session'
import { SelectableRegion, clearStuckBodyUserSelect } from '@/components/common/SelectableText'
import { ChatMessage } from '@/components/common/ChatMessage'
import { hasSelectionWithin } from '@/components/FileViewerPane/FileViewerPane'
import { clearTicketDraft, getTicketDraft, setTicketDraft } from '@/lib/composer-drafts'
import { primaryScope } from '@/kessel/server-scope'
import {
  askingSessionWakeAction,
  commentFeedback,
  type FeedbackSessionKind,
  type FeedbackShow,
  type QuickAnswerOption,
} from './feedback-api'

export type RailTab = 'thread' | 'agent'

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

export function TicketAgentRail({
  feedbackId,
  item,
  error,
  nowSec,
  options,
  optionsLive,
  onChanged,
  projectId,
  projectPath,
  sessionId,
  sessionKind,
  canonicalSessionId,
}: {
  feedbackId: string
  /** The loaded ticket (null while loading). */
  item: FeedbackShow | null
  error: string | null
  nowSec: number
  /** Quick answers: the brief's Options, else the structured `--options`. */
  options: QuickAnswerOption[]
  /** Picks are live until the ticket is closed. */
  optionsLive: boolean
  /** After a successful send: refetch the ticket + the list. */
  onChanged: () => void
  projectId: string
  projectPath: string | null
  sessionId: string | null
  sessionKind: FeedbackSessionKind
  canonicalSessionId: string | null | undefined
}): React.JSX.Element {
  const [tab, setTab] = useState<RailTab>('thread')

  return (
    <aside
      data-testid="ticket-agent-rail"
      className="flex flex-col min-h-0 border-l border-[var(--color-border)] flex-shrink-0 bg-[var(--color-bg)]"
      style={{ width: 'min(640px, 48vw)' }}
    >
      <div role="tablist" className="flex items-center gap-1 px-3 border-b border-[var(--color-border)] flex-shrink-0">
        {(['thread', 'agent'] as const).map((t) => (
          <button
            key={t}
            type="button"
            role="tab"
            aria-selected={tab === t}
            data-testid={`ticket-rail-tab-${t}`}
            onClick={() => setTab(t)}
            className={`px-3 py-2 text-[11px] font-medium border-b-2 -mb-px transition-colors cursor-pointer ${
              tab === t
                ? 'border-[var(--color-accent)] text-[var(--color-text-primary)]'
                : 'border-transparent text-[var(--color-text-muted)] hover:text-[var(--color-text-secondary)]'
            }`}
          >
            {t === 'thread' ? 'Thread' : 'Agent'}
          </button>
        ))}
      </div>

      {tab === 'thread' ? (
        <ThreadTab
          key={`thread-${feedbackId}`}
          item={item}
          error={error}
          nowSec={nowSec}
          ticketId={feedbackId}
          options={options}
          optionsLive={optionsLive}
          onChanged={onChanged}
        />
      ) : (
        <AgentTab
          feedbackId={feedbackId}
          projectId={projectId}
          projectPath={projectPath}
          sessionId={sessionId}
          sessionKind={sessionKind}
          canonicalSessionId={canonicalSessionId}
        />
      )}
    </aside>
  )
}

// ── Thread tab (the restored ThreadTab) ──────────────────────────────────

export function ThreadTab({
  item,
  error,
  nowSec,
  ticketId,
  options,
  optionsLive,
  onChanged,
}: {
  item: FeedbackShow | null
  error: string | null
  nowSec: number
  ticketId: string
  options: QuickAnswerOption[]
  optionsLive: boolean
  onChanged: () => void
}): React.JSX.Element {
  // Drafts survive unmount (leave ticket / switch tabs or pages).
  const [reply, setReply] = useState(() => getTicketDraft(ticketId))
  const [busy, setBusy] = useState(false)
  const [actionError, setActionError] = useState<string | null>(null)
  const [deliveryMiss, setDeliveryMiss] = useState<string | null>(null)
  const scrollRef = useRef<HTMLDivElement>(null)
  const textareaRef = useRef<HTMLTextAreaElement>(null)
  // Match Code Editor → Appearance → Font Size (default 12).
  const editorFontSize = useSettingsStore((s) => s.editor.fontSize) || 13

  // Focus the reply box ONCE per ticket id when the thread first becomes
  // ready — not on every live `item` refetch. Re-focus steals the caret
  // and kills drag-selection on message bodies.
  const focusedForTicketRef = useRef<string | null>(null)
  useEffect(() => {
    clearStuckBodyUserSelect()
    if (!item || error) return
    if (focusedForTicketRef.current === ticketId) return
    focusedForTicketRef.current = ticketId
    let cancelled = false
    let innerRaf = 0
    const outerRaf = window.requestAnimationFrame(() => {
      innerRaf = window.requestAnimationFrame(() => {
        if (cancelled) return
        if (hasSelectionWithin(scrollRef.current)) return
        textareaRef.current?.focus({ preventScroll: true })
      })
    })
    const t = window.setTimeout(() => {
      if (cancelled) return
      if (hasSelectionWithin(scrollRef.current)) return
      textareaRef.current?.focus({ preventScroll: true })
    }, 50)
    return () => {
      cancelled = true
      cancelAnimationFrame(outerRaf)
      cancelAnimationFrame(innerRaf)
      window.clearTimeout(t)
    }
  }, [ticketId, item, error])

  // Auto-grow the reply field with content.
  useEffect(() => {
    const el = textareaRef.current
    if (!el) return
    el.style.height = 'auto'
    el.style.height = `${Math.min(el.scrollHeight, 240)}px`
  }, [reply])

  const setReplyAndDraft = (text: string): void => {
    setReply(text)
    setTicketDraft(ticketId, text)
  }

  // Keep the newest message in view when the thread grows (and on first
  // load) — but not while the user is drag-selecting.
  const commentCount = item?.comments.length ?? 0
  const prevCommentCountRef = useRef(-1)
  useEffect(() => {
    if (!item) return
    const grew = commentCount > prevCommentCountRef.current
    prevCommentCountRef.current = commentCount
    if (!grew) return
    if (hasSelectionWithin(scrollRef.current)) return
    requestAnimationFrame(() => {
      if (hasSelectionWithin(scrollRef.current)) return
      const el = scrollRef.current?.parentElement
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
    return (
      <div className="flex-1 px-4 py-3 text-[11px] text-[var(--color-status-error-soft)] selectable-copy">
        Failed to load ticket: {error}
      </div>
    )
  }
  if (!item) {
    return (
      <div className="flex-1 flex items-center justify-center text-xs text-[var(--color-text-muted)]">
        Loading thread…
      </div>
    )
  }

  // Every message lands in the asking session (the daemon injects it and
  // wakes the agent). A pick is the answer; free text starts a discussion.
  // Keep delivered/deliveryReason (D8); a quiet miss is not a store error.
  const send = async (text: string, optionPick: boolean): Promise<void> => {
    const res = await commentFeedback(item.id, text, { optionPick })
    setDeliveryMiss(res.delivered === false ? res.deliveryReason ?? 'not delivered' : null)
  }

  const sendReply = (): void => {
    const text = reply.trim()
    if (!text) return
    void submit(async () => {
      await send(text, false)
      setReplyAndDraft('')
      clearTicketDraft(ticketId)
    })
  }

  return (
    <div className="flex-1 flex flex-col min-h-0" data-testid="ticket-rail-thread">
      <SelectableRegion className="flex-1 overflow-y-auto overflow-x-hidden min-h-0 px-4 py-3">
        <div ref={scrollRef} className="min-h-full" data-ticket-thread={ticketId}>
          <div className="flex flex-col gap-2.5">
            {item.comments.map((c, i) => {
              const isOwner = c.author === 'owner'
              return (
                <ChatMessage
                  key={`${c.at}-${i}`}
                  author={isOwner ? 'You' : c.author}
                  isOwner={isOwner}
                  timeLabel={formatRelativeTime(c.at, nowSec)}
                  body={c.body}
                  fontSize={editorFontSize}
                />
              )
            })}
          </div>
        </div>
      </SelectableRegion>

      <div data-testid="ticket-compose" className="border-t border-[var(--color-border)] px-4 py-3 flex-shrink-0">
        {actionError && (
          <div className="mb-2 text-[11px] text-[var(--color-status-error-soft)] selectable-copy">{actionError}</div>
        )}
        {deliveryMiss && (
          <p className="mb-2 text-[10px] text-[var(--color-text-muted)] selectable-copy">
            Saved. The agent did not receive it ({deliveryMiss}).
          </p>
        )}

        {options.length > 0 && (
          <div data-testid="ticket-quick-answers" className="mb-2 flex flex-wrap gap-2">
            {options.map((opt) => {
              const accepted = item.answer === opt.answer
              return (
                <button
                  key={opt.answer}
                  type="button"
                  data-testid="ticket-quick-answer"
                  disabled={!optionsLive || busy}
                  title={opt.detail === opt.label ? 'Send this as the answer' : opt.detail}
                  onClick={() => void submit(() => send(opt.answer, true))}
                  className={`max-w-full truncate px-3 py-1.5 text-[11px] font-medium border transition-colors ${
                    accepted
                      ? 'border-[var(--color-accent)] bg-[var(--color-accent)]/15 text-[var(--color-text-primary)]'
                      : optionsLive
                        ? 'border-[var(--color-border)] text-[var(--color-text-secondary)] hover:border-[var(--color-accent)] hover:text-[var(--color-text-primary)] cursor-pointer'
                        : 'border-[var(--color-border)] text-[var(--color-text-muted)] opacity-50'
                  } disabled:cursor-not-allowed`}
                >
                  {opt.label}
                </button>
              )
            })}
          </div>
        )}

        <textarea
          ref={textareaRef}
          data-testid="ticket-compose-input"
          value={reply}
          onChange={(e) => setReplyAndDraft(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === 'Enter' && (e.metaKey || e.ctrlKey)) {
              e.preventDefault()
              sendReply()
            }
          }}
          placeholder={
            options.length > 0
              ? 'Pick an option above to answer, or write a message — it starts a discussion'
              : 'Write a message — it lands in the agent’s session'
          }
          rows={2}
          className="min-w-0 w-full max-w-full px-2.5 py-2 bg-[var(--color-bg-elevated)] text-[var(--color-text-primary)] border border-[var(--color-border)] outline-none focus:border-[var(--color-accent)] resize-none overflow-x-hidden overflow-y-auto break-words placeholder:text-[var(--color-text-muted)] selectable-copy"
          style={{ fontSize: editorFontSize }}
        />
        <div className="flex items-center gap-2 mt-2">
          <div className="flex-1" />
          <button
            type="button"
            data-testid="ticket-compose-send"
            disabled={busy || reply.trim().length === 0}
            onClick={sendReply}
            className="px-3 py-1.5 text-[11px] font-medium bg-[var(--color-accent)]/15 text-[var(--color-text-primary)] hover:bg-[var(--color-accent)]/25 disabled:opacity-50 disabled:cursor-not-allowed transition-colors cursor-pointer flex items-center gap-1.5"
          >
            Send
            <span className="text-[9px] font-mono text-[var(--color-text-muted)]">
              <KeyCombo combo="⌘⏎" />
            </span>
          </button>
        </div>
      </div>
    </div>
  )
}

// ── Agent tab (the asking session's terminal, in place) ──────────────────

function AgentTab({
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
  if (phase.kind === 'checking') return <EmptyTermState title="Checking session…" />
  if (phase.kind === 'waking') return <EmptyTermState title="Waking session…" />
  if (phase.kind === 'error') {
    return (
      <div className="flex-1 flex flex-col items-center justify-center gap-3 px-6 text-center">
        <p className="text-xs text-[var(--color-status-error-soft)] max-w-[48ch]">{phase.message}</p>
        <button
          type="button"
          onClick={retry}
          className="px-3 py-1.5 text-[11px] font-medium bg-white/[0.06] text-[var(--color-text-primary)] hover:bg-[var(--color-wash-2)] transition-colors cursor-pointer"
        >
          Retry
        </button>
      </div>
    )
  }
  if (phase.kind === 'dormant') {
    return (
      <div
        data-testid="ticket-agent-dormant"
        className="flex-1 flex flex-col items-center justify-center gap-3 px-6 text-center"
      >
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

  // Live — attach in place. attachAgentName keys the idempotent v2/spawn
  // to the EXISTING daemon session (reused:true), so this never mints a
  // duplicate PTY.
  return (
    <PageLiveContext.Provider value={true}>
      <div className="flex-1 min-h-0" data-testid="ticket-agent-terminal">
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
      <p className="text-xs text-[var(--color-text-secondary)]">{title}</p>
      {detail && <p className="text-[11px] text-[var(--color-text-muted)] max-w-[44ch]">{detail}</p>}
    </div>
  )
}
