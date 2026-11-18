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
import { floatMaterial, MENU_LIST, MENU_ROW } from '@/components/ui/float-material'
import { invoke } from '@tauri-apps/api/core'
import { daemonCliGet } from '@/lib/daemon-cli'
import { useSettingsStore } from '@/stores/settings'
import { useToastStore } from '@/stores/toast'
import { formatRelativeTime } from '@/lib/format-relative-time'
import { useAnchoredMenu } from '@/hooks/useAnchoredMenu'
import { DrawerHeaderButton, DrawerToggleButton } from '@/components/Layout/DrawerTabBarToggle'
import { ChevronDownIcon, ExpandIcon, PopOutIcon } from '@/components/icons/K2Icons'
import { focusMenuStart, onMenuKeyDown, TICKET_MENU_BOX, TICKET_MENU_ROW } from './ticket-menu'
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
import { HtmlBriefBadge, PriorityBadge, StatusBadge, StatusMark } from './badges'
import { BriefFrame } from './BriefFrame'
import { cardAssigneeNames, MenuCheck } from './TicketCard'
import { TicketAgentRail } from './TicketAgentRail'
import { SelectableRegion, clearStuckBodyUserSelect } from '@/components/common/SelectableText'
import { ChatMessageBody } from '@/components/common/ChatMessage'
import { hasSelectionWithin } from '@/components/FileViewerPane/FileViewerPane'
import { primaryScope } from '@/kessel/server-scope'
import {
  readTicketBoardChrome,
  ticketBoardLayout,
  writeTicketBoardChrome,
  TICKET_DISCUSSION_DRAG_MAX_WIDTH,
  TICKET_DISCUSSION_MIN_WIDTH,
  TICKET_REPORT_LEAD_PX,
  type TicketDiscussionMode,
} from '@/lib/ticket-board-chrome'
import { useElementWidth } from '@/hooks/useElementWidth'
import { Button, Input, ScrollEdgeFrame, Surface, TILE_FOOT_CLIP } from '@/components/ui'
import { ResizeEdge } from '@/components/Layout/ResizeEdge'
import { ProjectStateCard, StateCanvas, StateRetry } from '@/components/Projects/StateCard'
import { PROJECT_BAR_ROW_HEIGHT } from '@/components/Projects/project-bar-height'

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
  /** Whether the person wants the discussion shown, when the board keeps
   *  it (else this view keeps it, remembered per window). */
  chatOpen?: boolean
  onChatOpenChange?: (open: boolean) => void
  /** Where the board's width rules put the discussion: beside the report,
   *  as a drawer over its right edge, or off. Without it (the ticket's own
   *  window) this view measures itself by the same rules. */
  discussion?: TicketDiscussionPlace
  /** The next ticket waiting on this person, offered once this one is
   *  answered or resolved, so the flow loops instead of dead-ending. */
  nextWaiting?: { title: string; open: () => void } | null
}

/** Where the discussion sits and how wide, and the report's width beside
 *  it (for how far the discussion's edge may go). */
export interface TicketDiscussionPlace {
  mode: TicketDiscussionMode
  width: number
  reportWidth: number
  /** The person dragged the discussion's edge (else this view keeps it). */
  onWidthChange?: (width: number) => void
}

/** The report's fades take its tile's fill: the page in Terminal, the tile
 *  in Glass (styles/glass/overrides.css sets --ticket-report-fill). */
const REPORT_EDGE_TINT = 'var(--ticket-report-fill, var(--color-bg))'

/** Ticket id → its HTML brief. Briefs never change after filing (H8), so
 *  one fetch per ticket per window is enough (H39). Exported for tests. */
export const briefCache = new Map<string, FeedbackBrief>()

/** The brief box fills the detail pane (the brief takes the stage). */
const BRIEF_HEIGHT_CLASS = 'h-[max(320px,calc(var(--k2-vh)-300px))]'

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

/** The header's glyphs (full screen, own window), at the icon size. */
const HEADER_GLYPH = 'w-[var(--control-search-icon)] h-[var(--control-search-icon)]'

