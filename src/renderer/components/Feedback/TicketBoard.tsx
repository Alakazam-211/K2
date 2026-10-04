// Tickets board — the list + detail layout shared by the Tickets page and
// the Projects page Feedback tab (0.43.2 quick redesign, Rosson).
//
//   - A narrow list (default 300 px) on the left, resizable with a drag
//     handle; the detail (header + HTML brief + action bar) takes the rest.
//   - Filters on top of the list: Mine (assigned to me, the default),
//     Waiting on me, All, plus search. The caller may add one extra control
//     (the Tickets page adds its workspace filter).
//   - A collapse button folds the list into a slim rail of agent avatars
//     with status dots; clicking one selects that ticket.
//   - List width and the collapsed state are remembered PER WINDOW in
//     localStorage (`ticket-board-chrome.ts`); never daemon state.

import React, { useCallback, useEffect, useMemo, useState } from 'react'
import { useProjectsStore } from '@/stores/projects'
import {
  readTicketBoardChrome,
  writeTicketBoardChrome,
  clampTicketListWidth,
  type TicketBoardChrome,
} from '@/lib/ticket-board-chrome'
import {
  DEFAULT_TICKET_SCOPE,
  filterByScope,
  filterBySearch,
  groupByStatus,
  TICKET_SCOPES,
  UNLINKED_WORKSPACE_LABEL,
  type FeedbackListRow,
  type TicketScope,
} from './feedback-api'
import { FeedbackCard, SectionHeader, TicketRailItem } from './TicketCard'
import { FeedbackItemView } from './FeedbackItemView'
import { useTicketMe } from './useTicketMe'

export interface TicketBoardProps {
  /** Rows to show (the caller's own scoping — workspace filter, project
   *  members — already applied). `null` = still loading. */
  rows: FeedbackListRow[] | null
  /** The FULL row set, so the open ticket survives a filter hiding it. */
  allRows?: FeedbackListRow[] | null
  error: string | null
  revision: number
  onMutated: () => void
  /** One extra control beside search (the Tickets page's workspace filter). */
  extraFilter?: React.ReactNode
  /** Empty-state hint when there are no tickets at all. */
  emptyHint?: string
  /** Selected ticket to open on mount (the "Open in window" target). */
  initialSelectedId?: string | null
}

