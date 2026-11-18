// Tickets board — the list + detail layout shared by the Tickets page and
// the Projects page Feedback tab (0.43.2 quick redesign, Rosson).
//
// One flow in three parts: pick a ticket in the LIST on the left, read its
// REPORT in the middle, and DISCUSS it on the right until it's resolved.
//
//   - The report is the one to read, so it gets the most room at every
//     width (`ticketBoardLayout`, lib/ticket-board-chrome.ts): the
//     discussion takes about a third of what the list leaves; while a
//     ticket is open the list narrows first, then folds to its avatar rail
//     by itself, and only then does the discussion become a drawer over the
//     report. The widths are the board's own, so the Projects page's
//     Feedback tab (beside the project nav) follows the same rules.
//   - Filters on top of the list: Mine (assigned to me), Waiting on me
//     (the default, GitHub #75: waiting or needs discussion), All, plus
//     search. Which tab it opens on is
//     the person's "Open the Tickets page on" preference (Settings →
//     Tickets, `lib/ticket-open-pref.ts`). An empty Waiting tab says
//     "Nothing is waiting on you" instead of falling back to All. The
//     caller may add one extra control (the Tickets page adds its
//     workspace filter).
//   - A collapse button (the nav's » / « drawer toggle) folds the list into
//     a slim rail of agent avatars with status dots; clicking one selects
//     that ticket. A rail the width folded opens the list over the report
//     (» on the rail), and picking a ticket there closes it again.
//   - List width, the collapsed state and the discussion's open state are
//     remembered PER WINDOW in localStorage (`ticket-board-chrome.ts`);
//     never daemon state.

import React, { useCallback, useEffect, useMemo, useState } from 'react'
import { useProjectsStore } from '@/stores/projects'
import {
  readTicketBoardChrome,
  writeTicketBoardChrome,
  ticketBoardLayout,
  TICKET_LIST_MAX_WIDTH,
  TICKET_EMPTY_PANE_MIN_WIDTH,
  TICKET_LIST_MIN_WIDTH,
  TICKET_REPORT_LEAD_PX,
  type TicketBoardChrome,
  type TicketBoardLayoutInput,
} from '@/lib/ticket-board-chrome'
import { useElementWidth } from '@/hooks/useElementWidth'
import { NavTile } from '@/components/Layout/NavTile'
import { ResizeEdge } from '@/components/Layout/ResizeEdge'
import { Button, ScrollEdgeFrame, SearchField, Segmented, Surface, TILE_FOOT_CLIP } from '@/components/ui'
import { PROJECT_BAR_ROW_HEIGHT } from '@/components/Projects/project-bar-height'
import {
  filterByScope,
  filterBySearch,
  groupByStatus,
  isUnlinked,
  isWaitingOnSomeone,
  TICKET_SCOPES,
  UNLINKED_WORKSPACE_LABEL,
  type FeedbackListRow,
  type TicketScope,
} from './feedback-api'
import { TicketRailItem } from './TicketCard'
import { DEFAULT_TICKETS_EMPTY_HINT, TicketListBody, type TicketSection } from './TicketListBody'
import { FeedbackItemView } from './FeedbackItemView'
import { useTicketMe } from './useTicketMe'
import { initialTicketScope, writeLastTicketTab } from '@/lib/ticket-open-pref'
import { DrawerToggleButton } from '@/components/Layout/DrawerTabBarToggle'

export interface TicketBoardProps {
  /** Rows to show (the caller's own scoping — workspace filter, project
   *  members — already applied). `null` = still loading. */
  rows: FeedbackListRow[] | null
  /** The FULL row set, so the open ticket survives a filter hiding it. */
  allRows?: FeedbackListRow[] | null
  error: string | null
  revision: number
  onMutated: () => void
  /** Load the list again after it failed (default: `onMutated`). */
  onRetry?: () => void | Promise<unknown>
  /** One extra control beside search (the Tickets page's workspace filter). */
  extraFilter?: React.ReactNode
  /** Empty-state hint when there are no tickets at all. */
  emptyHint?: React.ReactNode
  /** Selected ticket to open on mount (the "Open in window" target). */
  initialSelectedId?: string | null
}

