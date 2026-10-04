// Tickets board — the ticket detail (0.43.2 quick redesign, Rosson).
//
//   - A slim header that stays put: the title, "from {agent} → assigned to
//     {person}", the status (also the status menu), Expand and Open in
//     window.
//   - Below it, scrolling: the short summary (`--body`), the HTML brief
//     (it takes the stage), then comments and history, collapsed by
//     default when there is a brief.
//   - An action bar pinned to the bottom: quick-answer buttons (one per
//     brief Options item, or per structured `--options`), then Answer,
//     Resolve, Reassign, Chat with agent.
//
// Status model (daemon-first, feedback_routes.rs): a quick-answer pick
// posts `optionPick: true` → the ticket is answered. A typed Answer is free
// text → the ticket goes to needs_discussion until the agent settles it.
// Both land in the agent's session (wake=true) exactly as before.
//
// Chat with agent opens the right-hand rail (TicketAgentRail): the agent's
// Thread plus its terminal, reusing the Agents page session surfaces.
//
// An UNLINKED ticket (workspace removed, TB18) is read-only: no answer,
// no options, no reassign, no chat — only Resolve and Dismiss.

import React, { useCallback, useEffect, useRef, useState } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { daemonCliGet } from '@/lib/daemon-cli'
import { useSettingsStore } from '@/stores/settings'
import { formatRelativeTime } from '@/lib/format-relative-time'
import { KeyCombo } from '@/components/KeySymbol'
import { isWebClient } from '@/lib/is-web'
import {
  assignFeedback,
  commentFeedback,
  fetchFeedbackBrief,
  fetchFeedbackShow,
  formatFiledDate,
  isUnlinked,
  quickAnswerOptions,
  resolveFeedback,
  UNLINKED_WORKSPACE_LABEL,
  type FeedbackBrief,
  type FeedbackListRow,
  type FeedbackShow,
  type FeedbackStatus,
} from './feedback-api'
import { HtmlBriefBadge, PriorityBadge, StatusBadge } from './badges'
import { BriefFrame } from './BriefFrame'
import { CardStatusDropdown, cardAssigneeNames } from './TicketCard'
import { TicketAgentRail } from './TicketAgentRail'
import { SelectableRegion, clearStuckBodyUserSelect } from '@/components/common/SelectableText'
import { ChatMessage, ChatMessageBody } from '@/components/common/ChatMessage'
import { hasSelectionWithin } from '@/components/FileViewerPane/FileViewerPane'
import { clearTicketDraft, getTicketDraft, setTicketDraft } from '@/lib/composer-drafts'
import { primaryScope } from '@/kessel/server-scope'

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
  /** Rendered inside its own window: no "Open in window" button. */
  inOwnWindow?: boolean
}

/** Ticket id → its HTML brief. Briefs never change after filing (H8), so
 *  one fetch per ticket per window is enough (H39). Exported for tests. */
export const briefCache = new Map<string, FeedbackBrief>()

/** The brief box fills the detail pane (the brief takes the stage). */
const BRIEF_HEIGHT_CLASS = 'h-[max(320px,calc(100vh-300px))]'

/** Quick answers stay live until the ticket is closed. */
export function quickAnswersLive(status: FeedbackStatus): boolean {
  return status !== 'resolved' && status !== 'dismissed'
}

/** Open the ticket in its own app window (desktop only). */
export function openTicketWindow(ticketId: string): Promise<unknown> {
  return invoke('window_open_ticket', { ticketId })
}

