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

import React, { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { useProjectsStore } from '@/stores/projects'
import { useFeedbackStore } from '@/stores/feedback'
import { useToastStore } from '@/stores/toast'
import { formatRelativeTime } from '@/components/AgentOps/ops-api'
import ProjectAvatar from '@/components/Sidebar/ProjectAvatar'
import ServerSwitcher from '@/components/TopBar/ServerSwitcher'
import PageTabs from '@/components/TopBar/PageTabs'
import SettingsGearButton from '@/components/TopBar/SettingsGearButton'
import {
  countByStatus,
  fetchAllFeedback,
  filterBySearch,
  groupByStatus,
  resolveFeedback,
  type FeedbackListRow,
  type FeedbackStatus,
} from './feedback-api'
import { FeedbackItemView } from './FeedbackItemView'
import {
  parseProjectFilter,
  rowsForWorkspaceFilter,
  WorkspaceFilterDropdown,
  type FilterableWorkspace,
} from './WorkspaceFilterDropdown'
import { fetchProjectGroupShow } from '@/components/Projects/projects-api'
import { useProjectGroupsStore } from '@/stores/project-groups'
import { KindBadge, PriorityBadge } from './badges'

const TOPBAR_HEIGHT = 38

// ── Per-card status dropdown ──────────────────────────────────────────────

/** The manually selectable statuses (canonical four, NOT customizable):
 *  Waiting reopens a closed item; Resolved/Dismissed close it. Answered
 *  is DISPLAYED when it's the current state but never selectable — it's
 *  only reachable through an actual reply (a manual answered with a
 *  null answer would break the `ask --wait` contract). */
const SELECTABLE_STATUSES = ['waiting', 'resolved', 'dismissed'] as const

/** StatusBadge's palette, shared by the trigger so the dropdown reads
 *  as the card's status badge with a chevron. */
function statusChipClass(status: FeedbackStatus): string {
  return status === 'waiting'
    ? 'bg-orange-400/10 text-orange-400'
    : status === 'answered'
      ? 'bg-green-400/10 text-green-400'
      : 'bg-white/[0.06] text-[var(--color-text-muted)]'
}

function CardStatusDropdown({
  row,
  onMutated,
}: {
  row: FeedbackListRow
  onMutated: () => void
}): React.JSX.Element {
  const [open, setOpen] = useState(false)
  const [busy, setBusy] = useState(false)
  const rootRef = useRef<HTMLDivElement>(null)

  useEffect(() => {
    if (!open) return
    const onDown = (e: MouseEvent): void => {
      if (rootRef.current && !rootRef.current.contains(e.target as Node)) setOpen(false)
    }
    // Capture-phase so the first Esc closes THIS popover instead of
    // reaching the page-level handler (clear selection / close page).
    const onKey = (e: KeyboardEvent): void => {
      if (e.key === 'Escape') {
        e.stopPropagation()
        setOpen(false)
      }
    }
    document.addEventListener('mousedown', onDown)
    window.addEventListener('keydown', onKey, true)
    return () => {
      document.removeEventListener('mousedown', onDown)
      window.removeEventListener('keydown', onKey, true)
    }
  }, [open])

  const setStatus = async (status: (typeof SELECTABLE_STATUSES)[number]): Promise<void> => {
    if (busy || status === row.status) {
      setOpen(false)
      return
    }
    setBusy(true)
    try {
      await resolveFeedback(row.id, status)
      onMutated()
    } catch (e) {
      useToastStore
        .getState()
        .addToast(`Status change failed: ${e instanceof Error ? e.message : String(e)}`, 'error')
    } finally {
      setBusy(false)
      setOpen(false)
    }
  }

  return (
    // stopPropagation everywhere — interacting with the status control
    // must not select/deselect the card underneath.
    <div ref={rootRef} className="relative flex-shrink-0" onClick={(e) => e.stopPropagation()}>
      <button
        type="button"
        disabled={busy}
        onClick={() => setOpen((o) => !o)}
        className={`inline-flex items-center gap-1 px-2 py-0.5 text-[10px] font-medium uppercase tracking-wide cursor-pointer disabled:opacity-50 ${statusChipClass(row.status)}`}
        title="Change status"
      >
        {row.status}
        <svg
          className={`w-2 h-2 transition-transform ${open ? 'rotate-180' : ''}`}
          fill="none"
          viewBox="0 0 24 24"
          stroke="currentColor"
          strokeWidth={2.5}
        >
          <path strokeLinecap="round" strokeLinejoin="round" d="M19 9l-7 7-7-7" />
        </svg>
      </button>

      {open && (
        <div className="absolute right-0 top-full mt-0.5 z-20 min-w-[120px] bg-[var(--color-bg-surface)] border border-[var(--color-border)] shadow-lg py-0.5">
          {/* Answered shows as the current state but is not offered. */}
          {row.status === 'answered' && (
            <div
              className="flex items-center gap-2 px-2 py-1.5 text-[11px] text-green-400 opacity-70 cursor-default select-none"
              title="Answered is set by an actual reply, not by hand"
            >
              <span className="flex-1">Answered</span>
              <CheckGlyph />
            </div>
          )}
          {SELECTABLE_STATUSES.map((s) => {
            const current = row.status === s
            return (
              <button
                key={s}
                type="button"
                disabled={busy}
                onClick={() => void setStatus(s)}
                className={`flex items-center gap-2 w-full px-2 py-1.5 text-[11px] text-left capitalize transition-colors cursor-pointer disabled:opacity-50 ${
                  current
                    ? 'text-[var(--color-text-primary)] bg-white/[0.04]'
                    : 'text-[var(--color-text-secondary)] hover:bg-white/[0.06] hover:text-[var(--color-text-primary)]'
                }`}
                title={s === 'waiting' ? 'Reopen — back to waiting' : undefined}
              >
                <span className="flex-1">{s}</span>
                {current && <CheckGlyph />}
              </button>
            )
          })}
        </div>
      )}
    </div>
  )
}

function CheckGlyph(): React.JSX.Element {
  return (
    <svg className="w-2.5 h-2.5 flex-shrink-0" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2.5}>
      <path strokeLinecap="round" strokeLinejoin="round" d="M5 13l4 4L19 7" />
    </svg>
  )
}