export function FeedbackItemView({
  id,
  listRow,
  nowSec,
  revision,
  onMutated,
  inOwnWindow = false,
  chatOpen,
  onChatOpenChange,
  discussion,
  nextWaiting = null,
}: FeedbackItemViewProps): React.JSX.Element {
  const [item, setItem] = useState<FeedbackShow | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [expanded, setExpanded] = useState(false)
  // Open by default; remembered per window (ticket-board-chrome). The board
  // keeps it when it draws this view.
  const [ownRailOpen, setOwnRailOpen] = useState(() => readTicketBoardChrome().chatOpen)
  const railOpen = chatOpen ?? ownRailOpen
  const setRailOpen = useCallback(
    (open: boolean) => {
      if (onChatOpenChange) {
        onChatOpenChange(open)
        return
      }
      setOwnRailOpen(open)
      writeTicketBoardChrome({ chatOpen: open })
    },
    [onChatOpenChange],
  )
  // The discussion as a drawer (too narrow to sit beside the report): it
  // opens from its tab and closes from its » or Esc.
  const [drawerOpen, setDrawerOpen] = useState(false)
  const [wrapEl, setWrapEl] = useState<HTMLDivElement | null>(null)
  const ownWidth = useElementWidth(discussion ? null : wrapEl)
  // The discussion's width while its edge is dragged, and (in the ticket's
  // own window) the width it was dragged to.
  const [liveDiscussion, setLiveDiscussion] = useState<number | null>(null)
  const [ownDiscussionWidth, setOwnDiscussionWidth] = useState<number | null>(
    () => readTicketBoardChrome().discussionWidth,
  )

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
  // Try again on a failed report: a fresh request.
  const [briefTry, setBriefTry] = useState(0)
  const retryBrief = useCallback(() => {
    briefRequested.current = null
    setBriefError(null)
    setBriefTry((n) => n + 1)
  }, [])
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
          briefRequested.current = null
          setBriefError('The server sent no report for this ticket.')
        }
      },
      (e: unknown) => {
        briefRequested.current = null
        setBriefError(e instanceof Error ? e.message : String(e))
      },
    )
  }, [hasBrief, id, briefTry])

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
  // Where the discussion goes: the board's width rules, or this view's own
  // (the ticket window: no list, the same rules).
  const placed: TicketDiscussionPlace =
    discussion ??
    (() => {
      const gap = wrapEl && ownWidth > 0 ? parseFloat(getComputedStyle(wrapEl).columnGap) || 0 : 0
      const l = ticketBoardLayout({
        width: ownWidth,
        gap,
        hasList: false,
        listWidth: 0,
        collapsed: false,
        ticketOpen: true,
        chatOpen: showRail,
        discussionWidth: ownDiscussionWidth,
      })
      return { mode: l.discussion, width: l.discussionWidth, reportWidth: l.reportWidth }
    })()
  const discussionWidth = liveDiscussion ?? placed.width
  // The edge stops where the report would no longer lead the discussion.
  const discussionMax = Math.max(
    TICKET_DISCUSSION_MIN_WIDTH,
    Math.min(
      TICKET_DISCUSSION_DRAG_MAX_WIDTH,
      placed.reportWidth > 0
        ? Math.floor((placed.width + placed.reportWidth - TICKET_REPORT_LEAD_PX) / 2)
        : TICKET_DISCUSSION_DRAG_MAX_WIDTH,
    ),
  )
  const commitDiscussionWidth = (w: number): void => {
    setLiveDiscussion(null)
    if (placed.onWidthChange) placed.onWidthChange(w)
    else {
      setOwnDiscussionWidth(w)
      writeTicketBoardChrome({ discussionWidth: w })
    }
  }
  const discussionMode: TicketDiscussionMode = showRail ? placed.mode : 'off'
  const asDrawer = discussionMode === 'drawer'
  const discussionShown = discussionMode === 'side' || (asDrawer && drawerOpen)
  const toggleDiscussion = (): void => {
    if (asDrawer) setDrawerOpen(!drawerOpen)
    else setRailOpen(!railOpen)
  }

  // Esc closes the drawer first (the board's Esc then closes the ticket).
  useEffect(() => {
    if (!(asDrawer && drawerOpen)) return
    const onKey = (e: KeyboardEvent): void => {
      if (e.key !== 'Escape') return
      if ((e.target as HTMLElement | null)?.tagName === 'TEXTAREA') return
      e.preventDefault()
      e.stopPropagation()
      setDrawerOpen(false)
    }
    window.addEventListener('keydown', onKey, true)
    return () => window.removeEventListener('keydown', onKey, true)
  }, [asDrawer, drawerOpen])

  const onChanged = useCallback(() => {
    void load()
    onMutated()
  }, [load, onMutated])

  // The brief's Options (else the structured --options) as quick answers,
  // shown above the rail's text area.
  const quick = unlinked || !item ? [] : quickAnswerOptions(item, brief?.html)

  // Who answers it: the people's names, or "no one assigned".
  const assignedLabel = assigneeNames.length > 0 ? assigneeNames.join(', ') : 'no one assigned'

  return (
    <div
      ref={setWrapEl}
      className="relative flex-1 flex min-h-0 min-w-0 gap-[var(--gap-pane)]"
      data-testid="ticket-detail-wrap"
      data-discussion={discussionMode}
    >
      {/* The report: the middle of the flow, its own tile. */}
      <Surface
        role2="solid"
        material="tile"
        bordered={false}
        data-ticket-report=""
        className="flex-1 flex flex-col min-h-0 min-w-0"
        data-testid="ticket-detail"
      >
        {/* The header stays put: the title and its controls on one row,
            level with the list's top row and the discussion's. Its menus
            open downward over the body, hence the stacking context. */}
        <div data-testid="ticket-detail-header" className="relative z-20 flex-shrink-0">
          <div
            className="flex items-center gap-1 min-w-0 px-4 border-b border-[var(--color-border)]"
            style={{ height: PROJECT_BAR_ROW_HEIGHT }}
          >
            <span
              data-testid="ticket-detail-title"
              className="type-heading text-[var(--color-text-primary)] truncate flex-1 selectable-copy"
              title={view.title}
            >
              {view.title}
            </span>
            {/* The header's controls are glyphs with tooltips, so the title
                keeps its line: full screen, its own window, and (while the
                discussion is hidden) » to show it, as the list's « and ». */}
            {hasBrief && (
              <DrawerHeaderButton
                shape="glyph"
                label="Show the report full screen"
                data-testid="ticket-expand"
                disabled={!brief}
                onClick={() => setExpanded(true)}
                className="disabled:opacity-40 disabled:cursor-default"
              >
                <ExpandIcon className={HEADER_GLYPH} />
              </DrawerHeaderButton>
            )}
            {canOpenWindow && (
              <DrawerHeaderButton
                shape="glyph"
                label="Open in its own window"
                data-testid="ticket-open-window"
                onClick={() => {
                  openTicketWindow(id).catch((e: unknown) => console.warn('[tickets] open in window failed', e))
                }}
              >
                <PopOutIcon className={HEADER_GLYPH} />
              </DrawerHeaderButton>
            )}
            {!unlinked && !discussionShown && (
              <span className="flex self-stretch -mr-4">
                <DrawerToggleButton
                  side="right"
                  placement="room"
                  name="discussion"
                  testId="ticket-chat-toggle"
                  divider={false}
                  onToggle={toggleDiscussion}
                />
              </span>
            )}
          </div>
          {/* The report's first line: its status, and who asked whom. */}
          <div className="px-4 pt-2 flex items-center gap-2 min-w-0 type-hint text-[var(--color-text-muted)]">
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
            {/* "atlas asked Owner · 25m ago": who asked whom, the person a
                chip that reassigns. */}
            <span data-testid="ticket-detail-byline" className="flex items-center gap-1 min-w-0 selectable-copy">
              <span className="flex-shrink-0">
                <span className="text-[var(--color-text-secondary)]">{view.agentName}</span> asked
              </span>{' '}
              {unlinked ? (
                <span className="flex-shrink-0">{assignedLabel}</span>
              ) : (
                <ReassignControl item={item} label={assignedLabel} onSaved={onChanged} />
              )}{' '}
              <span className="truncate min-w-0">
                · {showWorkspace ? `${workspaceName} · ` : ''}
                {unlinked ? formatFiledDate(view.createdAt) : formatRelativeTime(view.createdAt, nowSec)}
              </span>
            </span>
          </div>
        </div>

        {nextWaiting && (view.status === 'answered' || view.status === 'resolved') && (
          <div
            data-testid="ticket-next-waiting"
            role="status"
            className="mx-4 mt-2 flex items-center gap-2 type-hint text-[var(--color-text-secondary)]"
          >
            <span className="min-w-0 truncate">
              Done here. Next waiting: <span className="text-[var(--color-text-primary)]">{nextWaiting.title}</span>
            </span>
            <Button variant="accent" className="flex-shrink-0" data-testid="ticket-next-waiting-open" onClick={nextWaiting.open}>
              Open it
            </Button>
          </div>
        )}

        <TicketBody
          key={`body-${id}`}
          item={item}
          error={error}
          ticketId={id}
          unlinked={unlinked}
          hasBrief={hasBrief}
          brief={brief}
          briefError={briefError}
          onRetryBrief={retryBrief}
          onRetry={() => void load()}
          title={view.title}
          expanded={expanded}
          onExpandedChange={setExpanded}
          onChanged={onChanged}
        />
      </Surface>

      {discussionMode === 'side' && (
        // The shared resize edge, in the gap before the discussion.
        <ResizeEdge
          value={discussionWidth}
          min={TICKET_DISCUSSION_MIN_WIDTH}
          max={discussionMax}
          grows="left"
          onLive={setLiveDiscussion}
          onCommit={commitDiscussionWidth}
          label="Resize the discussion"
          testId="ticket-discussion-resize"
          style={{ right: `calc(${discussionWidth}px + var(--gap-pane) / 2)`, transform: 'translateX(50%)' }}
        />
      )}
      {asDrawer && !drawerOpen && (
        // The closed drawer's tab, on the report's right edge, so the
        // discussion is always one click away.
        <button
          type="button"
          data-testid="ticket-discussion-tab"
          aria-expanded={false}
          onClick={() => setDrawerOpen(true)}
          title="Show the discussion"
          className="k2-focus absolute right-0 top-1/2 -translate-y-1/2 z-20 flex items-center gap-1.5 px-1.5 py-3 [writing-mode:vertical-rl] type-hint font-medium text-[var(--color-text-secondary)] hover:text-[var(--color-text-primary)] bg-[var(--color-bg-surface)] border border-r-0 border-[var(--color-border)] hover:bg-[var(--color-wash-1)] cursor-pointer"
        >
          Discussion{item ? ` · ${item.comments.length}` : ''}
        </button>
      )}
      {discussionShown && (
        <TicketAgentRail
          width={discussionWidth}
          drawer={asDrawer}
          onHide={asDrawer ? () => setDrawerOpen(false) : () => setRailOpen(false)}
          hideTestId={asDrawer ? 'ticket-discussion-hide' : 'ticket-chat-toggle'}
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
  onRetryBrief,
  onRetry,
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
  onRetryBrief: () => void
  onRetry: () => void
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
      <StateCanvas>
        <ProjectStateCard
          kind="error"
          title="Couldn’t load this ticket"
          detail={error}
          testId="ticket-load-error"
          action={<StateRetry onClick={onRetry} />}
        >
          The server didn’t send it. Your agents are fine.
        </ProjectStateCard>
      </StateCanvas>
    )
  }
  if (!item) {
    return <div className="flex-1 flex items-center justify-center type-row text-[var(--color-text-muted)]">Loading the ticket…</div>
  }

  const open =
    item.status === 'waiting' ||
    item.status === 'answered' ||
    item.status === 'planned' ||
    item.status === 'needs_discussion'

  return (
    <div className={`flex-1 flex flex-col min-h-0 ${TILE_FOOT_CLIP}`}>
      <SelectableRegion className="flex-1 flex flex-col min-h-0">
        <ScrollEdgeFrame
          className="flex-1 min-h-0"
          scrollClassName="overflow-y-auto overflow-x-hidden px-4 py-3 [&>div]:min-h-full [&>div]:flex [&>div]:flex-col"
          tint={REPORT_EDGE_TINT}
          testId="ticket-report-scroll"
        >
        <div className="flex-1 flex flex-col" data-ticket-thread={ticketId}>
          {item.body && (
            // The short summary: an inset well above the report.
            <Surface role2="inset" chamfer data-testid="ticket-summary" className="mb-3 px-3 py-2">
              <ChatMessageBody text={item.body} style={{ fontSize: editorFontSize }} />
            </Surface>
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
                fill
                onTile
              />
            ) : briefError ? (
              <div data-testid="brief-error" role="alert" className="mb-3 flex flex-col items-start gap-1.5">
                <p className="type-row text-[var(--color-status-error-text)]">This ticket’s report couldn’t be loaded.</p>
                <p className="type-hint text-[var(--color-text-muted)] selectable-copy">{briefError}</p>
                <Button variant="quiet" onClick={onRetryBrief}>
                  Try again
                </Button>
              </div>
            ) : (
              <div data-testid="brief-loading" className="mb-3 type-hint text-[var(--color-text-muted)]">
                Loading the report…
              </div>
            ))}
        </div>
        </ScrollEdgeFrame>
      </SelectableRegion>

      {unlinked && (
        <div data-testid="unlinked-thread-footer" className="border-t border-[var(--color-border)] px-4 py-3 flex-shrink-0">
          {actionError && <div className="mb-2 type-hint text-[var(--color-status-error-text)] selectable-copy">{actionError}</div>}
          <p className="mb-2 type-hint text-[var(--color-text-muted)]">
            This ticket’s workspace was removed from this server, so no agent can receive a reply. Resolve or
            Dismiss it here.
          </p>
          {open && (
            <div className="flex items-center gap-2">
              <Button variant="quiet" disabled={busy} onClick={() => void submit(() => resolveFeedback(item.id, 'resolved'))}>
                Resolve
              </Button>
              <Button variant="quiet" disabled={busy} onClick={() => void submit(() => resolveFeedback(item.id, 'dismissed'))}>
                Dismiss
              </Button>
            </div>
          )}
        </div>
      )}
    </div>
  )
}
/** A menu box placed by the app's anchored-menu layer: its element, for
 *  the keys, and the hook's own ref. */
