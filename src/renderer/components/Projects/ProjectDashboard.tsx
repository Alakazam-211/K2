// Projects V1 P5 → §6.8 (prd-projects-v1) — the Dashboard tab's TILING
// GRID: a VS-Code-style split tree over the canonical layout blob.
//
// Pane kinds (§6.2):
//   1. `terminal` — the member workspace's CANONICAL session, rendered
//      through the kessel attach idiom (`attachAgentName={workspaceId}`
//      keys TerminalPane's idempotent /cli/sessions/v2/spawn to the
//      existing daemon PTY — AgentChatPane's mechanism). Live/dormant
//      handling mirrors the feedback Terminal tab: liveness via
//      sessions/lookup-by-agent, a dormant session shows the wake
//      affordance → activateProject(member) + ensure-pinned-chat, then
//      attaches (PRD §4.3.1; without activate active_reaper reaps ~15s).
//   2. `htmlDoc` — a pinned HTML document (#587), rendered with the
//      FileViewerPane html-category machinery: host-aware fs/read-file,
//      sandboxed <iframe srcDoc> (allow-scripts, NO allow-same-origin),
//      2s poll guarded by hasSelectionWithin. A vanished file shows a
//      plain missing-doc state — it never breaks the layout.
//   Unknown kinds render an inert placeholder and survive saves (§6.3).
//
// Layout mechanics (§6.8, all pure logic in dashboard-layout.ts):
//   - the tree renders FLAT: computeLayoutGeometry turns the split
//     tree into absolute percent rects, panes are positioned divs
//     keyed by pane identity — so ANY restructure (drop-to-split,
//     move, preset re-tile) MOVES mounted panes instead of remounting
//     them, keeping live kessel grid-WS attachments,
//   - drop-to-split: dragging a member (nav drawer), an HTML doc, or
//     an existing pane by its header hit-tests 5 zones per pane
//     (left/right/top/bottom half → split 50/50; center → move/swap/
//     replace) + ~24px container edge bands (full-span insert),
//   - every divider (both axes) drags LIVE with immediate reflow
//     (kessel tolerates live resize — pin-to-size precedent); min pane
//     ~10%; save on release (coalesced),
//   - presets (§6.8.4) re-tile the existing panes in reading order —
//     the tab-row menu reaches the mounted dashboard through the
//     dashboard-dnd preset registry.
//
// Layout semantics (§6.3, unchanged from v1):
//   - CANONICAL, last-write-wins: every change (drop, move, resize
//     end, close, preset) saves through the trailing-300ms coalesced
//     saver; saves always write the v2 blob.
//   - APPLY-ON-OPEN: the blob is adopted once on mount (v1 blobs
//     convert on adopt); layout-changed revisions beyond what we
//     rendered/wrote only show the stale pill (with an explicit
//     Apply) — never a live rearrange. The echo guard: our own save's
//     revision ratchets `known` so its event/refetch echo can't flag
//     our own view.
//   - Fresh 'Main' (daemon seed, no `columns`/`root` key) renders the
//     PoC's canonical pane client-side; the first real edit writes it.
//
// Permissions (§6.3b): owners AND admins edit; a resolved viewer-mode
// window (presence S5, window-mode.ts — the same signal TerminalPane
// uses to suppress input/resize) is READ-ONLY: no drag, no dividers,
// no close, no presets, member clicks focus-only. The daemon's
// owner-or-admin gate on save-layout backstops all of it.

