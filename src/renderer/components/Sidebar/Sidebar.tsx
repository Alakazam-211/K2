import React, { useState, useCallback, useMemo, useRef, useEffect } from 'react'
import { useProjectsStore } from '@/stores/projects'
import { useFocusGroupsStore } from '@/stores/focus-groups'
import { useSettingsStore } from '@/stores/settings'
import { usePresetsStore, parseCommand } from '@/stores/presets'
import { useTabsStore } from '@/stores/tabs'
import { useAssistantStore } from '@/stores/assistant'
import { useToastStore } from '@/stores/toast'
import { useActiveAgentsStore } from '@/stores/active-agents'
import { useTerminalSettingsStore } from '@/stores/terminal-settings'
import { useCommandPaletteStore } from '@/stores/command-palette'
import { useAddWorkspaceDialogStore } from '@/stores/add-workspace-dialog'
import { useRemoveWorkspaceDialogStore } from '@/stores/remove-workspace-dialog'
import { invoke } from '@tauri-apps/api/core'
import { emit } from '@tauri-apps/api/event'
// Plan B — daemon-data writes (nav-visible / project pins / workspace
// delete) go through the host-aware `/cli/*` HTTP layer. Host-only ops
// (open-in-finder/editor, open-focus-window) stay on Tauri `invoke`.
import { daemonCliPost } from '@/lib/daemon-cli'
import { pickWorkspaceFolder } from '@/lib/pick-workspace-folder'
import { showContextMenu } from '@/lib/context-menu'
import { useConnectHostStore } from '@/stores/connect-host'
import { startCloneTo } from '@/lib/start-clone-to'
import { useGitInfo, useGitChanges } from '@/hooks/useGit'
import ResizeHandle from './ResizeHandle'
import WorktreeDialog from './WorktreeDialog'
import ProjectAvatar from './ProjectAvatar'
import SectionItem from './SectionItem'
import FocusGroupDropdown from './FocusGroupDropdown'
import ActiveBar from './ActiveBar'
import { KeyCombo } from '@/components/KeySymbol'

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
  daemonCliPost('workspaces/set-nav-visible', { id: worktreeId, visible }).catch(() => {})
}

export function addNavWorktree(worktreeId: string): void {
  patchWorkspaceNavVisible(worktreeId, true)
}

export function removeNavWorktree(worktreeId: string): void {
  patchWorkspaceNavVisible(worktreeId, false)
}

// ── Worktree git badge (shows changed files count) ──────────────────────────

function WorkspaceGitBadge({ path }: { path?: string }): React.JSX.Element | null {
  const { data } = useGitInfo(path)
  if (!data?.isRepo) return null

  const count = data.changedFiles + data.untrackedFiles
  if (count === 0) return null

  return (
    <span className="ml-auto text-[10px] tabular-nums font-medium px-1.5 py-0.5 bg-yellow-400/10 text-yellow-400 flex-shrink-0">
      {count}
    </span>
  )
}

// ── Worktree status dot (amber pulsing if dirty) ────────────────────────────

function WorkspaceStatusDot({ path }: { path?: string }): React.JSX.Element | null {
  const { data } = useGitInfo(path)
  if (!data?.isRepo) return null

  const count = data.changedFiles + data.untrackedFiles
  if (count === 0) return null

  return (
    <span
      className="status-dot-dirty flex-shrink-0"
      style={{
        width: 6,
        height: 6,
        backgroundColor: '#f59e0b'
      }}
    />
  )
}

// ── Diff stats for single-button workspaces ────────────────────────────────────