function useMenuBox(menuRef: React.RefCallback<HTMLDivElement>): {
  box: React.MutableRefObject<HTMLDivElement | null>
  ref: (el: HTMLDivElement | null) => void
} {
  const box = useRef<HTMLDivElement | null>(null)
  const ref = useCallback(
    (el: HTMLDivElement | null) => {
      box.current = el
      menuRef(el)
    },
    [menuRef],
  )
  return { box, ref }
}

/** The header's status chip, which is also the status dropdown: waiting,
 *  needs discussion, answered, resolved — all over `POST
 *  /cli/feedback/resolve`. Answered asks for the agreed outcome first (the
 *  daemon refuses `answered` without one). The menu floats from the chip
 *  (the app's anchored menu: never clipped by the tile, flips up when
 *  there's no room below), focus moves into it, and ↑ / ↓ move. */
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
  const close = useCallback(() => {
    setOpen(false)
    setAnswering(false)
    setMenuError(null)
  }, [])
  const menu = useAnchoredMenu<HTMLDivElement>({ open, onClose: close, width: 'min', minWidth: 200, estimatedHeight: 200 })
  const { box, ref } = useMenuBox(menu.menuRef)
  useEffect(() => {
    if (!open) return
    const frame = requestAnimationFrame(() => focusMenuStart(box.current))
    return () => cancelAnimationFrame(frame)
  }, [open, box])
  useEffect(() => {
    if (!open) return
    // Capture-phase so Esc closes this menu before the board's Esc.
    const onKey = (e: KeyboardEvent): void => {
      if (e.key !== 'Escape') return
      e.stopPropagation()
      close()
    }
    window.addEventListener('keydown', onKey, true)
    return () => window.removeEventListener('keydown', onKey, true)
  }, [open, close])

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
    <div ref={menu.anchorRef} className="relative flex-shrink-0">
      <button
        type="button"
        data-testid="ticket-status"
        aria-haspopup="menu"
        aria-expanded={open}
        disabled={busy}
        onClick={() => (open ? close() : setOpen(true))}
        data-status={status}
        className="k2-focus inline-flex items-center cursor-pointer disabled:opacity-50"
        title="Change status"
      >
        <StatusMark status={status}>
          <span className={`flex flex-shrink-0 transition-transform ${open ? 'rotate-180' : ''}`}>
            <ChevronDownIcon className="w-3 h-3" />
          </span>
        </StatusMark>
      </button>
      {open &&
        menu.portal(
          <div
            ref={ref}
            role="menu"
            aria-label="Change status"
            data-testid="ticket-header-status-menu"
            data-placement={menu.placement}
            data-k2-chamfer="surface"
            style={menu.style}
            onKeyDown={(e) => onMenuKeyDown(e, box.current, close)}
            {...floatMaterial('menu')}
            {...MENU_LIST}
            className={`min-w-[180px] ${TICKET_MENU_BOX}`}
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
                  data-k2-focus=""
                  {...MENU_ROW}
                  className={`${TICKET_MENU_ROW} ${
                    current ? 'text-[var(--color-text-primary)] bg-[var(--color-wash-1)]' : 'text-[var(--color-text-secondary)] hover:text-[var(--color-text-primary)]'
                  }`}
                >
                  <span className="flex-1">{statusLabel(s)}</span>
                  {current && <MenuCheck />}
                </button>
              )
            })}
            {answering && (
              <div className="border-t border-[var(--color-border)] mt-0.5 px-3 py-2">
                <label className="block mb-1 type-hint text-[var(--color-text-secondary)]" htmlFor={`answered-${ticketId}`}>
                  What was agreed?
                </label>
                <Input
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
                  className="w-full"
                />
                <div className="mt-2 flex justify-end gap-2">
                  <Button variant="ghost" onClick={() => setAnswering(false)}>
                    Cancel
                  </Button>
                  <Button
                    variant="accent"
                    data-testid="ticket-answered-save"
                    disabled={busy || answer.trim().length === 0}
                    onClick={() => void apply('answered', answer.trim())}
                  >
                    Mark answered
                  </Button>
                </div>
              </div>
            )}
            {menuError && (
              <div className="px-3 py-1 type-hint text-[var(--color-status-error-text)] selectable-copy">{menuError}</div>
            )}
          </div>,
        )}
    </div>
  )
}

