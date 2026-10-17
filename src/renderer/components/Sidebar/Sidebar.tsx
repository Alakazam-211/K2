import React, { useState, useCallback, useMemo, useRef, useEffect } from 'react'
import { useProjectsStore } from '@/stores/projects'
import { useFocusGroupsStore } from '@/stores/focus-groups'
import { useSettingsStore } from '@/stores/settings'
import { SidebarCollapseButton } from './SidebarCollapseButton'
import { useToastStore } from '@/stores/toast'
import { AgentStatusCluster } from '@/components/AgentStatus/AgentStatusCluster'
import { useAgentStatus } from '@/components/AgentStatus/useAgentStatus'
import { useTerminalSettingsStore } from '@/stores/terminal-settings'
import { useCommandPaletteStore } from '@/stores/command-palette'
import { useAddWorkspaceDialogStore } from '@/stores/add-workspace-dialog'
import { useRemoveWorkspaceDialogStore } from '@/stores/remove-workspace-dialog'
import { RENAME_AGENT_MENU_ID, renameAgentMenuItem, useRenameAgentDialogStore } from '@/stores/rename-agent-dialog'
import { invoke } from '@tauri-apps/api/core'
import { emit } from '@tauri-apps/api/event'
// Plan B — daemon-data writes (nav-visible / project pins / workspace
// delete) go through the host-aware `/cli/*` HTTP layer. Host-only ops
// (open-in-finder/editor, open-focus-window) stay on Tauri `invoke`.
import { daemonCliPost } from '@/lib/daemon-cli'
import { pickWorkspaceFolder } from '@/lib/pick-workspace-folder'
import { showContextMenu } from '@/lib/context-menu'
import type { ContextMenuItemDef } from '@/stores/context-menu'
import { useConnectHostStore } from '@/stores/connect-host'
import { ADD_TO_HOME_MENU_ID } from '@/stores/add-to-home-dialog'
import { addAgentToHomeFromMenu } from '@/lib/add-to-home'
import { activeHomeHostKey } from '@/lib/home-address'
import { workspacePlacementMenuItems } from './workspace-menu-items'
import { usePageViewStore } from '@/stores/page-view'
import { startCloneTo, startCloneToThisComputer } from '@/lib/start-clone-to'
import { fetchClonePerms, pullAllowed, pushAllowed } from '@/lib/clone-permissions'
import { useGitInfo } from '@/hooks/useGit'
import ResizeHandle from './ResizeHandle'
import WorktreeDialog from './WorktreeDialog'
import ProjectAvatar from './ProjectAvatar'
import PresenceWorkspaceAvatars from '@/components/Presence/PresenceWorkspaceAvatars'
import FocusGroupDropdown from './FocusGroupDropdown'
import { RowDropLine, reorderedZoneIds, useRowDragReorder } from './row-drag-reorder'
import ActiveBar from './ActiveBar'
import { NavProjectTags } from './NavProjectTags'
import { KeyCombo } from '@/components/KeySymbol'
import { primaryScope } from '@/kessel/server-scope'
import { GrokBotSidebarSection } from '@/components/GrokBots/GrokBotRows'
import { closeGrokChat } from '@/stores/grokbots'

// ── Nav-visible worktrees (DB-backed via workspace.navVisible field) ─────────

function patchWorkspaceNavVisible(worktreeId: string, visible: boolean): void {
  // Optimistically update the local store without a full refetch
  const state = useProjectsStore.getState()
  const updated = state.projects.map((p) => ({
    ...p,
    workspaces: p.workspaces.map((ws) =>
      ws.id === worktreeId ? { ...ws, navVisible: visible ? 1 : 0 } : ws
    ),
  }))
  useProjectsStore.setState({ projects: updated })
  // Persist to DB asynchronously. camelCase POST body; no cross-window
  // sync (the old `workspace_set_nav_visible` Tauri shim emitted none).
  daemonCliPost(primaryScope(), 'workspaces/set-nav-visible', { id: worktreeId, visible }).catch(() => {})
}

export function addNavWorktree(worktreeId: string): void {
  patchWorkspaceNavVisible(worktreeId, true)
}

export function removeNavWorktree(worktreeId: string): void {
  patchWorkspaceNavVisible(worktreeId, false)
}

// ── Worktree git badge (shows changed files count) ──────────────────────────

function WorkspaceGitBadge({ path }: { path?: string }): React.JSX.Element | null {
  const { data } = useGitInfo(primaryScope(), path)
  if (!data?.isRepo) return null

  const count = data.changedFiles + data.untrackedFiles
  if (count === 0) return null

  return (
    <span className="ml-auto text-[10px] tabular-nums font-medium px-1.5 py-0.5 bg-[color-mix(in_srgb,var(--color-status-warn-text)_10%,transparent)] text-[var(--color-status-warn-text)] flex-shrink-0">
      {count}
    </span>
  )
}

// ── Worktree changes (a neutral count; no pulse, no amber) ──────────────────

/** How many files a worktree has changed: a quiet count chip (amber means
 *  "needs you" only, plan-agent-status-unify-v1 §3.3). */
function WorkspaceDirtyCount({ path }: { path?: string }): React.JSX.Element | null {
  const { data } = useGitInfo(primaryScope(), path)
  if (!data?.isRepo) return null

  const count = data.changedFiles + data.untrackedFiles
  if (count === 0) return null

  return (
    <span
      className="flex-shrink-0 px-[5px] py-px font-mono text-[10px] leading-none tabular-nums text-[var(--color-text-secondary)] bg-[var(--color-wash-3)]"
      title={`${count} changed ${count === 1 ? 'file' : 'files'}`}
      data-worktree-dirty={count}
    >
      {count}
    </span>
  )
}

// ── Agent status (the shared cluster) ───────────────────────────────────────

/** A nav row's status cluster: working | heartbeat | status-or-live |
 *  number (plan-agent-status-unify-v1 §3.2). The window server's rollup
 *  (RL1, daemon) plus this client's "not viewed" (RL11). `index` is the
 *  row's chord position (null = a list without chords: the column stays). */
