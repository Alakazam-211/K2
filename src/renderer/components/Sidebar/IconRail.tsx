import { useCallback, useMemo, useState, useEffect } from 'react'
import { useProjectsStore, type ProjectWithWorkspaces } from '../../stores/projects'
import { useFocusGroupsStore } from '../../stores/focus-groups'
import { useSidebarStore } from '../../stores/sidebar'
import { useSettingsStore } from '../../stores/settings'
import { useActiveAgentsStore } from '../../stores/active-agents'
import { useTabsStore } from '../../stores/tabs'
import { useCommandPaletteStore } from '../../stores/command-palette'
import { useAddWorkspaceDialogStore } from '../../stores/add-workspace-dialog'
import { useRemoveWorkspaceDialogStore } from '../../stores/remove-workspace-dialog'
import { useGitInfo, useGitChanges } from '../../hooks/useGit'
import { invoke } from '@tauri-apps/api/core'
import { daemonCliPost } from '@/lib/daemon-cli'
import { showContextMenu } from '../../lib/context-menu'
import { pickWorkspaceFolder } from '../../lib/pick-workspace-folder'
import { useConnectHostStore } from '../../stores/connect-host'
import { startCloneTo } from '../../lib/start-clone-to'
import ProjectAvatar from './ProjectAvatar'

const RAIL_WIDTH = 48

function ProjectIcon({
  project,
  isActive,
  onClick,
  onContextMenu,
  shortcutIndex
}: {
  project: ProjectWithWorkspaces
  isActive: boolean
  onClick: () => void
  onContextMenu: (e: React.MouseEvent) => void
  shortcutIndex?: number
}): React.JSX.Element {
  const { data: gitInfo } = useGitInfo(project.path)
  const { data: changes } = useGitChanges(project.path)
  const agentStatus = useActiveAgentsStore((s) => s.getProjectStatus(project.id))

  const hasDirtyFiles =
    gitInfo?.isRepo && (gitInfo.changedFiles + gitInfo.untrackedFiles) > 0

  const added = changes.filter(
    (f) => f.status === 'added' || f.status === 'untracked'
  ).length
  const deleted = changes.filter((f) => f.status === 'deleted').length

  // Build tooltip: "WorkspaceName • branch • +N/-N changes"
  const tooltipParts = [project.name]
  if (gitInfo?.isRepo && gitInfo.currentBranch) {
    tooltipParts.push(gitInfo.currentBranch)
  }
  if (added > 0 || deleted > 0) {
    const diffParts: string[] = []
    if (added > 0) diffParts.push(`+${added}`)
    if (deleted > 0) diffParts.push(`-${deleted}`)
    tooltipParts.push(`${diffParts.join('/')} changes`)
  }
  const tooltip = tooltipParts.join(' \u2022 ')

  return (
    <button
      className={`no-drag relative flex items-center justify-center w-8 h-8 flex-shrink-0 transition-colors ${
        isActive
          ? 'bg-white/[0.12] text-[var(--color-text-primary)] icon-rail-active'
          : 'text-[var(--color-text-muted)] hover:bg-white/[0.06] hover:text-[var(--color-text-secondary)]'
      }`}
      style={
        isActive
          ? ({ '--accent-color': project.color } as React.CSSProperties)
          : undefined
      }
      onClick={onClick}
      onContextMenu={onContextMenu}
      title={tooltip}
    >
      <ProjectAvatar
        projectPath={project.path}
        projectName={project.name}
        projectColor={project.color}
        projectId={project.id}
        iconUrl={project.iconUrl}
        size={20}
      />
      {agentStatus === 'working' && <span className="icon-rail-badge agent-dot-working" />}
      {agentStatus === 'permission' && <span className="icon-rail-badge agent-dot-permission" />}
      {agentStatus === 'review' && <span className="icon-rail-badge agent-dot-review" />}
      {agentStatus === 'idle' && hasDirtyFiles && <span className="icon-rail-badge status-dot-dirty" />}
      {shortcutIndex !== undefined && shortcutIndex <= 9 && (
        <span className="absolute bottom-0 right-0.5 text-[7px] font-mono text-[var(--color-text-muted)] opacity-50 leading-none">
          {shortcutIndex}
        </span>
      )}
    </button>
  )
}

const TWENTY_FOUR_HOURS = 24 * 60 * 60

