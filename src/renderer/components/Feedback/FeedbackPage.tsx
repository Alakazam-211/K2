// Feedback F2 — the full-page agent→human ask queue
// (prd-agent-feedback-notifications §6 F2).
//
// Mirrors AgentOps' full-screen overlay idiom: a fixed inset view over the
// app, opened from the top-bar Feedback button (useFeedbackStore), its own
// draggable top bar with a back affordance, Esc to close. The list fans
// out `/cli/feedback/list?all=1` per registered workspace (feedback-api)
// and stays live via the store's `revision` (bumped by the
// feedback:created / feedback:answered listeners) — no polling loop.
//
// Layout is the AFSROW master-detail board: a persistent card list in the
// left column (search + workspace filter fixed above it, the list itself
// scrolls), a response panel in the right column. Selecting a card swaps
// the right panel in place — no navigation. Zero selection shows a
// dashed empty state; the selection survives filters hiding its card.

import React, { useCallback, useEffect, useMemo, useState } from 'react'
import { useProjectsStore } from '@/stores/projects'
import { useFeedbackStore } from '@/stores/feedback'
import { formatRelativeTime } from '@/components/AgentOps/ops-api'
import {
  fetchAllFeedback,
  filterBySearch,
  groupByStatus,
  type FeedbackListRow,
} from './feedback-api'
import { FeedbackItemView } from './FeedbackItemView'
import { KindBadge, PriorityBadge, StatusBadge } from './badges'

const TOPBAR_HEIGHT = 38

// ── List card ─────────────────────────────────────────────────────────────

function FeedbackCard({
  row,
  nowSec,
  selected,
  onSelect,
}: {
  row: FeedbackListRow
  nowSec: number
  selected: boolean
  onSelect: () => void
}): React.JSX.Element {
  const dimmed = row.status === 'resolved' || row.status === 'dismissed'
  return (
    <div
      onClick={onSelect}
      className={`border bg-[var(--color-bg-surface)] p-3 cursor-pointer transition-colors ${
        selected
          ? 'border-[var(--color-accent)] ring-1 ring-[var(--color-accent)]'
          : 'border-[var(--color-border)] hover:border-[var(--color-text-muted)]'
      } ${dimmed && !selected ? 'opacity-60' : ''}`}
    >
      {/* Top row: kind + priority left, status right (card anatomy). */}
      <div className="flex items-center gap-2">
        <KindBadge kind={row.kind} />
        <PriorityBadge priority={row.priority} />
        <div className="flex-1" />
        <StatusBadge status={row.status} />
      </div>
      {/* The ask itself is the card body. */}
      <p className="mt-2 text-sm text-[var(--color-text-primary)] break-words" title={row.title}>
        {row.title}
      </p>
      {/* Footer meta: submitter · workspace · when, comment count right. */}
      <div className="mt-2 flex items-center gap-1.5 text-[10px] text-[var(--color-text-muted)] min-w-0">
        <span className="truncate">{row.agentName}</span>
        <span className="opacity-60 flex-shrink-0">·</span>
        <span className="truncate opacity-80">{row.projectName}</span>
        <span className="opacity-60 flex-shrink-0">·</span>
        <span className="tabular-nums flex-shrink-0">
          {formatRelativeTime(row.createdAt, nowSec)}
        </span>
        {row.commentCount > 1 && (
          <span
            className="ml-auto inline-flex items-center gap-1 text-[var(--color-accent)] tabular-nums flex-shrink-0"
            title={`${row.commentCount} messages in thread`}
          >
            <svg width="10" height="10" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
              <path d="M21 15a2 2 0 0 1-2 2H7l-4 4V5a2 2 0 0 1 2-2h14a2 2 0 0 1 2 2z" />
            </svg>
            {row.commentCount}
          </span>
        )}
      </div>
    </div>
  )
}

function SectionHeader({ label, count }: { label: string; count: number }): React.JSX.Element {
  return (
    <div className="flex items-center gap-2 px-1 pt-4 pb-1.5">
      <span className="text-[10px] font-semibold uppercase tracking-wider text-[var(--color-text-muted)]">
        {label}
      </span>
      <span className="text-[10px] text-[var(--color-text-muted)] tabular-nums opacity-70">{count}</span>
    </div>
  )
}

// ── Main view ─────────────────────────────────────────────────────────────

