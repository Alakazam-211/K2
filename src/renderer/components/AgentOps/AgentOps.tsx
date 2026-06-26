// Agent Ops — the "prove the pane" Observability view (Phase D).
//
// One full-screen view of every live agent on the ACTIVE daemon:
//   - fleet list from GET /cli/ops/overview (one GET on open),
//   - live status flips over the GET /cli/ops/stream WS (no refetch for the
//     common case; a coalesced refetch only on structural / active changes),
//   - a per-row expand showing recent GET /cli/ops/activity history,
//   - click-through that switches the app to the row's workspace when it's
//     registered on this client.
//
// Single-daemon read view only — cross-daemon fleet is federation-gated and
// out of this slice. Respects feedback_dev_mode_performance: no fetchProjects
// in render/effect paths; the overview is one GET + optimistic live deltas.

import React, { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { daemonCliGet } from '@/lib/daemon-cli'
import { useProjectsStore } from '@/stores/projects'
import { useAgentOpsStore } from '@/stores/agent-ops'
import { subscribeToOpsStream } from './ops-stream'
import {
  applyStatusToRows,
  formatRelativeTime,
  interpretStreamEvent,
  shortAddress,
  workspaceBasename,
  type ActivityRow,
  type AgentStatus,
  type OverviewSession,
} from './ops-api'

const TOPBAR_HEIGHT = 38

// ── Status badge ──────────────────────────────────────────────────────────

function StatusBadge({ status }: { status: AgentStatus | null }): React.JSX.Element {
  if (status === 'working') {
    return (
      <span className="inline-flex items-center gap-1.5 px-2 py-0.5 text-[10px] font-medium uppercase tracking-wide bg-green-400/10 text-green-400">
        <span className="braille-spinner" />
        Working
      </span>
    )
  }
  if (status === 'permission') {
    return (
      <span className="inline-flex items-center gap-1.5 px-2 py-0.5 text-[10px] font-medium uppercase tracking-wide bg-red-400/10 text-red-400">
        Permission
      </span>
    )
  }
  if (status === 'idle') {
    return (
      <span className="inline-flex items-center gap-1.5 px-2 py-0.5 text-[10px] font-medium uppercase tracking-wide bg-white/[0.06] text-[var(--color-text-muted)]">
        Idle
      </span>
    )
  }
  return (
    <span className="inline-flex items-center px-2 py-0.5 text-[10px] font-medium uppercase tracking-wide text-[var(--color-text-muted)] opacity-50">
      —
    </span>
  )
}

// ── Per-row activity timeline (lazy, on expand) ───────────────────────────

function ActivityTimeline({
  projectId,
  nowSec,
}: {
  projectId: string | null
  nowSec: number
}): React.JSX.Element {
  const [rows, setRows] = useState<ActivityRow[] | null>(null)
  const [error, setError] = useState<string | null>(null)

  useEffect(() => {
    let cancelled = false
    if (!projectId) {
      setRows([])
      return
    }
    setRows(null)
    setError(null)
    daemonCliGet<ActivityRow[]>('ops/activity', { project: projectId, limit: 20 })
      .then((r) => {
        if (cancelled) return
        setRows(Array.isArray(r) ? r : [])
      })
      .catch((e) => {
        if (cancelled) return
        setError(e instanceof Error ? e.message : String(e))
      })
    return () => {
      cancelled = true
    }
  }, [projectId])

  if (!projectId) {
    return (
      <p className="text-[11px] text-[var(--color-text-muted)] italic px-1 py-2">
        This workspace isn&apos;t registered on this client — recent history is keyed by
        project and can&apos;t be resolved here.
      </p>
    )
  }
  if (error) {
    return <p className="text-[11px] text-red-400 px-1 py-2">Failed to load activity: {error}</p>
  }
  if (rows === null) {
    return <p className="text-[11px] text-[var(--color-text-muted)] px-1 py-2">Loading activity…</p>
  }
  if (rows.length === 0) {
    return <p className="text-[11px] text-[var(--color-text-muted)] px-1 py-2">No recent activity.</p>
  }

  return (
    <ul className="flex flex-col gap-1 py-1">
      {rows.map((row) => (
        <li key={row.id} className="flex items-baseline gap-2 text-[11px]">
          <span className="text-[var(--color-accent)] font-mono shrink-0">{row.eventType}</span>
          <span className="text-[var(--color-text-secondary)] truncate flex-1 min-w-0">
            {row.summary ?? (row.actor ? `by ${row.actor}` : '')}
          </span>
          <span className="text-[var(--color-text-muted)] tabular-nums shrink-0">
            {formatRelativeTime(row.createdAt, nowSec)}
          </span>
        </li>
      ))}
    </ul>
  )
}

// ── Fleet row ─────────────────────────────────────────────────────────────

function FleetRow({
  session,
  nowSec,
  expanded,
  onToggle,
  onOpen,
  canOpen,
  projectId,
}: {
  session: OverviewSession
  nowSec: number
  expanded: boolean
  onToggle: () => void
  onOpen: () => void
  canOpen: boolean
  projectId: string | null
}): React.JSX.Element {
  const name = workspaceBasename(session.workspacePath)
  return (
    <div className="border-b border-[var(--color-border)]">
      <button
        type="button"
        onClick={onToggle}
        className="w-full flex items-center gap-3 px-4 py-2.5 text-left hover:bg-white/[0.03] transition-colors cursor-pointer"
      >
        <svg
          className="w-3 h-3 text-[var(--color-text-muted)] flex-shrink-0 transition-transform"
          style={{ transform: expanded ? 'rotate(90deg)' : 'none' }}
          fill="none"
          viewBox="0 0 24 24"
          stroke="currentColor"
          strokeWidth={2.5}
        >
          <path strokeLinecap="round" strokeLinejoin="round" d="M9 5l7 7-7 7" />
        </svg>

        <div className="flex flex-col min-w-0 flex-1">
          <span className="text-sm text-[var(--color-text-primary)] truncate" title={session.workspacePath}>
            {name}
          </span>
          <span className="text-[10px] font-mono text-[var(--color-text-muted)] truncate" title={session.agentAddress}>
            {shortAddress(session.agentAddress)}
          </span>
        </div>

        {session.active && (
          <span
            className="inline-flex items-center gap-1 text-[10px] text-green-400/90"
            title="In the canonical Active set"
          >
            <span className="w-1.5 h-1.5 rounded-full bg-green-400" />
            Active
          </span>
        )}
        {session.heartbeatState === 'live' && (
          <span
            className="inline-flex items-center gap-1 text-[10px] text-[var(--color-accent)]"
            title="PTY backs a heartbeat's active terminal"
          >
            <span className="w-1.5 h-1.5 rounded-full bg-[var(--color-accent)]" />
            Heartbeat
          </span>
        )}

        <span className="text-[10px] text-[var(--color-text-muted)] tabular-nums w-16 text-right flex-shrink-0">
          {formatRelativeTime(session.lastActivityAt, nowSec)}
        </span>

        <StatusBadge status={session.agentStatus} />
      </button>

      {expanded && (
        <div className="px-9 pb-3 pt-1 bg-black/10">
          <div className="flex items-center justify-between mb-1">
            <span className="text-[10px] font-semibold uppercase tracking-wider text-[var(--color-text-muted)]">
              Recent activity
            </span>
            {canOpen && (
              <button
                type="button"
                onClick={(e) => {
                  e.stopPropagation()
                  onOpen()
                }}
                className="text-[11px] text-[var(--color-accent)] hover:underline cursor-pointer"
              >
                Open workspace →
              </button>
            )}
          </div>
          <ActivityTimeline projectId={projectId} nowSec={nowSec} />
          <p className="text-[10px] text-[var(--color-text-muted)] opacity-60 mt-1 font-mono break-all">
            session {session.sessionId}
          </p>
        </div>
      )}
    </div>
  )
}

// ── Main view ─────────────────────────────────────────────────────────────

export default function AgentOps(): React.JSX.Element | null {
  const isOpen = useAgentOpsStore((s) => s.isOpen)
  const close = useAgentOpsStore((s) => s.close)
  const projects = useProjectsStore((s) => s.projects)
  const setActiveWorkspace = useProjectsStore((s) => s.setActiveWorkspace)

  const [rows, setRows] = useState<OverviewSession[] | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [connected, setConnected] = useState(false)
  const [expandedId, setExpandedId] = useState<string | null>(null)
  const [nowSec, setNowSec] = useState(() => Math.floor(Date.now() / 1000))

  const rowsRef = useRef<OverviewSession[] | null>(null)
  rowsRef.current = rows
  const refetchTimer = useRef<ReturnType<typeof setTimeout> | null>(null)

  // One-shot overview pull. Stable identity so effects don't churn.
  const loadOverview = useCallback(async (): Promise<void> => {
    try {
      const data = await daemonCliGet<OverviewSession[]>('ops/overview')
      setRows(Array.isArray(data) ? data : [])
      setError(null)
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
      setRows((prev) => prev ?? [])
    }
  }, [])

  // Coalesced refetch — structural / active-set deltas ask for at most one
  // overview GET per 800ms window (never a per-event refetch).
  const scheduleRefetch = useCallback(() => {
    if (refetchTimer.current !== null) return
    refetchTimer.current = setTimeout(() => {
      refetchTimer.current = null
      void loadOverview()
    }, 800)
  }, [loadOverview])

  // Data lifecycle — only while the pane is open. One GET + the live stream;
  // teardown on close so we hold no socket when hidden.
  useEffect(() => {
    if (!isOpen) return
    setRows(null)
    setError(null)
    void loadOverview()

    const unsub = subscribeToOpsStream({
      onHello: () => {
        // (Re)connected — backfill anything missed during the drop window.
        void loadOverview()
      },
      onEnvelope: (env) => {
        const action = interpretStreamEvent(env, Math.floor(Date.now() / 1000))
        if (action.type === 'status') {
          const cur = rowsRef.current
          if (!cur) return
          setRows(applyStatusToRows(cur, action.sessionId, action.status, action.at))
        } else if (action.type === 'refetch') {
          scheduleRefetch()
        }
      },
      onConnectionChange: setConnected,
    })

    return () => {
      unsub()
      if (refetchTimer.current !== null) {
        clearTimeout(refetchTimer.current)
        refetchTimer.current = null
      }
    }
  }, [isOpen, loadOverview, scheduleRefetch])

  // Relative-time ticker — re-render "Xm ago" labels without refetching.
  useEffect(() => {
    if (!isOpen) return
    const id = setInterval(() => setNowSec(Math.floor(Date.now() / 1000)), 30_000)
    return () => clearInterval(id)
  }, [isOpen])

  // Esc closes the overlay.
  useEffect(() => {
    if (!isOpen) return
    const onKey = (e: KeyboardEvent): void => {
      if (e.key === 'Escape') {
        e.preventDefault()
        close()
      }
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [isOpen, close])

  // path → {projectId, workspaceId} resolution for the activity key + click-
  // through. Built from the already-loaded projects store (no fetch).
  const resolveByPath = useMemo(() => {
    const map = new Map<string, { projectId: string; workspaceId: string }>()
    for (const p of projects) {
      // Project root maps to its first workspace.
      const first = p.workspaces[0]
      if (first) map.set(p.path, { projectId: p.id, workspaceId: first.id })
      for (const ws of p.workspaces) {
        if (ws.worktreePath) map.set(ws.worktreePath, { projectId: p.id, workspaceId: ws.id })
      }
    }
    return map
  }, [projects])

  const handleOpenWorkspace = useCallback(
    (path: string) => {
      const hit = resolveByPath.get(path)
      if (!hit) return
      setActiveWorkspace(hit.projectId, hit.workspaceId)
      close()
    },
    [resolveByPath, setActiveWorkspace, close],
  )

  if (!isOpen) return null

  const liveCount = rows?.filter((r) => r.agentStatus === 'working').length ?? 0
  const total = rows?.length ?? 0

  return (
    <div className="fixed inset-0 z-50 flex flex-col bg-[var(--color-bg)]">
      {/* Top bar — mirrors Settings: traffic-light spacer + wordmark, draggable. */}
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
          <span className="text-[11px] text-[var(--color-text-secondary)]">Agent Ops</span>
          <span
            className="ml-1 inline-flex items-center gap-1.5 text-[10px] no-drag"
            title={connected ? 'Live stream connected' : 'Reconnecting…'}
          >
            <span
              className={`w-1.5 h-1.5 rounded-full ${connected ? 'bg-green-400' : 'bg-[var(--color-text-muted)]'}`}
              style={connected ? { boxShadow: '0 0 4px #4ade80' } : undefined}
            />
            <span className={connected ? 'text-green-400' : 'text-[var(--color-text-muted)]'}>
              {connected ? 'Live' : 'Offline'}
            </span>
          </span>
        </div>

        <div className="flex items-center gap-1 no-drag">
          <button
            type="button"
            onClick={() => void loadOverview()}
            className="px-2 py-1 text-[11px] text-[var(--color-text-secondary)] hover:text-[var(--color-text-primary)] hover:bg-white/[0.06] transition-colors cursor-pointer"
            title="Refresh"
          >
            Refresh
          </button>
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

      {/* Summary strip */}
      <div className="flex items-center gap-4 px-4 py-2 border-b border-[var(--color-border)] text-[11px] text-[var(--color-text-muted)] flex-shrink-0">
        <span>
          <span className="text-[var(--color-text-primary)] tabular-nums font-medium">{total}</span> live{' '}
          {total === 1 ? 'agent' : 'agents'}
        </span>
        <span>
          <span className="text-green-400 tabular-nums font-medium">{liveCount}</span> working
        </span>
      </div>

      {/* Fleet list */}
      <div className="flex-1 overflow-y-auto min-h-0">
        {error && (
          <div className="px-4 py-3 text-[11px] text-red-400">
            Failed to load fleet: {error}
          </div>
        )}
        {rows === null && !error && (
          <div className="px-4 py-8 text-center text-[var(--color-text-muted)] text-sm">Loading fleet…</div>
        )}
        {rows !== null && rows.length === 0 && (
          <div className="flex flex-col items-center justify-center h-full text-center px-8">
            <p className="text-sm text-[var(--color-text-secondary)]">No live agents</p>
            <p className="text-xs text-[var(--color-text-muted)] mt-1 opacity-70">
              Launch an agent in a workspace and it will appear here.
            </p>
          </div>
        )}
        {rows?.map((session) => {
          const hit = resolveByPath.get(session.workspacePath)
          return (
            <FleetRow
              key={session.sessionId}
              session={session}
              nowSec={nowSec}
              expanded={expandedId === session.sessionId}
              onToggle={() =>
                setExpandedId((cur) => (cur === session.sessionId ? null : session.sessionId))
              }
              onOpen={() => handleOpenWorkspace(session.workspacePath)}
              canOpen={!!hit}
              projectId={hit?.projectId ?? null}
            />
          )
        })}
      </div>
    </div>
  )
}
