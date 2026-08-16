import { useState, useEffect, useCallback, useMemo } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import { agentDisplayName } from '@/lib/workspace-agent'
// Plan B — projects_update / workspaces_delete AND the daemon-data git
// reads/mutations (git_remove_worktree) are host-aware: route through
// the `/cli/*` HTTP layer (local OR remote). open_in_finder + the
// k2so_agents_*/inbox/workspace_relations host calls stay on Tauri
// invoke (host-only, out of scope).
import { daemonCliGet, daemonCliPost } from '@/lib/daemon-cli'
import { useProjectsStore } from '@/stores/projects'
import { useTabsStore } from '@/stores/tabs'
import { usePageViewStore } from '@/stores/page-view'
import { showContextMenu } from '@/lib/context-menu'
import WorktreeDialog from '@/components/Sidebar/WorktreeDialog'
import { HeartbeatsPanel } from '@/components/HeartbeatsPanel/HeartbeatsPanel'
import { UrlsPortsSection } from './UrlsPortsSection'
import { WorkspaceApiSection } from './WorkspaceApiSection'
import { ConnectedAgentsSection } from './ConnectedAgentsSection'
import { WorkspaceCompletionSoundBell } from './WorkspaceCompletionSoundToggle'

// Phase 2.1c Item 2 — `WorkItem` interface removed. The badge fetch
// now uses `invoke<number>('k2so_inbox_count', ...)` so the only
// renderer-side type that survived the migration is the count itself.

// ── Helpers ──────────────────────────────────────────────────────────────

// ── Component ────────────────────────────────────────────────────────────

