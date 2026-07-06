// Projects V1 P5 (prd-projects-v1 §6.2/§6.3) — the Dashboard tab's pane
// grid: percent-width COLUMNS of panes over the canonical layout blob.
//
// Pane kinds (§6.2):
//   1. `terminal` — the member workspace's CANONICAL session, rendered
//      through the kessel attach idiom (`attachAgentName={workspaceId}`
//      keys TerminalPane's idempotent /cli/sessions/v2/spawn to the
//      existing daemon PTY — AgentChatPane's mechanism). Live/dormant
//      handling mirrors the feedback Terminal tab: liveness via
//      sessions/lookup-by-agent, a dormant session shows the wake
//      affordance → POST workspace/ensure-pinned-chat, then attaches.
//   2. `htmlDoc` — a pinned HTML document (#587), rendered with the
//      FileViewerPane html-category machinery: host-aware fs/read-file,
//      sandboxed <iframe srcDoc> (allow-scripts, NO allow-same-origin),
//      2s poll guarded by hasSelectionWithin. A vanished file shows a
//      plain missing-doc state — it never breaks the layout.
//   Unknown kinds render an inert placeholder and survive saves (§6.3).
//
// Layout semantics (§6.3, all pure logic in dashboard-layout.ts):
//   - CANONICAL, last-write-wins: every change (drop, reorder, resize
//     end, close) saves through the trailing-300ms coalesced saver.
//   - APPLY-ON-OPEN: the blob is adopted once on mount; layout-changed
//     revisions beyond what we rendered/wrote only show the stale pill
//     (with an explicit Apply) — never a live rearrange. The echo
//     guard: our own save's revision ratchets `known` so its event/
//     refetch echo can't flag our own view.
//   - Fresh 'Main' (daemon seed, no `columns` key) renders the PoC's
//     canonical pane client-side; the first real edit writes it.
//
// Permissions (§6.3b): owners AND admins edit; a resolved viewer-mode
// window (presence S5, window-mode.ts — the same signal TerminalPane
// uses to suppress input/resize) is READ-ONLY: no drag, no resize
// handles, no close, member clicks focus-only. The daemon's
// owner-or-admin gate on save-layout backstops all of it.