export function AgentRowStatus({
  projectId,
  path,
  heartbeatEnabled,
  index,
}: {
  projectId: string
  path?: string
  heartbeatEnabled?: number
  index?: number | null
}): React.JSX.Element {
  const view = useAgentStatus(primaryScope(), { projectId, path, heartbeatEnabled })
  return <AgentStatusCluster view={view} index={index} />
}

// ── Single Workspace Item (worktreeMode === 0) ─────────────────────────────────

const NavWorktreeRow = React.memo(function NavWorktreeRow({
  worktreeId,
  projectId,
  branchName,
  isSelected,
  color,
  onClose,
}: {
  worktreeId: string
  projectId: string
  branchName: string
  isSelected: boolean
  color: string
  onClose: () => void
}): React.JSX.Element {
  const [hovered, setHovered] = useState(false)
  const setActiveWorkspace = useProjectsStore((s) => s.setActiveWorkspace)

  return (
    <>
      <div className="border-t border-[var(--color-border)]" />
      <button
        className={`w-full flex items-center gap-2 px-3 text-left text-[11px] transition-colors cursor-default no-drag ${
          isSelected
            ? 'bg-white/[0.06] text-[var(--color-text-primary)]'
            : 'text-[var(--color-text-muted)] hover:bg-white/[0.04] hover:text-[var(--color-text-primary)]'
        }`}
        style={{
          height: '32px',
          borderLeft: isSelected ? `2px solid ${color}` : '2px solid transparent',
        }}
        onClick={() => setActiveWorkspace(projectId, worktreeId)}
        onMouseEnter={() => setHovered(true)}
        onMouseLeave={() => setHovered(false)}
      >
        <svg className={`w-3.5 h-3.5 flex-shrink-0 ${isSelected ? 'text-[var(--color-accent)] opacity-80' : 'opacity-50'}`} viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.3" strokeLinecap="round" strokeLinejoin="round">
          <circle cx="4" cy="4" r="1.5" />
          <circle cx="12" cy="4" r="1.5" />
          <circle cx="4" cy="12" r="1.5" />
          <path d="M4 5.5v5M4 8h6c1.1 0 2-.9 2-2v-.5" />
        </svg>
        <span className="truncate flex-1">{branchName}</span>
        {hovered && (
          <button
            className="flex-shrink-0 flex h-4 w-4 items-center justify-center text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)] hover:bg-[var(--color-wash-2)] transition-colors"
            onClick={(e) => { e.stopPropagation(); onClose() }}
            title="Hide from nav"
          >
            <svg width="8" height="8" viewBox="0 0 8 8" fill="none" stroke="currentColor" strokeWidth="1.5">
              <line x1="1" y1="1" x2="7" y2="7" />
              <line x1="7" y1="1" x2="1" y2="7" />
            </svg>
          </button>
        )}
      </button>
    </>
  )
})

// ── Shortcut badges (the pinned area's look; Home reuses them) ──────────────

/** How many rows a digit chord reaches with a badge (1–9). */
export const SHORTCUT_BADGE_LIMIT = 9

/** A nav row's position number on its second line — the digit of its
 *  1–9 chord. `index` is 0-based; nothing past the ninth row. `combo`
 *  paints the whole chord instead of the bare digit (the Home dropdown). */
export function ShortcutIndexBadge({ index, combo }: { index?: number; combo?: string }): React.JSX.Element | null {
  if (index === undefined || index < 0 || index >= SHORTCUT_BADGE_LIMIT) return null
  return (
    <span
      className="ml-auto -mr-1 text-[10px] font-mono text-[var(--color-text-muted)] tabular-nums flex-shrink-0 leading-none pl-2"
      data-shortcut-badge={index + 1}
    >
      {combo !== undefined ? <KeyCombo combo={combo} /> : index + 1}
    </span>
  )
}

/** A section header's modifier hint beside its count (e.g. `⌥⌘ 1-9`). */
export function ShortcutRangeHint({ combo }: { combo: string }): React.JSX.Element {
  return (
    <span className="text-[9px] font-mono text-[var(--color-text-muted)] opacity-50" data-shortcut-hint={combo}>
      <KeyCombo combo={combo} />
    </span>
  )
}

// ── Agent row shell (one workspace / agent in the nav) ──────────────────────

/** The nav row every agent list paints: 32px avatar, name + presence on
 *  the first line, a 14px second line 2px below it (tags, working glyph,
 *  shortcut).
 *  Agents' `SingleProjectItem` and the Home roster both render through it
 *  so the two sidebars stay one look. */
export function AgentRowButton({
  isActive,
  color,
  avatar,
  name,
  nameAside,
  subline,
  onClick,
  onContextMenu,
  title,
  dimmed = false,
}: {
  isActive: boolean
  color: string
  avatar: React.ReactNode
  name: string
  nameAside?: React.ReactNode
  subline?: React.ReactNode
  onClick: () => void
  onContextMenu: (e: React.MouseEvent) => void
  title?: string
  /** Paint the row muted (an agent whose server is offline). */
  dimmed?: boolean
}): React.JSX.Element {
  return (
    <button
      className={`w-full flex items-stretch gap-2.5 px-3 py-2 text-left text-sm transition-colors ${
        isActive
          ? 'bg-white/[0.06] text-[var(--color-text-primary)]'
          : 'text-[var(--color-text-secondary)] hover:bg-white/[0.04] hover:text-[var(--color-text-primary)]'
      }${dimmed ? ' opacity-60' : ''}`}
      style={{
        borderLeft: isActive ? `2px solid ${color}` : '2px solid transparent'
      }}
      onClick={onClick}
      onContextMenu={onContextMenu}
      title={title}
      aria-current={isActive ? 'true' : undefined}
      data-agent-row=""
    >
      <div className="flex items-center flex-shrink-0">{avatar}</div>
      <div className="flex flex-col justify-center min-w-0 flex-1">
        <div className="flex items-center gap-2 w-full leading-4">
          <span className="truncate flex-1 leading-4">{name}</span>
          <span className="flex items-center">{nameAside}</span>
        </div>
        {/* Line 2 sits 2px below line 1 (`mt-0.5`, Rosson 2026-10-16): the
            tag, cluster and number read apart from the name and presence.
            16 + 2 + 14 = 32 = the avatar, so the row height holds. */}
        <div data-agent-row-line2="" className="mt-0.5 h-3.5 min-h-3.5 leading-none flex items-center gap-1">{subline}</div>
      </div>
    </button>
  )
}

