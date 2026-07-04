// Feedback F2 — the full-page agent→human ask queue
// (prd-agent-feedback-notifications §6 F2).
//
// Mirrors AgentOps' full-screen overlay idiom: a fixed inset view over the
// app, opened from the top-bar Feedback button (useFeedbackStore), its own
// draggable top bar with a back affordance, Esc to close. The list fans
// out `/cli/feedback/list?all=1` per registered workspace (feedback-api)
// and stays live via the store's `revision` (bumped by the
// feedback:created / feedback:answered listeners) — no polling loop.

import React, { useCallback, useEffect, useMemo, useState } from 'react'
import { useProjectsStore } from '@/stores/projects'
import { useFeedbackStore } from '@/stores/feedback'
import { formatRelativeTime } from '@/components/AgentOps/ops-api'
import {
  fetchAllFeedback,
  groupByStatus,
  type FeedbackListRow,
} from './feedback-api'
import { FeedbackItemView } from './FeedbackItemView'
import { KindBadge, PriorityBadge, StatusBadge } from './badges'

const TOPBAR_HEIGHT = 38

// ── List row ──────────────────────────────────────────────────────────────

function FeedbackRow({
  row,
  nowSec,
  onOpen,
}: {
  row: FeedbackListRow
  nowSec: number
  onOpen: () => void
}): React.JSX.Element {
  const dimmed = row.status === 'resolved' || row.status === 'dismissed'
  return (
    <button
      type="button"
      onClick={onOpen}
      className={`w-full flex items-center gap-3 px-4 py-2.5 text-left border-b border-[var(--color-border)] hover:bg-white/[0.03] transition-colors cursor-pointer ${dimmed ? 'opacity-60' : ''}`}
    >
      <div className="flex flex-col min-w-0 flex-1">
        <span className="text-sm text-[var(--color-text-primary)] truncate" title={row.title}>
          {row.title}
        </span>
        <span className="text-[10px] text-[var(--color-text-muted)] truncate">
          {row.agentName}
          <span className="opacity-60"> · {row.projectName}</span>
        </span>
      </div>
      {row.commentCount > 1 && (
        <span
          className="inline-flex items-center gap-1 text-[10px] text-[var(--color-text-muted)] tabular-nums flex-shrink-0"
          title={`${row.commentCount} messages in thread`}
        >
          <svg width="10" height="10" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
            <path d="M21 15a2 2 0 0 1-2 2H7l-4 4V5a2 2 0 0 1 2-2h14a2 2 0 0 1 2 2z" />
          </svg>
          {row.commentCount}
        </span>
      )}
      <PriorityBadge priority={row.priority} />
      <KindBadge kind={row.kind} />
      <StatusBadge status={row.status} />
      <span className="text-[10px] text-[var(--color-text-muted)] tabular-nums w-16 text-right flex-shrink-0">
        {formatRelativeTime(row.createdAt, nowSec)}
      </span>
    </button>
  )
}

function SectionHeader({ label, count }: { label: string; count: number }): React.JSX.Element {
  return (
    <div className="flex items-center gap-2 px-4 pt-4 pb-1">
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

  // Esc — step back out of the item view first, then close the page.
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
    }
  }, [isOpen])

  const filtered = useMemo(() => {
    if (!rows) return null
    if (workspaceFilter === 'all') return rows
    return rows.filter((r) => r.projectId === workspaceFilter)
  }, [rows, workspaceFilter])

  const grouped = useMemo(() => (filtered ? groupByStatus(filtered) : null), [filtered])

  const selectedRow = useMemo(
    () => (selectedId ? rows?.find((r) => r.id === selectedId) ?? null : null),
    [rows, selectedId],
  )

  if (!isOpen) return null

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
          {/* Back — leaves the item view first; from the list it returns
              to the previous app view. */}
          <button
            type="button"
            onClick={() => (selectedId ? setSelectedId(null) : close())}
            className="flex items-center gap-1 px-1.5 py-1 text-[11px] text-[var(--color-text-secondary)] hover:text-[var(--color-text-primary)] hover:bg-white/[0.06] transition-colors cursor-pointer no-drag"
            title={selectedId ? 'Back to feedback list' : 'Back (Esc)'}
          >
            <svg width="10" height="10" viewBox="0 0 10 10" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round">
              <polyline points="6 2 3 5 6 8" />
            </svg>
            Back
          </button>
          <span className="text-[11px] text-[var(--color-text-secondary)]">Feedback</span>
        </div>

        <div className="flex items-center gap-2 no-drag">
          {/* Workspace filter (a project filter joins later). */}
          {!selectedId && (
            <select
              value={workspaceFilter}
              onChange={(e) => setWorkspaceFilter(e.target.value)}
              className="px-2 py-1 text-[11px] bg-[var(--color-bg-elevated)] text-[var(--color-text-secondary)] border border-[var(--color-border)] outline-none cursor-pointer"
              title="Filter by workspace"
            >
              <option value="all">All workspaces</option>
              {projects.map((p) => (
                <option key={p.id} value={p.id}>
                  {p.name}
                </option>
              ))}
            </select>
          )}
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
        <div className="flex-1 overflow-y-auto min-h-0">
          {error && (
            <div className="px-4 py-3 text-[11px] text-red-400">Failed to load feedback: {error}</div>
          )}
          {rows === null && !error && (
            <div className="px-4 py-8 text-center text-[var(--color-text-muted)] text-sm">Loading feedback…</div>
          )}
          {grouped && filtered !== null && filtered.length === 0 && (
            <div className="flex flex-col items-center justify-center h-full text-center px-8">
              <p className="text-sm text-[var(--color-text-secondary)]">No feedback yet</p>
              <p className="text-xs text-[var(--color-text-muted)] mt-1 opacity-70">
                Agents file asks here with `k2 feedback ask` — new items appear live.
              </p>
            </div>
          )}
          {grouped && grouped.waiting.length > 0 && (
            <>
              <SectionHeader label="Waiting on you" count={grouped.waiting.length} />
              {grouped.waiting.map((row) => (
                <FeedbackRow key={row.id} row={row} nowSec={nowSec} onOpen={() => setSelectedId(row.id)} />
              ))}
            </>
          )}
          {grouped && grouped.answered.length > 0 && (
            <>
              <SectionHeader label="Answered" count={grouped.answered.length} />
              {grouped.answered.map((row) => (
                <FeedbackRow key={row.id} row={row} nowSec={nowSec} onOpen={() => setSelectedId(row.id)} />
              ))}
            </>
          )}
          {grouped && grouped.closed.length > 0 && (
            <>
              <SectionHeader label="Closed" count={grouped.closed.length} />
              {grouped.closed.map((row) => (
                <FeedbackRow key={row.id} row={row} nowSec={nowSec} onOpen={() => setSelectedId(row.id)} />
              ))}
            </>
          )}
        </div>
      )}
    </div>
  )
}