function DiffStats({ path }: { path: string }): React.JSX.Element | null {
  const { data: changes } = useGitChanges(path)
  const defaultAgent = useSettingsStore((s) => s.defaultAgent)
  const presets = usePresetsStore((s) => s.presets)
  const [hovered, setHovered] = useState(false)

  const added = changes.filter((f) => f.status === 'added' || f.status === 'untracked').length
  const deleted = changes.filter((f) => f.status === 'deleted').length

  if (added === 0 && deleted === 0) return null

  const handleAiCommit = (e: React.MouseEvent) => {
    e.stopPropagation()

    const preset = presets.find((p) => p.id === defaultAgent)
    if (!preset) return

    const { command, args } = parseCommand(preset.command)

    const MAX_FILES = 80
    const fileLines = changes.slice(0, MAX_FILES).map((f) => `${f.status}: ${f.path}`)
    if (changes.length > MAX_FILES) {
      fileLines.push(`...and ${changes.length - MAX_FILES} more files`)
    }

    const prompt = `Review the following changes in this repository and create a well-structured commit with an appropriate commit message.\n\nChanged files:\n${fileLines.join('\n')}`

    const tabsStore = useTabsStore.getState()
    const activeGroup = tabsStore.activeGroupIndex
    tabsStore.addTabToGroup(activeGroup, path, {
      title: 'AI Commit',
      command,
      args: [...args, prompt]
    })
  }

  return (
    <span
      className="flex items-center gap-1 text-[10px] tabular-nums font-medium flex-shrink-0 px-1.5 py-0.5 bg-white/[0.06] font-mono cursor-pointer"
      onMouseEnter={() => setHovered(true)}
      onMouseLeave={() => setHovered(false)}
      onClick={handleAiCommit}
    >
      {hovered ? (
        <span className="text-[var(--color-accent)] whitespace-nowrap">AI Commit</span>
      ) : (
        <>
          {added > 0 && <span className="text-green-400">+{added}</span>}
          {deleted > 0 && <span className="text-red-400">-{deleted}</span>}
        </>
      )}
    </span>
  )
}

// ── Agent status or diff stats (shows spinner when agent is working) ─────────

/** Shows braille spinner or "done" label — for placement next to shortcut numbers */
function AgentSpinner({ projectId }: { projectId: string }): React.JSX.Element | null {
  const projectStatus = useActiveAgentsStore((s) => s.getProjectStatus(projectId))

  if (projectStatus === 'working' || projectStatus === 'permission') {
    return (
      <span className={`flex-shrink-0 text-[11px] font-mono ${
        projectStatus === 'permission' ? 'text-red-400' : 'text-[var(--color-text-muted)]'
      }`}>
        <span className="braille-spinner" />
      </span>
    )
  }

  if (projectStatus === 'review') {
    return (
      <span className="flex-shrink-0 text-[10px] text-green-400 font-mono">done</span>
    )
  }

  return null
}

/** Shows diff stats (always visible, even when agent is running — spinner is on a separate row) */
function AgentOrDiffStats({ projectId, path }: { projectId: string; path: string }): React.JSX.Element | null {
  return <DiffStats path={path} />
}

// ── Aggregated diff stats across all worktrees ──────────────────────────────

function AggregatedDiffStats({
  paths
}: {
  paths: string[]
}): React.JSX.Element | null {
  // We call useGitChanges for each path — this works because the array is stable
  // We render a sub-component per path and aggregate in a wrapper
  return <AggregatedDiffStatsInner paths={paths} />
}

function AggregatedDiffStatsInner({ paths }: { paths: string[] }): React.JSX.Element | null {
  let totalAdded = 0
  let totalDeleted = 0

  for (const path of paths) {
    const { data: changes } = useGitChanges(path)
    totalAdded += changes.filter((f) => f.status === 'added' || f.status === 'untracked').length
    totalDeleted += changes.filter((f) => f.status === 'deleted').length
  }

  if (totalAdded === 0 && totalDeleted === 0) return null

  return (
    <span className="flex items-center gap-1 text-[10px] tabular-nums font-medium flex-shrink-0">
      {totalAdded > 0 && <span className="text-green-400">+{totalAdded}</span>}
      {totalDeleted > 0 && <span className="text-red-400">-{totalDeleted}</span>}
    </span>
  )
}

// ── Ahead/Behind indicators ──────────────────────────────────────────────────