/** Reassign: the "assigned to …" control in the byline and its menu, a
 *  multi-select from the server's users + `owner`. Replaces the whole
 *  assignee set on Save. A name the daemon does not know still assigns; its
 *  `assignee_unknown` warning is shown as a toast. The menu floats from the
 *  control (never clipped), with checkbox rows and the menu keys. */
function ReassignControl({
  item,
  label,
  onSaved,
}: {
  item: FeedbackShow | null
  label: React.ReactNode
  onSaved: () => void
}): React.JSX.Element {
  const [open, setOpen] = useState(false)
  const close = useCallback(() => setOpen(false), [])
  const menu = useAnchoredMenu<HTMLSpanElement>({ open, onClose: close, width: 'min', minWidth: 200, estimatedHeight: 260 })
  return (
    <span ref={menu.anchorRef} className="relative flex-shrink-0">
      {/* A chip with a chevron, so it reads as the control it is. */}
      <button
        type="button"
        data-testid="ticket-reassign"
        aria-haspopup="menu"
        aria-expanded={open}
        disabled={!item}
        onClick={() => setOpen((o) => !o)}
        title="Choose who answers this ticket"
        data-k2-chamfer="chip"
        className="k2-focus inline-flex items-center gap-1 px-2 rounded-[var(--radius-pill)] bg-[var(--color-wash-2)] text-[var(--color-text-secondary)] hover:bg-[var(--color-wash-1)] hover:text-[var(--color-text-primary)] cursor-pointer disabled:cursor-default transition-colors"
      >
        {label}
        <ChevronDownIcon className="w-3 h-3 flex-shrink-0" />
      </button>
      {open && item && (
        <ReassignMenu
          menuRef={menu.menuRef}
          style={menu.style}
          portal={menu.portal}
          ticketId={item.id}
          assignees={item.assignees ?? []}
          onClose={close}
          onSaved={() => {
            close()
            onSaved()
          }}
        />
      )}
    </span>
  )
}