export default function FeedbackPage(): React.JSX.Element | null {
  const isOpen = useFeedbackStore((s) => s.isOpen)
  const close = useFeedbackStore((s) => s.close)
  const revision = useFeedbackStore((s) => s.revision)
  const projects = useProjectsStore((s) => s.projects)

  const [rows, setRows] = useState<FeedbackListRow[] | null>(null)
  const [error, setError] = useState<string | null>(null)
  // Workspace filter — project id, or 'all' (the default; a project
  // filter joins later with the projects feature).
  const [workspaceFilter, setWorkspaceFilter] = useState<string>('all')
  // Tokenized free-text search across title / agent / workspace / id.
  const [search, setSearch] = useState('')
  const [selectedId, setSelectedId] = useState<string | null>(null)
  const [nowSec, setNowSec] = useState(() => Math.floor(Date.now() / 1000))

  const loadList = useCallback(async (): Promise<void> => {
    try {
      const data = await fetchAllFeedback(
        useProjectsStore.getState().projects.map((p) => ({ id: p.id, name: p.name, path: p.path })),
      )
      setRows(data)
      setError(null)
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
      setRows((prev) => prev ?? [])
    }
  }, [])

  // Fetch while open: on open, on every feedback event (revision), and
  // when the registered-projects set changes.
  useEffect(() => {
    if (!isOpen) return
    void loadList()
  }, [isOpen, revision, projects, loadList])

  // Relative-time ticker (AgentOps idiom) — labels only, no refetch.
  useEffect(() => {
    if (!isOpen) return
    const id = setInterval(() => setNowSec(Math.floor(Date.now() / 1000)), 30_000)
    return () => clearInterval(id)
  }, [isOpen])

  // Esc — clear the selection first, then close the page.
  useEffect(() => {
    if (!isOpen) return
    const onKey = (e: KeyboardEvent): void => {
      if (e.key === 'Escape') {
        e.preventDefault()
        if (selectedId !== null) setSelectedId(null)
        else close()
      }
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [isOpen, selectedId, close])

  // Reset transient view state when the page closes.
  useEffect(() => {
    if (!isOpen) {
      setSelectedId(null)
      setRows(null)
      setError(null)
      setSearch('')
    }
  }, [isOpen])

  const filtered = useMemo(() => {
    if (!rows) return null
    const byWorkspace =
      workspaceFilter === 'all' ? rows : rows.filter((r) => r.projectId === workspaceFilter)
    return filterBySearch(byWorkspace, search)
  }, [rows, workspaceFilter, search])

  const grouped = useMemo(() => (filtered ? groupByStatus(filtered) : null), [filtered])

  // Resolved against the FULL row set (not the filtered subset) so a
  // filter hiding the selected card never blanks the open thread.
  const selectedRow = useMemo(
    () => (selectedId ? rows?.find((r) => r.id === selectedId) ?? null : null),
    [rows, selectedId],
  )

  if (!isOpen) return null

  const sections = grouped
    ? ([
        { label: 'Waiting on you', rows: grouped.waiting },
        { label: 'Answered', rows: grouped.answered },
        { label: 'Closed', rows: grouped.closed },
      ] as const)
    : []

  return (
    <div className="fixed inset-0 z-50 flex flex-col bg-[var(--color-bg)]">
      {/* Top bar — mirrors AgentOps: traffic-light spacer + wordmark, draggable. */}
      <div
        className="flex items-center border-b border-[var(--color-border)] bg-[var(--color-bg-surface)] px-3 select-none flex-shrink-0"
        data-tauri-drag-region
        style={{ height: TOPBAR_HEIGHT, minHeight: TOPBAR_HEIGHT }}
      >
        <div className="flex items-center gap-2 flex-1">
          <div style={{ width: 70 }} />
          <span className="text-[10px] font-bold tracking-widest text-[var(--color-text-muted)] uppercase flex-shrink-0">
            K2
          </span>
          {/* Back — returns to the previous app view (page-level). */}
          <button
            type="button"
            onClick={close}
            className="flex items-center gap-1 px-1.5 py-1 text-[11px] text-[var(--color-text-secondary)] hover:text-[var(--color-text-primary)] hover:bg-white/[0.06] transition-colors cursor-pointer no-drag"
            title="Back (Esc)"
          >
            <svg width="10" height="10" viewBox="0 0 10 10" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round">
              <polyline points="6 2 3 5 6 8" />
            </svg>
            Back
          </button>
          <span className="text-[11px] text-[var(--color-text-secondary)]">Feedback</span>
        </div>

        <div className="flex items-center gap-2 no-drag">
          <button
            type="button"
            onClick={close}
            className="flex items-center justify-center w-7 h-7 text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)] hover:bg-white/[0.06] transition-colors cursor-pointer"
            title="Close (Esc)"
          >
            <svg width="12" height="12" viewBox="0 0 12 12" fill="none" stroke="currentColor" strokeWidth="1.5">
              <line x1="2" y1="2" x2="10" y2="10" />
              <line x1="10" y1="2" x2="2" y2="10" />
            </svg>
          </button>
        </div>
      </div>

      {/* Master-detail: fluid list column left, response panel right.
          Below lg the columns stack 50/50 (the app window can go down to
          800px wide); each half keeps its own internal scroll. */}
      <div className="flex-1 min-h-0 grid grid-cols-1 grid-rows-[minmax(0,1fr)_minmax(0,1fr)] lg:grid-cols-[minmax(0,1fr)_720px] lg:grid-rows-[minmax(0,1fr)]">
        {/* LEFT COLUMN — fixed [search + workspace filter] row above the
            scrollable card list. */}
        <div className="flex flex-col min-h-0 min-w-0">
          <div className="flex items-center gap-2 px-3 py-2 border-b border-[var(--color-border)] flex-shrink-0">
            <div className="relative flex-1 min-w-0">
              <input
                type="text"
                value={search}
                onChange={(e) => setSearch(e.target.value)}
                placeholder="Search by title, agent, workspace, id… (any order)"
                className="w-full px-2.5 py-1.5 pr-7 text-[11px] bg-[var(--color-bg-elevated)] text-[var(--color-text-primary)] border border-[var(--color-border)] outline-none focus:border-[var(--color-accent)] placeholder:text-[var(--color-text-muted)]"
              />
              {search && (
                <button
                  type="button"
                  onClick={() => setSearch('')}
                  aria-label="Clear search"
                  className="absolute right-1.5 top-1/2 -translate-y-1/2 flex items-center justify-center w-4 h-4 text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)] transition-colors cursor-pointer"
                >
                  <svg width="9" height="9" viewBox="0 0 12 12" fill="none" stroke="currentColor" strokeWidth="1.5">
                    <line x1="2" y1="2" x2="10" y2="10" />
                    <line x1="10" y1="2" x2="2" y2="10" />
                  </svg>
                </button>
              )}
            </div>
            {/* Workspace filter (a project filter joins later). */}
            <select
              value={workspaceFilter}
              onChange={(e) => setWorkspaceFilter(e.target.value)}
              className="px-2 py-1.5 text-[11px] bg-[var(--color-bg-elevated)] text-[var(--color-text-secondary)] border border-[var(--color-border)] outline-none cursor-pointer flex-shrink-0"
              title="Filter by workspace"
            >
              <option value="all">All workspaces</option>
              {projects.map((p) => (
                <option key={p.id} value={p.id}>
                  {p.name}
                </option>
              ))}
            </select>
          </div>

          <div className="flex-1 overflow-y-auto min-h-0 px-3 pb-3">
            {error && (
              <div className="px-1 py-3 text-[11px] text-red-400">Failed to load feedback: {error}</div>
            )}
            {rows === null && !error && (
              <div className="px-1 py-8 text-center text-[var(--color-text-muted)] text-sm">Loading feedback…</div>
            )}
            {grouped && filtered !== null && filtered.length === 0 && (
              <div className="flex flex-col items-center justify-center h-full text-center px-8">
                {rows !== null && rows.length > 0 ? (
                  <p className="text-sm text-[var(--color-text-secondary)]">No feedback matches your filters</p>
                ) : (
                  <>
                    <p className="text-sm text-[var(--color-text-secondary)]">No feedback yet</p>
                    <p className="text-xs text-[var(--color-text-muted)] mt-1 opacity-70">
                      Agents file asks here with `k2 feedback ask` — new items appear live.
                    </p>
                  </>
                )}
              </div>
            )}
            {sections.map(
              (section) =>
                section.rows.length > 0 && (
                  <React.Fragment key={section.label}>
                    <SectionHeader label={section.label} count={section.rows.length} />
                    <div className="flex flex-col gap-2">
                      {section.rows.map((row) => (
                        <FeedbackCard
                          key={row.id}
                          row={row}
                          nowSec={nowSec}
                          selected={selectedId === row.id}
                          onSelect={() => setSelectedId(row.id)}
                        />
                      ))}
                    </div>
                  </React.Fragment>
                ),
            )}
          </div>
        </div>

        {/* RIGHT COLUMN — the response panel. Swaps to whichever item is
            selected (Thread | Terminal tabs at its top); scrolls
            internally, like the list on the left. */}
        <div className="flex flex-col min-h-0 min-w-0 border-t lg:border-t-0 lg:border-l border-[var(--color-border)]">
          {selectedRow ? (
            <FeedbackItemView
              key={selectedRow.id}
              id={selectedRow.id}
              listRow={selectedRow}
              nowSec={nowSec}
              revision={revision}
              onMutated={() => {
                void loadList()
                void useFeedbackStore.getState().refreshWaitingCount()
              }}
            />
          ) : (
            <div className="flex-1 flex items-center justify-center m-4 border border-dashed border-[var(--color-border)] text-xs text-[var(--color-text-muted)] text-center px-6">
              Select a feedback item to open its thread.
            </div>
          )}
        </div>
      </div>
    </div>
  )
}