function AheadBehind({ path }: { path: string }): React.JSX.Element | null {
  const { data } = useGitInfo(path)
  if (!data?.isRepo) return null
  if (data.ahead === 0 && data.behind === 0) return null

  return (
    <span className="flex items-center gap-1 text-[10px] tabular-nums font-medium flex-shrink-0">
      {data.ahead > 0 && <span className="text-green-400">{'\u2191'}{data.ahead}</span>}
      {data.behind > 0 && <span className="text-red-400">{'\u2193'}{data.behind}</span>}
    </span>
  )
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
            className="flex-shrink-0 flex h-4 w-4 items-center justify-center text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)] hover:bg-white/10 transition-colors"
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

function SingleProjectItem({
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
  const { data: gitInfo } = useGitInfo(isActive ? project.path : undefined)

  const firstWorkspace = project.workspaces[0]
  const isItemActive = isActive && firstWorkspace && activeWorkspaceId === firstWorkspace.id

  // Worktrees visible in the nav for this project (DB-backed via navVisible field)
  const visibleWorktrees = useMemo(() =>
    project.workspaces.filter((ws) => ws.type === 'worktree' && ws.navVisible === 1),
    [project.workspaces]
  )

  // Branch name for worktree-mode projects
  const worktreeBranchName = useMemo(() => {
    if (!project.worktreeMode) return null
    return gitInfo?.isRepo ? gitInfo.currentBranch : null
  }, [project.worktreeMode, gitInfo])

  const handleClick = useCallback(() => {
    if (firstWorkspace) {
      setActiveWorkspace(project.id, firstWorkspace.id)
    }
  }, [project.id, firstWorkspace, setActiveWorkspace])

  return (
    <div className="no-drag">
      <button
        className={`w-full flex items-stretch gap-2.5 px-3 py-2 text-left text-sm transition-colors ${
          isItemActive
            ? 'bg-white/[0.06] text-[var(--color-text-primary)]'
            : 'text-[var(--color-text-secondary)] hover:bg-white/[0.04] hover:text-[var(--color-text-primary)]'
        }`}
        style={{
          borderLeft: isItemActive ? `2px solid ${project.color}` : '2px solid transparent'
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
            <span className="truncate flex-1">{project.name}</span>
            <AgentOrDiffStats projectId={project.id} path={project.path} />
          </div>
          <div className="flex items-center gap-1">
            {gitInfo?.isRepo && gitInfo.currentBranch && (
              <span
                className="text-[10px] font-mono text-[var(--color-text-muted)] truncate flex-1"
                title={gitInfo.currentBranch}
              >
                {gitInfo.currentBranch}
              </span>
            )}
            {!(gitInfo?.isRepo && gitInfo.currentBranch) && <span className="flex-1" />}
            <AgentSpinner projectId={project.id} />
            {shortcutIndex !== undefined && shortcutIndex < 9 && (
              <span className="text-[10px] font-mono text-[var(--color-text-muted)] tabular-nums flex-shrink-0 py-0.5" style={{ paddingLeft: 8, paddingRight: 8 }}>
                {shortcutIndex + 1}
              </span>
            )}
          </div>
        </div>
      </button>
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
          {isWorktree && <WorkspaceStatusDot path={workspacePath} />}

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

          <AgentOrDiffStats projectId={workspace.projectId} path={workspacePath} />
          <AgentSpinner projectId={workspace.projectId} />
          {shortcutIndex !== undefined && shortcutIndex < 9 && (
            <span className="text-[10px] font-mono text-[var(--color-text-muted)] tabular-nums flex-shrink-0 py-0.5" style={{ paddingLeft: 8, paddingRight: 8 }}>
              {shortcutIndex + 1}
            </span>
          )}
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
  const assignWorkspaceToSection = useProjectsStore((s) => s.assignWorkspaceToSection)
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

      const ws = project.workspaces.find((w) => w.id === workspaceId)
      if (!ws) return

      const sections = project.sections || []
      const isWorktree = ws.type === 'worktree'
      const workspacePath = ws.worktreePath ?? project.path

      // Fetch installed editors for the submenu
      let editors: Array<{ id: string; label: string }> = []
      try {
        editors = await invoke<Array<{ id: string; label: string }>>('projects_get_editors')
      } catch {
        // ignore
      }

      const menuItems: Array<{ id: string; label: string; type?: string }> = []

      // Open in Finder
      menuItems.push({ id: 'ws-open-finder', label: 'Open in Finder' })

      // Open in Editor submenu
      if (editors.length > 0) {
        menuItems.push({ id: 'ws-editor-sep', label: '', type: 'separator' })
        for (const editor of editors) {
          menuItems.push({ id: `ws-editor:${editor.id}`, label: `Open in ${editor.label}` })
        }
      }

      // Move to Section (only if sections exist)
      if (sections.length > 0) {
        menuItems.push({ id: 'move-header', label: '', type: 'separator' })
        for (const sec of sections) {
          const isCurrent = ws.sectionId === sec.id
          menuItems.push({
            id: `section:${sec.id}`,
            label: `${sec.name}${isCurrent ? ' *' : ''}`
          })
        }
        menuItems.push({ id: 'section:none', label: 'No Section' })
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
      } else if (clickedId?.startsWith('ws-editor:')) {
        const editorId = clickedId.replace('ws-editor:', '')
        await invoke('projects_open_in_editor', { editorId, path: workspacePath })
      } else if (clickedId === 'section:none') {
        await assignWorkspaceToSection(workspaceId, null)
      } else if (clickedId?.startsWith('section:')) {
        const sectionId = clickedId.replace('section:', '')
        await assignWorkspaceToSection(workspaceId, sectionId)
      } else if (clickedId === 'ws-close') {
        // Prevent closing the last worktree
        if (project.workspaces.length <= 1) {
          await showContextMenu([
            { id: 'error', label: 'Cannot close the last worktree' }
          ])
          return
        }
        // Remove from DB only, keep files on disk. workspaces_delete
        // emitted no cross-window sync; fetchProjects refreshes locally.
        await daemonCliPost('workspaces/delete', { id: workspaceId })
        await fetchProjects()
      } else if (clickedId === 'ws-recycle') {
        // Prevent recycling the last worktree
        if (project.workspaces.length <= 1) {
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
          await daemonCliPost('git/remove-worktree', {
            worktreePath: workspacePath,
            projectPath: project.path,
            workspaceId: workspaceId
          })
          await fetchProjects()
        }
      }
    },
    [project, assignWorkspaceToSection, fetchProjects]
  )

  // Collect worktree paths for aggregated diff stats
  const workspacePaths = project.workspaces.map(
    (ws) => ws.worktreePath ?? project.path
  )

  // Split worktrees into ungrouped and grouped by section
  const ungroupedWorkspaces = project.workspaces.filter((ws) => !ws.sectionId)
  const sections = project.sections || []

  // Only poll git info for the active project to avoid hammering git on all repos
  const { data: gitInfo } = useGitInfo(isActive ? project.path : undefined)

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
              {project.workspaces.length}
            </span>
          </div>
          {gitInfo?.isRepo && gitInfo.currentBranch && (
            <span
              className="text-[10px] font-mono text-[var(--color-text-muted)] truncate"
              title={gitInfo.currentBranch}
            >
              {gitInfo.currentBranch}
            </span>
          )}
        </div>
        {/* New worktree + expand/collapse controls */}
        <div className="flex flex-col items-center justify-center gap-0.5 flex-shrink-0">
          <span
            onClick={(e) => {
              e.stopPropagation()
              setShowWorktreeDialog(true)
              if (!isExpanded) setIsExpanded(true)
            }}
            className="w-5 h-5 flex items-center justify-center text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)] hover:bg-white/[0.1] transition-colors cursor-pointer"
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
          {/* Ungrouped worktrees first */}
          {ungroupedWorkspaces.map((workspace, wsIdx) => (
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

          {/* Sections with their worktrees */}
          {sections.map((section) => {
            const sectionWorkspaces = project.workspaces.filter(
              (ws) => ws.sectionId === section.id
            )

            return (
              <SectionItem
                key={section.id}
                section={section}
                workspaces={sectionWorkspaces}
                projectPath={project.path}
                activeWorkspaceId={activeWorkspaceId}
                onWorkspaceClick={handleWorkspaceClick}
                onWorkspaceContextMenu={(e, wsId) => handleWorkspaceContextMenu(e, wsId)}
              />
            )
          })}
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

function FocusGroupSelector(): React.JSX.Element {
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
          options={focusGroups.map((g) => ({ id: g.id, name: g.name, color: g.color }))}
          value={activeFocusGroupId}
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

export default function Sidebar(): React.JSX.Element {
  const projects = useProjectsStore((s) => s.projects)
  const activeProjectId = useProjectsStore((s) => s.activeProjectId)
  const removeProject = useProjectsStore((s) => s.removeProject)
  const addProject = useProjectsStore((s) => s.addProject)
  const renameProject = useProjectsStore((s) => s.renameProject)
  const fetchProjects = useProjectsStore((s) => s.fetchProjects)
  const createSection = useProjectsStore((s) => s.createSection)

  const reorderProjects = useProjectsStore((s) => s.reorderProjects)
  const focusGroupsEnabled = useFocusGroupsStore((s) => s.focusGroupsEnabled)
  const activeFocusGroupId = useFocusGroupsStore((s) => s.activeFocusGroupId)

  const agenticEnabled = useSettingsStore((s) => s.agenticSystemsEnabled)

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

  const filteredProjects = useMemo(() => {
    const unpinned = projects.filter((p) => !p.pinned)
    if (!focusGroupsEnabled || activeFocusGroupId === null) return unpinned
    // 0.39.45 (GH #26): ungrouped projects (focusGroupId = null — e.g.
    // CLI-created via `k2so workspace create`, or onboarded against a
    // focus-group name that doesn't exist) render in EVERY group view.
    // Filtering them out made them unreachable from the nav entirely.
    return unpinned.filter(
      (p) => p.focusGroupId === activeFocusGroupId || p.focusGroupId == null,
    )
  }, [projects, focusGroupsEnabled, activeFocusGroupId])

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

  // ── Drag-to-reorder state ──────────────────────────────────────────
  const [dragId, setDragId] = useState<string | null>(null)
  const [dragZone, setDragZone] = useState<'pinned' | 'unpinned' | null>(null)
  const [dropIndex, setDropIndex] = useState<number | null>(null)
  // 0.39.0: agentsRef retired — Agents drag zone no longer exists.
  const pinnedRef = useRef<HTMLDivElement>(null)
  const unpinnedRef = useRef<HTMLDivElement>(null)
  const dropIndexRef = useRef<number | null>(null)

  const handleProjectMouseDown = useCallback((
    e: React.MouseEvent,
    projectId: string,
    zone: 'pinned' | 'unpinned'
  ) => {
    if (e.button !== 0) return
    if ((e.target as HTMLElement).closest('button')) return

    const startX = e.clientX
    const startY = e.clientY
    let started = false

    const handleMouseMove = (ev: MouseEvent): void => {
      if (!started && (Math.abs(ev.clientX - startX) > 3 || Math.abs(ev.clientY - startY) > 5)) {
        started = true
        setDragId(projectId)
        setDragZone(zone)
        document.body.style.cursor = 'grabbing'
        document.body.style.userSelect = 'none'
      }

      if (!started) return

      // Determine drop index based on mouse Y
      const containerEl = (zone === 'pinned' ? pinnedRef : unpinnedRef).current
      if (!containerEl) return

      const items = containerEl.querySelectorAll('[data-project-id]')
      let idx = 0
      for (let i = 0; i < items.length; i++) {
        const rect = items[i].getBoundingClientRect()
        if (ev.clientY > rect.top + rect.height / 2) {
          idx = i + 1
        }
      }
      dropIndexRef.current = idx
      setDropIndex(idx)
    }

    const handleMouseUp = (): void => {
      document.removeEventListener('mousemove', handleMouseMove)
      document.removeEventListener('mouseup', handleMouseUp)
      document.body.style.cursor = ''
      document.body.style.userSelect = ''

      if (started) {
        const allProjects = useProjectsStore.getState().projects
        // 0.39.0: 'agents' drag zone retired; pinned zone now accepts ALL
        // pinned workspaces regardless of agentMode.
        const list = zone === 'pinned'
          ? [...allProjects.filter((p) => p.pinned)]
          : [...allProjects.filter((p) => !p.pinned)]
        const di = dropIndexRef.current
        const fromIdx = list.findIndex((p) => p.id === projectId)
        if (fromIdx >= 0 && di !== null && fromIdx !== di && fromIdx !== di - 1) {
          const item = list.splice(fromIdx, 1)[0]
          const insertAt = di > fromIdx ? di - 1 : di
          list.splice(insertAt, 0, item)
          reorderProjects(list.map((p) => p.id))
        }
      }

      setDragId(null)
      setDragZone(null)
      setDropIndex(null)
      dropIndexRef.current = null
    }

    document.addEventListener('mousemove', handleMouseMove)
    document.addEventListener('mouseup', handleMouseUp)
  }, [reorderProjects])

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
      onConfirm: async () => {
        await addProject(folderPath)
        try {
          await daemonCliPost('agents/run-workspace-ingest', { project_path: folderPath })
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
        editors = await invoke<Array<{ id: string; label: string }>>('projects_get_editors')
      } catch {
        // ignore
      }

      // Build menu items -- add worktree option if worktreeMode is enabled
      const menuItems: Array<{
        id: string
        label: string
        type?: string
      }> = [
        { id: 'settings', label: 'Workspace Settings' },
        { id: 'separator-settings', label: '', type: 'separator' },
        { id: 'rename', label: 'Rename' },
        { id: 'open-finder', label: 'Open in Finder' },
        { id: 'focus-window', label: 'Open in Focus Window' }
      ]

      // Add editor items
      if (editors.length > 0) {
        menuItems.push({ id: 'separator-editors', label: '', type: 'separator' })
        for (const editor of editors) {
          menuItems.push({ id: `editor:${editor.id}`, label: `Open in ${editor.label}` })
        }
      }

      menuItems.push(
        { id: 'separator-wt', label: '', type: 'separator' },
        { id: 'new-worktree', label: 'New Worktree...' },
        { id: 'new-section', label: 'New Section...' }
      )

      menuItems.push(
        { id: 'separator-pin', label: '', type: 'separator' },
        { id: 'toggle-pin', label: project.pinned ? 'Unpin' : 'Pin to Top' },
        { id: 'toggle-active', label: project.manuallyActive ? 'Remove from Active Bar' : 'Add to Active Bar' },
        ...(!project.manuallyActive ? [{ id: 'active-24h', label: 'Active for 24hrs' }] : [])
      )

      // "Clone to <host>" — migrate this LOCAL workspace onto a connected K2
      // Connect host. Only offered when we're on the local daemon (the
      // orchestration bundles + reads from the local machine first).
      const { activeHost, hosts } = useConnectHostStore.getState()
      if (activeHost === 'local' && hosts.length > 0) {
        menuItems.push({ id: 'separator-clone', label: '', type: 'separator' })
        for (const host of hosts) {
          menuItems.push({ id: `clone-to:${host.id}`, label: `Clone to ${host.label}` })
        }
      }

      menuItems.push(
        { id: 'separator', label: '', type: 'separator' },
        { id: 'remove', label: 'Remove Workspace' }
      )

      const clickedId = await showContextMenu(menuItems)

      if (clickedId === 'settings') {
        useSettingsStore.getState().openSettings('projects', project.id)
      } else if (clickedId?.startsWith('clone-to:')) {
        const hostId = clickedId.replace('clone-to:', '')
        const host = useConnectHostStore.getState().hosts.find((h) => h.id === hostId)
        if (host) void startCloneTo(project.path, project.name, host)
      } else if (clickedId === 'rename') {
        const newName = window.prompt('Rename workspace:', project.name)
        if (newName && newName.trim() && newName !== project.name) {
          await renameProject(projectId, newName.trim())
        }
      } else if (clickedId === 'open-finder') {
        await invoke('projects_open_in_finder', { path: project.path })
      } else if (clickedId === 'focus-window') {
        await invoke('projects_open_focus_window', { projectId: project.id })
      } else if (clickedId?.startsWith('editor:')) {
        const editorId = clickedId.replace('editor:', '')
        await invoke('projects_open_in_editor', { editorId, path: project.path })
      } else if (clickedId === 'new-worktree') {
        setWorktreeDialog({ projectId: project.id, projectPath: project.path })
      } else if (clickedId === 'new-section') {
        const sectionName = window.prompt('Section name:')
        if (sectionName && sectionName.trim()) {
          await createSection(project.id, sectionName.trim())
        }
      } else if (clickedId === 'toggle-pin') {
        await daemonCliPost('projects/update', { id: projectId, pinned: project.pinned ? 0 : 1 })
        void emit('sync:projects').catch(() => {})
        await fetchProjects()
      } else if (clickedId === 'toggle-active') {
        await daemonCliPost('projects/update', { id: projectId, manuallyActive: project.manuallyActive ? 0 : 1 })
        void emit('sync:projects').catch(() => {})
        await fetchProjects()
      } else if (clickedId === 'active-24h') {
        // Set lastInteractionAt to now — the Active Bar keeps projects with
        // interaction < 24hrs. projects_touch_interaction emitted no sync.
        await daemonCliPost('projects/touch-interaction', { id: projectId })
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
    [projects, renameProject, fetchProjects, createSection]
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
            <span className="text-[9px] font-mono text-[var(--color-text-muted)] opacity-50">
              <KeyCombo combo={useTerminalSettingsStore.getState().shortcutLayout === 'cmd-active-cmdshift-pinned' ? '⌥⌘ 1-9' : '⌘ 1-9'} />
            </span>
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
                style={{ opacity: dragId === project.id ? 0.4 : 1 }}
                onMouseDown={(e) => handleProjectMouseDown(e, project.id, 'pinned')}
              >
                {dragZone === 'pinned' && dropIndex === idx && (
                  <div className="h-[2px] bg-[var(--color-accent)] mx-3" />
                )}
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
            {dragZone === 'pinned' && dropIndex === pinnedProjects.length && (
              <div className="h-[2px] bg-[var(--color-accent)] mx-3" />
            )}
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
              style={{ opacity: dragId === project.id ? 0.4 : 1 }}
              onMouseDown={(e) => handleProjectMouseDown(e, project.id, 'unpinned')}
            >
              {dragZone === 'unpinned' && dropIndex === idx && (
                <div className="h-[2px] bg-[var(--color-accent)] mx-3" />
              )}
              <SingleProjectItem
                project={project}
                isActive={project.id === activeProjectId}
                onContextMenu={handleContextMenu}
              />
              {idx < filteredProjects.length - 1 && !(dragZone === 'unpinned' && dropIndex === idx + 1) && (
                <div className="border-b border-[var(--color-border)]" />
              )}
            </div>
          ))}
          {dragZone === 'unpinned' && dropIndex === filteredProjects.length && (
            <div className="h-[2px] bg-[var(--color-accent)] mx-3" />
          )}
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
      </div>

      {/* Active workspaces dock */}
      <ActiveBar />

      {/* Workspace limit warning */}
      {!focusGroupsEnabled && projects.length >= 15 && (
        <div className="px-3 py-2 border-t border-[var(--color-border)]">
          <p className="text-[10px] text-red-400 font-medium leading-snug">
            Too many workspaces without Focus Groups. Enable Focus Groups to organize your workspaces before adding more.{' '}
            <button
              className="underline text-red-400 hover:text-red-300 cursor-pointer"
              onClick={() => useSettingsStore.getState().openSettings('projects')}
            >
              Open Settings
            </button>
          </p>
        </div>
      )}

      {/* Add Workspace + Assistant buttons */}
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
        <button
          className="no-drag flex items-center gap-1.5 px-2.5 py-2 text-xs text-[var(--color-text-secondary)] hover:text-[var(--color-text-primary)] bg-white/[0.04] hover:bg-white/[0.08] transition-colors"
          onClick={() => useAssistantStore.getState().toggle()}
          title="Toggle Assistant (⌘L)"
        >
          <svg className="w-3.5 h-3.5" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth={2} strokeLinecap="round" strokeLinejoin="round">
            <polyline points="4 17 10 11 4 5" />
            <line x1="12" y1="19" x2="20" y2="19" />
          </svg>
        </button>
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