export default function IconRail(): React.JSX.Element {
  const projects = useProjectsStore((s) => s.projects)
  const activeProjectId = useProjectsStore((s) => s.activeProjectId)
  const setActiveProject = useProjectsStore((s) => s.setActiveProject)
  const removeProject = useProjectsStore((s) => s.removeProject)
  const addProject = useProjectsStore((s) => s.addProject)

  const expand = useSidebarStore((s) => s.expand)

  const focusGroupsEnabled = useFocusGroupsStore((s) => s.focusGroupsEnabled)
  const activeFocusGroupId = useFocusGroupsStore((s) => s.activeFocusGroupId)
  const backgroundWorkspaces = useTabsStore((s) => s.backgroundWorkspaces)
  const hasActiveAgents = useActiveAgentsStore((s) => s.hasActiveAgents())
  const paneStatuses = useActiveAgentsStore((s) => s.paneStatuses)

  // Zone 1: Pinned (manually pinned by the user). Pre-0.39.0 we
  // auto-pinned every agent-mode workspace (Custom + K2 Agent) to
  // the top regardless of user intent. 0.39.0 retires that — users
  // can still pin any workspace including agent-mode ones, but the
  // forced top placement is gone. Unpinned agent-mode workspaces
  // flow through Zone 2 / Zone 3 like any other workspace.
  const pinnedProjects = useMemo(() =>
    projects.filter((p) => p.pinned), [projects])
  const topSection = pinnedProjects

  // Zone 2: Workspaces in current focus group (unpinned). Post-0.39.0
  // unpinned agent-mode workspaces appear here, not force-pinned to
  // Zone 1.
  const filteredProjects = useMemo(() => {
    const unpinned = projects.filter((p) => !p.pinned)
    if (!focusGroupsEnabled || activeFocusGroupId === null) return unpinned
    // 0.39.45 (GH #26): ungrouped projects (focusGroupId = null) render
    // in EVERY group view — filtering them out made CLI-created
    // workspaces unreachable from the nav entirely.
    return unpinned.filter(
      (p) => p.focusGroupId === activeFocusGroupId || p.focusGroupId == null,
    )
  }, [projects, focusGroupsEnabled, activeFocusGroupId])

  // Zone 3: Active. Workspaces with recent activity — post-0.39.0
  // agent-mode workspaces are eligible (no longer auto-shunted to
  // Zone 1).
  const [tick, setTick] = useState(0)
  useEffect(() => {
    const interval = setInterval(() => setTick((t) => t + 1), 60000)
    return () => clearInterval(interval)
  }, [])
  const activeProjects = useMemo(() => {
    const now = Math.floor(Date.now() / 1000)
    const hasHookActivity = paneStatuses.size > 0 && Array.from(paneStatuses.values()).some(
      (s) => s === 'working' || s === 'permission' || s === 'review'
    )
    // Exclude pinned (Zone 1). Agent-mode is no longer a force-exclude.
    const pinnedIds = new Set(pinnedProjects.map((p) => p.id))
    return projects.filter((p) => {
      if (pinnedIds.has(p.id)) return false
      if (p.manuallyActive) return true
      if (p.lastInteractionAt && (now - p.lastInteractionAt) < TWENTY_FOUR_HOURS) return true
      if (p.id === activeProjectId && (hasActiveAgents || hasHookActivity)) return true
      const hasBackground = Object.keys(backgroundWorkspaces).some((key) => key.startsWith(`${p.id}:`))
      if (hasBackground) return true
      return false
    })
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [projects, pinnedProjects, activeProjectId, hasActiveAgents, paneStatuses, backgroundWorkspaces, tick])

  const handleAddProject = useCallback(async () => {
    const folderPath = await pickWorkspaceFolder()
    if (!folderPath) return
    // Harness fan-out is off by default (canonical-agents PRD §4), so the
    // add has no destructive effects — no consent preview needed. The
    // confirmation dialog simply names the workspace before committing.
    useAddWorkspaceDialogStore.getState().open({
      path: folderPath,
      onConfirm: async () => {
        await addProject(folderPath)
        // Trigger the skill write immediately so the user sees the effect
        // of the migration without having to restart K2.
        try {
          await daemonCliPost('agents/run-workspace-ingest', { project_path: folderPath })
        } catch (err) {
          console.warn('[add-workspace] run ingest failed:', err)
        }
      },
    })
  }, [addProject])

  const handleProjectClick = useCallback(
    (projectId: string) => {
      setActiveProject(projectId)
    },
    [setActiveProject]
  )

  const handleContextMenu = useCallback(
    async (e: React.MouseEvent, projectId: string) => {
      e.preventDefault()
      const project = projects.find((p) => p.id === projectId)
      if (!project) return

      // Fetch installed editors
      let editors: Array<{ id: string; label: string }> = []
      try {
        editors = await invoke<Array<{ id: string; label: string }>>('projects_get_editors')
      } catch {
        // ignore
      }

      const menuItems: Array<{ id: string; label: string; type?: string }> = [
        { id: 'settings', label: 'Workspace Settings' },
        { id: 'separator-settings', label: '', type: 'separator' },
        { id: 'expand', label: 'Expand Sidebar' },
        { id: 'separator', label: '', type: 'separator' },
        { id: 'open-finder', label: 'Open in Finder' },
        { id: 'focus-window', label: 'Open in Focus Window' }
      ]

      if (editors.length > 0) {
        menuItems.push({ id: 'separator-editors', label: '', type: 'separator' })
        for (const editor of editors) {
          menuItems.push({ id: `editor:${editor.id}`, label: `Open in ${editor.label}` })
        }
      }

      // "Clone to <host>" — migrate this LOCAL workspace onto a connected K2
      // Connect host. Only offered when we're currently on the local daemon
      // (the orchestration bundles + reads from the local machine first).
      const { activeHost, hosts } = useConnectHostStore.getState()
      if (activeHost === 'local' && hosts.length > 0) {
        menuItems.push({ id: 'separator-clone', label: '', type: 'separator' })
        for (const host of hosts) {
          menuItems.push({ id: `clone-to:${host.id}`, label: `Clone to ${host.label}` })
        }
      }

      menuItems.push(
        { id: 'separator2', label: '', type: 'separator' },
        { id: 'remove', label: 'Remove Workspace' }
      )

      const clickedId = await showContextMenu(menuItems)

      if (clickedId === 'settings') {
        useSettingsStore.getState().openSettings('projects', projectId)
      } else if (clickedId?.startsWith('clone-to:')) {
        const hostId = clickedId.replace('clone-to:', '')
        const host = useConnectHostStore.getState().hosts.find((h) => h.id === hostId)
        if (host) void startCloneTo(project.path, project.name, host)
      } else if (clickedId === 'expand') {
        expand()
      } else if (clickedId === 'open-finder') {
        await invoke('projects_open_in_finder', { path: project.path })
      } else if (clickedId === 'focus-window') {
        await invoke('projects_open_focus_window', { projectId: project.id })
      } else if (clickedId?.startsWith('editor:')) {
        const editorId = clickedId.replace('editor:', '')
        await invoke('projects_open_in_editor', { editorId, path: project.path })
      } else if (clickedId === 'remove') {
        // Open the teardown confirmation dialog; it handles the actual
        // removeProject call after the user picks a mode (or deregister-only).
        useRemoveWorkspaceDialogStore.getState().open({
          projectId,
          projectName: project.name,
          projectPath: project.path,
        })
      }
    },
    [projects, expand]
  )

  return (
    <div
      className="flex flex-col items-center h-full bg-[var(--color-bg-surface)] border-r border-[var(--color-border)] py-2 flex-shrink-0"
      style={{ width: RAIL_WIDTH }}
    >
      {/* Zone 1: Pinned (0.39.0: agent-mode workspaces flow through here
          as manual pins, not auto-promoted as a separate section) */}
      {topSection.length > 0 && (
        <div className="flex flex-col items-center gap-0.5 pb-1.5 w-full">
          {topSection.map((project, i) => (
            <ProjectIcon
              key={project.id}
              project={project}
              isActive={project.id === activeProjectId}
              onClick={() => handleProjectClick(project.id)}
              onContextMenu={(e) => handleContextMenu(e, project.id)}
              shortcutIndex={i + 1}
            />
          ))}
          <div className="w-6 border-b border-[var(--color-border)] mt-1" />
        </div>
      )}

      {/* Search / Focus Group button */}
      <button
        className="no-drag flex items-center justify-center w-8 h-8 flex-shrink-0 text-[var(--color-text-muted)] hover:text-[var(--color-text-secondary)] hover:bg-white/[0.06] transition-colors mb-1"
        onClick={() => useCommandPaletteStore.getState().toggle()}
        title="Search Workspaces (⌘K)"
      >
        <svg className="w-3.5 h-3.5" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth={2} strokeLinecap="round" strokeLinejoin="round">
          <circle cx="11" cy="11" r="8" />
          <line x1="21" y1="21" x2="16.65" y2="16.65" />
        </svg>
      </button>

      {/* Zone 2: Workspaces in focus group */}
      <div className="flex-1 flex flex-col items-center gap-0.5 overflow-y-auto overflow-x-hidden w-full">
        {filteredProjects.map((project) => (
          <ProjectIcon
            key={project.id}
            project={project}
            isActive={project.id === activeProjectId}
            onClick={() => handleProjectClick(project.id)}
            onContextMenu={(e) => handleContextMenu(e, project.id)}
          />
        ))}
      </div>

      {/* Zone 3: Active */}
      {activeProjects.length > 0 && (
        <div className="flex flex-col items-center gap-0.5 pt-1.5 w-full">
          <div className="w-6 border-b border-[var(--color-border)] mb-1" />
          {activeProjects.map((project, i) => (
            <ProjectIcon
              key={project.id}
              project={project}
              isActive={project.id === activeProjectId}
              onClick={() => handleProjectClick(project.id)}
              onContextMenu={(e) => handleContextMenu(e, project.id)}
              shortcutIndex={i + 1}
            />
          ))}
        </div>
      )}

      {/* Add workspace button */}
      <button
        className="no-drag flex items-center justify-center w-8 h-8 flex-shrink-0 text-[var(--color-text-muted)] hover:text-[var(--color-text-secondary)] hover:bg-white/[0.06] transition-colors mt-1"
        onClick={handleAddProject}
        title="Add Workspace"
      >
        <svg className="w-4 h-4" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2}>
          <path strokeLinecap="round" strokeLinejoin="round" d="M12 4v16m8-8H4" />
        </svg>
      </button>
    </div>
  )
}