export function TicketBoard({
  rows,
  allRows,
  error,
  revision,
  onMutated,
  extraFilter,
  emptyHint = 'Agents file asks with `k2 tickets ask` — new items appear live.',
  initialSelectedId = null,
}: TicketBoardProps): React.JSX.Element {
  const projects = useProjectsStore((s) => s.projects)
  const me = useTicketMe()

  const [scope, setScope] = useState<TicketScope>(DEFAULT_TICKET_SCOPE)
  const [search, setSearch] = useState('')
  const [selectedId, setSelectedId] = useState<string | null>(initialSelectedId)
  const [nowSec, setNowSec] = useState(() => Math.floor(Date.now() / 1000))
  const [chrome, setChrome] = useState<TicketBoardChrome>(() => readTicketBoardChrome())
  const [liveWidth, setLiveWidth] = useState<number | null>(null)
  const [resizing, setResizing] = useState(false)

  // Relative-time ticker — labels only, no refetch.
  useEffect(() => {
    const id = setInterval(() => setNowSec(Math.floor(Date.now() / 1000)), 30_000)
    return () => clearInterval(id)
  }, [])

  // Capture-phase Esc clears the selection BEFORE the page's own Esc
  // (close the Tickets page / leave Projects) sees it. An open brief
  // overlay or status menu handles its own Esc first.
  useEffect(() => {
    if (selectedId === null) return
    const onKey = (e: KeyboardEvent): void => {
      if (e.key !== 'Escape') return
      if (document.querySelector('[data-testid="brief-overlay"]')) return
      // The answer box closes itself on Esc; keep the ticket open.
      if ((e.target as HTMLElement | null)?.tagName === 'TEXTAREA') return
      e.preventDefault()
      e.stopPropagation()
      setSelectedId(null)
    }
    window.addEventListener('keydown', onKey, true)
    return () => window.removeEventListener('keydown', onKey, true)
  }, [selectedId])

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

  const sections = grouped
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

  const listWidth = liveWidth ?? chrome.listWidth
  const startResize = useCallback(
    (e: React.MouseEvent): void => {
      if (e.button !== 0) return
      e.preventDefault()
      const startX = e.clientX
      const base = chrome.listWidth
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
    [chrome.listWidth],
  )

  const emptyList = filtered !== null && filtered.length === 0

  return (
    <div className="flex-1 min-h-0 min-w-0 flex" data-testid="ticket-board">
      {chrome.collapsed ? (
        <div
          data-testid="ticket-list-rail"
          className="flex flex-col items-center gap-1 py-2 border-r border-[var(--color-border)] flex-shrink-0 overflow-y-auto"
          style={{ width: 48 }}
        >
          <button
            type="button"
            data-testid="ticket-list-expand"
            onClick={() => setCollapsed(false)}
            title="Show the ticket list"
            aria-label="Show the ticket list"
            className="flex items-center justify-center w-7 h-7 mb-1 text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)] hover:bg-white/[0.06] cursor-pointer"
          >
            <ChevronGlyph dir="right" />
          </button>
          {ordered.map((row) => (
            <TicketRailItem
              key={row.id}
              row={row}
              workspace={projectById.get(row.projectId)}
              selected={selectedId === row.id}
              onSelect={() => setSelectedId(row.id)}
            />
          ))}
        </div>
      ) : (
        <div
          data-testid="ticket-list"
          className="relative flex flex-col min-h-0 border-r border-[var(--color-border)] flex-shrink-0"
          style={{ width: listWidth }}
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
                          : 'text-[var(--color-text-secondary)] hover:text-[var(--color-text-primary)] hover:bg-white/[0.04]'
                      }`}
                    >
                      <span className="truncate">{s.label}</span>
                      <span className={`tabular-nums ${active ? 'text-[var(--color-accent)]' : 'text-[var(--color-text-muted)]'}`}>
                        {scopeCounts[s.value]}
                      </span>
                    </button>
                  )
                })}
              </div>
              <button
                type="button"
                data-testid="ticket-list-collapse"
                onClick={() => setCollapsed(true)}
                title="Collapse the ticket list"
                aria-label="Collapse the ticket list"
                className="flex items-center justify-center w-6 h-6 flex-shrink-0 text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)] hover:bg-white/[0.06] cursor-pointer"
              >
                <ChevronGlyph dir="left" />
              </button>
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
            {error && (
              <div className="px-1 py-3 text-[11px] text-[var(--color-status-error-soft)] selectable-copy">
                Failed to load tickets: {error}
              </div>
            )}
            {rows === null && !error && (
              <div className="px-1 py-8 text-center text-[var(--color-text-muted)] text-xs">Loading tickets…</div>
            )}
            {emptyList && (
              <div data-testid="ticket-list-empty" className="flex flex-col items-center justify-center h-full text-center px-4">
                {rows !== null && rows.length > 0 ? (
                  <>
                    <p className="text-xs text-[var(--color-text-secondary)]">
                      {scope === 'all' || search ? 'No tickets match your search' : 'Nothing assigned to you here'}
                    </p>
                    {scope !== 'all' && (
                      <button
                        type="button"
                        onClick={() => setScope('all')}
                        className="mt-1.5 text-[11px] text-[var(--color-accent)] hover:underline cursor-pointer"
                      >
                        Show all tickets
                      </button>
                    )}
                  </>
                ) : (
                  <>
                    <p className="text-xs text-[var(--color-text-secondary)]">No tickets yet</p>
                    <p className="text-[11px] text-[var(--color-text-muted)] mt-1 opacity-70">{emptyHint}</p>
                  </>
                )}
              </div>
            )}
            {sections.map(
              (section) =>
                section.rows.length > 0 && (
                  <React.Fragment key={section.label}>
                    <SectionHeader label={section.label} count={section.rows.length} />
                    <div className="flex flex-col gap-1.5">
                      {section.rows.map((row) => (
                        <FeedbackCard
                          key={row.id}
                          row={row}
                          nowSec={nowSec}
                          selected={selectedId === row.id}
                          onSelect={() => setSelectedId(row.id)}
                          onMutated={onMutated}
                        />
                      ))}
                    </div>
                  </React.Fragment>
                ),
            )}
          </div>

          <div
            data-testid="ticket-list-resize"
            role="separator"
            aria-orientation="vertical"
            aria-label="Resize the ticket list"
            className="absolute top-0 bottom-0 z-10 cursor-col-resize hover:bg-[var(--color-accent)]/40 transition-colors"
            style={{ right: -4, width: 7 }}
            onMouseDown={startResize}
          />
          {resizing && <div className="fixed inset-0 z-50" style={{ cursor: 'col-resize' }} />}
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

function ChevronGlyph({ dir }: { dir: 'left' | 'right' }): React.JSX.Element {
  return (
    <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth={2} aria-hidden>
      {dir === 'left' ? (
        <path strokeLinecap="round" strokeLinejoin="round" d="M15 18l-6-6 6-6" />
      ) : (
        <path strokeLinecap="round" strokeLinejoin="round" d="M9 18l6-6-6-6" />
      )}
    </svg>
  )
}