// ── List card ─────────────────────────────────────────────────────────────

export function FeedbackCard({
  row,
  workspace,
  nowSec,
  selected,
  onSelect,
  onMutated,
}: {
  row: FeedbackListRow
  /** The host workspace's store row (icon/color) — undefined when the
   *  project has been unregistered since the ask was filed. */
  workspace: FilterableWorkspace | undefined
  nowSec: number
  selected: boolean
  onSelect: () => void
  onMutated: () => void
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
      {/* Top row: the host WORKSPACE (icon + name — instantly see where
          the ask comes from) left, priority + status dropdown right. */}
      <div className="flex items-center gap-2 min-w-0">
        <div className="flex items-center gap-1.5 flex-1 min-w-0">
          {workspace && (
            <ProjectAvatar
              projectPath={workspace.path}
              projectName={workspace.name}
              projectColor={workspace.color}
              projectId={workspace.id}
              iconUrl={workspace.iconUrl}
              size={18}
            />
          )}
          <span className="text-[11px] text-[var(--color-text-secondary)] truncate">
            {row.projectName}
          </span>
        </div>
        <PriorityBadge priority={row.priority} />
        <CardStatusDropdown row={row} onMutated={onMutated} />
      </div>
      {/* The ask itself is the card body. */}
      <p className="mt-2 text-sm text-[var(--color-text-primary)] break-words" title={row.title}>
        {row.title}
      </p>
      {/* Footer meta: kind tag + submitter · when, comment count right. */}
      <div className="mt-2 flex items-center gap-1.5 text-[10px] text-[var(--color-text-muted)] min-w-0">
        <KindBadge kind={row.kind} />
        <span className="truncate">{row.agentName}</span>
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

export function SectionHeader({ label, count }: { label: string; count: number }): React.JSX.Element {
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
  // Workspace filter — a workspace id, `project:<groupId>` (§6.6), or
  // 'all' (the default).
  const [workspaceFilter, setWorkspaceFilter] = useState<string>('all')
  // Member workspace ids of the selected project filter — resolved via
  // /cli/project-group/show when a project is picked; null = no project
  // filter active OR still resolving.
  const [projectMemberIds, setProjectMemberIds] = useState<Set<string> | null>(null)
  // Tokenized free-text search across title / agent / workspace / id.
  const [search, setSearch] = useState('')
  // Status filter — one status or 'all'; the chips show per-status
  // counts (AFSROW idiom), counted after workspace + search filtering.
  const [statusFilter, setStatusFilter] = useState<FeedbackStatus | 'all'>('all')
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

  // Resolve the selected project filter's membership (§6.6) — refreshed
  // on project-group events (members-changed rides pgRevision) so the
  // filter tracks membership live. A failed resolve logs and leaves the
  // set null (= empty result), never blanking the page.
  const pgRevision = useProjectGroupsStore((s) => s.revision)
  const projectFilterId = parseProjectFilter(workspaceFilter)
  useEffect(() => {
    if (!isOpen || projectFilterId === null) {
      setProjectMemberIds(null)
      return
    }
    let cancelled = false
    fetchProjectGroupShow(projectFilterId)
      .then((show) => {
        if (cancelled) return
        setProjectMemberIds(new Set(show.members.map((m) => m.workspaceId)))
      })
      .catch((err) => {
        if (cancelled) return
        console.warn('[feedback] project-filter membership resolve failed:', err)
        setProjectMemberIds(null)
      })
    return () => {
      cancelled = true
    }
  }, [isOpen, projectFilterId, pgRevision])

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
      setStatusFilter('all')
    }
  }, [isOpen])

  // Workspace + search first; the status chips count THIS set (so each
  // chip says exactly how many cards selecting it reveals), then the
  // active status chip narrows it.
  const searched = useMemo(() => {
    if (!rows) return null
    return filterBySearch(rowsForWorkspaceFilter(rows, workspaceFilter, projectMemberIds), search)
  }, [rows, workspaceFilter, projectMemberIds, search])

  const statusCounts = useMemo(() => (searched ? countByStatus(searched) : null), [searched])

  const filtered = useMemo(() => {
    if (!searched) return null
    return statusFilter === 'all' ? searched : searched.filter((r) => r.status === statusFilter)
  }, [searched, statusFilter])

  const grouped = useMemo(() => (filtered ? groupByStatus(filtered) : null), [filtered])

  // Resolved against the FULL row set (not the filtered subset) so a
  // filter hiding the selected card never blanks the open thread.
  const selectedRow = useMemo(
    () => (selectedId ? rows?.find((r) => r.id === selectedId) ?? null : null),
    [rows, selectedId],
  )

  // Store rows by id for the cards' workspace avatar (icon + color).
  const projectById = useMemo(() => new Map(projects.map((p) => [p.id, p])), [projects])

  // Shared post-mutation refresh (card status dropdown + item view).
  const onMutated = useCallback((): void => {
    void loadList()
    void useFeedbackStore.getState().refreshWaitingCount()
  }, [loadList])

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
          {/* §6.0 — the server dropdown + page switcher stay visible on
              every page; the Feedback tab reads selected here. (Replaces
              the old Back button — Esc still returns to Agents.) */}
          <ServerSwitcher />
          <div className="no-drag">
            <PageTabs />
          </div>
          {/* §6.5 relocation — the settings cog is on every page, right
              after the tabs: K2 | Server | Tabs | ⚙ | … */}
          <SettingsGearButton />
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
        {/* LEFT COLUMN — fixed filter rows (search + workspace filter,
            then the status chips) above the scrollable card list. */}
        <div className="flex flex-col min-h-0 min-w-0">
          <div className="flex flex-col gap-2 px-3 py-2 border-b border-[var(--color-border)] flex-shrink-0">
            <div className="flex items-center gap-2">
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
            {/* Workspace filter — custom dropdown mirroring the
                Settings → Workspaces list (search, focus groups,
                icons), plus the Projects section (§6.6): picking a
                project filters to its member workspaces. */}
            <WorkspaceFilterDropdown
              projects={projects}
              value={workspaceFilter}
              onChange={setWorkspaceFilter}
            />
            </div>

            {/* Status filter — per-status counts always visible (the
                AFSROW board idiom); one status, or All. */}
            <div className="flex items-center gap-1 flex-wrap">
              {(['all', 'waiting', 'answered', 'resolved', 'dismissed'] as const).map((s) => {
                const active = statusFilter === s
                return (
                  <button
                    key={s}
                    type="button"
                    onClick={() => setStatusFilter(s)}
                    className={`flex items-center gap-1.5 px-2 py-1 text-[10px] font-medium border transition-colors cursor-pointer ${
                      active
                        ? 'border-[var(--color-accent)] bg-[var(--color-accent)]/15 text-[var(--color-text-primary)]'
                        : 'border-[var(--color-border)] text-[var(--color-text-secondary)] hover:text-[var(--color-text-primary)] hover:border-[var(--color-text-muted)]'
                    }`}
                    title={s === 'all' ? 'Show every status' : `Show only ${s} items`}
                  >
                    <span className="capitalize">{s === 'all' ? 'All' : s}</span>
                    <span
                      className={`tabular-nums ${
                        active ? 'text-[var(--color-accent)]' : 'text-[var(--color-text-muted)]'
                      }`}
                    >
                      {statusCounts ? statusCounts[s] : 0}
                    </span>
                  </button>
                )
              })}
            </div>
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
                          workspace={projectById.get(row.projectId)}
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
        </div>

        {/* RIGHT COLUMN — the response panel. Swaps to whichever item is
            selected (Thread | Agent tabs at its top); scrolls
            internally, like the list on the left. */}
        <div className="flex flex-col min-h-0 min-w-0 border-t lg:border-t-0 lg:border-l border-[var(--color-border)]">
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
              Select a feedback item to open its thread.
            </div>
          )}
        </div>
      </div>
    </div>
  )
}