import React, { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { TerminalPane } from '@/kessel-term/TerminalPane'
import { daemonCliGet, daemonCliPost } from '@/lib/daemon-cli'
import { useProjectGroupsStore } from '@/stores/project-groups'
import { useToastStore } from '@/stores/toast'
import { useWindowModeStore } from '@/stores/window-mode'
import { hasSelectionWithin } from '@/components/FileViewerPane/FileViewerPane'
import { FILE_POLL_INTERVAL } from '@shared/constants'
import {
  adoptLayout,
  adoptRevision,
  computeDropTarget,
  computeInsertBoundary,
  createLayoutSaver,
  dropMember,
  findTerminalColumn,
  initialFreshness,
  insertColumnAt,
  isHtmlDocPane,
  isTerminalPane,
  moveColumn,
  observeOwnSave,
  observeRevision,
  removeColumnAt,
  resizePair,
  type DashboardColumn,
  type DropTarget,
  type FreshnessState,
  type LayoutSaver,
} from './dashboard-layout'
import { registerMemberDropHandler, useDashboardDndStore } from './dashboard-dnd'
import { resolveEscFocusPane } from './project-tabs'
import {
  saveDashboardLayout,
  type ProjectGroupDashboard,
  type ProjectGroupMemberInfo,
  type ProjectGroupShow,
} from './projects-api'

// Sidebar's drag threshold (handleProjectMouseDown): 3px x / 5px y.
const DRAG_THRESHOLD_X = 3
const DRAG_THRESHOLD_Y = 5

// ── Drop-marker geometry (view-side companion to computeDropTarget) ───────

type Marker =
  | { kind: 'insert'; left: number }
  | { kind: 'replace'; left: number; width: number }
  | { kind: 'empty' }

function columnRects(grid: HTMLElement): Array<{ left: number; right: number }> {
  return Array.from(grid.querySelectorAll<HTMLElement>('[data-dash-col]')).map((el) => {
    const r = el.getBoundingClientRect()
    return { left: r.left, right: r.right }
  })
}

function markerForTarget(
  grid: HTMLElement,
  target: DropTarget,
  rects: Array<{ left: number; right: number }>,
): Marker {
  if (rects.length === 0) return { kind: 'empty' }
  const gridLeft = grid.getBoundingClientRect().left
  if (target.type === 'replace') {
    const r = rects[target.index]
    return { kind: 'replace', left: r.left - gridLeft, width: r.right - r.left }
  }
  const x =
    target.index === 0
      ? rects[0].left
      : target.index >= rects.length
        ? rects[rects.length - 1].right
        : (rects[target.index - 1].right + rects[target.index].left) / 2
  return { kind: 'insert', left: x - gridLeft }
}

// ── Pane chrome (title + close + drag-to-reorder header) ─────────────────

function PaneChrome({
  title,
  hint,
  readOnly,
  focused,
  onClose,
  onHeaderMouseDown,
  children,
}: {
  title: string
  hint?: string
  readOnly: boolean
  focused: boolean
  onClose: () => void
  onHeaderMouseDown?: (e: React.MouseEvent) => void
  children: React.ReactNode
}): React.JSX.Element {
  return (
    <div
      className={`flex flex-col h-full min-w-0 min-h-0 border bg-[var(--color-bg-surface)] transition-colors ${
        focused ? 'border-[var(--color-accent)]' : 'border-[var(--color-border)]'
      }`}
    >
      <div
        className={`flex items-center gap-2 px-2 py-1 border-b border-[var(--color-border)] flex-shrink-0 select-none ${
          readOnly ? '' : 'cursor-grab'
        }`}
        onMouseDown={readOnly ? undefined : onHeaderMouseDown}
        title={readOnly ? undefined : 'Drag to reorder'}
      >
        <span className="text-[10px] font-semibold text-[var(--color-text-secondary)] truncate">
          {title}
        </span>
        {hint && (
          <span className="text-[9px] text-[var(--color-text-muted)] truncate opacity-70">
            {hint}
          </span>
        )}
        <span className="flex-1" />
        {!readOnly && (
          <button
            type="button"
            onClick={onClose}
            onMouseDown={(e) => e.stopPropagation()}
            className="flex items-center justify-center w-4 h-4 flex-shrink-0 text-[var(--color-text-muted)] hover:text-red-400 transition-colors cursor-pointer"
            title="Close pane"
          >
            <svg width="8" height="8" viewBox="0 0 12 12" fill="none" stroke="currentColor" strokeWidth="1.5">
              <line x1="2" y1="2" x2="10" y2="10" />
              <line x1="10" y1="2" x2="2" y2="10" />
            </svg>
          </button>
        )}
      </div>
      <div className="flex-1 min-h-0 min-w-0">{children}</div>
    </div>
  )
}

// ── Terminal pane (canonical session — the feedback TerminalTab idiom) ────

type TermPhase =
  | { kind: 'checking' }
  | { kind: 'live'; sessionId?: string }
  | { kind: 'dormant' }
  | { kind: 'waking' }
  | { kind: 'error'; message: string }

function DashboardTerminalPane({
  workspaceId,
  member,
  dashboardId,
}: {
  workspaceId: string
  /** null when the workspace is no longer a project member / was
   *  unregistered — the pane degrades to a placeholder, never breaks. */
  member: ProjectGroupMemberInfo | null
  dashboardId: string
}): React.JSX.Element {
  const projectPath = member?.path ?? null
  const [phase, setPhase] = useState<TermPhase>({ kind: 'checking' })

  // Canonical sessions run under the workspace's bare project-id key
  // (the AgentChatPane attach key) — liveness via lookup-by-agent, the
  // same resolution the feedback Terminal tab uses.
  const resolve = useCallback(async (): Promise<TermPhase> => {
    try {
      const lookup = await daemonCliGet<{ sessionAlive: boolean; sessionId: string | null }>(
        'sessions/lookup-by-agent',
        { agent: workspaceId },
      )
      if (lookup.sessionAlive) return { kind: 'live', sessionId: lookup.sessionId ?? undefined }
      return { kind: 'dormant' }
    } catch (e) {
      return { kind: 'error', message: e instanceof Error ? e.message : String(e) }
    }
  }, [workspaceId])

  useEffect(() => {
    if (!projectPath) return
    let cancelled = false
    setPhase({ kind: 'checking' })
    void resolve().then((p) => {
      if (!cancelled) setPhase(p)
    })
    return () => {
      cancelled = true
    }
  }, [projectPath, resolve])

  // Wake a dormant canonical session — the daemon-owned find-or-spawn
  // AgentChatPane and the feedback tab ride (ensure-pinned-chat), then
  // attach under the same key.
  const wake = useCallback(async (): Promise<void> => {
    if (!projectPath) return
    setPhase({ kind: 'waking' })
    try {
      await daemonCliPost('workspace/ensure-pinned-chat', { project: projectPath })
      setPhase({ kind: 'live' })
    } catch (e) {
      setPhase({ kind: 'error', message: e instanceof Error ? e.message : String(e) })
    }
  }, [projectPath])

  if (!member || !projectPath) {
    return (
      <PaneBody>
        <p className="text-[11px] text-[var(--color-text-muted)] max-w-[36ch]">
          This workspace is no longer available — it left the project or was
          removed from the server.
        </p>
      </PaneBody>
    )
  }

  if (phase.kind === 'checking') return <PaneBody>Checking session…</PaneBody>
  if (phase.kind === 'waking') return <PaneBody>Waking session…</PaneBody>
  if (phase.kind === 'error') {
    return (
      <PaneBody>
        <p className="text-[11px] text-red-400 max-w-[36ch]">{phase.message}</p>
        <PaneActionButton onClick={() => void resolve().then(setPhase)}>Retry</PaneActionButton>
      </PaneBody>
    )
  }
  if (phase.kind === 'dormant') {
    return (
      <PaneBody>
        <p className="text-xs font-semibold text-[var(--color-text-primary)]">Session is dormant</p>
        <p className="text-[11px] text-[var(--color-text-muted)] max-w-[36ch]">
          This agent&apos;s canonical session isn&apos;t running right now.
        </p>
        <PaneActionButton accent onClick={() => void wake()}>
          Wake session
        </PaneActionButton>
      </PaneBody>
    )
  }

  // live — attach in place: attachAgentName keys the idempotent
  // v2/spawn to the EXISTING daemon PTY (reused:true) — never a
  // duplicate.
  return (
    <TerminalPane
      terminalId={`proj-dash:${dashboardId}:${workspaceId}`}
      cwd={projectPath}
      attachAgentName={workspaceId}
      sessionId={phase.sessionId}
    />
  )
}

function PaneBody({ children }: { children: React.ReactNode }): React.JSX.Element {
  return (
    <div className="h-full flex flex-col items-center justify-center gap-2 px-4 text-center text-[11px] text-[var(--color-text-muted)]">
      {children}
    </div>
  )
}

function PaneActionButton({
  onClick,
  accent,
  children,
}: {
  onClick: () => void
  accent?: boolean
  children: React.ReactNode
}): React.JSX.Element {
  return (
    <button
      type="button"
      onClick={onClick}
      className={`px-3 py-1.5 text-[11px] font-medium transition-colors cursor-pointer ${
        accent
          ? 'bg-[var(--color-accent)]/15 text-[var(--color-text-primary)] hover:bg-[var(--color-accent)]/25'
          : 'bg-white/[0.06] text-[var(--color-text-primary)] hover:bg-white/[0.1]'
      }`}
    >
      {children}
    </button>
  )
}

// ── htmlDoc pane (#587 machinery: srcDoc sandbox + guarded poll) ─────────

type DocPhase =
  | { kind: 'loading' }
  | { kind: 'ready'; content: string }
  | { kind: 'missing' }

function HtmlDocPane({ filePath }: { filePath: string }): React.JSX.Element {
  const rootRef = useRef<HTMLDivElement>(null)
  const [phase, setPhase] = useState<DocPhase>({ kind: 'loading' })
  const fileName = filePath.split('/').pop() || filePath

  // Initial host-aware read + the FileViewerPane 2s poll, guarded by
  // hasSelectionWithin so a copy-in-progress never collapses. A read
  // failure = missing-doc state (§6.3: it never breaks the layout);
  // the poll keeps trying, so a re-created file comes back live.
  useEffect(() => {
    let cancelled = false
    const read = async (): Promise<void> => {
      if (hasSelectionWithin(rootRef.current)) return
      try {
        const result = await daemonCliGet<{ content: string }>('fs/read-file', { path: filePath })
        if (cancelled) return
        setPhase((prev) =>
          prev.kind === 'ready' && prev.content === result.content
            ? prev
            : { kind: 'ready', content: result.content },
        )
      } catch {
        if (!cancelled) setPhase((prev) => (prev.kind === 'ready' ? prev : { kind: 'missing' }))
      }
    }
    void read()
    const interval = setInterval(() => void read(), FILE_POLL_INTERVAL)
    return () => {
      cancelled = true
      clearInterval(interval)
    }
  }, [filePath])

  if (phase.kind === 'loading') return <PaneBody>Loading document…</PaneBody>
  if (phase.kind === 'missing') {
    return (
      <PaneBody>
        <p className="text-xs font-semibold text-[var(--color-text-primary)]">Document unavailable</p>
        <p className="text-[11px] text-[var(--color-text-muted)] max-w-[40ch] font-mono break-all">
          {filePath}
        </p>
      </PaneBody>
    )
  }

  // Sandboxed <iframe srcDoc> — NEVER dangerouslySetInnerHTML. With
  // `allow-scripts` and crucially WITHOUT `allow-same-origin`, the
  // document's scripts run (interactive dashboards) but can't reach
  // the K2 app, its storage, or the filesystem. White background so
  // light HTML docs don't bleed dark chrome (FileViewerPane parity).
  return (
    <div ref={rootRef} className="h-full overflow-hidden bg-white">
      <iframe title={fileName} srcDoc={phase.content} sandbox="allow-scripts" className="w-full h-full border-0 bg-white" />
    </div>
  )
}

// ── The dashboard ─────────────────────────────────────────────────────────

export default function ProjectDashboard({
  show,
  dashboard,
}: {
  show: ProjectGroupShow
  dashboard: ProjectGroupDashboard
}): React.JSX.Element {
  // Presence-arc read-only signal: a RESOLVED viewer-mode window
  // (window-mode.ts, the same store TerminalPane consults to suppress
  // input/resize). Owners/admins resolve claimer; unresolved never
  // restricts (the daemon's owner-or-admin save gate backstops).
  const readOnly = useWindowModeStore((s) => s.resolved && s.mode === 'viewer')
  const readOnlyRef = useRef(readOnly)
  readOnlyRef.current = readOnly

  // ── Canonical columns (adopted ONCE per mount — apply-on-open) ─────────
  const [columns, setColumns] = useState<DashboardColumn[]>(() =>
    adoptLayout(dashboard.layoutJson, show.pocWorkspaceId),
  )
  const columnsRef = useRef(columns)

  const [freshness, setFreshness] = useState<FreshnessState>(() =>
    initialFreshness(dashboard.revision),
  )
  const freshnessRef = useRef(freshness)
  freshnessRef.current = freshness

  // Transient focus flash (member click / drop focuses a pane).
  const [focusIndex, setFocusIndex] = useState<number | null>(null)
  const focusTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null)
  const flashFocus = useCallback((index: number): void => {
    setFocusIndex(index)
    if (focusTimerRef.current !== null) clearTimeout(focusTimerRef.current)
    focusTimerRef.current = setTimeout(() => setFocusIndex(null), 1400)
  }, [])
  useEffect(
    () => () => {
      if (focusTimerRef.current !== null) clearTimeout(focusTimerRef.current)
    },
    [],
  )

  // ── Coalesced canonical saver (every change saves — §6.3a) ─────────────
  const saverRef = useRef<LayoutSaver | null>(null)
  useEffect(() => {
    const saver = createLayoutSaver(
      (layoutJson) =>
        saveDashboardLayout(show.id, dashboard.id, layoutJson).then((d) => ({
          revision: d.revision,
        })),
      {
        onSaved: (revision) => {
          // Echo guard: ratchet `known` so this save's layout-changed
          // event / show refetch can't mark our own view stale.
          setFreshness((s) => observeOwnSave(s, revision))
        },
        onError: (err) => {
          useToastStore
            .getState()
            .addToast(
              `Dashboard layout save failed: ${err instanceof Error ? err.message : String(err)}`,
              'error',
            )
        },
      },
    )
    saverRef.current = saver
    return () => {
      // Unmount flush: a pending change still saves (best-effort).
      saver.dispose()
      saverRef.current = null
    }
    // Mount-stable: the component is keyed by dashboard.id upstream.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [])

  /** Single mutation gate: ref-sync + render + coalesced save. */
  const applyColumns = useCallback((next: DashboardColumn[], save: boolean): void => {
    columnsRef.current = next
    setColumns(next)
    if (save && !readOnlyRef.current) saverRef.current?.schedule(next)
  }, [])

  // ── Staleness (apply-on-open; NEVER live-rearrange — §6.3a) ────────────
  // The refetched `show` carries the dashboard's latest revision (the
  // store bumps on project-group:layout-changed). Beyond what we've
  // rendered/written it only sets the stale pill.
  useEffect(() => {
    setFreshness((s) => observeRevision(s, dashboard.revision))
  }, [dashboard.revision])

  const applyLatest = useCallback((): void => {
    columnsRef.current = adoptLayout(dashboard.layoutJson, show.pocWorkspaceId)
    setColumns(columnsRef.current)
    setFreshness((s) => adoptRevision(s, dashboard.revision))
  }, [dashboard.layoutJson, dashboard.revision, show.pocWorkspaceId])

  // ── §6.7.4 — last-used pane tracking + Esc-to-pane ─────────────────────
  // Every open/focus path notes the pane on the store (keyed by
  // dashboard id, session-only); Esc focuses it — falling back to the
  // dashboard's first terminal pane, no-op when there is none. The
  // actual keyboard handoff goes to the pane's kessel shadow textarea
  // (where TerminalPane routes keystrokes).
  const notePaneFocus = useCallback(
    (workspaceId: string): void => {
      useProjectGroupsStore.getState().notePaneFocus(dashboard.id, workspaceId)
    },
    [dashboard.id],
  )

  const focusPaneInput = useCallback((workspaceId: string): void => {
    const col = document.querySelector<HTMLElement>(`[data-dash-pane-ws="${workspaceId}"]`)
    col?.querySelector<HTMLTextAreaElement>('textarea')?.focus()
  }, [])

  useEffect(() => {
    const onKey = (e: KeyboardEvent): void => {
      if (e.key !== 'Escape') return
      // Inputs keep Esc local via stopPropagation (composer, renames,
      // create form); belt-and-braces for the rest: never steal Esc
      // from an editable target — crucially including the pane's own
      // kessel shadow textarea, whose Esc belongs to the TUI.
      const from = e.target as HTMLElement | null
      if (
        from &&
        (from.tagName === 'INPUT' ||
          from.tagName === 'TEXTAREA' ||
          from.tagName === 'SELECT' ||
          from.isContentEditable)
      ) {
        return
      }
      const last =
        useProjectGroupsStore.getState().lastFocusedPaneByDashboard[dashboard.id] ?? null
      const target = resolveEscFocusPane(columnsRef.current, last)
      if (target === null) return
      e.preventDefault()
      const index = findTerminalColumn(columnsRef.current, target)
      if (index >= 0) flashFocus(index)
      notePaneFocus(target)
      focusPaneInput(target)
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [dashboard.id, flashFocus, notePaneFocus, focusPaneInput])

  // ── Member-click pane requests (nav drawer → open/focus — §6.1) ────────
  const paneRequest = useProjectGroupsStore((s) => s.paneRequest)
  useEffect(() => {
    if (!paneRequest) return
    useProjectGroupsStore.getState().clearPaneRequest()
    const existing = findTerminalColumn(columnsRef.current, paneRequest.workspaceId)
    if (existing >= 0) {
      flashFocus(existing)
      notePaneFocus(paneRequest.workspaceId)
      return
    }
    if (readOnlyRef.current) {
      useToastStore
        .getState()
        .addToast('View-only — an owner or admin can add panes to this dashboard.', 'info')
      return
    }
    const next = insertColumnAt(columnsRef.current, columnsRef.current.length, {
      kind: 'terminal',
      workspaceId: paneRequest.workspaceId,
    })
    applyColumns(next, true)
    flashFocus(next.length - 1)
    notePaneFocus(paneRequest.workspaceId)
  }, [paneRequest, applyColumns, flashFocus, notePaneFocus])

  // ── DnD: markers + drops ───────────────────────────────────────────────
  const gridRef = useRef<HTMLDivElement>(null)
  const [marker, setMarker] = useState<Marker | null>(null)
  // Index of the column being header-dragged (dims it while dragging).
  const [paneDragIndex, setPaneDragIndex] = useState<number | null>(null)

  /** Full §6.2 target (insertion boundaries + replace zones) at a
   *  viewport point, or null when outside the grid. */
  const targetAtPoint = useCallback((x: number, y: number): DropTarget | null => {
    const grid = gridRef.current
    if (!grid) return null
    const rect = grid.getBoundingClientRect()
    if (x < rect.left || x > rect.right || y < rect.top || y > rect.bottom) return null
    return computeDropTarget(x, columnRects(grid))
  }, [])

  // Member drag from the nav drawer (dashboard-dnd store) → markers.
  const memberDrag = useDashboardDndStore((s) => s.memberDrag)
  useEffect(() => {
    if (!memberDrag || readOnly) {
      setMarker(null)
      return
    }
    const grid = gridRef.current
    const target = targetAtPoint(memberDrag.x, memberDrag.y)
    if (!grid || !target) {
      setMarker(null)
      return
    }
    setMarker(markerForTarget(grid, target, columnRects(grid)))
  }, [memberDrag, readOnly, targetAtPoint])

  // Drop handler for member drags (mouseup lands here via the
  // registry; unregistered when this dashboard unmounts).
  useEffect(() => {
    return registerMemberDropHandler((workspaceId, x, y) => {
      setMarker(null)
      if (readOnlyRef.current) return
      const target = targetAtPoint(x, y)
      if (!target) return
      const result = dropMember(columnsRef.current, workspaceId, target)
      if (result.changed) applyColumns(result.columns, true)
      if (result.focusIndex >= 0) {
        flashFocus(result.focusIndex)
        notePaneFocus(workspaceId)
      }
    })
  }, [targetAtPoint, applyColumns, flashFocus, notePaneFocus])

  // Header drag → reorder columns (Sidebar threshold + boundary index).
  const startPaneDrag = useCallback(
    (e: React.MouseEvent, fromIndex: number): void => {
      if (readOnlyRef.current || e.button !== 0) return
      if ((e.target as HTMLElement).closest('button')) return
      e.preventDefault()
      const startX = e.clientX
      const startY = e.clientY
      let started = false
      let boundary: number | null = null

      const handleMove = (ev: MouseEvent): void => {
        if (
          !started &&
          (Math.abs(ev.clientX - startX) > DRAG_THRESHOLD_X ||
            Math.abs(ev.clientY - startY) > DRAG_THRESHOLD_Y)
        ) {
          started = true
          setPaneDragIndex(fromIndex)
          document.body.style.cursor = 'grabbing'
          document.body.style.userSelect = 'none'
        }
        if (!started) return
        const grid = gridRef.current
        if (!grid) return
        const rects = columnRects(grid)
        boundary = computeInsertBoundary(ev.clientX, rects)
        setMarker(markerForTarget(grid, { type: 'insert', index: boundary }, rects))
      }

      const handleUp = (): void => {
        document.removeEventListener('mousemove', handleMove)
        document.removeEventListener('mouseup', handleUp)
        document.body.style.cursor = ''
        document.body.style.userSelect = ''
        if (started && boundary !== null) {
          const next = moveColumn(columnsRef.current, fromIndex, boundary)
          if (next !== columnsRef.current) applyColumns(next, true)
        }
        setPaneDragIndex(null)
        setMarker(null)
      }

      document.addEventListener('mousemove', handleMove)
      document.addEventListener('mouseup', handleUp)
    },
    [applyColumns],
  )

  // Boundary resize handle → live width preview, save on release
  // (§6.3a: resize END saves).
  const startResize = useCallback(
    (e: React.MouseEvent, leftIndex: number): void => {
      if (readOnlyRef.current || e.button !== 0) return
      e.preventDefault()
      const startX = e.clientX
      const startColumns = columnsRef.current
      const grid = gridRef.current
      const gridWidth = grid ? grid.getBoundingClientRect().width || 1 : 1
      document.body.style.cursor = 'col-resize'
      document.body.style.userSelect = 'none'

      const handleMove = (ev: MouseEvent): void => {
        const deltaPct = ((ev.clientX - startX) / gridWidth) * 100
        applyColumns(resizePair(startColumns, leftIndex, deltaPct), false)
      }
      const handleUp = (): void => {
        document.removeEventListener('mousemove', handleMove)
        document.removeEventListener('mouseup', handleUp)
        document.body.style.cursor = ''
        document.body.style.userSelect = ''
        // Persist the final widths (coalesced).
        if (!readOnlyRef.current) saverRef.current?.schedule(columnsRef.current)
      }
      document.addEventListener('mousemove', handleMove)
      document.addEventListener('mouseup', handleUp)
    },
    [applyColumns],
  )

  // ── Render ─────────────────────────────────────────────────────────────

  const membersById = useMemo(() => {
    const map = new Map<string, ProjectGroupMemberInfo>()
    for (const m of show.members) map.set(m.workspaceId, m)
    return map
  }, [show.members])

  // Stable React keys: terminal panes key by workspace (unique per
  // dashboard), doc/unknown panes by identity + occurrence, so a
  // reorder MOVES mounted panes (keeping live grid-WS attachments)
  // instead of remounting them.
  const columnKeys = useMemo(() => {
    const seen = new Map<string, number>()
    return columns.map((c) => {
      const base = isTerminalPane(c.pane)
        ? `t:${c.pane.workspaceId}`
        : isHtmlDocPane(c.pane)
          ? `h:${c.pane.workspaceId}:${c.pane.filePath}`
          : `u:${c.pane.kind}`
      const n = seen.get(base) ?? 0
      seen.set(base, n + 1)
      return n === 0 ? base : `${base}#${n}`
    })
  }, [columns])

  const stale = freshness.staleRevision !== null
  const poc = membersById.get(show.pocWorkspaceId ?? '')

  return (
    <div className="flex-1 flex flex-col min-h-0 min-w-0 relative" data-projects-dashboard-panes>
      {/* Stale pill (§6.3a): another client saved a newer layout. It
          NEVER auto-applies — apply-on-open, or explicitly here. */}
      {stale && (
        <div className="absolute top-1.5 right-1.5 z-20 flex items-center gap-2 px-2 py-1 bg-[var(--color-bg-elevated)] border border-amber-500/40 shadow-lg">
          <span className="w-1.5 h-1.5 rounded-full bg-amber-400 flex-shrink-0" />
          <span className="text-[10px] text-[var(--color-text-secondary)]">
            Layout updated elsewhere
          </span>
          <button
            type="button"
            onClick={applyLatest}
            className="text-[10px] font-medium text-[var(--color-accent)] hover:underline cursor-pointer"
          >
            Apply
          </button>
        </div>
      )}

      {columns.length === 0 ? (
        /* gridRef also lives here so a drop on the empty dashboard
           resolves (no [data-dash-col] rects → insert at 0). */
        <div
          ref={gridRef}
          className={`flex-1 flex items-center justify-center border border-dashed text-center px-8 transition-colors ${
            memberDrag && !readOnly
              ? 'border-[var(--color-accent)] bg-[var(--color-accent)]/5'
              : 'border-[var(--color-border)]'
          }`}
        >
          <div>
            <p className="text-sm text-[var(--color-text-secondary)]">No panes</p>
            <p className="text-xs text-[var(--color-text-muted)] mt-1 opacity-70">
              {show.members.length === 0
                ? 'Add a member to this project — its agent becomes the Point of Contact and the first pane.'
                : readOnly
                  ? 'This dashboard is empty.'
                  : `Click or drag a member from the drawer${poc ? ` — start with ${poc.agentName ?? poc.name ?? 'the PoC'}` : ''}.`}
            </p>
          </div>
        </div>
      ) : (
        <div ref={gridRef} className="relative flex-1 flex min-h-0 gap-px">
          {columns.map((col, i) => {
            const pane = col.pane
            const member = isTerminalPane(pane) || isHtmlDocPane(pane)
              ? membersById.get(pane.workspaceId) ?? null
              : null
            const title = isTerminalPane(pane)
              ? member?.agentName ?? member?.name ?? pane.workspaceId.slice(0, 8)
              : isHtmlDocPane(pane)
                ? pane.filePath.split('/').pop() || pane.filePath
                : `Unknown pane (${pane.kind})`
            const hint = isHtmlDocPane(pane) ? member?.name ?? undefined : undefined
            return (
              <React.Fragment key={columnKeys[i]}>
                <div
                  data-dash-col
                  data-dash-pane-ws={isTerminalPane(pane) ? pane.workspaceId : undefined}
                  className="flex flex-col min-w-0 min-h-0"
                  style={{
                    flexGrow: col.widthPct,
                    flexShrink: 1,
                    flexBasis: 0,
                    opacity: paneDragIndex === i ? 0.4 : 1,
                  }}
                  // §6.7.4 — any click inside a terminal pane makes it
                  // the dashboard's last-used pane (capture: the
                  // header/body handlers must not swallow it).
                  onMouseDownCapture={
                    isTerminalPane(pane) ? () => notePaneFocus(pane.workspaceId) : undefined
                  }
                >
                  <PaneChrome
                    title={title}
                    hint={hint}
                    readOnly={readOnly}
                    focused={focusIndex === i}
                    onClose={() => applyColumns(removeColumnAt(columnsRef.current, i), true)}
                    onHeaderMouseDown={(e) => startPaneDrag(e, i)}
                  >
                    {isTerminalPane(pane) ? (
                      <DashboardTerminalPane
                        workspaceId={pane.workspaceId}
                        member={member}
                        dashboardId={dashboard.id}
                      />
                    ) : isHtmlDocPane(pane) ? (
                      <HtmlDocPane filePath={pane.filePath} />
                    ) : (
                      /* §6.3 forward-compat: inert placeholder; the
                         pane object round-trips untouched on save. */
                      <PaneBody>
                        <p className="text-[11px] text-[var(--color-text-muted)] max-w-[36ch]">
                          This pane was made by a newer K2 and renders after an update.
                        </p>
                      </PaneBody>
                    )}
                  </PaneChrome>
                </div>
                {i < columns.length - 1 && (
                  <div
                    className={`flex-shrink-0 w-1.5 -mx-0.5 z-10 ${
                      readOnly ? '' : 'cursor-col-resize hover:bg-[var(--color-accent)]/40'
                    } transition-colors`}
                    onMouseDown={readOnly ? undefined : (e) => startResize(e, i)}
                    title={readOnly ? undefined : 'Drag to resize'}
                  />
                )}
              </React.Fragment>
            )
          })}

          {/* Drop-target hints (§6.2): insertion bar / replace ring. */}
          {marker && marker.kind === 'insert' && (
            <div
              className="absolute top-0 bottom-0 w-0.5 bg-[var(--color-accent)] z-20 pointer-events-none"
              style={{ left: Math.max(0, marker.left - 1) }}
            />
          )}
          {marker && marker.kind === 'replace' && (
            <div
              className="absolute top-0 bottom-0 border-2 border-[var(--color-accent)] bg-[var(--color-accent)]/10 z-20 pointer-events-none"
              style={{ left: marker.left, width: marker.width }}
            />
          )}
        </div>
      )}
    </div>
  )
}