/** The list's fades take its tile's fill (the nav panel's surface in
 *  Terminal). */
const TICKET_LIST_EDGE_TINT = 'var(--tile-fill, var(--color-bg-surface))'

/** The board's pane gap, as the page draws it (0 in Terminal). */
function paneGap(el: HTMLElement | null): number {
  if (!el || typeof getComputedStyle === 'undefined') return 0
  const gap = parseFloat(getComputedStyle(el).columnGap)
  return Number.isFinite(gap) ? gap : 0
}

export function TicketBoard({
  rows,
  allRows,
  error,
  revision,
  onMutated,
  onRetry,
  extraFilter,
  emptyHint = DEFAULT_TICKETS_EMPTY_HINT,
  initialSelectedId = null,
}: TicketBoardProps): React.JSX.Element {
  const projects = useProjectsStore((s) => s.projects)
  const me = useTicketMe()

  const [scope, setScopeState] = useState<TicketScope>(() => initialTicketScope())
  // Every pick is remembered for "Remember my last tab".
  const setScope = useCallback((next: TicketScope): void => {
    setScopeState(next)
    writeLastTicketTab(next)
  }, [])
  const [search, setSearch] = useState('')
  const [selectedId, setSelectedId] = useState<string | null>(initialSelectedId)
  const [nowSec, setNowSec] = useState(() => Math.floor(Date.now() / 1000))
  const [chrome, setChrome] = useState<TicketBoardChrome>(() => readTicketBoardChrome())
  const [liveWidth, setLiveWidth] = useState<number | null>(null)
  const [retrying, setRetrying] = useState(false)
  // The list shown over the report while the width keeps it folded.
  const [listOverlay, setListOverlay] = useState(false)
  const [boardEl, setBoardEl] = useState<HTMLDivElement | null>(null)
  const boardWidth = useElementWidth(boardEl)
  const gap = useMemo(() => (boardWidth > 0 ? paneGap(boardEl) : 0), [boardEl, boardWidth])

  // Relative-time ticker — labels only, no refetch.
  useEffect(() => {
    const id = setInterval(() => setNowSec(Math.floor(Date.now() / 1000)), 30_000)
    return () => clearInterval(id)
  }, [])

  // Capture-phase Esc clears the selection BEFORE the page's own Esc
  // (close the Tickets page / leave Projects) sees it. An open brief
  // overlay, discussion drawer, list overlay or status menu handles its own
  // Esc first.
  useEffect(() => {
    if (selectedId === null && !listOverlay) return
    const onKey = (e: KeyboardEvent): void => {
      if (e.key !== 'Escape') return
      if (document.querySelector('[data-testid="brief-overlay"]')) return
      if (document.querySelector('[data-ticket-discussion-drawer]')) return
      // An open menu (a status menu, the assignees, the workspace filter)
      // closes on this Esc, not the ticket.
      if (document.querySelector('[data-k2-menu]')) return
      // The answer box closes itself on Esc; keep the ticket open.
      if ((e.target as HTMLElement | null)?.tagName === 'TEXTAREA') return
      // The search field clears its text on Esc first (SearchField).
      const field = e.target as HTMLInputElement | null
      if (field?.matches?.('input[data-k2-search]') && field.value) return
      e.preventDefault()
      e.stopPropagation()
      if (listOverlay) setListOverlay(false)
      else setSelectedId(null)
    }
    window.addEventListener('keydown', onKey, true)
    return () => window.removeEventListener('keydown', onKey, true)
  }, [selectedId, listOverlay])

  const searched = useMemo(() => (rows ? filterBySearch(rows, search) : null), [rows, search])
  const scopeCounts = useMemo(() => {
    const counts: Record<TicketScope, number> = { mine: 0, waiting: 0, all: 0 }
    if (!searched) return counts
    for (const s of TICKET_SCOPES) counts[s.value] = filterByScope(searched, s.value, me ?? []).length
    return counts
  }, [searched, me])
  const filtered = useMemo(
    () => (searched ? filterByScope(searched, scope, me ?? []) : null),
    [searched, scope, me],
  )
  const grouped = useMemo(() => (filtered ? groupByStatus(filtered) : null), [filtered])

  const selectedRow = useMemo(() => {
    if (!selectedId) return null
    return (allRows ?? rows)?.find((r) => r.id === selectedId) ?? null
  }, [allRows, rows, selectedId])

  const projectById = useMemo(() => new Map(projects.map((p) => [p.id, p])), [projects])

  // The tickets waiting on this person, in the list's order (waiting, then
  // needs discussion): the next one to offer once a ticket is settled, and
  // the oldest one to start with.
  const waitingQueue = useMemo(() => {
    if (!rows) return []
    const g = groupByStatus(filterByScope(rows, 'waiting', me ?? []))
    return [...g.waiting, ...g.needs_discussion]
  }, [rows, me])
  const nextWaitingRow = waitingQueue.find((r) => r.id !== selectedId) ?? null
  const oldestWaiting = waitingQueue.reduce<FeedbackListRow | null>(
    (old, r) => (old === null || r.createdAt < old.createdAt ? r : old),
    null,
  )

  const sections: TicketSection[] = grouped
    ? [
        { label: 'Waiting on you', rows: grouped.waiting },
        { label: 'Needs discussion', rows: grouped.needs_discussion },
        { label: 'Answered', rows: grouped.answered },
        { label: 'Planned', rows: grouped.planned },
        { label: UNLINKED_WORKSPACE_LABEL, rows: grouped.unlinked },
        { label: 'Closed', rows: grouped.closed },
      ]
    : []
  const ordered = sections.flatMap((s) => s.rows)

  const setCollapsed = useCallback((collapsed: boolean): void => {
    setChrome(writeTicketBoardChrome({ collapsed }))
  }, [])
  const setChatOpen = useCallback((chatOpen: boolean): void => {
    setChrome(writeTicketBoardChrome({ chatOpen }))
  }, [])
  const setDiscussionWidth = useCallback((discussionWidth: number): void => {
    setChrome(writeTicketBoardChrome({ discussionWidth }))
  }, [])

  // The width rules: the report gets the most room (lib/ticket-board-chrome).
  const layoutInput: TicketBoardLayoutInput = {
    width: boardWidth,
    gap,
    listWidth: liveWidth ?? chrome.listWidth,
    collapsed: chrome.collapsed,
    ticketOpen: selectedRow !== null,
    // An unlinked ticket has no discussion.
    chatOpen: chrome.chatOpen && !(selectedRow && isUnlinked(selectedRow)),
    discussionWidth: chrome.discussionWidth,
  }
  const layout = ticketBoardLayout(layoutInput)
  const listWidth = liveWidth ?? layout.listWidth
  const showOverlay = listOverlay && layout.list === 'rail'
  const overlayWidth = Math.max(
    TICKET_LIST_MIN_WIDTH,
    Math.min(chrome.listWidth, boardWidth > 0 ? boardWidth - 2 * TICKET_REPORT_LEAD_PX : chrome.listWidth),
  )

  const select = useCallback((id: string): void => {
    setSelectedId(id)
    setListOverlay(false)
  }, [])

  // » on the rail: unfold the list; if the width still keeps it folded,
  // show it over the report.
  const expandList = (): void => {
    if (chrome.collapsed) setCollapsed(false)
    if (ticketBoardLayout({ ...layoutInput, collapsed: false }).list === 'rail') setListOverlay(true)
  }

  // How far the list's edge may go: while a ticket is open, only as far as
  // the report still leads both side parts; with none, up to the empty
  // middle's floor.
  const listMax = ((): number => {
    const fit = (n: number): number => Math.round(Math.min(TICKET_LIST_MAX_WIDTH, Math.max(TICKET_LIST_MIN_WIDTH, n)))
    if (!(boardWidth > 0)) return TICKET_LIST_MAX_WIDTH
    if (!selectedRow) return fit(boardWidth - gap - TICKET_EMPTY_PANE_MIN_WIDTH)
    const { reportWidth: r, listWidth: l, discussionWidth: d } = layout
    const grow = Math.min((r - l - TICKET_REPORT_LEAD_PX) / 2, d > 0 ? r - d - TICKET_REPORT_LEAD_PX : Infinity)
    return fit(l + Math.max(0, Math.floor(grow)))
  })()

  const retry = useCallback((): void => {
    setRetrying(true)
    void Promise.resolve(onRetry ? onRetry() : onMutated()).finally(() => setRetrying(false))
  }, [onRetry, onMutated])

  // A failed load is never an empty list (and its tabs don't claim 0).
  const failedEmpty = error !== null && (rows === null || rows.length === 0)
  // Mine / Waiting wait for whoami before saying the list is empty.
  const emptyList =
    !failedEmpty && filtered !== null && filtered.length === 0 && (me !== null || scope === 'all')
  // Waiting / needs-discussion tickets on someone else's board (or
  // nobody's): told in the empty Waiting state so "nothing is waiting on
  // you" is never a lie by omission.
  const otherWaiting = useMemo(
    () => (searched ? searched.filter((r) => isWaitingOnSomeone(r.status)).length - scopeCounts.waiting : 0),
    [searched, scopeCounts],
  )

  // The list's parts: its top row (level with the report's header and the
  // discussion's: one height for the three), search under it, and the
  // cards, whose ends fade under the rows beside them.
  const listParts = (overlay: boolean): React.JSX.Element => (
    <>
      <div
        data-ticket-list-header=""
        className="flex items-center gap-1 pl-2 flex-shrink-0 border-b border-[var(--color-border)]"
        style={{ height: PROJECT_BAR_ROW_HEIGHT }}
      >
        {/* Which tickets: the app's segmented control (Frost in Glass, cut in
            Terminal, the arrow keys move and choose), Waiting on me first as
            on the phone; each count in a small pill. */}
        <Segmented<TicketScope>
          aria-label="Which tickets"
          value={scope}
          onChange={setScope}
          activation="auto"
          className="flex-1 min-w-0"
          data-testid="ticket-scopes"
          options={TICKET_SCOPES.map((s) => ({
            value: s.value,
            label: (
              <span className="inline-flex items-center gap-1.5">
                <span>{s.label}</span>{' '}
                {!failedEmpty && (
                  <span data-scope-count="" className="type-badge tabular-nums px-1.5 rounded-[var(--radius-pill)] bg-[var(--color-wash-2)]">
                    {scopeCounts[s.value]}
                  </span>
                )}
              </span>
            ),
            buttonProps: { 'data-testid': `ticket-scope-${s.value}`, className: 'cursor-pointer' },
          }))}
        />
        <DrawerToggleButton
          side="left"
          placement="drawer"
          name="ticket list"
          testId="ticket-list-collapse"
          onToggle={() => (overlay ? setListOverlay(false) : setCollapsed(true))}
        />
      </div>
      <div className="flex flex-wrap items-center gap-1.5 px-2 pt-2 pb-1 flex-shrink-0">
        {/* The app's one search field: a magnifier, × and Esc clear it. */}
        <SearchField
          value={search}
          onChange={(e) => setSearch(e.target.value)}
          placeholder="Search tickets…"
          aria-label="Search tickets"
          className="flex-1 basis-40 min-w-0"
        />
        {extraFilter}
      </div>

      {/* The cards scroll in a frame whose ends fade, tinted with the tile's
          own fill; at the tile's foot it keeps inside the rim's curve. */}
      <ScrollEdgeFrame
        className={`flex-1 min-h-0 ${TILE_FOOT_CLIP}`}
        scrollClassName="overflow-y-auto overflow-x-hidden px-2 pb-2 [&>div]:min-h-full [&>div]:flex [&>div]:flex-col"
        tint={TICKET_LIST_EDGE_TINT}
        testId="ticket-list-scroll"
      >
        <TicketListBody
          rows={rows}
          error={error}
          sections={sections}
          ordered={ordered}
          selectedId={selectedId}
          onSelect={select}
          nowSec={nowSec}
          onMutated={onMutated}
          onRetry={retry}
          retrying={retrying}
          emptyList={emptyList}
          scope={scope}
          search={search}
          setScope={setScope}
          otherWaiting={otherWaiting}
          emptyHint={emptyHint}
        />
      </ScrollEdgeFrame>
    </>
  )

  return (
    <div
      ref={setBoardEl}
      className="relative flex-1 min-h-0 min-w-0 flex gap-[var(--gap-pane)]"
      data-testid="ticket-board"
      data-list-mode={layout.list}
      data-discussion-mode={layout.discussion}
    >
      {layout.list === 'rail' ? (
        // The folded list: its own tile at the icon-rail width, as the
        // Projects nav's rail.
        <NavTile
          data-testid="ticket-list-rail"
          data-auto-folded={layout.listAutoFolded ? '' : undefined}
          aria-label="Tickets"
          className="flex flex-col items-center"
          style={{ width: layout.listWidth }}
        >
          <div
            data-rail-top-row=""
            className="flex w-full flex-shrink-0 items-center justify-center border-b border-[var(--color-border)]"
            style={{ height: PROJECT_BAR_ROW_HEIGHT }}
          >
            <DrawerToggleButton
              side="left"
              placement="room"
              name="ticket list"
              testId="ticket-list-expand"
              // The row's own line is the edge; no divider mid-rail.
              divider={false}
              onToggle={expandList}
            />
          </div>
          <div className="flex min-h-0 w-full flex-1 flex-col items-center gap-1 overflow-y-auto overflow-x-hidden py-2">
            {ordered.map((row) => (
              <TicketRailItem
                key={row.id}
                row={row}
                workspace={projectById.get(row.projectId)}
                selected={selectedId === row.id}
                onSelect={() => select(row.id)}
              />
            ))}
          </div>
        </NavTile>
      ) : (
        <NavTile data-testid="ticket-list" className="flex flex-col min-h-0" style={{ width: listWidth }}>
          {listParts(false)}
        </NavTile>
      )}
      {layout.list === 'open' && (
        // The shared resize edge, centred in the gap after the list.
        <ResizeEdge
          value={layout.listWidth}
          min={TICKET_LIST_MIN_WIDTH}
          max={listMax}
          grows="right"
          onLive={setLiveWidth}
          onCommit={(w) => {
            setLiveWidth(null)
            setChrome(writeTicketBoardChrome({ listWidth: w }))
          }}
          label="Resize the ticket list"
          testId="ticket-list-resize"
          style={{ left: `calc(${listWidth}px + var(--gap-pane) / 2)`, transform: 'translateX(-50%)' }}
        />
      )}

      {showOverlay && (
        // The list over the report while the width keeps it folded: a
        // floating tile. A pick, « or Esc closes it.
        <div data-testid="ticket-list-overlay" className="absolute inset-y-0 left-0 z-30 flex">
          <Surface
            role2="surface"
            material="tile"
            bordered={false}
            elevation={3}
            data-nav-tile=""
            data-testid="ticket-list"
            data-overlay=""
            className="relative flex flex-col min-h-0 h-full border-r border-[var(--color-border)]"
            style={{ width: overlayWidth }}
          >
            {listParts(true)}
          </Surface>
        </div>
      )}

      <div className="flex-1 min-w-0 min-h-0 flex flex-col">
        {selectedRow ? (
          <FeedbackItemView
            key={selectedRow.id}
            id={selectedRow.id}
            listRow={selectedRow}
            nowSec={nowSec}
            revision={revision}
            onMutated={onMutated}
            chatOpen={chrome.chatOpen}
            onChatOpenChange={setChatOpen}
            nextWaiting={nextWaitingRow ? { title: nextWaitingRow.title, open: () => select(nextWaitingRow.id) } : null}
            discussion={{
              mode: layout.discussion,
              width: layout.discussionWidth,
              reportWidth: layout.reportWidth,
              onWidthChange: setDiscussionWidth,
            }}
          />
        ) : (
          // Nothing open yet: what the middle is for, and where to start.
          <Surface
            role2="solid"
            material="tile"
            bordered={false}
            data-ticket-none-open=""
            className="flex-1 flex flex-col items-center justify-center gap-3 text-center px-6"
          >
            <p className="type-row text-[var(--color-text-secondary)]">Pick a ticket to read its report and answer the agent.</p>
            {oldestWaiting && (
              <Button variant="accent" size="md" data-testid="ticket-start-oldest" onClick={() => select(oldestWaiting.id)}>
                Start with the oldest waiting ticket
              </Button>
            )}
          </Surface>
        )}
      </div>
    </div>
  )
}
