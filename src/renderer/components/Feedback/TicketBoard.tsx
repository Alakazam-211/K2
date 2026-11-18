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
  clampTicketListWidth,
  ticketBoardLayout,
  TICKET_LIST_MIN_WIDTH,
  TICKET_REPORT_LEAD_PX,
  type TicketBoardChrome,
  type TicketBoardLayoutInput,
} from '@/lib/ticket-board-chrome'
import { useElementWidth } from '@/hooks/useElementWidth'
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
  const [resizing, setResizing] = useState(false)
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
      // The answer box closes itself on Esc; keep the ticket open.
      if ((e.target as HTMLElement | null)?.tagName === 'TEXTAREA') return
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

  // The width rules: the report gets the most room (lib/ticket-board-chrome).
  const layoutInput: TicketBoardLayoutInput = {
    width: boardWidth,
    gap,
    listWidth: liveWidth ?? chrome.listWidth,
    collapsed: chrome.collapsed,
    ticketOpen: selectedRow !== null,
    // An unlinked ticket has no discussion.
    chatOpen: chrome.chatOpen && !(selectedRow && isUnlinked(selectedRow)),
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

  const startResize = useCallback(
    (e: React.MouseEvent): void => {
      if (e.button !== 0) return
      e.preventDefault()
      const startX = e.clientX
      const base = layout.listWidth
      let last = base
      document.body.style.cursor = 'col-resize'
      setResizing(true)
      const onMove = (ev: MouseEvent): void => {
        last = clampTicketListWidth(base + (ev.clientX - startX))
        setLiveWidth(last)
      }
      const onUp = (): void => {
        document.removeEventListener('mousemove', onMove)
        document.removeEventListener('mouseup', onUp)
        document.body.style.cursor = ''
        setResizing(false)
        setLiveWidth(null)
        setChrome(writeTicketBoardChrome({ listWidth: last }))
      }
      document.addEventListener('mousemove', onMove)
      document.addEventListener('mouseup', onUp)
    },
    [layout.listWidth],
  )

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

  const listPanel = (width: number, overlay: boolean): React.JSX.Element => (
    <div
      data-testid="ticket-list"
      data-overlay={overlay ? '' : undefined}
      className={`relative flex flex-col min-h-0 border-r border-[var(--color-border)] flex-shrink-0 ${
        overlay ? 'h-full bg-[var(--color-bg)] [box-shadow:var(--shadow-3)]' : ''
      }`}
      style={{ width }}
    >
      <div className="flex flex-col gap-1.5 px-2 py-2 border-b border-[var(--color-border)] flex-shrink-0">
        <div className="flex items-center gap-1">
          <div role="tablist" aria-label="Which tickets" className="flex flex-1 min-w-0 border border-[var(--color-border)]">
            {TICKET_SCOPES.map((s) => {
              const active = scope === s.value
              return (
                <button
                  key={s.value}
                  type="button"
                  role="tab"
                  aria-selected={active}
                  data-testid={`ticket-scope-${s.value}`}
                  onClick={() => setScope(s.value)}
                  className={`flex-1 min-w-0 flex items-center justify-center gap-1 px-1.5 py-1 text-[10px] font-medium truncate cursor-pointer transition-colors ${
                    active
                      ? 'bg-[var(--color-accent)]/15 text-[var(--color-text-primary)]'
                      : 'text-[var(--color-text-secondary)] hover:text-[var(--color-text-primary)] hover:bg-[var(--color-wash-1)]'
                  }`}
                >
                  <span className="truncate">{s.label}</span>
                  {!failedEmpty && (
                    <span className={`tabular-nums ${active ? 'text-[var(--color-accent)]' : 'text-[var(--color-text-muted)]'}`}>
                      {scopeCounts[s.value]}
                    </span>
                  )}
                </button>
              )
            })}
          </div>
          <DrawerToggleButton
            side="left"
            placement="drawer"
            name="ticket list"
            testId="ticket-list-collapse"
            onToggle={() => (overlay ? setListOverlay(false) : setCollapsed(true))}
          />
        </div>
        <div className="flex items-center gap-1.5">
          <div className="relative flex-1 min-w-0">
            <input
              type="text"
              value={search}
              onChange={(e) => setSearch(e.target.value)}
              placeholder="Search tickets…"
              aria-label="Search tickets"
              className="w-full px-2 py-1.5 pr-6 text-[11px] bg-[var(--color-bg-elevated)] text-[var(--color-text-primary)] border border-[var(--color-border)] outline-none focus:border-[var(--color-accent)] placeholder:text-[var(--color-text-muted)]"
            />
            {search && (
              <button
                type="button"
                onClick={() => setSearch('')}
                aria-label="Clear search"
                className="absolute right-1 top-1/2 -translate-y-1/2 flex items-center justify-center w-4 h-4 text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)] cursor-pointer"
              >
                <svg width="9" height="9" viewBox="0 0 12 12" fill="none" stroke="currentColor" strokeWidth="1.5">
                  <line x1="2" y1="2" x2="10" y2="10" />
                  <line x1="10" y1="2" x2="2" y2="10" />
                </svg>
              </button>
            )}
          </div>
          {extraFilter}
        </div>
      </div>

      <div className="flex-1 overflow-y-auto min-h-0 px-2 pb-2">
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
      </div>

      {!overlay && (
        <div
          data-testid="ticket-list-resize"
          role="separator"
          aria-orientation="vertical"
          aria-label="Resize the ticket list"
          data-resize-edge=""
          className="absolute top-0 bottom-0 z-10 cursor-col-resize hover:bg-[var(--color-accent)]/40 transition-colors"
          style={{ right: -4, width: 7 }}
          onMouseDown={startResize}
        />
      )}
      {resizing && <div className="fixed inset-0 z-50" style={{ cursor: 'col-resize' }} />}
    </div>
  )

  return (
    <div
      ref={setBoardEl}
      className="relative flex-1 min-h-0 min-w-0 flex"
      data-testid="ticket-board"
      data-list-mode={layout.list}
      data-discussion-mode={layout.discussion}
    >
      {layout.list === 'rail' ? (
        <div
          data-testid="ticket-list-rail"
          data-auto-folded={layout.listAutoFolded ? '' : undefined}
          className="flex flex-col items-center gap-1 py-2 border-r border-[var(--color-border)] flex-shrink-0 overflow-y-auto"
          style={{ width: layout.listWidth }}
        >
          <div className="flex h-8 mb-1 flex-shrink-0">
            <DrawerToggleButton
              side="left"
              placement="room"
              name="ticket list"
              testId="ticket-list-expand"
              // The rail's own border-r is the line; a divider here would
              // sit mid-rail.
              divider={false}
              onToggle={expandList}
            />
          </div>
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
      ) : (
        listPanel(listWidth, false)
      )}

      {showOverlay && (
        <div data-testid="ticket-list-overlay" className="absolute inset-y-0 left-0 z-30 flex">
          {listPanel(overlayWidth, true)}
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
            discussion={{ mode: layout.discussion, width: layout.discussionWidth }}
          />
        ) : (
          <div className="flex-1 flex items-center justify-center m-4 border border-dashed border-[var(--color-border)] text-xs text-[var(--color-text-muted)] text-center px-6">
            Select a ticket to open its brief.
          </div>
        )}
      </div>
    </div>
  )
}