export function FeedbackItemView({
  id,
  listRow,
  nowSec,
  revision,
  onMutated,
  inOwnWindow = false,
}: FeedbackItemViewProps): React.JSX.Element {
  const [item, setItem] = useState<FeedbackShow | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [expanded, setExpanded] = useState(false)
  const [railOpen, setRailOpen] = useState(false)

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

  // Event-driven refresh: any feedback event bumps the store revision and
  // the OPEN ticket refetches. Bursts coalesce on a trailing 300ms window.
  // Deferred while the user is drag-selecting ticket text.
  const seenRevision = useRef(revision)
  useEffect(() => {
    if (revision === seenRevision.current) return
    seenRevision.current = revision
    const timer = setTimeout(() => {
      const thread = document.querySelector(`[data-ticket-thread="${id}"]`) as HTMLElement | null
      if (hasSelectionWithin(thread)) return
      void load()
    }, 300)
    return () => clearTimeout(timer)
  }, [revision, load, id])

  // HTML brief (prd-ticket-html-brief-v1 H39): fetched ONCE per ticket via
  // `show?brief=1`; the event refetch stays on plain `show`.
  const hasBrief = item?.hasBrief === true || listRow.hasBrief === true
  const [brief, setBrief] = useState<FeedbackBrief | null>(() => briefCache.get(id) ?? null)
  const [briefError, setBriefError] = useState<string | null>(null)
  const briefRequested = useRef<string | null>(null)
  useEffect(() => {
    if (!hasBrief) return
    const cached = briefCache.get(id)
    if (cached) {
      setBrief(cached)
      return
    }
    if (briefRequested.current === id) return
    briefRequested.current = id
    fetchFeedbackBrief(id).then(
      (b) => {
        if (b) briefCache.set(id, b)
        if (b) {
          setBrief(b)
          setBriefError(null)
        } else {
          setBriefError('the server returned no brief for this ticket')
        }
      },
      (e: unknown) => {
        briefRequested.current = null
        setBriefError(e instanceof Error ? e.message : String(e))
      },
    )
  }, [hasBrief, id])

  const unlinked = isUnlinked(listRow)
  const projectPath = unlinked ? null : item?.projectPath ?? listRow.projectPath
  const workspaceName = unlinked
    ? UNLINKED_WORKSPACE_LABEL
    : item?.workspace ?? listRow.projectName ?? UNLINKED_WORKSPACE_LABEL
  const view = item ?? listRow
  const statusRow: FeedbackListRow = { ...listRow, status: view.status }
  const assigneeNames = cardAssigneeNames(view.assignees)
  const canOpenWindow = !inOwnWindow && !isWebClient()

  return (
    <div className="flex-1 flex min-h-0 min-w-0" data-testid="ticket-detail-wrap">
      <div className="flex-1 flex flex-col min-h-0 min-w-0" data-testid="ticket-detail">
        {/* Slim header — outside the scroller, so it stays put. */}
        <div
          data-testid="ticket-detail-header"
          className="px-4 py-2 border-b border-[var(--color-border)] flex-shrink-0 bg-[var(--color-bg)]"
        >
          <div className="flex items-center gap-2 min-w-0">
            <span
              data-testid="ticket-detail-title"
              className="text-sm font-medium text-[var(--color-text-primary)] truncate flex-1 selectable-copy"
              title={view.title}
            >
              {view.title}
            </span>
            {hasBrief && (
              <button
                type="button"
                data-testid="ticket-expand"
                disabled={!brief}
                onClick={() => setExpanded(true)}
                className="px-2 py-0.5 text-[10px] text-[var(--color-text-secondary)] border border-[var(--color-border)] hover:text-[var(--color-text-primary)] hover:border-[var(--color-text-muted)] disabled:opacity-40 cursor-pointer flex-shrink-0"
              >
                Expand
              </button>
            )}
            {canOpenWindow && (
              <button
                type="button"
                data-testid="ticket-open-window"
                onClick={() => {
                  openTicketWindow(id).catch((e: unknown) =>
                    console.warn('[tickets] open in window failed', e),
                  )
                }}
                className="px-2 py-0.5 text-[10px] text-[var(--color-text-secondary)] border border-[var(--color-border)] hover:text-[var(--color-text-primary)] hover:border-[var(--color-text-muted)] cursor-pointer flex-shrink-0"
              >
                Open in window
              </button>
            )}
          </div>
          <div className="mt-1 flex items-center gap-1.5 min-w-0 text-[10px] text-[var(--color-text-muted)]">
            {unlinked ? <StatusBadge status={view.status} /> : <CardStatusDropdown row={statusRow} onMutated={() => { void load(); onMutated() }} />}
            <PriorityBadge priority={view.priority} />
            {hasBrief && <HtmlBriefBadge />}
            <span data-testid="ticket-detail-byline" className="truncate selectable-copy">
              from <span className="text-[var(--color-text-secondary)]">{view.agentName}</span>
              {' → '}
              {assigneeNames.length > 0 ? (
                <>assigned to <span className="text-[var(--color-text-secondary)]">{assigneeNames.join(', ')}</span></>
              ) : (
                <span className="italic">unassigned</span>
              )}
              <span className="opacity-70">
                {' '}· {workspaceName} · asked{' '}
                {unlinked ? formatFiledDate(view.createdAt) : formatRelativeTime(view.createdAt, nowSec)}
              </span>
            </span>
          </div>
        </div>

        <TicketBody
          key={`body-${id}`}
          item={item}
          error={error}
          nowSec={nowSec}
          ticketId={id}
          unlinked={unlinked}
          hasBrief={hasBrief}
          brief={brief}
          briefError={briefError}
          title={view.title}
          expanded={expanded}
          onExpandedChange={setExpanded}
          railOpen={railOpen}
          onToggleRail={() => setRailOpen((o) => !o)}
          onChanged={() => {
            void load()
            onMutated()
          }}
        />
      </div>

      {railOpen && !unlinked && (
        <TicketAgentRail
          feedbackId={id}
          agentName={view.agentName}
          projectId={view.projectId}
          projectPath={projectPath}
          sessionId={view.sessionId}
          sessionKind={view.sessionKind}
          canonicalSessionId={item?.canonicalSessionId}
          onClose={() => setRailOpen(false)}
        />
      )}
    </div>
  )
}