export default function WorkspacePanel(): React.JSX.Element {
  const [displayName, setDisplayName] = useState('')
  const [wsInboxCount, setWsInboxCount] = useState(0)
  const [showWorktreeDialog, setShowWorktreeDialog] = useState(false)

  // Use stable selectors — avoid creating new references on every store change
  const activeProjectId = useProjectsStore((s) => s.activeProjectId)
  const activeProject = useProjectsStore(useCallback((s) => {
    return s.activeProjectId ? s.projects.find((p) => p.id === s.activeProjectId) ?? null : null
  }, []))
  const openAgentPane = useTabsStore((s) => s.openAgentPane)

  // Worktrees section collapse state, persisted per-workspace.
  // Default OPEN — worktrees are the action surface for review work,
  // most users want them visible by default.
  const worktreesKey = activeProjectId ? `worktrees.section-collapsed.${activeProjectId}` : null
  const [worktreesOpen, setWorktreesOpen] = useState<boolean>(() => {
    if (!worktreesKey) return true
    return localStorage.getItem(worktreesKey) !== 'closed'
  })
  useEffect(() => {
    if (!worktreesKey) {
      setWorktreesOpen(true)
      return
    }
    setWorktreesOpen(localStorage.getItem(worktreesKey) !== 'closed')
  }, [worktreesKey])
  const toggleWorktrees = useCallback((): void => {
    setWorktreesOpen((cur) => {
      const next = !cur
      if (worktreesKey) localStorage.setItem(worktreesKey, next ? 'open' : 'closed')
      return next
    })
  }, [worktreesKey])

  // Heartbeat data is fetched + polled by HeartbeatsPanel via its own
  // store; we no longer duplicate that fetch here.
  const activeProjectPath = activeProject?.path
  useEffect(() => {
    if (!activeProjectId || !activeProjectPath) return
    let cancelled = false
    const load = async (): Promise<void> => {
      // Phase 2.1c Item 2 — workspace inbox primitive count endpoint
      // (replaces the legacy `k2so_agents_workspace_inbox_list` whose
      // full-payload fetch was wasted bandwidth for a sidebar badge).
      try {
        const count = await invoke<number>('k2so_inbox_count', { projectPath: activeProjectPath })
        if (!cancelled) setWsInboxCount(count)
      } catch { if (!cancelled) setWsInboxCount(0) }
    }
    load()
    const interval = setInterval(load, 30000) // 30s, not 15s — reduce IPC chatter
    return () => { cancelled = true; clearInterval(interval) }
  }, [activeProjectId, activeProjectPath])

  useEffect(() => {
    if (!activeProjectPath) {
      setDisplayName('')
      return
    }
    let cancelled = false
    const loadName = (): void => {
      void agentDisplayName(activeProjectPath)
        .then((n) => { if (!cancelled) setDisplayName(n) })
        .catch(() => { if (!cancelled) setDisplayName('') })
    }
    loadName()
    let unlisten: (() => void) | undefined
    void listen('sync:projects', () => { loadName() }).then((fn) => {
      if (cancelled) fn()
      else unlisten = fn
    })
    return () => {
      cancelled = true
      unlisten?.()
    }
  }, [activeProjectPath])

  const agentMode = activeProject?.agentMode || 'off'
  const workspaces = activeProject?.workspaces ?? []
  // Filter to only worktree workspaces (not the main workspace)
  const worktrees = useMemo(() =>
    workspaces.filter((ws) => ws.worktreePath && ws.type !== 'main'),
    [workspaces]
  )

  if (!activeProject) {
    return (
      <div className="h-full flex items-center justify-center p-4">
        <p className="text-[10px] text-[var(--color-text-muted)] text-center">
          No workspace selected
        </p>
      </div>
    )
  }

  return (
    <div className="h-full flex flex-col overflow-hidden">
      {/* ── Status ── */}
      <div className="px-3 py-3 border-b border-[var(--color-border)]">
        <div className="flex items-center justify-between gap-2">
          <div className="flex items-baseline gap-2 min-w-0">
            {(displayName || activeProject.handle) && (
              <>
                <span className="text-[10px] uppercase tracking-wider text-[var(--color-text-muted)] flex-shrink-0">
                  Agent Name
                </span>
                <span className="text-xs text-[var(--color-text-primary)] truncate">
                  {displayName || activeProject.name}
                </span>
                {activeProject.handle && (
                  <span className="text-[10px] font-mono text-[var(--color-text-primary)] truncate">
                    {activeProject.handle}
                  </span>
                )}
              </>
            )}
          </div>
          <WorkspaceCompletionSoundBell project={activeProject} />
        </div>

        {/* Off mode inbox shortcut — navigation affordance when the
            workspace has undelegated inbox items. */}
        {agentMode === 'off' && wsInboxCount > 0 && (
          <div
            className="mt-2.5 px-2 py-1.5 hover:bg-[var(--color-bg-elevated)] transition-colors cursor-pointer -mx-1"
            onClick={() => openAgentPane('__workspace__', activeProject.path)}
          >
            <div className="flex items-center gap-2">
              <svg className="w-3 h-3 text-[var(--color-accent)] flex-shrink-0" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth={2}>
                <rect x="3" y="3" width="18" height="18" rx="2" />
                <path d="M3 9h18" />
              </svg>
              <span className="text-[11px] text-[var(--color-text-primary)]">View Inbox</span>
            </div>
          </div>
        )}

        {/* Workspace knowledge base / brain map (prd-workspace-kb). */}
        <div
          className="mt-2.5 px-2 py-1.5 hover:bg-[var(--color-bg-elevated)] transition-colors cursor-pointer -mx-1"
          onClick={() => usePageViewStore.getState().openWiki(activeProject.path)}
          title="Open the workspace knowledge base graph"
        >
          <div className="flex items-center gap-2">
            <svg className="w-3 h-3 text-[var(--color-accent)] flex-shrink-0" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth={2} strokeLinecap="round" strokeLinejoin="round">
              <circle cx="6" cy="6" r="2.5" />
              <circle cx="18" cy="8" r="2.5" />
              <circle cx="10" cy="18" r="2.5" />
              <path d="M8 7.5l7 0.5M8 16.5l8-7" />
            </svg>
            <span className="text-[11px] text-[var(--color-text-primary)]">View Wiki</span>
          </div>
        </div>
      </div>

      {/* ── Heartbeats ── HeartbeatsPanel renders its own full-width
          collapsible header (matching the Worktrees pattern below)
          and a content area, both with their own border-b dividers
          that span edge-to-edge. */}
      <HeartbeatsPanel />

      {/* ── Connected Agents (present connections only) ── */}
      <ConnectedAgentsSection projectId={activeProject.id} />

      {/* ── URLs & Ports ── (collapsible; K2 Connect tunnel surface +
          nested subdomain map, live via tunnel_status_changed /
          tunnel_subdomains_changed) */}
      <UrlsPortsSection projectId={activeProject.id} />

      <WorkspaceApiSection project={activeProject} />

      {/* ── Worktrees ── (collapsible; chevron toggles, count badge
          and new-worktree button stay clickable via stopPropagation) */}
      <button
        onClick={toggleWorktrees}
        className="w-full flex items-center justify-between px-3 py-2 border-b border-[var(--color-border)] cursor-pointer no-drag hover:bg-white/[0.02] transition-colors"
        title={worktreesOpen ? 'Collapse Worktrees' : 'Expand Worktrees'}
      >
        <span className="flex items-center gap-1.5 text-[11px] text-[var(--color-text-secondary)]">
          <svg
            className={`w-2 h-2 text-[var(--color-text-muted)] transition-transform ${worktreesOpen ? 'rotate-90' : ''}`}
            viewBox="0 0 8 8"
            fill="none"
            stroke="currentColor"
            strokeWidth="1.5"
            strokeLinecap="round"
            strokeLinejoin="round"
          >
            <path d="M2 1 L6 4 L2 7" />
          </svg>
          {/* Same branch-fork glyph used on each worktree row below
              (line ~466). Tinted blue (accent) here so the section
              header matches the Heartbeats heart-EKG icon's visual
              weight. Items in the list keep the muted variant. */}
          <svg
            className="w-3 h-3 text-[var(--color-accent)] flex-shrink-0"
            viewBox="0 0 16 16"
            fill="none"
            stroke="currentColor"
            strokeWidth="1.3"
            strokeLinecap="round"
            strokeLinejoin="round"
          >
            <circle cx="4" cy="4" r="1.5" />
            <circle cx="12" cy="4" r="1.5" />
            <circle cx="4" cy="12" r="1.5" />
            <path d="M4 5.5v5M4 8h6c1.1 0 2-.9 2-2v-.5" />
          </svg>
          Worktrees
          {worktrees.length > 0 && (
            <span className="text-[9px] tabular-nums font-medium px-1 py-0.5 bg-[var(--color-wash-1)] text-[var(--color-text-muted)]">
              {worktrees.length}
            </span>
          )}
        </span>
        <span
          onClick={(e) => {
            e.stopPropagation()
            setShowWorktreeDialog(true)
          }}
          className="w-5 h-5 flex items-center justify-center text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)] hover:bg-[var(--color-wash-2)] transition-colors no-drag cursor-pointer"
          title="New worktree"
        >
          <svg width="10" height="10" viewBox="0 0 10 10" fill="none" stroke="currentColor" strokeWidth="1.5">
            <line x1="5" y1="1" x2="5" y2="9" />
            <line x1="1" y1="5" x2="9" y2="5" />
          </svg>
        </span>
      </button>

      {showWorktreeDialog && (
        <WorktreeDialog
          projectId={activeProject.id}
          projectPath={activeProject.path}
          open={true}
          onClose={() => setShowWorktreeDialog(false)}
        />
      )}

      {worktreesOpen && (
        <div className="flex-1 overflow-y-auto">
          {worktrees.length === 0 ? (
            <div className="p-3">
              <p className="text-[10px] text-[var(--color-text-muted)]">
                No worktrees open. Click + to create one or let the manager delegate work.
              </p>
            </div>
          ) : (
            worktrees.map((ws) => {
              // Strip agent/<name>/ prefix from display name
              const displayName = ws.name?.replace(/^agent\/[^/]+\//, '') || ws.branch || 'worktree'
              // Check if this was created by our agent system
              const agentMatch = ws.name?.match(/^agent\/([^/]+)\//)
              const agentTemplate = agentMatch?.[1]

              return (
                <WorktreeRow
                  key={ws.id}
                  workspaceId={ws.id}
                  projectId={activeProject.id}
                  projectPath={activeProject.path}
                  worktreePath={ws.worktreePath}
                  displayName={displayName}
                  branch={ws.branch}
                  agentTemplate={agentTemplate}
                />
              )
            })
          )}
        </div>
      )}
    </div>
  )
}

// ── WorktreeRow ──────────────────────────────────────────────────────────

function WorktreeRow({
  workspaceId,
  projectId,
  projectPath,
  worktreePath,
  displayName,
  branch,
  agentTemplate,
}: {
  workspaceId: string
  projectId: string
  projectPath: string
  worktreePath: string | null
  displayName: string
  branch: string | null
  agentTemplate?: string
}): React.JSX.Element {
  const openAgentPane = useTabsStore((s) => s.openAgentPane)
  const tabTitle = displayName || branch || 'Worktree'

  const handleContextMenu = useCallback(async (e: React.MouseEvent) => {
    e.preventDefault()
    e.stopPropagation()

    const menuItems = [
      { id: 'open', label: 'Open' },
      { id: 'open-finder', label: 'Show in Finder' },
      { id: 'separator-1', label: '', type: 'separator' },
      { id: 'close', label: 'Close Worktree' },
      { id: 'recycle', label: 'Recycle Worktree' },
    ]

    const clickedId = await showContextMenu(menuItems)

    if (clickedId === 'open') {
      openAgentPane(`__wt:${workspaceId}`, projectPath, tabTitle)
    } else if (clickedId === 'open-finder' && worktreePath) {
      await invoke('projects_open_in_finder', { path: worktreePath })
    } else if (clickedId === 'close') {
      // Remove from DB, keep files on disk
      await daemonCliPost('workspaces/delete', { id: workspaceId })
      // Optimistic removal from store
      const state = useProjectsStore.getState()
      const updated = state.projects.map((p) => {
        if (p.id !== projectId) return p
        return { ...p, workspaces: p.workspaces.filter((ws) => ws.id !== workspaceId) }
      })
      useProjectsStore.setState({ projects: updated })
    } else if (clickedId === 'recycle') {
      // Remove git worktree from disk + remove from DB
      try {
        if (worktreePath) {
          await daemonCliPost('git/remove-worktree', {
            projectPath,
            worktreePath,
            workspaceId,
          })
        } else {
          await daemonCliPost('workspaces/delete', { id: workspaceId })
        }
      } catch {
        // If git remove fails, just delete the record
        await daemonCliPost('workspaces/delete', { id: workspaceId })
      }
      // Optimistic removal from store
      const state = useProjectsStore.getState()
      const updated = state.projects.map((p) => {
        if (p.id !== projectId) return p
        return { ...p, workspaces: p.workspaces.filter((ws) => ws.id !== workspaceId) }
      })
      useProjectsStore.setState({ projects: updated })
    }
  }, [workspaceId, projectId, projectPath, worktreePath, openAgentPane, tabTitle])

  return (
    <div
      className="px-3 py-2 border-b border-[var(--color-border)] last:border-b-0 hover:bg-[var(--color-bg-elevated)] transition-colors cursor-pointer"
      onClick={() => openAgentPane(`__wt:${workspaceId}`, projectPath, tabTitle)}
      onContextMenu={handleContextMenu}
    >
      <div className="flex items-center gap-2">
        <svg className="w-3.5 h-3.5 text-[var(--color-text-muted)] flex-shrink-0 opacity-60" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.3" strokeLinecap="round" strokeLinejoin="round">
          <circle cx="4" cy="4" r="1.5" />
          <circle cx="12" cy="4" r="1.5" />
          <circle cx="4" cy="12" r="1.5" />
          <path d="M4 5.5v5M4 8h6c1.1 0 2-.9 2-2v-.5" />
        </svg>
        <div className="flex-1 min-w-0">
          <div className="flex items-center gap-1.5">
            <span className="text-xs text-[var(--color-text-primary)] truncate">{displayName}</span>
            {agentTemplate && (
              <span className="text-[9px] font-medium text-[var(--color-text-muted)] bg-[var(--color-wash-1)] px-1 py-0.5 flex-shrink-0">
                {agentTemplate}
              </span>
            )}
          </div>
          {branch && (
            <div className="text-[10px] text-[var(--color-text-muted)] truncate">{branch}</div>
          )}
        </div>
      </div>
    </div>
  )
}
