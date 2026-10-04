// Tickets board — the ticket detail (0.43.2 quick redesign, Rosson).
//
//   - A slim header that stays put: the title, Expand, Open in window and
//     the chat toggle (Hide chat / Show chat, remembered per window); under
//     it the status (a dropdown: waiting / needs discussion / answered /
//     resolved) and "from {agent} → assigned to {person}", where the
//     assignee part opens the reassign picker (users on this box).
//   - Below it, scrolling: the short summary (`--body`), the HTML brief
//     (it takes the stage) with its compact links list. No action bar and
//     no comments/history section: the conversation lives in the rail's
//     Thread tab.
//   - The right-hand chat rail (TicketAgentRail): the pre-redesign Thread |
//     Agent tabs — the ticket thread with the quick answers above its text
//     area and Send, and the asking session's terminal.
//
// Status model (daemon-first, feedback_routes.rs): a quick-answer pick
// posts `optionPick: true` → the ticket is answered. Free text → the ticket
// goes to needs_discussion until the agent settles it. Both land in the
// agent's session. The header's Answered asks for the agreed outcome (the
// daemon's `resolve` needs one).
//
// An UNLINKED ticket (workspace removed, TB18) is read-only: no options,
// no reassign, no chat — only Resolve and Dismiss.

import React, { useCallback, useEffect, useRef, useState } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { daemonCliGet } from '@/lib/daemon-cli'
import { useSettingsStore } from '@/stores/settings'
import { useToastStore } from '@/stores/toast'
import { formatRelativeTime } from '@/lib/format-relative-time'
import { isWebClient } from '@/lib/is-web'
import {
  assignFeedback,
  fetchFeedbackBrief,
  fetchFeedbackShow,
  formatFiledDate,
  isUnlinked,
  quickAnswerOptions,
  resolveFeedback,
  statusLabel,
  UNLINKED_WORKSPACE_LABEL,
  type FeedbackBrief,
  type FeedbackListRow,
  type FeedbackShow,
  type FeedbackStatus,
} from './feedback-api'
import { HtmlBriefBadge, PriorityBadge, StatusBadge } from './badges'
import { BriefFrame } from './BriefFrame'
import { cardAssigneeNames, statusChipClass } from './TicketCard'
import { TicketAgentRail } from './TicketAgentRail'
import { SelectableRegion, clearStuckBodyUserSelect } from '@/components/common/SelectableText'
import { ChatMessageBody } from '@/components/common/ChatMessage'
import { hasSelectionWithin } from '@/components/FileViewerPane/FileViewerPane'
import { primaryScope } from '@/kessel/server-scope'
import { readTicketBoardChrome, writeTicketBoardChrome } from '@/lib/ticket-board-chrome'

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

/** The header status dropdown's choices (Rosson). */
export const HEADER_STATUSES = ['waiting', 'needs_discussion', 'answered', 'resolved'] as const
export type HeaderStatus = (typeof HEADER_STATUSES)[number]

/** Open the ticket in its own app window (desktop only). */
export function openTicketWindow(ticketId: string): Promise<unknown> {
  return invoke('window_open_ticket', { ticketId })
}