import React, { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { TerminalPane } from '@/kessel-term/TerminalPane'
import { daemonCliGet, daemonCliPost } from '@/lib/daemon-cli'
import { activateProject } from '@/stores/projects'
import { useProjectGroupsStore } from '@/stores/project-groups'
import { useToastStore } from '@/stores/toast'
import { useWindowModeStore, noteViewerInteractionBlocked } from '@/stores/window-mode'
import {
  activateOnLiveSessionAttach,
  resolveAllMemberSessions,
  wakeCanonicalMemberSession,
  type MemberSessionPhase,
} from './wake-member-session'
import {
  FileViewerPane,
  getFileCategory,
  hasSelectionWithin,
} from '@/components/FileViewerPane/FileViewerPane'
import { Surface } from '@/components/ui'
import { FILE_POLL_INTERVAL } from '@shared/constants'
import {
  EDGE_BAND_PX,
  adoptLayout,
  adoptRevision,
  applyDrop,
  computeLayoutGeometry,
  createLayoutSaver,
  findTerminalPaneId,
  initialFreshness,
  insertEdge,
  isHtmlDocPane,
  isTerminalPane,
  paneKey,
  observeOwnSave,
  observeRevision,
  readingOrder,
  removePane,
  resizeDivider,
  resolveDropZone,
  tileIntoPreset,
  type DividerGeom,
  type DragSource,
  type DropZone,
  type FreshnessState,
  type LayoutNode,
  type LayoutSaver,
} from './dashboard-layout'
import {
  registerDashDropHandler,
  registerPaneShortcutHandler,
  registerPresetHandler,
  useDashboardDndStore,
} from './dashboard-dnd'
import {
  paneByNumber,
  paneNumbersById,
  resolveEscFocusPane,
  terminalPaneNumbers,
} from './project-tabs'
import {
  saveDashboardLayout,
  type ProjectGroupDashboard,
  type ProjectGroupMemberInfo,
  type ProjectGroupShow,
} from './projects-api'

// Sidebar's drag threshold (handleProjectMouseDown): 3px x / 5px y.
const DRAG_THRESHOLD_X = 3
const DRAG_THRESHOLD_Y = 5

// Divider hit-area thickness (px), centered on the boundary line.
const DIVIDER_HIT_PX = 7

// ── Pane chrome (title + close + drag-to-move header) ────────────────────

function PaneChrome({
  title,
  hint,
  shortcutNum,
  readOnly,
  focused,
  onClose,
  onHeaderMouseDown,
  children,
}: {
  title: string
  hint?: string
  /** ⌘N badge — reading-order pane number (first 9 panes only). */
  shortcutNum?: number
  readOnly: boolean
  focused: boolean
  onClose: () => void
  onHeaderMouseDown?: (e: React.MouseEvent) => void
  children: React.ReactNode
}): React.JSX.Element {
  return (
    <Surface
      role2="surface"
      bordered={false}
      className={`flex flex-col h-full min-w-0 min-h-0 border transition-colors ${
        focused ? 'border-[var(--color-accent)]' : 'border-[var(--color-border)]'
      }`}
    >
      <div
        className={`flex items-center gap-2 px-2 py-1 border-b border-[var(--color-border)] flex-shrink-0 select-none ${
          readOnly ? '' : 'cursor-grab'
        }`}
        onMouseDown={readOnly ? undefined : onHeaderMouseDown}
        title={readOnly ? undefined : 'Drag to move'}
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
        {/* ⌘N pane-switch badge (the ActiveBar shortcutNum look),
            immediately left of the close button. */}
        {shortcutNum !== undefined && (
          <span
            className="text-[10px] font-mono text-[var(--color-text-muted)] tabular-nums flex-shrink-0"
            title={`⌘${shortcutNum} focuses this pane`}
          >
            <span className="key-symbol">⌘</span>
            {shortcutNum}
          </span>
        )}
        {!readOnly && (
          <button
            type="button"
            onClick={onClose}
            onMouseDown={(e) => e.stopPropagation()}
            className="flex items-center justify-center w-4 h-4 flex-shrink-0 text-[var(--color-text-muted)] hover:text-[var(--color-status-error-soft)] transition-colors cursor-pointer"
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
    </Surface>
  )
}

// ── Terminal pane (canonical session — the feedback TerminalTab idiom) ────

type TermPhase =
  | { kind: 'checking' }
  | MemberSessionPhase
  | { kind: 'waking' }

/** Stable placeholder while parent batch resolve runs (avoid new object each render). */
const PHASE_CHECKING: TermPhase = { kind: 'checking' }
const PHASE_DORMANT: TermPhase = { kind: 'dormant' }

/** One lookup-by-agent (retry path after batch). */
async function resolveMemberSession(workspaceId: string): Promise<TermPhase> {
  const map = await resolveAllMemberSessions([workspaceId], async (agent) => {
    const lookup = await daemonCliGet<{ sessionAlive: boolean; sessionId: string | null }>(
      'sessions/lookup-by-agent',
      { agent },
    )
    return { sessionAlive: lookup.sessionAlive, sessionId: lookup.sessionId }
  })
  return map[workspaceId] ?? PHASE_DORMANT
}

/** Terminal workspace ids currently in the layout tree (deduped). */
function terminalWorkspaceIdsInLayout(root: LayoutNode | null): string[] {
  const ids: string[] = []
  for (const pane of readingOrder(root)) {
    if (isTerminalPane(pane) && !ids.includes(pane.workspaceId)) ids.push(pane.workspaceId)
  }
  return ids
}

const lookupByAgentForDashboard: (
  workspaceId: string,
) => Promise<{ sessionAlive: boolean; sessionId: string | null }> = async (agent) => {
  const lookup = await daemonCliGet<{ sessionAlive: boolean; sessionId: string | null }>(
    'sessions/lookup-by-agent',
    { agent },
  )
  return { sessionAlive: lookup.sessionAlive, sessionId: lookup.sessionId }
}

function DashboardTerminalPane({
  workspaceId,
  member,
  dashboardId,
  /** Parent-batched liveness (Promise.all). When still checking, parent
   *  has not committed the batch yet — show a brief placeholder. */
  initialPhase,
}: {
  workspaceId: string
  /** null when the workspace is no longer a project member / was
   *  unregistered — the pane degrades to a placeholder, never breaks. */
  member: ProjectGroupMemberInfo | null
  dashboardId: string
  initialPhase: TermPhase
}): React.JSX.Element {
  const projectPath = member?.path ?? null
  const [phase, setPhase] = useState<TermPhase>(initialPhase)

  // Track parent batch updates (e.g. layout gained a new pane → re-batch).
  // Compare by kind+sessionId so stable PHASE_CHECKING / same live id
  // does not thrash local wake/error state.
  useEffect(() => {
    setPhase((prev) => {
      if (prev.kind === 'waking') return prev // local wake in flight
      if (
        prev.kind === initialPhase.kind &&
        (prev.kind !== 'live' ||
          initialPhase.kind !== 'live' ||
          prev.sessionId === initialPhase.sessionId)
      ) {
        return prev
      }
      return initialPhase
    })
  }, [initialPhase])

  // Live attach (wake success or passive attach of an already-alive PTY):
  // client is watching ⇒ Active. activateProject is deduped / no-op when
  // already active for this member id.
  useEffect(() => {
    if (phase.kind !== 'live') return
    activateOnLiveSessionAttach(workspaceId, activateProject)
  }, [phase.kind, workspaceId])

  // Wake a dormant canonical session — activate (PRD §4.3.1) then the
  // daemon-owned find-or-spawn AgentChatPane and the feedback tab ride
  // (ensure-pinned-chat), then attach under the same key.
  // Without activate, active_reaper reaps the chat PTY after ~15s.
  const wake = useCallback(async (): Promise<void> => {
    if (!projectPath) return
    setPhase({ kind: 'waking' })
    try {
      await wakeCanonicalMemberSession(workspaceId, projectPath, {
        activateProject,
        ensurePinnedChat: (project) =>
          daemonCliPost('workspace/ensure-pinned-chat', { project }),
      })
      setPhase({ kind: 'live' })
    } catch (e) {
      setPhase({ kind: 'error', message: e instanceof Error ? e.message : String(e) })
    }
  }, [projectPath, workspaceId])

  const retry = useCallback(async (): Promise<void> => {
    setPhase({ kind: 'checking' })
    const p = await resolveMemberSession(workspaceId)
    setPhase(p)
  }, [workspaceId])

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
        <p className="text-[11px] text-[var(--color-status-error-soft)] max-w-[36ch]">{phase.message}</p>
        <PaneActionButton onClick={() => void retry()}>Retry</PaneActionButton>
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
  // duplicate. Parent batched lookups so N live panes mount in one
  // commit → concurrent attach (not one-by-one).
  return (
    <TerminalPane
      terminalId={`proj-dash:${dashboardId}:${workspaceId}`}
      cwd={projectPath}
      attachAgentName={workspaceId}
      sessionId={phase.sessionId}
      syncSizeOnShow
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
          : 'bg-white/[0.06] text-[var(--color-text-primary)] hover:bg-[var(--color-wash-2)]'
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

function HtmlDocPane({
  filePath,
  workspaceId,
}: {
  filePath: string
  workspaceId: string
}): React.JSX.Element {
  if (getFileCategory(filePath) !== 'html') {
    const id = `dash-fv:${workspaceId}:${filePath}`
    return (
      <div className="h-full min-h-0 overflow-hidden">
        <FileViewerPane filePath={filePath} paneId={id} tabId={id} />
      </div>
    )
  }
  return <HtmlIframePane filePath={filePath} />
}

function HtmlIframePane({ filePath }: { filePath: string }): React.JSX.Element {
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

  // ── Canonical tree (adopted ONCE per mount — apply-on-open; v1
  //    blobs convert here, §6.8.1) ─────────────────────────────────────────
  const [root, setRoot] = useState<LayoutNode | null>(() =>
    adoptLayout(dashboard.layoutJson, show.pocWorkspaceId),
  )
  const rootRef = useRef(root)

  const [freshness, setFreshness] = useState<FreshnessState>(() =>
    initialFreshness(dashboard.revision),
  )
  const freshnessRef = useRef(freshness)
  freshnessRef.current = freshness

  // Transient focus flash (member click / drop focuses a pane).
  const [focusPaneId, setFocusPaneId] = useState<string | null>(null)
  const focusTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null)
  const flashFocus = useCallback((paneId: string): void => {
    setFocusPaneId(paneId)
    if (focusTimerRef.current !== null) clearTimeout(focusTimerRef.current)
    focusTimerRef.current = setTimeout(() => setFocusPaneId(null), 1400)
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
  const applyRoot = useCallback((next: LayoutNode | null, save: boolean): void => {
    rootRef.current = next
    setRoot(next)
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
    rootRef.current = adoptLayout(dashboard.layoutJson, show.pocWorkspaceId)
    setRoot(rootRef.current)
    setFreshness((s) => adoptRevision(s, dashboard.revision))
  }, [dashboard.layoutJson, dashboard.revision, show.pocWorkspaceId])

  // ── §6.7.4 — last-used pane tracking + Esc-to-pane ─────────────────────
  // Every open/focus path notes the pane on the store (keyed by
  // dashboard id, session-only); Esc focuses it — falling back to the
  // dashboard's first terminal pane (reading order), no-op when there
  // is none. The actual keyboard handoff goes to the pane's kessel
  // shadow textarea (where TerminalPane routes keystrokes).
  const notePaneFocus = useCallback(
    (workspaceId: string): void => {
      useProjectGroupsStore.getState().notePaneFocus(dashboard.id, workspaceId)
    },
    [dashboard.id],
  )

  const focusPaneInput = useCallback((workspaceId: string): void => {
    const el = document.querySelector<HTMLElement>(`[data-dash-pane-ws="${workspaceId}"]`)
    el?.querySelector<HTMLTextAreaElement>('textarea')?.focus()
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
      const target = resolveEscFocusPane(rootRef.current, last)
      if (target === null) return
      e.preventDefault()
      const paneId = findTerminalPaneId(rootRef.current, target)
      if (paneId !== null) flashFocus(paneId)
      notePaneFocus(target)
      focusPaneInput(target)
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [dashboard.id, flashFocus, notePaneFocus, focusPaneInput])

  // ── Member-click pane requests (nav drawer → open/focus — §6.1) ────────
  // Already present → focus. Fresh → the §6.8 sensible default: append
  // as a RIGHT-MOST full-height region (insertEdge 'right').
  const paneRequest = useProjectGroupsStore((s) => s.paneRequest)
  useEffect(() => {
    if (!paneRequest) return
    useProjectGroupsStore.getState().clearPaneRequest()
    if (paneRequest.filePath) {
      const spec = {
        kind: 'htmlDoc' as const,
        workspaceId: paneRequest.workspaceId,
        filePath: paneRequest.filePath,
      }
      const existing = paneKey(spec)
      const already = readingOrder(rootRef.current).some(
        (p) => isHtmlDocPane(p) && p.workspaceId === spec.workspaceId && p.filePath === spec.filePath,
      )
      if (already) {
        flashFocus(existing)
        return
      }
      if (noteViewerInteractionBlocked()) return
      applyRoot(insertEdge(rootRef.current, 'right', spec), true)
      flashFocus(existing)
      return
    }
    const existing = findTerminalPaneId(rootRef.current, paneRequest.workspaceId)
    if (existing !== null) {
      flashFocus(existing)
      notePaneFocus(paneRequest.workspaceId)
      return
    }
    if (noteViewerInteractionBlocked()) {
      return
    }
    const next = insertEdge(rootRef.current, 'right', {
      kind: 'terminal',
      workspaceId: paneRequest.workspaceId,
    })
    applyRoot(next, true)
    flashFocus(`t:${paneRequest.workspaceId}`)
    notePaneFocus(paneRequest.workspaceId)
  }, [paneRequest, applyRoot, flashFocus, notePaneFocus])

  // ── Presets (§6.8.4 — the tab-row menu re-tiles the open tree) ─────────
  useEffect(() => {
    return registerPresetHandler((shape) => {
      if (readOnlyRef.current) return
      const panes = readingOrder(rootRef.current)
      if (panes.length === 0) return
      applyRoot(tileIntoPreset(shape, panes), true)
    })
  }, [applyRoot])

  // ── ⌘1…⌘9 pane switching (the page-level capture keydown lands
  //    here via the dashboard-dnd registry) — the Esc-to-pane focus
  //    path: terminal panes flash + take keyboard focus; htmlDoc/
  //    unknown panes just flash (every pane is already on screen in
  //    the tiled grid). Focus-only, so viewers get it too. ───────────────
  useEffect(() => {
    return registerPaneShortcutHandler((num) => {
      const entry = paneByNumber(rootRef.current, num)
      if (entry === null) return
      flashFocus(entry.paneId)
      if (isTerminalPane(entry.pane)) {
        notePaneFocus(entry.pane.workspaceId)
        focusPaneInput(entry.pane.workspaceId)
      }
    })
  }, [flashFocus, notePaneFocus, focusPaneInput])

  // Publish the current tree's terminal pane numbers so the member
  // drawer/rail rows can badge them; {} on unmount (tab switch /
  // project switch — the next dashboard republishes its own).
  useEffect(() => {
    useProjectGroupsStore.getState().setDashPaneNumbers(terminalPaneNumbers(root))
  }, [root])
  useEffect(
    () => () => {
      useProjectGroupsStore.getState().setDashPaneNumbers({})
    },
    [],
  )

  // ── DnD: 5-zone hit-testing + drops (§6.8.2) ───────────────────────────
  const gridRef = useRef<HTMLDivElement>(null)
  const [dropZone, setDropZone] = useState<DropZone | null>(null)
  // paneId being header-dragged (dims it while dragging).
  const [dragPaneId, setDragPaneId] = useState<string | null>(null)
  // Divider drag in progress (suppresses pane pointer events, below).
  const [dividerDragging, setDividerDragging] = useState(false)

  /** Hit-test a viewport point against the container + live pane
   *  rects; null outside (or in a divider gap). */
  const zoneAtPoint = useCallback((x: number, y: number): DropZone | null => {
    const grid = gridRef.current
    if (!grid) return null
    const rect = grid.getBoundingClientRect()
    const panes = Array.from(grid.querySelectorAll<HTMLElement>('[data-pane-id]')).map((el) => {
      const r = el.getBoundingClientRect()
      return {
        paneId: el.dataset.paneId as string,
        left: r.left,
        top: r.top,
        right: r.right,
        bottom: r.bottom,
      }
    })
    return resolveDropZone(
      x,
      y,
      { left: rect.left, top: rect.top, right: rect.right, bottom: rect.bottom },
      panes,
    )
  }, [])

  /** Shared drop application (external drags + pane-header moves). */
  const performDrop = useCallback(
    (source: DragSource, x: number, y: number): void => {
      if (noteViewerInteractionBlocked()) return
      const zone = zoneAtPoint(x, y)
      if (!zone) return
      const result = applyDrop(rootRef.current, source, zone)
      if (result.changed) applyRoot(result.root, true)
      if (result.focusPaneId !== null) {
        flashFocus(result.focusPaneId)
        if (source.type === 'member') notePaneFocus(source.workspaceId)
      }
    },
    [zoneAtPoint, applyRoot, flashFocus, notePaneFocus],
  )

  // External drag (member / html-doc from the dashboard-dnd store) →
  // live zone highlight.
  const externalDrag = useDashboardDndStore((s) => s.drag)
  useEffect(() => {
    if (!externalDrag || readOnly) {
      setDropZone(null)
      return
    }
    setDropZone(zoneAtPoint(externalDrag.x, externalDrag.y))
  }, [externalDrag, readOnly, zoneAtPoint])

  // Drop handler for external drags (mouseup lands here via the
  // registry; unregistered when this dashboard unmounts).
  useEffect(() => {
    return registerDashDropHandler((payload, x, y) => {
      setDropZone(null)
      performDrop(payload, x, y)
    })
  }, [performDrop])

  // Header drag → MOVE the pane (threshold + 5-zone hit-test; §6.8.2:
  // an existing pane always moves, never duplicates).
  const startPaneDrag = useCallback(
    (e: React.MouseEvent, paneId: string): void => {
      if (e.button !== 0) return
      if (noteViewerInteractionBlocked()) return
      if ((e.target as HTMLElement).closest('button')) return
      e.preventDefault()
      const startX = e.clientX
      const startY = e.clientY
      let started = false
      let last: { x: number; y: number } | null = null

      const handleMove = (ev: MouseEvent): void => {
        if (
          !started &&
          (Math.abs(ev.clientX - startX) > DRAG_THRESHOLD_X ||
            Math.abs(ev.clientY - startY) > DRAG_THRESHOLD_Y)
        ) {
          started = true
          setDragPaneId(paneId)
          document.body.style.cursor = 'grabbing'
          document.body.style.userSelect = 'none'
        }
        if (!started) return
        last = { x: ev.clientX, y: ev.clientY }
        setDropZone(zoneAtPoint(ev.clientX, ev.clientY))
      }

      const handleUp = (): void => {
        document.removeEventListener('mousemove', handleMove)
        document.removeEventListener('mouseup', handleUp)
        document.body.style.cursor = ''
        document.body.style.userSelect = ''
        if (started && last !== null) {
          performDrop({ type: 'pane', paneId }, last.x, last.y)
        }
        setDragPaneId(null)
        setDropZone(null)
      }

      document.addEventListener('mousemove', handleMove)
      document.addEventListener('mouseup', handleUp)
    },
    [zoneAtPoint, performDrop],
  )

  // Divider drag → LIVE reflow from the drag-start tree (percent math
  // against the split's own span), save on release (§6.8.3).
  const startDividerDrag = useCallback(
    (e: React.MouseEvent, divider: DividerGeom): void => {
      if (e.button !== 0) return
      if (noteViewerInteractionBlocked()) return
      e.preventDefault()
      const grid = gridRef.current
      if (!grid) return
      const startX = e.clientX
      const startY = e.clientY
      const startRoot = rootRef.current
      const box = grid.getBoundingClientRect()
      const axisPx =
        divider.dir === 'row'
          ? Math.max(1, (box.width * divider.span) / 100)
          : Math.max(1, (box.height * divider.span) / 100)
      document.body.style.cursor = divider.dir === 'row' ? 'col-resize' : 'row-resize'
      document.body.style.userSelect = 'none'
      setDividerDragging(true)

      const handleMove = (ev: MouseEvent): void => {
        const dpx = divider.dir === 'row' ? ev.clientX - startX : ev.clientY - startY
        const deltaPct = (dpx / axisPx) * 100
        applyRoot(
          resizeDivider(startRoot, divider.splitPath, divider.index, deltaPct),
          false,
        )
      }
      const handleUp = (): void => {
        document.removeEventListener('mousemove', handleMove)
        document.removeEventListener('mouseup', handleUp)
        document.body.style.cursor = ''
        document.body.style.userSelect = ''
        setDividerDragging(false)
        // Persist the final sizes (coalesced).
        if (!readOnlyRef.current) saverRef.current?.schedule(rootRef.current)
      }
      document.addEventListener('mousemove', handleMove)
      document.addEventListener('mouseup', handleUp)
    },
    [applyRoot],
  )

  // ── Parallel session liveness (QoL: multi-pane simultaneous attach) ────
  // Every terminal pane used to self-resolve lookup-by-agent, so N panes
  // painted "Checking…" → TerminalPane one completion at a time. Batch
  // with Promise.all and commit once so all live attaches start together.
  const terminalWsIds = useMemo(() => terminalWorkspaceIdsInLayout(root), [root])
  const terminalWsKey = terminalWsIds.join('\0')
  const [sessionByWs, setSessionByWs] = useState<Record<string, TermPhase>>({})
  const [sessionsBatchReady, setSessionsBatchReady] = useState(false)

  useEffect(() => {
    let cancelled = false
    const ids = terminalWsKey.length === 0 ? [] : terminalWsKey.split('\0')
    if (ids.length === 0) {
      setSessionByWs({})
      setSessionsBatchReady(true)
      return
    }
    setSessionsBatchReady(false)
    void resolveAllMemberSessions(ids, lookupByAgentForDashboard).then((map) => {
      if (cancelled) return
      // Activate all live members in one pass (PRD §4.3.1) before panes
      // mount TerminalPane — same obligation as per-pane attach, batched.
      for (const [wsId, phase] of Object.entries(map)) {
        if (phase.kind === 'live') activateOnLiveSessionAttach(wsId, activateProject)
      }
      setSessionByWs(map)
      setSessionsBatchReady(true)
    })
    return () => {
      cancelled = true
    }
  }, [terminalWsKey])

  // ── Render ─────────────────────────────────────────────────────────────

  const membersById = useMemo(() => {
    const map = new Map<string, ProjectGroupMemberInfo>()
    for (const m of show.members) map.set(m.workspaceId, m)
    return map
  }, [show.members])

  // Tree → flat absolute percent rects + divider positions. Panes key
  // by identity (paneKey + occurrence), so ANY restructure MOVES
  // mounted panes (keeping live kessel grid-WS attachments) instead of
  // remounting them.
  const geometry = useMemo(() => computeLayoutGeometry(root), [root])

  // ⌘N badges — paneId → 1…9 in reading order (first 9 panes).
  const paneNumbers = useMemo(() => paneNumbersById(root), [root])

  // The hovered drop zone's highlight overlay (§6.8.2: half-highlights
  // for side splits, a full ring for center, edge bands for full-span
  // inserts).
  const zoneOverlay = useMemo(():
    | { kind: 'half' | 'center'; x: number; y: number; w: number; h: number }
    | { kind: 'edge'; side: 'left' | 'right' | 'top' | 'bottom' }
    | null => {
    if (!dropZone) return null
    if (dropZone.type === 'edge') return { kind: 'edge', side: dropZone.side }
    const p = geometry.panes.find((g) => g.paneId === dropZone.paneId)
    if (!p) return null
    switch (dropZone.region) {
      case 'left':
        return { kind: 'half', x: p.x, y: p.y, w: p.w / 2, h: p.h }
      case 'right':
        return { kind: 'half', x: p.x + p.w / 2, y: p.y, w: p.w / 2, h: p.h }
      case 'top':
        return { kind: 'half', x: p.x, y: p.y, w: p.w, h: p.h / 2 }
      case 'bottom':
        return { kind: 'half', x: p.x, y: p.y + p.h / 2, w: p.w, h: p.h / 2 }
      case 'center':
        return { kind: 'center', x: p.x, y: p.y, w: p.w, h: p.h }
    }
  }, [dropZone, geometry])

  const stale = freshness.staleRevision !== null
  const poc = membersById.get(show.pocWorkspaceId ?? '')

  // While ANY drag/resize is live, panes go pointer-events:none so the
  // htmlDoc <iframe>s can't swallow the document mousemove/mouseup
  // stream (hit-testing is rect math, not event targets, so this
  // changes nothing else).
  const dragActive = externalDrag !== null || dragPaneId !== null || dividerDragging

  return (
    <div className="flex-1 flex flex-col min-h-0 min-w-0 relative" data-projects-dashboard-panes>
      {/* Stale pill (§6.3a): another client saved a newer layout. It
          NEVER auto-applies — apply-on-open, or explicitly here. */}
      {stale && (
        <div className="absolute top-1.5 right-1.5 z-30 flex items-center gap-2 px-2 py-1 bg-[var(--color-bg-elevated)] border border-[color-mix(in_srgb,var(--color-status-warn)_40%,transparent)] shadow-lg">
          <span className="w-1.5 h-1.5 rounded-full bg-[var(--color-status-warn-amber)] flex-shrink-0" />
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

      {root === null ? (
        /* gridRef also lives here so a drop on the empty dashboard
           resolves (no [data-pane-id] rects → the whole box is one
           first-pane target). */
        <div
          ref={gridRef}
          className={`flex-1 flex items-center justify-center border border-dashed text-center px-8 transition-colors ${
            externalDrag && !readOnly
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
        <div ref={gridRef} className="relative flex-1 min-h-0 min-w-0">
          {geometry.panes.map((g) => {
            const pane = g.pane
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
              <div
                key={g.paneId}
                data-pane-id={g.paneId}
                data-dash-pane-ws={isTerminalPane(pane) ? pane.workspaceId : undefined}
                className="absolute flex flex-col min-w-0 min-h-0 p-px"
                style={{
                  left: `${g.x}%`,
                  top: `${g.y}%`,
                  width: `${g.w}%`,
                  height: `${g.h}%`,
                  opacity: dragPaneId === g.paneId ? 0.4 : 1,
                  pointerEvents: dragActive ? 'none' : undefined,
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
                  shortcutNum={paneNumbers.get(g.paneId)}
                  readOnly={readOnly}
                  focused={focusPaneId === g.paneId}
                  onClose={() => applyRoot(removePane(rootRef.current, g.paneId), true)}
                  onHeaderMouseDown={(e) => startPaneDrag(e, g.paneId)}
                >
                  {isTerminalPane(pane) ? (
                    <DashboardTerminalPane
                      workspaceId={pane.workspaceId}
                      member={member}
                      dashboardId={dashboard.id}
                      initialPhase={
                        sessionsBatchReady
                          ? (sessionByWs[pane.workspaceId] ?? PHASE_DORMANT)
                          : PHASE_CHECKING
                      }
                    />
                  ) : isHtmlDocPane(pane) ? (
                    <HtmlDocPane filePath={pane.filePath} workspaceId={pane.workspaceId} />
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
            )
          })}

          {/* Dividers (§6.8.3) — every boundary drags on BOTH axes,
              live reflow, ~10% floor. Hidden for viewers. */}
          {!readOnly &&
            geometry.dividers.map((d) => (
              <div
                key={`div:${d.splitPath.join('.')}:${d.index}:${d.dir}`}
                className={`absolute z-10 ${
                  d.dir === 'row' ? 'cursor-col-resize' : 'cursor-row-resize'
                } hover:bg-[var(--color-accent)]/40 transition-colors`}
                style={
                  d.dir === 'row'
                    ? {
                        left: `calc(${d.x}% - ${DIVIDER_HIT_PX / 2}px)`,
                        top: `${d.y}%`,
                        height: `${d.length}%`,
                        width: DIVIDER_HIT_PX,
                      }
                    : {
                        top: `calc(${d.y}% - ${DIVIDER_HIT_PX / 2}px)`,
                        left: `${d.x}%`,
                        width: `${d.length}%`,
                        height: DIVIDER_HIT_PX,
                      }
                }
                onMouseDown={(e) => startDividerDrag(e, d)}
                title="Drag to resize"
              />
            ))}

          {/* Drop-zone highlight (§6.8.2): side half / center ring /
              container edge band. */}
          {zoneOverlay && zoneOverlay.kind !== 'edge' && (
            <div
              className={`absolute z-20 pointer-events-none ${
                zoneOverlay.kind === 'center'
                  ? 'border-2 border-[var(--color-accent)] bg-[var(--color-accent)]/10'
                  : 'border border-[var(--color-accent)] bg-[var(--color-accent)]/20'
              }`}
              style={{
                left: `${zoneOverlay.x}%`,
                top: `${zoneOverlay.y}%`,
                width: `${zoneOverlay.w}%`,
                height: `${zoneOverlay.h}%`,
              }}
            />
          )}
          {zoneOverlay && zoneOverlay.kind === 'edge' && (
            <div
              className="absolute z-20 pointer-events-none border border-[var(--color-accent)] bg-[var(--color-accent)]/20"
              style={
                zoneOverlay.side === 'left'
                  ? { left: 0, top: 0, bottom: 0, width: EDGE_BAND_PX }
                  : zoneOverlay.side === 'right'
                    ? { right: 0, top: 0, bottom: 0, width: EDGE_BAND_PX }
                    : zoneOverlay.side === 'top'
                      ? { top: 0, left: 0, right: 0, height: EDGE_BAND_PX }
                      : { bottom: 0, left: 0, right: 0, height: EDGE_BAND_PX }
              }
            />
          )}
        </div>
      )}
    </div>
  )
}