// ── Body: summary + brief + comments, then the pinned action bar ─────────

function TicketBody({
  item,
  error,
  nowSec,
  ticketId,
  unlinked,
  hasBrief,
  brief,
  briefError,
  title,
  expanded,
  onExpandedChange,
  railOpen,
  onToggleRail,
  onChanged,
}: {
  item: FeedbackShow | null
  error: string | null
  nowSec: number
  ticketId: string
  unlinked: boolean
  hasBrief: boolean
  brief: FeedbackBrief | null
  briefError: string | null
  title: string
  expanded: boolean
  onExpandedChange: (v: boolean) => void
  railOpen: boolean
  onToggleRail: () => void
  onChanged: () => void
}): React.JSX.Element {
  const [answering, setAnswering] = useState(false)
  const [reply, setReply] = useState(() => getTicketDraft(ticketId))
  const [busy, setBusy] = useState(false)
  const [actionError, setActionError] = useState<string | null>(null)
  const [deliveryMiss, setDeliveryMiss] = useState<string | null>(null)
  // Comments + history: collapsed by default under a brief (the brief is
  // the ticket); open when there is no brief (they are all there is).
  const [historyOpen, setHistoryOpen] = useState(!hasBrief)
  const [reassignOpen, setReassignOpen] = useState(false)
  const scrollRef = useRef<HTMLDivElement>(null)
  const textareaRef = useRef<HTMLTextAreaElement>(null)
  const editorFontSize = useSettingsStore((s) => s.editor.fontSize) || 13

  useEffect(() => {
    clearStuckBodyUserSelect()
  }, [ticketId])

  // A brief arriving after first paint collapses the history once.
  const collapsedForBrief = useRef(hasBrief)
  useEffect(() => {
    if (hasBrief && !collapsedForBrief.current) {
      collapsedForBrief.current = true
      setHistoryOpen(false)
    }
  }, [hasBrief])

  useEffect(() => {
    if (!answering) return
    textareaRef.current?.focus({ preventScroll: true })
  }, [answering])

  useEffect(() => {
    const el = textareaRef.current
    if (!el) return
    el.style.height = 'auto'
    el.style.height = `${Math.min(el.scrollHeight, 200)}px`
  }, [reply, answering])

  const setReplyAndDraft = (text: string): void => {
    setReply(text)
    setTicketDraft(ticketId, text)
  }

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
        Loading ticket…
      </div>
    )
  }

  const quick = unlinked ? [] : quickAnswerOptions(item, brief?.html)
  const quickLive = quickAnswersLive(item.status)
  const open =
    item.status === 'waiting' ||
    item.status === 'answered' ||
    item.status === 'planned' ||
    item.status === 'needs_discussion'

  // Every person's message lands in the agent's session. A pick is the
  // answer; free text starts a discussion (the daemon sets the status).
  const send = async (text: string, optionPick: boolean): Promise<void> => {
    const res = await commentFeedback(item.id, text, { optionPick })
    setDeliveryMiss(res.delivered === false ? res.deliveryReason ?? 'not delivered' : null)
  }

  const sendAnswer = (): void => {
    const text = reply.trim()
    if (!text) return
    void submit(async () => {
      await send(text, false)
      setReplyAndDraft('')
      clearTicketDraft(ticketId)
      setAnswering(false)
    })
  }

  const comments = item.comments ?? []

  return (
    <div className="flex-1 flex flex-col min-h-0">
      <SelectableRegion className="flex-1 overflow-y-auto overflow-x-hidden min-h-0 px-4 py-3">
        <div ref={scrollRef} className="min-h-full" data-ticket-thread={ticketId}>
          {item.body && (
            <div className="mb-3 px-3 py-2 bg-white/[0.03] border border-[var(--color-border)]">
              <ChatMessageBody text={item.body} style={{ fontSize: editorFontSize }} />
            </div>
          )}

          {hasBrief &&
            (brief ? (
              <BriefFrame
                brief={brief}
                title={title}
                hideToolbar
                expanded={expanded}
                onExpandedChange={onExpandedChange}
                heightClass={BRIEF_HEIGHT_CLASS}
              />
            ) : briefError ? (
              <div data-testid="brief-error" className="mb-3 text-[11px] text-[var(--color-status-error-soft)] selectable-copy">
                Failed to load the brief: {briefError}
              </div>
            ) : (
              <div data-testid="brief-loading" className="mb-3 text-[11px] text-[var(--color-text-muted)]">
                Loading brief…
              </div>
            ))}

          <button
            type="button"
            data-testid="ticket-history-toggle"
            aria-expanded={historyOpen}
            onClick={() => setHistoryOpen((o) => !o)}
            className="flex items-center gap-1.5 w-full py-1.5 text-[10px] font-semibold uppercase tracking-wider text-[var(--color-text-muted)] hover:text-[var(--color-text-secondary)] cursor-pointer"
          >
            <svg
              width="9"
              height="9"
              viewBox="0 0 24 24"
              fill="none"
              stroke="currentColor"
              strokeWidth={2.5}
              className={`transition-transform ${historyOpen ? 'rotate-90' : ''}`}
              aria-hidden
            >
              <path strokeLinecap="round" strokeLinejoin="round" d="M9 18l6-6-6-6" />
            </svg>
            Comments and history
            <span className="tabular-nums font-normal opacity-70">{comments.length}</span>
          </button>
          {historyOpen && (
            <div data-testid="ticket-history" className="flex flex-col gap-2.5 pt-1 pb-2">
              {comments.map((c, i) => {
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
          )}
        </div>
      </SelectableRegion>

      {unlinked ? (
        <div data-testid="unlinked-thread-footer" className="border-t border-[var(--color-border)] px-4 py-3 flex-shrink-0">
          {actionError && <div className="mb-2 text-[11px] text-[var(--color-status-error-soft)] selectable-copy">{actionError}</div>}
          <p className="mb-2 text-[10px] text-[var(--color-text-muted)]">
            This ticket&apos;s workspace was removed from this server, so no agent can receive a
            reply. Resolve or Dismiss it here.
          </p>
          {open && (
            <div className="flex items-center gap-2">
              <button
                type="button"
                disabled={busy}
                onClick={() => void submit(() => resolveFeedback(item.id, 'resolved'))}
                className="px-3 py-1.5 text-[11px] text-[var(--color-text-secondary)] hover:text-[var(--color-text-primary)] hover:bg-white/[0.06] disabled:opacity-50 transition-colors cursor-pointer"
              >
                Resolve
              </button>
              <button
                type="button"
                disabled={busy}
                onClick={() => void submit(() => resolveFeedback(item.id, 'dismissed'))}
                className="px-3 py-1.5 text-[11px] text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)] hover:bg-white/[0.06] disabled:opacity-50 transition-colors cursor-pointer"
              >
                Dismiss
              </button>
            </div>
          )}
        </div>
      ) : (
        <div data-testid="ticket-action-bar" className="border-t border-[var(--color-border)] px-4 py-2.5 flex-shrink-0 bg-[var(--color-bg)]">
          {actionError && <div className="mb-2 text-[11px] text-[var(--color-status-error-soft)] selectable-copy">{actionError}</div>}
          {deliveryMiss && (
            <p className="mb-2 text-[10px] text-[var(--color-text-muted)] selectable-copy">
              Saved. The agent did not receive it ({deliveryMiss}).
            </p>
          )}

          {quick.length > 0 && (
            <div data-testid="ticket-quick-answers" className="mb-2 flex flex-wrap gap-1.5">
              {quick.map((opt) => {
                const accepted = item.answer === opt.answer
                return (
                  <button
                    key={opt.answer}
                    type="button"
                    data-testid="ticket-quick-answer"
                    disabled={!quickLive || busy}
                    title={opt.detail === opt.label ? 'Send this as the answer' : opt.detail}
                    onClick={() => void submit(() => send(opt.answer, true))}
                    className={`max-w-full truncate px-2.5 py-1 text-[11px] font-medium border transition-colors ${
                      accepted
                        ? 'border-[var(--color-accent)] bg-[var(--color-accent)]/15 text-[var(--color-text-primary)]'
                        : quickLive
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

          {answering && (
            <div className="mb-2" data-testid="ticket-answer-box">
              <textarea
                ref={textareaRef}
                value={reply}
                onChange={(e) => setReplyAndDraft(e.target.value)}
                onKeyDown={(e) => {
                  if (e.key === 'Enter' && (e.metaKey || e.ctrlKey)) {
                    e.preventDefault()
                    sendAnswer()
                  }
                  if (e.key === 'Escape') {
                    e.stopPropagation()
                    setAnswering(false)
                  }
                }}
                placeholder={
                  quick.length > 0
                    ? 'Write a message — it starts a discussion. Pick an option above to answer.'
                    : 'Write a message — it lands in the agent’s session and starts a discussion'
                }
                rows={2}
                className="min-w-0 w-full max-w-full px-2.5 py-2 bg-[var(--color-bg-elevated)] text-[var(--color-text-primary)] border border-[var(--color-border)] outline-none focus:border-[var(--color-accent)] resize-none overflow-x-hidden overflow-y-auto break-words placeholder:text-[var(--color-text-muted)] selectable-copy"
                style={{ fontSize: editorFontSize }}
              />
              <div className="flex items-center gap-2 mt-1.5">
                <span className="text-[10px] text-[var(--color-text-muted)]">
                  The agent replies, then marks it answered or resolved.
                </span>
                <span className="flex-1" />
                <button
                  type="button"
                  onClick={() => setAnswering(false)}
                  className="px-2 py-1 text-[11px] text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)] cursor-pointer"
                >
                  Cancel
                </button>
                <button
                  type="button"
                  data-testid="ticket-answer-send"
                  disabled={busy || reply.trim().length === 0}
                  onClick={sendAnswer}
                  className="px-3 py-1 text-[11px] font-medium bg-[var(--color-accent)]/15 text-[var(--color-text-primary)] hover:bg-[var(--color-accent)]/25 disabled:opacity-50 disabled:cursor-not-allowed cursor-pointer flex items-center gap-1.5"
                >
                  Send
                  <span className="text-[9px] font-mono text-[var(--color-text-muted)]">
                    <KeyCombo combo="⌘⏎" />
                  </span>
                </button>
              </div>
            </div>
          )}

          <div className="flex items-center gap-1.5 relative">
            <ActionButton testId="ticket-action-answer" active={answering} onClick={() => setAnswering((a) => !a)}>
              Answer
            </ActionButton>
            <ActionButton
              testId="ticket-action-resolve"
              disabled={busy || item.status === 'resolved'}
              onClick={() => void submit(() => resolveFeedback(item.id, 'resolved'))}
            >
              Resolve
            </ActionButton>
            <div className="relative">
              <ActionButton testId="ticket-action-reassign" active={reassignOpen} onClick={() => setReassignOpen((o) => !o)}>
                Reassign
              </ActionButton>
              {reassignOpen && (
                <ReassignMenu
                  ticketId={item.id}
                  assignees={item.assignees ?? []}
                  onClose={() => setReassignOpen(false)}
                  onSaved={() => {
                    setReassignOpen(false)
                    onChanged()
                  }}
                />
              )}
            </div>
            <span className="flex-1" />
            <ActionButton testId="ticket-action-chat" active={railOpen} onClick={onToggleRail}>
              Chat with agent
            </ActionButton>
          </div>
        </div>
      )}
    </div>
  )
}

function ActionButton({
  testId,
  active = false,
  disabled = false,
  onClick,
  children,
}: {
  testId: string
  active?: boolean
  disabled?: boolean
  onClick: () => void
  children: React.ReactNode
}): React.JSX.Element {
  return (
    <button
      type="button"
      data-testid={testId}
      aria-pressed={active}
      disabled={disabled}
      onClick={onClick}
      className={`px-3 py-1.5 text-[11px] font-medium border transition-colors cursor-pointer disabled:opacity-50 disabled:cursor-not-allowed ${
        active
          ? 'border-[var(--color-accent)] bg-[var(--color-accent)]/15 text-[var(--color-text-primary)]'
          : 'border-[var(--color-border)] text-[var(--color-text-secondary)] hover:text-[var(--color-text-primary)] hover:border-[var(--color-text-muted)]'
      }`}
    >
      {children}
    </button>
  )
}

/** Reassign: multi-select from the server's users + `owner`, opening upward
 *  from the action bar. Replaces the whole assignee set. */
function ReassignMenu({
  ticketId,
  assignees,
  onClose,
  onSaved,
}: {
  ticketId: string
  assignees: string[]
  onClose: () => void
  onSaved: () => void
}): React.JSX.Element {
  const [candidates, setCandidates] = useState<string[]>(['owner'])
  const [local, setLocal] = useState<string[]>(assignees)
  const [saving, setSaving] = useState(false)
  const [saveError, setSaveError] = useState<string | null>(null)
  const rootRef = useRef<HTMLDivElement>(null)

  useEffect(() => {
    let cancelled = false
    void daemonCliGet<{ users?: Array<{ username: string }> }>(primaryScope(), 'users', {})
      .then((res) => {
        if (cancelled) return
        const names = (res.users ?? []).map((u) => u.username).filter(Boolean)
        setCandidates(['owner', ...names.filter((n) => n !== 'owner')])
      })
      .catch(() => {
        // A login that may not list users can still assign the owner.
        if (!cancelled) setCandidates(['owner'])
      })
    return () => {
      cancelled = true
    }
  }, [])

  useEffect(() => {
    const onDown = (e: MouseEvent): void => {
      if (rootRef.current && !rootRef.current.contains(e.target as Node)) onClose()
    }
    document.addEventListener('mousedown', onDown)
    return () => document.removeEventListener('mousedown', onDown)
  }, [onClose])

  const all = [...new Set([...candidates, ...local])]
  const toggle = (name: string): void => {
    setLocal((prev) => (prev.includes(name) ? prev.filter((n) => n !== name) : [...prev, name]))
  }
  const save = async (): Promise<void> => {
    setSaving(true)
    setSaveError(null)
    try {
      await assignFeedback(ticketId, local)
      onSaved()
    } catch (e) {
      setSaveError(e instanceof Error ? e.message : String(e))
    } finally {
      setSaving(false)
    }
  }

  return (
    <div
      ref={rootRef}
      data-testid="ticket-reassign-menu"
      className="absolute left-0 bottom-full mb-1 z-20 min-w-[180px] max-h-56 overflow-y-auto bg-[var(--color-bg)] border border-[var(--color-border)] shadow-lg py-1"
    >
      {all.map((name) => {
        const on = local.includes(name)
        return (
          <button
            key={name}
            type="button"
            onClick={() => toggle(name)}
            className={`flex w-full items-center gap-2 px-2 py-1.5 text-[11px] text-left cursor-pointer ${
              on ? 'bg-white/[0.04] text-[var(--color-text-primary)]' : 'text-[var(--color-text-secondary)] hover:bg-white/[0.06]'
            }`}
          >
            <span className="w-3 text-[var(--color-accent)]">{on ? '✓' : ''}</span>
            {name}
          </button>
        )
      })}
      {saveError && <div className="px-2 py-1 text-[10px] text-[var(--color-status-error-soft)]">{saveError}</div>}
      <div className="border-t border-[var(--color-border)] mt-1 pt-1 px-2 pb-1 flex justify-end gap-2">
        <button type="button" onClick={onClose} className="text-[10px] text-[var(--color-text-muted)] cursor-pointer">
          Cancel
        </button>
        <button
          type="button"
          disabled={saving}
          onClick={() => void save()}
          className="text-[10px] text-[var(--color-accent)] cursor-pointer disabled:opacity-50"
        >
          Save
        </button>
      </div>
    </div>
  )
}