const HEADER_BUTTON_CLASS =
  'px-2 py-0.5 text-[10px] text-[var(--color-text-secondary)] border border-[var(--color-border)] hover:text-[var(--color-text-primary)] hover:border-[var(--color-text-muted)] disabled:opacity-40 cursor-pointer flex-shrink-0'

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
  const [reassignOpen, setReassignOpen] = useState(false)
  // Open by default; remembered per window (ticket-board-chrome).
  const [railOpen, setRailOpenState] = useState(() => readTicketBoardChrome().chatOpen)
  const setRailOpen = useCallback((open: boolean) => {
    setRailOpenState(open)
    writeTicketBoardChrome({ chatOpen: open })
  }, [])

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
  // Deferred while the user is drag-selecting ticket text (the body or the
  // rail's thread).
  const seenRevision = useRef(revision)
  useEffect(() => {
    if (revision === seenRevision.current) return
    seenRevision.current = revision
    const timer = setTimeout(() => {
      const threads = document.querySelectorAll(`[data-ticket-thread="${id}"]`)
      for (const t of Array.from(threads)) {
        if (hasSelectionWithin(t as HTMLElement)) return
      }
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
  const assigneeNames = cardAssigneeNames(view.assignees)
  // The asker's agentName defaults to the workspace name (feedback_routes),
  // so the workspace label is shown only when it adds something: an
  // unlinked ticket, or a workspace that differs from the agent's name.
  const showWorkspace =
    unlinked || workspaceName.trim().toLowerCase() !== (view.agentName ?? '').trim().toLowerCase()
  const canOpenWindow = !inOwnWindow && !isWebClient()
  const showRail = railOpen && !unlinked

  const onChanged = useCallback(() => {
    void load()
    onMutated()
  }, [load, onMutated])

  // The brief's Options (else the structured --options) as quick answers,
  // shown above the rail's text area.
  const quick = unlinked || !item ? [] : quickAnswerOptions(item, brief?.html)

  const assignedLabel =
    assigneeNames.length > 0 ? (
      <>
        assigned to <span className="text-[var(--color-text-secondary)]">{assigneeNames.join(', ')}</span>
      </>
    ) : (
      <span className="italic">unassigned</span>
    )

  return (
    <div className="flex-1 flex min-h-0 min-w-0" data-testid="ticket-detail-wrap">
      <div className="flex-1 flex flex-col min-h-0 min-w-0" data-testid="ticket-detail">
        {/* Slim header — outside the scroller, so it stays put. Its menus
            open downward over the body, hence the stacking context. */}
        <div
          data-testid="ticket-detail-header"
          className="relative z-20 px-4 py-2 border-b border-[var(--color-border)] flex-shrink-0 bg-[var(--color-bg)]"
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
                className={HEADER_BUTTON_CLASS}
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
                className={HEADER_BUTTON_CLASS}
              >
                Open in window
              </button>
            )}
            {!unlinked && (
              <button
                type="button"
                data-testid="ticket-chat-toggle"
                aria-pressed={railOpen}
                onClick={() => setRailOpen(!railOpen)}
                title={railOpen ? 'Hide the chat with the agent' : 'Show the chat with the agent'}
                className={`${HEADER_BUTTON_CLASS} inline-flex items-center gap-1`}
              >
                <svg width="10" height="10" viewBox="0 0 12 12" fill="none" stroke="currentColor" strokeWidth="1.3" aria-hidden>
                  <rect x="1" y="1.5" width="10" height="9" />
                  <line x1="7.5" y1="1.5" x2="7.5" y2="10.5" />
                </svg>
                {railOpen ? 'Hide chat' : 'Show chat'}
              </button>
            )}
          </div>
          <div className="mt-1 flex items-center gap-1.5 min-w-0 text-[10px] text-[var(--color-text-muted)]">
            {unlinked ? (
              <StatusBadge status={view.status} />
            ) : (
              <HeaderStatusMenu
                ticketId={id}
                status={view.status}
                currentAnswer={item?.answer ?? null}
                onChanged={onChanged}
              />
            )}
            <PriorityBadge priority={view.priority} />
            {hasBrief && <HtmlBriefBadge />}
            <span data-testid="ticket-detail-byline" className="flex items-center min-w-0 selectable-copy">
              <span className="flex-shrink-0">
                from <span className="text-[var(--color-text-secondary)]">{view.agentName}</span>
                {' → '}
              </span>
              {unlinked ? (
                <span className="flex-shrink-0 ml-1">{assignedLabel}</span>
              ) : (
                <span className="relative flex-shrink-0 ml-1">
                  <button
                    type="button"
                    data-testid="ticket-reassign"
                    aria-expanded={reassignOpen}
                    disabled={!item}
                    onClick={() => setReassignOpen((o) => !o)}
                    title="Reassign"
                    className="hover:underline cursor-pointer disabled:cursor-default"
                  >
                    {assignedLabel}
                  </button>
                  {reassignOpen && item && (
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
                </span>
              )}
              <span className="truncate opacity-70 min-w-0">
                {' '}· {showWorkspace ? `${workspaceName} · ` : ''}asked{' '}
                {unlinked ? formatFiledDate(view.createdAt) : formatRelativeTime(view.createdAt, nowSec)}
              </span>
            </span>
          </div>
        </div>

        <TicketBody
          key={`body-${id}`}
          item={item}
          error={error}
          ticketId={id}
          unlinked={unlinked}
          hasBrief={hasBrief}
          brief={brief}
          briefError={briefError}
          title={view.title}
          expanded={expanded}
          onExpandedChange={setExpanded}
          onChanged={onChanged}
        />
      </div>

      {showRail && (
        <TicketAgentRail
          feedbackId={id}
          item={item}
          error={error}
          nowSec={nowSec}
          options={quick}
          optionsLive={item ? quickAnswersLive(item.status) : false}
          onChanged={onChanged}
          projectId={view.projectId}
          projectPath={projectPath}
          sessionId={view.sessionId}
          sessionKind={view.sessionKind}
          canonicalSessionId={item?.canonicalSessionId}
        />
      )}
    </div>
  )
}

// ── Body: summary + brief (no action bar; the thread is in the rail) ─────

function TicketBody({
  item,
  error,
  ticketId,
  unlinked,
  hasBrief,
  brief,
  briefError,
  title,
  expanded,
  onExpandedChange,
  onChanged,
}: {
  item: FeedbackShow | null
  error: string | null
  ticketId: string
  unlinked: boolean
  hasBrief: boolean
  brief: FeedbackBrief | null
  briefError: string | null
  title: string
  expanded: boolean
  onExpandedChange: (v: boolean) => void
  onChanged: () => void
}): React.JSX.Element {
  const [busy, setBusy] = useState(false)
  const [actionError, setActionError] = useState<string | null>(null)
  const editorFontSize = useSettingsStore((s) => s.editor.fontSize) || 13

  useEffect(() => {
    clearStuckBodyUserSelect()
  }, [ticketId])

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

  const open =
    item.status === 'waiting' ||
    item.status === 'answered' ||
    item.status === 'planned' ||
    item.status === 'needs_discussion'

  return (
    <div className="flex-1 flex flex-col min-h-0">
      <SelectableRegion className="flex-1 overflow-y-auto overflow-x-hidden min-h-0 px-4 py-3">
        <div className="min-h-full" data-ticket-thread={ticketId}>
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
        </div>
      </SelectableRegion>

      {unlinked && (
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
      )}
    </div>
  )
}

/** Closes a header popover on an outside mousedown or Esc (capture phase,
 *  so the first Esc closes the popover and not the open ticket). */
function useDismiss(rootRef: React.RefObject<HTMLElement | null>, onClose: () => void, active = true): void {
  useEffect(() => {
    if (!active) return
    const onDown = (e: MouseEvent): void => {
      if (rootRef.current && !rootRef.current.contains(e.target as Node)) onClose()
    }
    const onKey = (e: KeyboardEvent): void => {
      if (e.key === 'Escape') {
        e.stopPropagation()
        onClose()
      }
    }
    document.addEventListener('mousedown', onDown)
    window.addEventListener('keydown', onKey, true)
    return () => {
      document.removeEventListener('mousedown', onDown)
      window.removeEventListener('keydown', onKey, true)
    }
  }, [rootRef, onClose, active])
}

/** The header's status chip, which is also the status dropdown: waiting,
 *  needs discussion, answered, resolved — all over `POST
 *  /cli/feedback/resolve`. Answered asks for the agreed outcome first (the
 *  daemon refuses `answered` without one). */
export function HeaderStatusMenu({
  ticketId,
  status,
  currentAnswer,
  onChanged,
}: {
  ticketId: string
  status: FeedbackStatus
  currentAnswer: string | null
  onChanged: () => void
}): React.JSX.Element {
  const [open, setOpen] = useState(false)
  const [busy, setBusy] = useState(false)
  const [menuError, setMenuError] = useState<string | null>(null)
  const [answering, setAnswering] = useState(false)
  const [answer, setAnswer] = useState(currentAnswer ?? '')
  const rootRef = useRef<HTMLDivElement>(null)
  const close = useCallback(() => {
    setOpen(false)
    setAnswering(false)
    setMenuError(null)
  }, [])
  useDismiss(rootRef, close, open)

  const apply = async (next: HeaderStatus, agreed?: string): Promise<void> => {
    if (busy) return
    setBusy(true)
    setMenuError(null)
    try {
      await resolveFeedback(ticketId, next, agreed)
      onChanged()
      close()
    } catch (e) {
      setMenuError(e instanceof Error ? e.message : String(e))
    } finally {
      setBusy(false)
    }
  }

  const choose = (next: HeaderStatus): void => {
    if (next === status) {
      close()
      return
    }
    if (next === 'answered') {
      setAnswer(currentAnswer ?? '')
      setAnswering(true)
      return
    }
    void apply(next)
  }

  return (
    <div ref={rootRef} className="relative flex-shrink-0">
      <button
        type="button"
        data-testid="ticket-status"
        aria-haspopup="menu"
        aria-expanded={open}
        disabled={busy}
        onClick={() => (open ? close() : setOpen(true))}
        className={`inline-flex items-center gap-1 px-1.5 py-0.5 text-[9px] font-medium uppercase tracking-wide cursor-pointer disabled:opacity-50 ${statusChipClass(status)}`}
        title="Change status"
      >
        {statusLabel(status)}
        <svg
          className={`w-2 h-2 transition-transform ${open ? 'rotate-180' : ''}`}
          fill="none"
          viewBox="0 0 24 24"
          stroke="currentColor"
          strokeWidth={2.5}
          aria-hidden
        >
          <path strokeLinecap="round" strokeLinejoin="round" d="M19 9l-7 7-7-7" />
        </svg>
      </button>
      {open && (
        <div
          role="menu"
          data-testid="ticket-header-status-menu"
          className="absolute left-0 top-full mt-1 z-30 min-w-[180px] bg-[var(--color-bg)] border border-[var(--color-border)] shadow-lg py-0.5"
        >
          {HEADER_STATUSES.map((s) => {
            const current = status === s
            return (
              <button
                key={s}
                type="button"
                role="menuitemradio"
                aria-checked={current}
                data-testid={`ticket-status-option-${s}`}
                disabled={busy}
                onClick={() => choose(s)}
                className={`flex items-center gap-2 w-full px-2 py-1.5 text-[11px] text-left transition-colors cursor-pointer disabled:opacity-50 ${
                  current
                    ? 'text-[var(--color-text-primary)] bg-white/[0.04]'
                    : 'text-[var(--color-text-secondary)] hover:bg-white/[0.06] hover:text-[var(--color-text-primary)]'
                }`}
              >
                <span className="flex-1">{statusLabel(s)}</span>
                {current && <span className="text-[var(--color-accent)]">✓</span>}
              </button>
            )
          })}
          {answering && (
            <div className="border-t border-[var(--color-border)] mt-0.5 px-2 py-1.5">
              <label className="block mb-1 text-[10px] text-[var(--color-text-muted)]" htmlFor={`answered-${ticketId}`}>
                What was agreed?
              </label>
              <input
                id={`answered-${ticketId}`}
                data-testid="ticket-answered-input"
                autoFocus
                value={answer}
                onChange={(e) => setAnswer(e.target.value)}
                onKeyDown={(e) => {
                  if (e.key === 'Enter' && answer.trim()) {
                    e.preventDefault()
                    void apply('answered', answer.trim())
                  }
                }}
                className="w-full px-1.5 py-1 text-[11px] bg-[var(--color-bg-elevated)] text-[var(--color-text-primary)] border border-[var(--color-border)] outline-none focus:border-[var(--color-accent)]"
              />
              <div className="mt-1.5 flex justify-end gap-2">
                <button
                  type="button"
                  onClick={() => setAnswering(false)}
                  className="text-[10px] text-[var(--color-text-muted)] cursor-pointer"
                >
                  Cancel
                </button>
                <button
                  type="button"
                  data-testid="ticket-answered-save"
                  disabled={busy || answer.trim().length === 0}
                  onClick={() => void apply('answered', answer.trim())}
                  className="text-[10px] text-[var(--color-accent)] cursor-pointer disabled:opacity-50"
                >
                  Mark answered
                </button>
              </div>
            </div>
          )}
          {menuError && (
            <div className="px-2 py-1 text-[10px] text-[var(--color-status-error-soft)] selectable-copy">{menuError}</div>
          )}
        </div>
      )}
    </div>
  )
}

/** Reassign: multi-select from the server's users + `owner`, opening under
 *  the header's "assigned to" text. Replaces the whole assignee set. A name
 *  the daemon does not know still assigns; its `assignee_unknown` warning
 *  is shown as a toast. */
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
  useDismiss(rootRef, onClose)

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

  const all = [...new Set([...candidates, ...local])]
  const toggle = (name: string): void => {
    setLocal((prev) => (prev.includes(name) ? prev.filter((n) => n !== name) : [...prev, name]))
  }
  const save = async (): Promise<void> => {
    setSaving(true)
    setSaveError(null)
    try {
      const res = await assignFeedback(ticketId, local)
      for (const w of res.warnings ?? []) {
        useToastStore.getState().addToast(w.hint, 'warning')
      }
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
      className="absolute left-0 top-full mt-1 z-30 min-w-[180px] max-h-56 overflow-y-auto bg-[var(--color-bg)] border border-[var(--color-border)] shadow-lg py-1"
    >
      {all.map((name) => {
        const on = local.includes(name)
        return (
          <button
            key={name}
            type="button"
            data-testid="ticket-reassign-option"
            aria-pressed={on}
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
          data-testid="ticket-reassign-save"
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