export function SingleProjectItem({
  project,
  isActive,
  onContextMenu,
  shortcutIndex
}: {
  project: ReturnType<typeof useProjectsStore.getState>['projects'][number]
  isActive: boolean
  onContextMenu: (e: React.MouseEvent, projectId: string) => void
  shortcutIndex?: number
}): React.JSX.Element {
  const setActiveWorkspace = useProjectsStore((s) => s.setActiveWorkspace)
  const activeWorkspaceId = useProjectsStore((s) => s.activeWorkspaceId)
  // Only poll git info for the active project to avoid hammering git on all repos
  const firstWorkspace = project.workspaces[0]
  const isItemActive = isActive && firstWorkspace && activeWorkspaceId === firstWorkspace.id

  // Worktrees visible in the nav for this project (DB-backed via navVisible field)
  const visibleWorktrees = useMemo(() =>
    (project.workspaces ?? []).filter((ws) => ws.type === 'worktree' && ws.navVisible === 1),
    [project.workspaces]
  )

  const handleClick = useCallback(() => {
    // Picking a K2 agent gives the main area back from a Grok Bot chat.
    closeGrokChat()
    if (firstWorkspace) {
      setActiveWorkspace(project.id, firstWorkspace.id)
    }
  }, [project.id, firstWorkspace, setActiveWorkspace])

  return (
    <div className="no-drag">
      <AgentRowButton
        isActive={!!isItemActive}
        color={project.color}
        onClick={handleClick}
        onContextMenu={(e) => onContextMenu(e, project.id)}
        avatar={
          <ProjectAvatar
            projectPath={project.path}
            projectName={project.name}
            projectColor={project.color}
            projectId={project.id}
            iconUrl={project.iconUrl}
            size={32}
          />
        }
        name={project.name}
        nameAside={<PresenceWorkspaceAvatars path={project.path} />}
        subline={
          <>
            <NavProjectTags workspaceId={project.id} />
            <AgentRowStatus
              projectId={project.id}
              path={project.path}
              heartbeatEnabled={project.heartbeatEnabled}
              index={shortcutIndex ?? null}
            />
          </>
        }
      />
      {visibleWorktrees.map((ws) => (
        <NavWorktreeRow
          key={ws.id}
          worktreeId={ws.id}
          projectId={project.id}
          branchName={(ws.name?.replace(/^agent\/[^/]+\//, '') || ws.branch || 'worktree')}
          isSelected={isActive && activeWorkspaceId === ws.id}
          color={project.color}
          onClose={() => removeNavWorktree(ws.id)}
        />
      ))}
    </div>
  )
}

// ── Worktree button (shared between ungrouped and section-grouped) ──────────

function WorkspaceButton({
  workspace,
  projectPath,
  isActive,
  onClick,
  onContextMenu,
  shortcutIndex
}: {
  workspace: ReturnType<typeof useProjectsStore.getState>['projects'][number]['workspaces'][number]
  projectPath: string
  isActive: boolean
  onClick: () => void
  onContextMenu: (e: React.MouseEvent) => void
  shortcutIndex?: number
}): React.JSX.Element {
  const workspacePath = workspace.worktreePath ?? projectPath
  const isWorktree = workspace.type === 'worktree'
  const branchName = isWorktree ? workspace.branch : undefined

  return (
    <div>
      <button
        className={`w-full flex flex-col gap-0.5 px-3 py-1.5 text-left text-xs transition-colors font-light ${
          isActive
            ? 'bg-white/[0.06] text-[var(--color-accent)]'
            : 'text-[var(--color-text-muted)] hover:text-[var(--color-text-secondary)] hover:bg-white/[0.04]'
        }`}
        style={{
          borderLeft: isActive ? '2px solid var(--color-accent)' : '2px solid transparent'
        }}
        onClick={onClick}
        onContextMenu={onContextMenu}
      >
        <div className="flex items-center gap-2 w-full">

          {isWorktree ? (
            /* Git branch/worktree icon */
            <svg className="w-3.5 h-3.5 flex-shrink-0 opacity-60" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.3" strokeLinecap="round" strokeLinejoin="round">
              <circle cx="4" cy="4" r="1.5" />
              <circle cx="12" cy="4" r="1.5" />
              <circle cx="4" cy="12" r="1.5" />
              <path d="M4 5.5v5M4 8h6c1.1 0 2-.9 2-2v-.5" />
            </svg>
          ) : (
            /* Local/computer icon for main branch */
            <svg className="w-3.5 h-3.5 flex-shrink-0 opacity-60" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.3" strokeLinecap="round" strokeLinejoin="round">
              <rect x="2" y="2" width="12" height="9" rx="1" />
              <path d="M5 14h6" />
              <path d="M8 11v3" />
            </svg>
          )}

          <span className="truncate flex-1" title={workspace.name}>
            {/* Strip agent/<name>/ prefix for readability — show just the task portion */}
            {workspace.name.replace(/^agent\/[^/]+\//, '')}
          </span>

          <PresenceWorkspaceAvatars path={workspacePath} />
          {isWorktree && <WorkspaceDirtyCount path={workspacePath} />}
          <AgentRowStatus projectId={workspace.projectId} path={workspacePath} index={shortcutIndex ?? null} />
        </div>
      </button>
    </div>
  )
}

// ── Workspace Item (worktreeMode === 1, expandable) ────────────────────────────

function ProjectItem({
  project,
  isActive,
  onContextMenu,
  shortcutIndex
}: {
  project: ReturnType<typeof useProjectsStore.getState>['projects'][number]
  isActive: boolean
  onContextMenu: (e: React.MouseEvent, projectId: string) => void
  shortcutIndex?: number
}): React.JSX.Element {
  const [isExpanded, setIsExpanded] = useState(isActive)
  const [showWorktreeDialog, setShowWorktreeDialog] = useState(false)
  const activeWorkspaceId = useProjectsStore((s) => s.activeWorkspaceId)
  const setActiveWorkspace = useProjectsStore((s) => s.setActiveWorkspace)
  const fetchProjects = useProjectsStore((s) => s.fetchProjects)

  const handleClick = useCallback(() => {
    setIsExpanded((prev) => !prev)
  }, [])

  const handleWorkspaceClick = useCallback(
    (workspaceId: string) => {
      setActiveWorkspace(project.id, workspaceId)
    },
    [project.id, setActiveWorkspace]
  )

  const handleWorkspaceContextMenu = useCallback(
    async (e: React.MouseEvent, workspaceId: string) => {
      e.preventDefault()
      e.stopPropagation()

      const ws = (project.workspaces ?? []).find((w) => w.id === workspaceId)
      if (!ws) return

      const isWorktree = ws.type === 'worktree'
      const workspacePath = ws.worktreePath ?? project.path

      // Fetch installed editors for the submenu
      let editors: Array<{ id: string; label: string }> = []
      try {
        const raw = await invoke<Array<{ id: string; label: string }>>('projects_get_editors')
        editors = Array.isArray(raw) ? raw : []
      } catch {
        // ignore
      }

      const menuItems: Array<{ id: string; label: string; type?: string }> = []

      // Open in Finder / path / Wiki
      menuItems.push({ id: 'ws-open-finder', label: 'Open in Finder' })
      menuItems.push({ id: 'ws-copy-path', label: 'Copy Path' })
      menuItems.push({ id: 'ws-view-wiki', label: 'View Wiki' })

      // Open in Editor submenu
      if (editors.length > 0) {
        menuItems.push({ id: 'ws-editor-sep', label: '', type: 'separator' })
        for (const editor of editors) {
          menuItems.push({ id: `ws-editor:${editor.id}`, label: `Open in ${editor.label}` })
        }
      }

      // Close / Recycle (only for worktree type, not the main branch workspace)
      if (isWorktree) {
        menuItems.push({ id: 'ws-close-sep', label: '', type: 'separator' })
        menuItems.push({ id: 'ws-close', label: 'Close Worktree' })
        menuItems.push({ id: 'ws-recycle', label: 'Recycle Worktree' })
      }

      const clickedId = await showContextMenu(menuItems)

      if (clickedId === 'ws-open-finder') {
        await invoke('projects_open_in_finder', { path: workspacePath })
      } else if (clickedId === 'ws-copy-path') {
        await navigator.clipboard.writeText(workspacePath).catch((err) =>
          console.warn('[sidebar] ws-copy-path', err),
        )
      } else if (clickedId === 'ws-view-wiki') {
        // Wiki lives under the worktree path when present (same as Finder).
        usePageViewStore.getState().openWiki(workspacePath)
      } else if (clickedId?.startsWith('ws-editor:')) {
        const editorId = clickedId.replace('ws-editor:', '')
        await invoke('projects_open_in_editor', { editorId, path: workspacePath })
      } else if (clickedId === 'ws-close') {
        // Prevent closing the last worktree
        if ((project.workspaces ?? []).length <= 1) {
          await showContextMenu([
            { id: 'error', label: 'Cannot close the last worktree' }
          ])
          return
        }
        // Remove from DB only, keep files on disk. workspaces_delete
        // emitted no cross-window sync; fetchProjects refreshes locally.
        await daemonCliPost(primaryScope(), 'workspaces/delete', { id: workspaceId })
        await fetchProjects()
      } else if (clickedId === 'ws-recycle') {
        // Prevent recycling the last worktree
        if ((project.workspaces ?? []).length <= 1) {
          await showContextMenu([
            { id: 'error', label: 'Cannot recycle the last worktree' }
          ])
          return
        }
        // Show confirmation via second context menu
        const confirmId = await showContextMenu([
          { id: 'confirm-recycle', label: `Recycle worktree at ${workspacePath}?` },
          { id: 'confirm-sep', label: '', type: 'separator' },
          { id: 'do-recycle', label: 'Confirm Recycle' },
          { id: 'cancel-recycle', label: 'Cancel' }
        ])
        if (confirmId === 'do-recycle') {
          // Plan B — git_remove_worktree moves to the host-aware daemon
          // route (git cluster, Bulk-2). camelCase POST body; no
          // cross-window sync emit (the old git command had none — the
          // fetchProjects() below covers the workspaces-row delete).
          await daemonCliPost(primaryScope(), 'git/remove-worktree', {
            worktreePath: workspacePath,
            projectPath: project.path,
            workspaceId: workspaceId
          })
          await fetchProjects()
        }
      }
    },
    [project, fetchProjects]
  )

  const workspaces = project.workspaces ?? []

  return (
    <div className="no-drag">
      <button
        className={`w-full flex items-stretch gap-2.5 px-3 py-2 text-left text-sm transition-colors group ${
          isActive
            ? 'bg-white/[0.08] text-[var(--color-text-primary)]'
            : 'text-[var(--color-text-secondary)] hover:bg-white/[0.04] hover:text-[var(--color-text-primary)]'
        }`}
        style={{
          borderLeft: isActive ? `2px solid ${project.color}` : '2px solid transparent'
        }}
        onClick={handleClick}
        onContextMenu={(e) => onContextMenu(e, project.id)}
      >
        <div className="flex items-center flex-shrink-0">
          <ProjectAvatar
            projectPath={project.path}
            projectName={project.name}
            projectColor={project.color}
            projectId={project.id}
            iconUrl={project.iconUrl}
            size={32}
          />
        </div>
        <div className="flex flex-col justify-center min-w-0 flex-1">
          <div className="flex items-center gap-2 w-full">
            <span className="truncate">{project.name}</span>
            <span className="text-[10px] text-[var(--color-text-muted)] tabular-nums flex-shrink-0 px-1.5 py-0.5 bg-white/[0.06] font-mono">
              {(project.workspaces ?? []).length}
            </span>
          </div>
          <NavProjectTags workspaceId={project.id} />
        </div>
        {/* New worktree + expand/collapse controls */}
        <div className="flex flex-col items-center justify-center gap-0.5 flex-shrink-0">
          <span
            onClick={(e) => {
              e.stopPropagation()
              setShowWorktreeDialog(true)
              if (!isExpanded) setIsExpanded(true)
            }}
            className="w-5 h-5 flex items-center justify-center text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)] hover:bg-[var(--color-wash-2)] transition-colors cursor-pointer"
            title="New workspace"
          >
            <svg width="10" height="10" viewBox="0 0 10 10" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round">
              <path d="M5 1v8M1 5h8" />
            </svg>
          </span>
          <svg
            className={`w-3 h-3 text-[var(--color-text-muted)] transition-transform ${
              isExpanded ? 'rotate-90' : ''
            }`}
            fill="none"
            viewBox="0 0 24 24"
            stroke="currentColor"
            strokeWidth={2}
          >
            <path strokeLinecap="round" strokeLinejoin="round" d="M9 5l7 7-7 7" />
          </svg>
        </div>
      </button>

      {isExpanded && (
        <div className="mt-0.5 mb-0.5">
          {workspaces.map((workspace, wsIdx) => (
            <WorkspaceButton
              key={workspace.id}
              workspace={workspace}
              projectPath={project.path}
              isActive={activeWorkspaceId === workspace.id}
              onClick={() => handleWorkspaceClick(workspace.id)}
              onContextMenu={(e) => handleWorkspaceContextMenu(e, workspace.id)}
              shortcutIndex={shortcutIndex !== undefined ? shortcutIndex + wsIdx : undefined}
            />
          ))}
        </div>
      )}

      {/* Worktree creation dialog */}
      {showWorktreeDialog && (
        <WorktreeDialog
          projectId={project.id}
          projectPath={project.path}
          open={true}
          onClose={() => setShowWorktreeDialog(false)}
        />
      )}
    </div>
  )
}

// ── Sidebar ──────────────────────────────────────────────────────────────────

// ── Focus Group Selector ──────────────────────────────────────────────────────

type NavProject = ReturnType<typeof useProjectsStore.getState>['projects'][number]

/** The workspaces the nav's main list shows for a focus group pick (null =
 *  no group: all of them). Pinned ones never: they have the Pinned list.
 *  Ungrouped ones in every group view — 0.39.45 (GH #26): ungrouped
 *  projects (CLI-created, or onboarded against a focus-group name that
 *  doesn't exist) filtered out were unreachable from the nav entirely.
 *  The picker's counts use this too, so a count is what the list shows. */
export function navListedProjects<P extends Pick<NavProject, 'pinned' | 'focusGroupId'>>(
  projects: readonly P[],
  groupId: string | null,
): P[] {
  const unpinned = projects.filter((p) => !p.pinned)
  if (groupId === null) return unpinned
  return unpinned.filter((p) => p.focusGroupId === groupId || p.focusGroupId == null)
}

function FocusGroupSelector(): React.JSX.Element {
  const projects = useProjectsStore((s) => s.projects)
  const focusGroups = useFocusGroupsStore((s) => s.focusGroups)
  const activeFocusGroupId = useFocusGroupsStore((s) => s.activeFocusGroupId)
  const setActiveFocusGroup = useFocusGroupsStore((s) => s.setActiveFocusGroup)
  const focusGroupsEnabled = useFocusGroupsStore((s) => s.focusGroupsEnabled)

  if (!focusGroupsEnabled) {
    return (
      <div className="px-4 pt-3 pb-2 no-drag">
        <span className="text-[11px] font-semibold tracking-wide text-[var(--color-text-muted)] uppercase">
          Workspaces
        </span>
      </div>
    )
  }

  return (
    <div className="px-3 pt-3 pb-2 no-drag flex items-center gap-1.5">
      <div className="flex-1 min-w-0">
        <FocusGroupDropdown
          options={focusGroups.map((g) => ({
            id: g.id,
            name: g.name,
            color: g.color,
            count: navListedProjects(projects, g.id).length,
          }))}
          value={activeFocusGroupId}
          triggerCount={navListedProjects(projects, activeFocusGroupId).length}
          onChange={setActiveFocusGroup}
        />
      </div>
      <button
        className="flex-shrink-0 flex items-center gap-1 px-1.5 py-1 text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)] hover:bg-white/[0.06] transition-colors cursor-pointer"
        onClick={() => useCommandPaletteStore.getState().toggle()}
        title="Command Palette (⌘K)"
      >
        <svg className="w-3.5 h-3.5" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth={2} strokeLinecap="round" strokeLinejoin="round">
          <circle cx="11" cy="11" r="8" />
          <line x1="21" y1="21" x2="16.65" y2="16.65" />
        </svg>
      </button>
    </div>
  )
}

// ── Sidebar ──────────────────────────────────────────────────────────────────

/** The nav's two reorderable lists. */
type NavZone = 'pinned' | 'unpinned'

export default function Sidebar(): React.JSX.Element {
  const projects = useProjectsStore((s) => s.projects)
  const activeProjectId = useProjectsStore((s) => s.activeProjectId)
  const removeProject = useProjectsStore((s) => s.removeProject)
  const addProject = useProjectsStore((s) => s.addProject)
  const fetchProjects = useProjectsStore((s) => s.fetchProjects)

  const reorderProjects = useProjectsStore((s) => s.reorderProjects)
  const focusGroupsEnabled = useFocusGroupsStore((s) => s.focusGroupsEnabled)
  const activeFocusGroupId = useFocusGroupsStore((s) => s.activeFocusGroupId)

  // 0.37.13 — Agents & Pinned section collapse + scroll.
  // Persists across launches via localStorage so users with many
  // workspaces don't have to re-collapse on every reopen.
  const [agentsPinnedCollapsed, setAgentsPinnedCollapsedRaw] = useState<boolean>(() => {
    try { return localStorage.getItem('k2so:sidebar:agentsPinnedCollapsed') === '1' }
    catch { return false }
  })
  const setAgentsPinnedCollapsed = useCallback((v: boolean | ((prev: boolean) => boolean)) => {
    setAgentsPinnedCollapsedRaw((prev) => {
      const next = typeof v === 'function' ? v(prev) : v
      try { localStorage.setItem('k2so:sidebar:agentsPinnedCollapsed', next ? '1' : '0') } catch { /* ignore */ }
      return next
    })
  }, [])

  // 0.39.0: retired forced floating of agent-mode workspaces to the top
  // (was a separate `agentProjects` zone above `pinnedProjects`). Agent-mode
  // workspaces now flow through the same Pinned / focus group / ungrouped
  // lists as any other workspace. Users pin manually if they want them
  // surfaced. Single-list-of-workspaces model end-to-end.
  const pinnedProjects = useMemo(() =>
    projects.filter((p) => p.pinned), [projects])

  const filteredProjects = useMemo(
    () => navListedProjects(projects, focusGroupsEnabled ? activeFocusGroupId : null),
    [projects, focusGroupsEnabled, activeFocusGroupId],
  )

  // ── Nudge to enable focus groups at 10+ workspaces (every 3 hours) ──
  useEffect(() => {
    if (focusGroupsEnabled || projects.length < 10) return

    const NUDGE_KEY = 'k2so:focus-group-nudge-last'
    const THREE_HOURS = 3 * 60 * 60 * 1000
    const lastNudge = parseInt(localStorage.getItem(NUDGE_KEY) || '0', 10)
    const now = Date.now()

    if (now - lastNudge < THREE_HOURS) return

    localStorage.setItem(NUDGE_KEY, String(now))
    useToastStore.getState().addToast(
      `You have ${projects.length} workspaces. Enable Focus Groups to keep things organized.`,
      'info',
      8000,
      {
        label: 'Settings',
        onClick: () => useSettingsStore.getState().openSettings('projects'),
      }
    )
  }, [projects.length, focusGroupsEnabled])

  // ── Drag-to-reorder (shared with Home: same ghost, line, cursor) ──
  // Writes the daemon's canonical workspace order (`projects/reorder`,
  // optimistic in the store), the order Settings → Workspaces shows. A row
  // moves only within its own list: Pinned, or the list under the focus
  // group picker. Rows of other focus groups (not on screen) keep their
  // slots; moving a workspace into another group stays a Settings drop.
  const rowDrag = useRowDragReorder<NavZone>()
  const startRowDrag = rowDrag.onRowMouseDown
  const pinnedRef = useRef<HTMLDivElement>(null)
  const unpinnedRef = useRef<HTMLDivElement>(null)
  // The lists as painted, read at drop time (the closure is from mousedown).
  const zoneIdsRef = useRef<Record<NavZone, string[]>>({ pinned: [], unpinned: [] })
  zoneIdsRef.current = {
    pinned: pinnedProjects.map((p) => p.id),
    unpinned: filteredProjects.map((p) => p.id),
  }

  const handleProjectMouseDown = useCallback((
    e: React.MouseEvent,
    projectId: string,
    zone: NavZone,
    index: number,
  ) => {
    startRowDrag(e, {
      zone,
      index,
      container: () => (zone === 'pinned' ? pinnedRef : unpinnedRef).current,
      rowSelector: '[data-project-id]',
      onDrop: (_from, gap) => {
        const allIds = useProjectsStore.getState().projects.map((p) => p.id)
        const ids = reorderedZoneIds(allIds, zoneIdsRef.current[zone], projectId, gap)
        if (ids) void reorderProjects(ids)
      },
    })
  }, [startRowDrag, reorderProjects])

  // Worktree dialog state
  const [worktreeDialog, setWorktreeDialog] = useState<{
    projectId: string
    projectPath: string
  } | null>(null)


  const handleAddProject = useCallback(async () => {
    const folderPath = await pickWorkspaceFolder()
    if (!folderPath) return
    // Harness fan-out is off by default (canonical-agents PRD §4), so the
    // add has no destructive effects — no consent preview needed.
    useAddWorkspaceDialogStore.getState().open({
      path: folderPath,
      onConfirm: async ({ seedWiki, seedAgentsMd, fanout }) => {
        await addProject(folderPath, { seedWiki, seedAgentsMd, fanout })
        try {
          await daemonCliPost(primaryScope(), 'agents/run-workspace-ingest', { project_path: folderPath })
        } catch (err) {
          console.warn('[add-workspace] run ingest failed:', err)
        }
      },
    })
  }, [addProject])

  const handleContextMenu = useCallback(
    async (e: React.MouseEvent, projectId: string) => {
      e.preventDefault()
      const project = projects.find((p) => p.id === projectId)
      if (!project) return

      // Fetch installed editors for the submenu
      let editors: Array<{ id: string; label: string }> = []
      try {
        const raw = await invoke<Array<{ id: string; label: string }>>('projects_get_editors')
        editors = Array.isArray(raw) ? raw : []
      } catch {
        // ignore
      }

      // Build menu items -- pin/Active sit above Open in [IDE] so the
      // day-to-day placement actions stay above the (often long) editor list.
      const menuItems: ContextMenuItemDef[] = [
        { id: 'settings', label: 'Workspace Settings' },
        { id: 'view-wiki', label: 'View Wiki' },
        { id: 'separator-settings', label: '', type: 'separator' },
        // Rename agent… — the agent's DISPLAY name only. The handle (the
        // address) and the folder never change from here. Disabled when
        // this window's server would refuse the write.
        renameAgentMenuItem(primaryScope()),
        { id: 'open-finder', label: 'Open in Finder' },
        { id: 'copy-path', label: 'Copy Path' },
        { id: 'focus-window', label: 'Open in Focus Window' },
        // Pin/Unpin, "Add to Home" right under it, then the Active Bar rows.
        ...workspacePlacementMenuItems(project),
      ]

      // Open in [IDE] — after pin/Active so a long editor list doesn't bury them.
      if (editors.length > 0) {
        menuItems.push({ id: 'separator-editors', label: '', type: 'separator' })
        for (const editor of editors) {
          menuItems.push({ id: `editor:${editor.id}`, label: `Open in ${editor.label}` })
        }
      }

      menuItems.push(
        { id: 'separator-wt', label: '', type: 'separator' },
        { id: 'new-worktree', label: 'New Worktree...' }
      )

      // "Clone to <host>" — migrate this LOCAL workspace onto a connected K2
      // Connect host. Only offered when we're on the local daemon (the
      // orchestration bundles + reads from the local machine first).
      // On a REMOTE host the direction flips: "Clone to This Computer"
      // pulls the remote workspace home (0.40.22, clone-pull.ts).
      const { activeHost, hosts } = useConnectHostStore.getState()
      // 0.46.0 clone permissions (whoami): hide what the gate would refuse.
      const clonePerms = await fetchClonePerms(primaryScope(), activeHost === 'local' ? 'local' : activeHost.id)
      if (activeHost === 'local' && hosts.length > 0 && pushAllowed(clonePerms)) {
        menuItems.push({ id: 'separator-clone', label: '', type: 'separator' })
        for (const host of hosts) {
          menuItems.push({ id: `clone-to:${host.id}`, label: `Clone to ${host.label}` })
        }
      } else if (activeHost !== 'local' && pullAllowed(clonePerms)) {
        menuItems.push({ id: 'separator-clone', label: '', type: 'separator' })
        menuItems.push({ id: 'clone-here', label: 'Clone to This Computer' })
      }

      menuItems.push(
        { id: 'separator', label: '', type: 'separator' },
        { id: 'remove', label: 'Remove Workspace' }
      )

      const clickedId = await showContextMenu(menuItems)

      if (clickedId === 'settings') {
        useSettingsStore.getState().openSettings('projects', project.id)
      } else if (clickedId === RENAME_AGENT_MENU_ID) {
        useRenameAgentDialogStore.getState().open({
          projectId: project.id,
          projectPath: project.path,
          currentName: project.name,
          handle: project.handle ?? '',
        })
      } else if (clickedId === 'view-wiki') {
        usePageViewStore.getState().openWiki(project.path)
      } else if (clickedId === 'clone-here') {
        startCloneToThisComputer(project.path, project.name)
      } else if (clickedId?.startsWith('clone-to:')) {
        const hostId = clickedId.replace('clone-to:', '')
        const host = useConnectHostStore.getState().hosts.find((h) => h.id === hostId)
        if (host) void startCloneTo(project.path, project.name, host)
      } else if (clickedId === 'open-finder') {
        await invoke('projects_open_in_finder', { path: project.path })
      } else if (clickedId === 'copy-path') {
        await navigator.clipboard.writeText(project.path).catch((err) =>
          console.warn('[sidebar] copy-path', err),
        )
      } else if (clickedId === 'focus-window') {
        await invoke('projects_open_focus_window', { projectId: project.id })
      } else if (clickedId?.startsWith('editor:')) {
        const editorId = clickedId.replace('editor:', '')
        await invoke('projects_open_in_editor', { editorId, path: project.path })
      } else if (clickedId === 'new-worktree') {
        setWorktreeDialog({ projectId: project.id, projectPath: project.path })
      } else if (clickedId === ADD_TO_HOME_MENU_ID) {
        // Homes are this device's view state: no daemon write, no refetch.
        // The row's host is the server this window was on when the menu
        // opened (a remote's own key, `local` for this computer) — the same
        // key Home's "This server" picker uses.
        addAgentToHomeFromMenu(activeHomeHostKey(activeHost), project)
      } else if (clickedId === 'toggle-pin') {
        await daemonCliPost(primaryScope(), 'projects/update', { id: projectId, pinned: project.pinned ? 0 : 1 })
        void emit('sync:projects').catch(() => {})
        await fetchProjects()
      } else if (clickedId === 'toggle-active') {
        await daemonCliPost(primaryScope(), 'projects/update', { id: projectId, manuallyActive: project.manuallyActive ? 0 : 1 })
        void emit('sync:projects').catch(() => {})
        await fetchProjects()
      } else if (clickedId === 'active-24h') {
        // Set lastInteractionAt to now — the Active Bar keeps projects with
        // interaction < 24hrs. projects_touch_interaction emitted no sync.
        await daemonCliPost(primaryScope(), 'projects/touch-interaction', { id: projectId })
        const store = useProjectsStore.getState()
        const updated = store.projects.map((p) =>
          p.id === projectId ? { ...p, lastInteractionAt: Math.floor(Date.now() / 1000) } : p
        )
        useProjectsStore.setState({ projects: updated })
      } else if (clickedId === 'remove') {
        useRemoveWorkspaceDialogStore.getState().open({
          projectId,
          projectName: project.name,
          projectPath: project.path,
        })
      }
    },
    [projects, fetchProjects]
  )

  return (
    <div className="relative flex flex-col h-full">
      <ResizeHandle />

      {/* Pinned workspaces — always visible above focus groups.
          0.37.13: collapsible header + inner scroll when > 10 items so the
          section doesn't push focus groups + active dock off-screen.
          0.39.0: agent-mode workspaces now flow through this single Pinned
          section (no dedicated Agents zone above). */}
      {pinnedProjects.length > 0 && (() => {
        const totalCount = pinnedProjects.length
        const SCROLL_THRESHOLD = 10
        const SCROLL_MAX_HEIGHT = 360 // ~7-8 items visible before scrolling
        const useScroll = totalCount > SCROLL_THRESHOLD
        return (
        <div className="border-b border-[var(--color-border)]">
          <button
            type="button"
            onClick={() => setAgentsPinnedCollapsed((v) => !v)}
            className="no-drag w-full flex items-center gap-1.5 px-4 pt-3 pb-2 text-left cursor-pointer hover:bg-white/[0.02] transition-colors"
            aria-expanded={!agentsPinnedCollapsed}
            aria-controls="sidebar-agents-pinned-content"
          >
            <span className="text-[10px] font-semibold tracking-wider text-[var(--color-text-muted)] uppercase">
              Pinned
            </span>
            <span className="text-[10px] text-[var(--color-text-muted)] tabular-nums px-1.5 py-0.5 bg-white/[0.06] font-mono">
              {totalCount}
            </span>
            <ShortcutRangeHint combo={useTerminalSettingsStore.getState().shortcutLayout === 'cmd-active-cmdshift-pinned' ? '⌥⌘ 1-9' : '⌘ 1-9'} />
            <span className="flex-1" />
            <svg
              className="w-2.5 h-2.5 text-[var(--color-text-muted)] flex-shrink-0"
              style={{ transition: 'transform 0.2s ease', transform: agentsPinnedCollapsed ? 'rotate(0deg)' : 'rotate(90deg)' }}
              fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2.5}
            >
              <path strokeLinecap="round" strokeLinejoin="round" d="M9 5l7 7-7 7" />
            </svg>
          </button>
          <div
            id="sidebar-agents-pinned-content"
            style={{
              overflow: 'hidden',
              maxHeight: agentsPinnedCollapsed ? 0 : 1200,
              transition: 'max-height 0.2s ease',
            }}
          >
            <div style={useScroll ? { maxHeight: SCROLL_MAX_HEIGHT, overflowY: 'auto' } : undefined}>
          {/* Pinned zone — reorderable. 0.39.0: includes ALL pinned
              workspaces regardless of agentMode (the separate Agents
              zone above this was retired). */}
          <div ref={pinnedRef}>
            {(() => {
              let flatIdx = 0
              return pinnedProjects.map((project, idx) => {
                const myStartIdx = flatIdx
                flatIdx += 1
                return (
              <div
                key={project.id}
                data-project-id={project.id}
                style={{ opacity: rowDrag.isDragging('pinned', idx) ? 0.4 : 1 }}
                onMouseDown={(e) => handleProjectMouseDown(e, project.id, 'pinned', idx)}
                onClickCapture={rowDrag.onRowClickCapture}
              >
                {rowDrag.showsDropLineAt('pinned', idx) && <RowDropLine />}
                <div>
                  <SingleProjectItem
                    project={project}
                    isActive={project.id === activeProjectId}
                    onContextMenu={handleContextMenu}
                    shortcutIndex={myStartIdx}
                  />
                </div>
              </div>
                )
              })
            })()}
            {rowDrag.showsDropLineAt('pinned', pinnedProjects.length) && <RowDropLine />}
          </div>
            </div>{/* scroll wrapper */}
          </div>{/* collapse wrapper */}
        </div>
        )
      })()}

      {/* Focus Group Selector (replaces branding area) */}
      <FocusGroupSelector />

      {/* Workspace list */}
      <div className="flex-1 overflow-y-auto overflow-x-hidden py-1">
        <div ref={unpinnedRef}>
          {filteredProjects.map((project, idx) => (
            <div
              key={project.id}
              data-project-id={project.id}
              style={{ opacity: rowDrag.isDragging('unpinned', idx) ? 0.4 : 1 }}
              onMouseDown={(e) => handleProjectMouseDown(e, project.id, 'unpinned', idx)}
              onClickCapture={rowDrag.onRowClickCapture}
            >
              {rowDrag.showsDropLineAt('unpinned', idx) && <RowDropLine />}
              <SingleProjectItem
                project={project}
                isActive={project.id === activeProjectId}
                onContextMenu={handleContextMenu}
              />
              {idx < filteredProjects.length - 1 && !rowDrag.showsDropLineAt('unpinned', idx + 1) && (
                <div className="border-b border-[var(--color-border)]" />
              )}
            </div>
          ))}
          {rowDrag.showsDropLineAt('unpinned', filteredProjects.length) && <RowDropLine />}
        </div>

        {filteredProjects.length === 0 && (
          <div className="px-3 py-6 text-center">
            <p className="text-xs text-[var(--color-text-muted)]">
              {projects.length === 0 ? 'No workspaces yet' : 'No workspaces in this group'}
            </p>
            <p className="text-xs text-[var(--color-text-muted)] mt-1 opacity-60">
              {projects.length === 0 ? 'Add a folder to get started' : 'Assign workspaces in Settings'}
            </p>
          </div>
        )}

        {/* Local Grok Bots (0.45.7): only when the server has Show on. */}
        <GrokBotSidebarSection />
      </div>

      {/* Active workspaces dock */}
      <ActiveBar />

      {/* Workspace limit warning */}
      {!focusGroupsEnabled && projects.length >= 15 && (
        <div className="px-3 py-2 border-t border-[var(--color-border)]">
          <p className="text-[10px] text-[var(--color-status-error-soft)] font-medium leading-snug">
            Too many workspaces without Focus Groups. Enable Focus Groups to organize your workspaces before adding more.{' '}
            <button
              className="underline text-[var(--color-status-error-soft)] hover:text-[var(--color-status-error-bright)] cursor-pointer"
              onClick={() => useSettingsStore.getState().openSettings('projects')}
            >
              Open Settings
            </button>
          </p>
        </div>
      )}

      {/* Add Workspace + collapse the nav */}
      <div className="p-3 border-t border-[var(--color-border)] flex gap-2">
        <button
          className={`no-drag flex-1 flex items-center justify-center gap-2 px-3 py-2 text-xs bg-white/[0.04] transition-colors ${
            !focusGroupsEnabled && projects.length >= 15
              ? 'text-[var(--color-text-muted)] opacity-50 cursor-not-allowed'
              : 'text-[var(--color-text-secondary)] hover:text-[var(--color-text-primary)] hover:bg-white/[0.08]'
          }`}
          onClick={!focusGroupsEnabled && projects.length >= 15 ? undefined : handleAddProject}
          disabled={!focusGroupsEnabled && projects.length >= 15}
        >
          <svg
            className="w-3.5 h-3.5"
            fill="none"
            viewBox="0 0 24 24"
            stroke="currentColor"
            strokeWidth={2}
          >
            <path strokeLinecap="round" strokeLinejoin="round" d="M12 4v16m8-8H4" />
          </svg>
          Add Workspace
        </button>
        <SidebarCollapseButton />
      </div>

      {/* Worktree creation dialog */}
      {worktreeDialog && (
        <WorktreeDialog
          projectId={worktreeDialog.projectId}
          projectPath={worktreeDialog.projectPath}
          open={true}
          onClose={() => setWorktreeDialog(null)}
        />
      )}

    </div>
  )
}