function ReassignMenu({
  menuRef,
  style,
  portal,
  ticketId,
  assignees,
  onClose,
  onSaved,
}: {
  menuRef: React.RefCallback<HTMLDivElement>
  style: React.CSSProperties
  portal: (node: React.ReactNode) => React.ReactPortal | null
  ticketId: string
  assignees: string[]
  onClose: () => void
  onSaved: () => void
}): React.JSX.Element | null {
  const [candidates, setCandidates] = useState<string[]>(['owner'])
  const [local, setLocal] = useState<string[]>(assignees)
  const [saving, setSaving] = useState(false)
  const [saveError, setSaveError] = useState<string | null>(null)
  const { box, ref } = useMenuBox(menuRef)

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
    const frame = requestAnimationFrame(() => focusMenuStart(box.current))
    return () => cancelAnimationFrame(frame)
  }, [box])
  useEffect(() => {
    const onKey = (e: KeyboardEvent): void => {
      if (e.key !== 'Escape') return
      e.stopPropagation()
      onClose()
    }
    window.addEventListener('keydown', onKey, true)
    return () => window.removeEventListener('keydown', onKey, true)
  }, [onClose])

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

  return portal(
    <div
      ref={ref}
      role="menu"
      aria-label="Who answers this ticket"
      data-testid="ticket-reassign-menu"
      data-k2-chamfer="surface"
      style={style}
      onKeyDown={(e) => onMenuKeyDown(e, box.current, onClose)}
      {...floatMaterial('menu')}
      {...MENU_LIST}
      className={`min-w-[180px] max-h-56 overflow-y-auto ${TICKET_MENU_BOX}`}
    >
      {all.map((name) => {
        const on = local.includes(name)
        return (
          <button
            key={name}
            type="button"
            role="menuitemcheckbox"
            aria-checked={on}
            data-testid="ticket-reassign-option"
            onClick={() => toggle(name)}
            data-k2-focus=""
            {...MENU_ROW}
            className={`${TICKET_MENU_ROW} ${
              on ? 'bg-[var(--color-wash-1)] text-[var(--color-text-primary)]' : 'text-[var(--color-text-secondary)]'
            }`}
          >
            <span className="flex w-3 flex-shrink-0">{on && <MenuCheck />}</span>
            {name}
          </button>
        )
      })}
      {saveError && <div className="px-3 py-1 type-hint text-[var(--color-status-error-text)]">{saveError}</div>}
      <div className="border-t border-[var(--color-border)] mt-1 pt-1.5 px-2 pb-1.5 flex justify-end gap-2">
        <Button variant="ghost" onClick={onClose}>
          Cancel
        </Button>
        <Button variant="accent" data-testid="ticket-reassign-save" disabled={saving} onClick={() => void save()}>
          Save
        </Button>
      </div>
    </div>,
  )
}
