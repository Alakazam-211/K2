import React from 'react'
import { useEffect, useState, useCallback, useRef, useMemo } from 'react'
import { listen, emit } from '@tauri-apps/api/event'
import { invoke } from '@tauri-apps/api/core'
import { daemonCliGet, daemonCliPost } from '@/lib/daemon-cli'
import { isBuiltinAgentType } from '@/lib/agent-type'
import {
  addRemoteConnection,
  removeRemoteConnection,
  listRemoteConnections,
  listFederationPeers,
  trustedPeers,
  fetchPeerRoster,
  formatAgentHost,
  type RemoteConnectionEntry,
  type FederationPeer,
  type RosterAgent,
} from '@/lib/federation'
import { agentDisplayName, agentHandle, setAgentDisplayName, setAgentHandle } from '@/lib/workspace-agent'
import { useSettingsStore } from '@/stores/settings'
import { useProjectsStore, type ProjectWithWorkspaces } from '@/stores/projects'
import { useFocusGroupsStore } from '@/stores/focus-groups'
import { usePresetsStore, parseCommand } from '@/stores/presets'
import { useResolvedAgentCommand } from '@/hooks/useResolvedAgentCommand'
import { useTabsStore } from '@/stores/tabs'
import { useConfirmDialogStore } from '@/stores/confirm-dialog'
import { pickWorkspaceFolder } from '@/lib/pick-workspace-folder'
import { pickIconImage } from '@/lib/pick-remote-image'
import IconCropDialog from '../IconCropDialog'
import ProjectAvatar from '@/components/Sidebar/ProjectAvatar'
import AgentIcon from '@/components/AgentIcon/AgentIcon'
import { AgentPersonaEditor } from '@/components/AgentPersonaEditor/AgentPersonaEditor'
import { AIFileEditor } from '@/components/AIFileEditor/AIFileEditor'
import { buildEditorAgentArgs } from '@/lib/editor-agent-args'
import Markdown from '@/components/Markdown/Markdown'
import remarkGfm from 'remark-gfm'
import { CodeEditor } from '@/components/FileViewerPane/CodeEditor'
import { CustomThemeCreator } from '../CustomThemeCreator'
import { SettingsGroup, SettingDropdown } from '../controls/SettingControls'
import { showContextMenu } from '@/lib/context-menu'
import { SectionErrorBoundary } from '../SectionErrorBoundary'
import type { SettingEntry } from '../searchManifest'
import { HeartbeatsPanel, HistoryPanel, WakeupEditor, type HeartbeatRow } from './HeartbeatsSection'
import { ContextStackEditor, type ContextEditTarget } from './ContextStackEditor'
import { RoleSkillEditor } from './RoleSkillEditor'
import { CanonicalAgentModal } from './CanonicalAgentModal'
import { type HarnessProbe } from './canonicalState'
import { RoleSkillButton, CanonicalAgentButton } from './CanonicalAgentButtons'
import { WorkspaceApiKeysPanel } from './ApiTokensSection'
import { HideApiSessionsToggle } from '@/components/WorkspacePanel/HideApiSessionsToggle'
import { WorkspaceCompletionSoundToggle } from '@/components/WorkspacePanel/WorkspaceCompletionSoundToggle'
import { workspaceGrantSlug } from './api-keys-api'
import { useTunnelUrls } from '@/hooks/useTunnelUrls'

/**
 * Plan B cross-window sync: the old Tauri `projects_update` /
 * `projects_detect_icon` / `projects_clear_icon` shims emitted
 * `sync:projects` from Rust so other windows re-fetch. Now that these go
 * through the host-aware daemon HTTP layer, re-emit the same event from
 * the renderer after each successful mutation. Fire-and-forget.
 */
function emitProjectsChanged(): void {
  void emit('sync:projects').catch((e) =>
    console.warn('[projects-section] sync:projects emit failed:', e),
  )
}

export const PROJECTS_MANIFEST: SettingEntry[] = [
  { id: 'projects.list', section: 'projects', label: 'Workspaces', description: 'All registered projects + focus groups', keywords: ['workspaces', 'projects', 'focus groups'] },
  { id: 'projects.add', section: 'projects', label: 'Add Workspace', description: 'Register a new project directory', keywords: ['add', 'new', 'workspace', 'project', 'folder'] },
  { id: 'projects.focus-groups', section: 'projects', label: 'Focus Groups', description: 'Organize workspaces into tabbed folders', keywords: ['focus', 'groups', 'tabs'] },
  { id: 'projects.context-stack', section: 'projects', label: 'Always-on context stack', description: 'Toggle ROLE.md / PROJECT.md / layers that compose into AGENTS.md', keywords: ['context', 'stack', 'agents.md', 'project.md', 'persona', 'wiki', 'role.md'] },
  { id: 'projects.context-stack', section: 'projects', label: 'Always-on context (AGENTS.md stack)', description: 'Pinned + optional markdown layers composed into .k2/AGENTS.md', keywords: ['context', 'stack', 'stack', 'agents.md', 'layers', 'wiki', 'always-on'] },
  { id: 'projects.heartbeat', section: 'projects', label: 'Heartbeat Schedule', description: 'Scheduled / hourly / off per-project heartbeat mode', keywords: ['heartbeat', 'schedule', 'cron', 'hourly', 'scheduled'] },
  { id: 'projects.agents', section: 'projects', label: 'Project Agents', description: 'Custom agent personas + wake-up files per workspace', keywords: ['agent', 'persona', 'wakeup', 'create'] },
  { id: 'projects.worktrees', section: 'projects', label: 'Worktree Folders', description: 'Enable/disable per-agent git worktrees', keywords: ['worktree', 'git', 'branch'] },
  { id: 'projects.relations', section: 'projects', label: 'Connections', description: 'Local and federated connections for this workspace', keywords: ['relations', 'connected', 'connections', 'cross-workspace', 'links', 'federation'] },
  { id: 'projects.cursor-migrate', section: 'projects', label: 'Cursor Session Migration', description: 'Port Cursor IDE sessions into K2', keywords: ['cursor', 'migrate', 'session', 'import'] },
  { id: 'projects.default-model', section: 'projects', label: 'Default model', description: 'Per-workspace default LLM model for new sessions', keywords: ['default model', 'opus', 'sonnet', 'workspace model'] },
  { id: 'projects.force-model-on-resume', section: 'projects', label: 'Force model on resume', description: 'Pass workspace default model when resuming a session', keywords: ['resume', 'model', 'force'] },
]

export function ProjectsSection(): React.JSX.Element {
  const projects = useProjectsStore((s) => s.projects)
  const removeProject = useProjectsStore((s) => s.removeProject)
  const fetchProjects = useProjectsStore((s) => s.fetchProjects)

  const focusGroups = useFocusGroupsStore((s) => s.focusGroups)
  const focusGroupsEnabled = useFocusGroupsStore((s) => s.focusGroupsEnabled)
  const setFocusGroupsEnabled = useFocusGroupsStore((s) => s.setFocusGroupsEnabled)
  const createFocusGroup = useFocusGroupsStore((s) => s.createFocusGroup)
  const deleteFocusGroup = useFocusGroupsStore((s) => s.deleteFocusGroup)
  const renameFocusGroup = useFocusGroupsStore((s) => s.renameFocusGroup)
  const assignProjectToGroup = useFocusGroupsStore((s) => s.assignProjectToGroup)

  const initialProjectId = useSettingsStore((s) => s.initialProjectId)
  const [selectedProjectId, setSelectedProjectId] = useState<string | null>(
    initialProjectId ?? (projects.length > 0 ? projects[0].id : null)
  )

  // When initialProjectId changes (e.g. right-click a different project), update selection
  useEffect(() => {
    if (initialProjectId) {
      setSelectedProjectId(initialProjectId)
    }
  }, [initialProjectId])

  // Projects often load after mount — seed the first row so the detail
  // panel isn't blank on a cold open of Settings → Projects.
  useEffect(() => {
    if (selectedProjectId) return
    if (projects.length === 0) return
    setSelectedProjectId(projects[0].id)
  }, [projects, selectedProjectId])

  const [newGroupName, setNewGroupName] = useState('')
  const [searchQuery, setSearchQuery] = useState('')
  const searchInputRef = useRef<HTMLInputElement>(null)
  const [keyboardIndex, setKeyboardIndex] = useState(-1)
  const [collapsedGroups, setCollapsedGroups] = useState<Set<string>>(new Set())
  const [dragProjectId, setDragProjectId] = useState<string | null>(null)
  const [dragOverGroupId, setDragOverGroupId] = useState<string | null>(null)

  // ── Focus group reorder state ──
  const [groupDragId, setGroupDragId] = useState<string | null>(null)
  const [groupDropIndex, setGroupDropIndex] = useState<number | null>(null)
  const groupDragIdRef = useRef<string | null>(null)
  const groupDropRef = useRef<number | null>(null)
  const reorderFocusGroups = useFocusGroupsStore((s) => s.reorderFocusGroups)
  const [renamingGroupId, setRenamingGroupId] = useState<string | null>(null)
  const [renamingGroupName, setRenamingGroupName] = useState('')
  const renameGroupInputRef = useRef<HTMLInputElement>(null)

  const handleGroupContextMenu = useCallback(async (e: React.MouseEvent, groupId: string) => {
    e.preventDefault()
    e.stopPropagation()
    const group = focusGroups.find((g) => g.id === groupId)
    if (!group) return

    const clickedId = await showContextMenu([
      { id: 'rename', label: 'Rename' },
      { id: 'delete', label: 'Delete' },
    ])

    if (clickedId === 'rename') {
      setRenamingGroupId(groupId)
      setRenamingGroupName(group.name)
      requestAnimationFrame(() => renameGroupInputRef.current?.focus())
    } else if (clickedId === 'delete') {
      await deleteFocusGroup(groupId)
      await fetchProjects()
    }
  }, [focusGroups, deleteFocusGroup, fetchProjects])

  const handleGroupRenameConfirm = useCallback(async () => {
    if (renamingGroupId && renamingGroupName.trim()) {
      await renameFocusGroup(renamingGroupId, renamingGroupName.trim())
    }
    setRenamingGroupId(null)
    setRenamingGroupName('')
  }, [renamingGroupId, renamingGroupName, renameFocusGroup])

  const handleGroupReorderMouseDown = useCallback((e: React.MouseEvent, groupId: string) => {
    if (e.button !== 0) return
    // Don't start drag from interactive elements
    if ((e.target as HTMLElement).closest('button, input')) return
    const startY = e.clientY
    let started = false

    const handleMouseMove = (ev: MouseEvent): void => {
      if (!started && Math.abs(ev.clientY - startY) > 5) {
        started = true
        groupDragIdRef.current = groupId
        setGroupDragId(groupId)
        document.body.style.cursor = 'grabbing'
        document.body.style.userSelect = 'none'
      }
      if (!started) return

      const container = document.querySelector('[data-focus-group-reorder-container]')
      if (!container) return
      const items = container.querySelectorAll('[data-focus-group-reorder-id]')
      let dropIdx = 0
      for (let i = 0; i < items.length; i++) {
        const rect = items[i].getBoundingClientRect()
        if (ev.clientY > rect.top + rect.height / 2) dropIdx = i + 1
      }
      groupDropRef.current = dropIdx
      setGroupDropIndex(dropIdx)
    }

    const handleMouseUp = async (): Promise<void> => {
      document.removeEventListener('mousemove', handleMouseMove)
      document.removeEventListener('mouseup', handleMouseUp)
      document.body.style.cursor = ''
      document.body.style.userSelect = ''

      if (started) {
        const dragId = groupDragIdRef.current
        const dropIdx = groupDropRef.current
        if (dragId && dropIdx !== null) {
          const currentGroups = useFocusGroupsStore.getState().focusGroups
          const fromIdx = currentGroups.findIndex((g) => g.id === dragId)
          if (fromIdx >= 0 && fromIdx !== dropIdx && fromIdx !== dropIdx - 1) {
            const list = [...currentGroups]
            const [moved] = list.splice(fromIdx, 1)
            const insertAt = dropIdx > fromIdx ? dropIdx - 1 : dropIdx
            list.splice(insertAt, 0, moved)
            await reorderFocusGroups(list.map((g) => g.id))
          }
        }
      }

      setGroupDragId(null)
      setGroupDropIndex(null)
      groupDragIdRef.current = null
      groupDropRef.current = null
    }

    document.addEventListener('mousemove', handleMouseMove)
    document.addEventListener('mouseup', handleMouseUp)
  }, [reorderFocusGroups])

  const selectedProject = projects.find((p) => p.id === selectedProjectId) ?? null

  const toggleGroupCollapse = useCallback((groupId: string) => {
    setCollapsedGroups((prev) => {
      const next = new Set(prev)
      if (next.has(groupId)) next.delete(groupId)
      else next.add(groupId)
      return next
    })
  }, [])

  const handleCreateGroup = useCallback(async () => {
    if (!newGroupName.trim()) return
    await createFocusGroup(newGroupName.trim())
    setNewGroupName('')
  }, [newGroupName, createFocusGroup])

  const handleDrop = useCallback(async (groupId: string | null) => {
    if (!dragProjectId) return
    await assignProjectToGroup(dragProjectId, groupId)
    await fetchProjects()
    setDragProjectId(null)
    setDragOverGroupId(null)
  }, [dragProjectId, assignProjectToGroup, fetchProjects])

  // ── Reorder state ──────────────────────────────────────────────────
  const [reorderDragId, setReorderDragId] = useState<string | null>(null)
  const [reorderDropIndex, setReorderDropIndex] = useState<number | null>(null)
  const [reorderZone, setReorderZone] = useState<string | null>(null)
  const reorderDropRef = useRef<number | null>(null)
  const reorderZoneRef = useRef<string | null>(null)
  const dragOverGroupRef = useRef<string | null>(null)

  // Auto-focus search when navigating to Workspaces page
  useEffect(() => {
    requestAnimationFrame(() => searchInputRef.current?.focus())
  }, [])

  // Filter helper for search
  const matchesSearch = useCallback((p: typeof projects[0]) => {
    if (!searchQuery.trim()) return true
    const q = searchQuery.toLowerCase()
    return p.name.toLowerCase().includes(q) || p.path.toLowerCase().includes(q)
  }, [searchQuery])

  // 0.39.0: retired the per-agent forced-pinned section. Agent-mode
  // workspaces flow through the regular Pinned + ungrouped lists like
  // any other workspace. Users pin them manually if they want them
  // surfaced; otherwise they appear in their focus group / ungrouped
  // section. Same one-list-of-workspaces model end-to-end.
  const pinnedProjects = useMemo(() => projects.filter((p) => p.pinned && matchesSearch(p)), [projects, matchesSearch])
  const ungroupedProjects = projects.filter((p) => !p.focusGroupId && !p.pinned && matchesSearch(p))
  const reorderProjects = useProjectsStore((s) => s.reorderProjects)
  const setManuallyActive = useProjectsStore((s) => s.setManuallyActive)

  const handleReorderMouseDown = useCallback((
    e: React.MouseEvent,
    projectId: string,
    zone: string,
    containerSelector: string
  ) => {
    if (e.button !== 0) return
    const startX = e.clientX
    const startY = e.clientY
    let started = false

    const handleMouseMove = (ev: MouseEvent): void => {
      if (!started && (Math.abs(ev.clientX - startX) > 3 || Math.abs(ev.clientY - startY) > 5)) {
        started = true
        setReorderDragId(projectId)
        setReorderZone(zone)
        reorderZoneRef.current = zone
        document.body.style.cursor = 'grabbing'
        document.body.style.userSelect = 'none'
      }
      if (!started) return

      // Check if hovering over a focus group header
      const el = document.elementFromPoint(ev.clientX, ev.clientY)
      const groupHeader = el?.closest('[data-focus-group-id]') as HTMLElement | null
      if (groupHeader) {
        const gid = groupHeader.dataset.focusGroupId!
        dragOverGroupRef.current = gid
        setDragOverGroupId(gid)
        setReorderDropIndex(null)
        reorderDropRef.current = null
        return
      } else {
        dragOverGroupRef.current = null
        setDragOverGroupId(null)
      }

      // Check within-zone reorder
      const container = document.querySelector(containerSelector)
      if (!container) return
      const items = container.querySelectorAll('[data-settings-project-id]')
      let idx = 0
      for (let i = 0; i < items.length; i++) {
        const rect = items[i].getBoundingClientRect()
        if (ev.clientY > rect.top + rect.height / 2) idx = i + 1
      }
      reorderDropRef.current = idx
      setReorderDropIndex(idx)
    }

    const handleMouseUp = async (): Promise<void> => {
      document.removeEventListener('mousemove', handleMouseMove)
      document.removeEventListener('mouseup', handleMouseUp)
      document.body.style.cursor = ''
      document.body.style.userSelect = ''

      if (started) {
        // Check if dropped on a focus group header → move to that group
        const hoveredGroupId = dragOverGroupRef.current
        if (hoveredGroupId && hoveredGroupId !== '__ungrouped__') {
          await assignProjectToGroup(projectId, hoveredGroupId)
        } else if (hoveredGroupId === '__ungrouped__') {
          await assignProjectToGroup(projectId, null)
        } else {
          // Within-zone reorder
          const currentProjects = useProjectsStore.getState().projects
          let list: typeof projects = []
          const z = reorderZoneRef.current
          if (z === 'pinned') {
            // 0.39.0: pinned zone includes ALL pinned workspaces
            // regardless of agentMode. The pre-0.39.0 'agents' zone
            // was retired — agent-mode workspaces now flow through
            // the same zones as any other workspace.
            list = [...currentProjects.filter((p) => p.pinned)]
          } else if (z === 'ungrouped' || z === 'flat') {
            list = [...currentProjects.filter((p) => !p.pinned && !p.focusGroupId)]
          } else if (z?.startsWith('group:')) {
            const gid = z.slice(6)
            list = [...currentProjects.filter((p) => p.focusGroupId === gid)]
          }

          const di = reorderDropRef.current
          const fromIdx = list.findIndex((p) => p.id === projectId)
          if (fromIdx >= 0 && di !== null && fromIdx !== di && fromIdx !== di - 1) {
            const item = list.splice(fromIdx, 1)[0]
            const insertAt = di > fromIdx ? di - 1 : di
            list.splice(insertAt, 0, item)
            reorderProjects(list.map((p) => p.id))
          }
        }
      }

      setReorderDragId(null)
      setReorderZone(null)
      setReorderDropIndex(null)
      setDragOverGroupId(null)
      reorderDropRef.current = null
      reorderZoneRef.current = null
    }

    document.addEventListener('mousemove', handleMouseMove)
    document.addEventListener('mouseup', handleMouseUp)
  }, [reorderProjects, assignProjectToGroup])


  // Build flat list of all visible projects for keyboard navigation
  const allVisibleProjects = useMemo(() => {
    const result: typeof projects = []
    result.push(...pinnedProjects)
    if (focusGroupsEnabled) {
      for (const group of focusGroups) {
        const gp = projects.filter((p) => p.focusGroupId === group.id && !p.pinned && matchesSearch(p))
        result.push(...gp)
      }
      result.push(...ungroupedProjects)
    } else {
      const flat = projects.filter((p) => !p.pinned && matchesSearch(p))
      result.push(...flat)
    }
    return result
  }, [pinnedProjects, focusGroups, focusGroupsEnabled, projects, ungroupedProjects, matchesSearch])

  // Reset keyboard index when search changes
  useEffect(() => { setKeyboardIndex(-1) }, [searchQuery])

  // Keyboard navigation in search
  const handleSearchKeyDown = useCallback((e: React.KeyboardEvent) => {
    if (e.key === 'ArrowDown') {
      e.preventDefault()
      setKeyboardIndex((prev) => Math.min(prev + 1, allVisibleProjects.length - 1))
    } else if (e.key === 'ArrowUp') {
      e.preventDefault()
      setKeyboardIndex((prev) => Math.max(prev - 1, 0))
    } else if (e.key === 'Enter' && keyboardIndex >= 0 && keyboardIndex < allVisibleProjects.length) {
      e.preventDefault()
      setSelectedProjectId(allVisibleProjects[keyboardIndex].id)
    }
  }, [allVisibleProjects, keyboardIndex])

  // Scroll keyboard-selected item into view
  useEffect(() => {
    if (keyboardIndex >= 0 && allVisibleProjects[keyboardIndex]) {
      const el = document.querySelector(`[data-settings-project-id="${allVisibleProjects[keyboardIndex].id}"]`)
      el?.scrollIntoView({ block: 'nearest' })
    }
  }, [keyboardIndex, allVisibleProjects])

  // Right-click context menu for workspace rows
  const handleProjectContextMenu = useCallback(async (e: React.MouseEvent, p: typeof projects[number]) => {
    e.preventDefault()
    e.stopPropagation()

    const menuItems: { id: string; label: string }[] = [
      { id: 'pin', label: p.pinned ? 'Unpin' : 'Pin to top' },
    ]

    // Add "Move to" options if focus groups exist
    if (focusGroupsEnabled && focusGroups.length > 0) {
      menuItems.push({ id: '__separator__', label: '─' })
      for (const group of focusGroups) {
        if (p.focusGroupId === group.id) continue // skip current group
        menuItems.push({ id: `move:${group.id}`, label: `Move to ${group.name}` })
      }
      if (p.focusGroupId) {
        menuItems.push({ id: 'move:__none__', label: 'Remove from group' })
      }
    }

    const clickedId = await showContextMenu(menuItems)
    if (!clickedId) return

    if (clickedId === 'pin') {
      await setManuallyActive(p.id, !p.pinned)
    } else if (clickedId.startsWith('move:')) {
      const groupId = clickedId.replace('move:', '')
      await assignProjectToGroup(p.id, groupId === '__none__' ? null : groupId)
    }
  }, [focusGroupsEnabled, focusGroups, assignProjectToGroup, setManuallyActive])

  // Workspace row renderer (called as function, NOT as <Component/>, to avoid unmount/remount flicker)
  const renderProjectRow = (p: typeof projects[number], zone: string, containerSelector: string) => {
    const isSelected = selectedProjectId === p.id
    const isDragged = reorderDragId === p.id
    const kbIdx = allVisibleProjects.findIndex((vp) => vp.id === p.id)
    const isKeyboardHighlighted = kbIdx >= 0 && kbIdx === keyboardIndex
    return (
      <div
        data-settings-project-id={p.id}
        onClick={() => setSelectedProjectId(p.id)}
        onContextMenu={(e) => handleProjectContextMenu(e, p)}
        onMouseDown={(e) => { if (e.button === 0) handleReorderMouseDown(e, p.id, zone, containerSelector) }}
        className={`flex items-center gap-2 px-2 py-1.5 transition-colors no-drag cursor-pointer group select-none ${
          isSelected
            ? 'bg-[var(--color-accent)]/15 text-[var(--color-text-primary)]'
            : isKeyboardHighlighted
              ? 'bg-white/[0.06] text-[var(--color-text-primary)]'
              : 'text-[var(--color-text-secondary)] hover:bg-white/[0.04] hover:text-[var(--color-text-primary)]'
        } ${isDragged ? 'opacity-30' : ''} cursor-grab active:cursor-grabbing`}
      >
        <ProjectAvatar
          projectPath={p.path}
          projectName={p.name}
          projectColor={p.color}
          projectId={p.id}
          iconUrl={p.iconUrl}
          size={20}
        />
        <span className="text-xs truncate flex-1">{p.name}</span>
        <button
          onClick={async (e) => {
            e.stopPropagation()
            const newPinned = p.pinned ? 0 : 1
            await daemonCliPost('projects/update', { id: p.id, pinned: newPinned })
            emitProjectsChanged()
            const store = useProjectsStore.getState()
            useProjectsStore.setState({
              projects: store.projects.map((proj) =>
                proj.id === p.id ? { ...proj, pinned: newPinned } : proj
              )
            })
          }}
          className={`flex-shrink-0 p-0.5 transition-colors ${
            p.pinned
              ? 'text-[var(--color-accent)]'
              : 'text-transparent group-hover:text-[var(--color-text-muted)] hover:!text-[var(--color-accent)]'
          }`}
          title={p.pinned ? 'Unpin' : 'Pin to top'}
        >
          <svg width="10" height="10" viewBox="0 0 16 16" fill="currentColor">
            <path d="M9.828.722a.5.5 0 0 1 .354.146l4.95 4.95a.5.5 0 0 1-.707.707l-.71-.71-3.18 3.18a3.5 3.5 0 0 1-.4.3L11 11.106V14.5a.5.5 0 0 1-.854.354L7.5 12.207 4.854 14.854a.5.5 0 0 1-.708-.708L6.793 11.5 4.146 8.854A.5.5 0 0 1 4.5 8h3.394a3.5 3.5 0 0 0 .3-.4l3.18-3.18-.71-.71a.5.5 0 0 1 .354-.854z" />
          </svg>
        </button>
      </div>
    )
  }

  return (
    <div className="flex h-full min-h-0">
      {/* ── Left panel: focus group toggle + organized workspace list ── */}
      <div className="w-60 flex-shrink-0 border-r border-[var(--color-border)] flex flex-col">
        {/* Focus groups toggle at top */}
        <div className="px-3 pt-3 pb-2 border-b border-[var(--color-border)] flex items-center justify-between">
          <span className="text-[10px] font-semibold text-[var(--color-text-muted)] uppercase tracking-wider">
            Focus Groups
          </span>
          <button
            onClick={() => setFocusGroupsEnabled(!focusGroupsEnabled)}
            className={`w-7 h-3.5 flex items-center transition-colors no-drag cursor-pointer flex-shrink-0 ${
              focusGroupsEnabled ? 'bg-[var(--color-accent)]' : 'bg-[var(--color-border)]'
            }`}
          >
            <span
              className={`w-2.5 h-2.5 bg-[var(--color-on-accent)] block transition-transform ${
                focusGroupsEnabled ? 'translate-x-3.5' : 'translate-x-0.5'
              }`}
            />
          </button>
        </div>

        {/* Alphabetize buttons */}
        <div className="px-3 py-1.5 flex gap-1.5 border-b border-[var(--color-border)]">
          <button
            onClick={async () => {
              if (focusGroupsEnabled) {
                const sorted = [...focusGroups].sort((a, b) => a.name.localeCompare(b.name))
                await reorderFocusGroups(sorted.map((g) => g.id))
              }
            }}
            className="flex-1 px-1.5 py-1 text-[10px] text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)] bg-white/[0.03] hover:bg-white/[0.06] transition-colors no-drag cursor-pointer"
            title="Sort focus groups A-Z"
          >
            A→Z Groups
          </button>
          <button
            onClick={async () => {
              const sorted = [...projects].sort((a, b) => a.name.localeCompare(b.name))
              await reorderProjects(sorted.map((p) => p.id))
            }}
            className="flex-1 px-1.5 py-1 text-[10px] text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)] bg-white/[0.03] hover:bg-white/[0.06] transition-colors no-drag cursor-pointer"
            title="Sort workspaces A-Z within groups"
          >
            A→Z Workspaces
          </button>
        </div>

        {/* Search bar */}
        <div className="px-2 py-1.5">
          <input
            ref={searchInputRef}
            type="text"
            value={searchQuery}
            onChange={(e) => setSearchQuery(e.target.value)}
            onKeyDown={handleSearchKeyDown}
            placeholder="Search workspaces..."
            className="w-full px-2 py-1.5 text-xs bg-[var(--color-bg)] border border-[var(--color-border)] text-[var(--color-text-primary)] placeholder:text-[var(--color-text-muted)] focus:outline-none focus:border-[var(--color-accent)] no-drag"
          />
        </div>

        {/* Workspace list — pinned at top, then groups or flat */}
        {/* 0.39.0: retired the dedicated Agents section — agent-mode
            workspaces flow through Pinned / focus groups / ungrouped
            like any other workspace. Single Pinned section below. */}
        <div className="flex-1 overflow-y-auto px-1 py-1">
          {/* ── Pinned workspaces ── */}
          {pinnedProjects.length > 0 && (
            <div className="mb-1 pb-1 border-b border-[var(--color-border)]">
              <div className="px-2 pt-1 pb-1">
                <span className="text-[10px] font-semibold text-[var(--color-text-muted)] uppercase tracking-wider">
                  Pinned
                </span>
              </div>
              <div data-reorder-zone="pinned">
                {pinnedProjects.map((p, idx) => (
                  <div key={p.id}>
                    {reorderZone === 'pinned' && reorderDropIndex === idx && (
                      <div className="h-[2px] bg-[var(--color-accent)] mx-2" />
                    )}
                    {renderProjectRow(p, 'pinned', "[data-reorder-zone='pinned']")}
                  </div>
                ))}
                {reorderZone === 'pinned' && reorderDropIndex === pinnedProjects.length && (
                  <div className="h-[2px] bg-[var(--color-accent)] mx-2" />
                )}
              </div>
            </div>
          )}

          {focusGroupsEnabled ? (
            <>
              {/* Focus group folders */}
              <div data-focus-group-reorder-container>
              {focusGroups.map((group, groupIdx) => {
                const groupProjects = projects.filter((p) => p.focusGroupId === group.id && !p.pinned && matchesSearch(p))
                const isCollapsed = collapsedGroups.has(group.id)
                const isDragOver = dragOverGroupId === group.id
                const zoneId = `group:${group.id}`
                const isGroupDragged = groupDragId === group.id
                const showGroupDropBefore = groupDropIndex === groupIdx
                const showGroupDropAfter = groupDropIndex === focusGroups.length && groupIdx === focusGroups.length - 1

                // Hide empty focus groups when searching
                if (searchQuery.trim() && groupProjects.length === 0) return null

                return (
                  <div key={group.id} className={`mb-0.5 ${isGroupDragged ? 'opacity-30' : ''}`} data-focus-group-reorder-id={group.id}>
                    {showGroupDropBefore && <div className="h-[2px] bg-[var(--color-accent)] mx-2 mb-0.5" />}
                    {/* Group folder header */}
                    <div
                      data-focus-group-id={group.id}
                      className={`flex items-center gap-1.5 px-2 py-1 cursor-pointer no-drag select-none transition-all duration-150 ${
                        isDragOver
                          ? 'bg-[var(--color-accent)]/15 ring-1 ring-inset ring-[var(--color-accent)] scale-[1.02]'
                          : 'hover:bg-white/[0.03]'
                      }`}
                      onClick={() => { if (renamingGroupId !== group.id) toggleGroupCollapse(group.id) }}
                      onMouseDown={(e) => handleGroupReorderMouseDown(e, group.id)}
                      onContextMenu={(e) => handleGroupContextMenu(e, group.id)}
                    >
                      {group.color && (
                        <span className="w-1 h-3 flex-shrink-0" style={{ backgroundColor: isDragOver ? 'var(--color-accent)' : group.color }} />
                      )}
                      <svg
                        className={`w-2.5 h-2.5 text-[var(--color-text-muted)] transition-transform flex-shrink-0 ${
                          isCollapsed ? '' : 'rotate-90'
                        }`}
                        fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2}
                      >
                        <path strokeLinecap="round" strokeLinejoin="round" d="M9 5l7 7-7 7" />
                      </svg>
                      {renamingGroupId === group.id ? (
                        <input
                          ref={renameGroupInputRef}
                          type="text"
                          value={renamingGroupName}
                          onChange={(e) => setRenamingGroupName(e.target.value)}
                          onKeyDown={(e) => {
                            if (e.key === 'Enter') handleGroupRenameConfirm()
                            else if (e.key === 'Escape') { setRenamingGroupId(null); setRenamingGroupName('') }
                          }}
                          onBlur={handleGroupRenameConfirm}
                          onClick={(e) => e.stopPropagation()}
                          className="text-[11px] font-medium text-[var(--color-text-primary)] flex-1 bg-transparent border-b border-[var(--color-accent)] outline-none px-0 py-0"
                        />
                      ) : (
                        <span className="text-[11px] font-medium text-[var(--color-text-secondary)] flex-1 truncate">
                          {group.name}
                        </span>
                      )}
                      {isDragOver ? (
                        <span className="text-[9px] text-[var(--color-accent)] flex-shrink-0 font-medium">
                          Drop here
                        </span>
                      ) : (
                        <span className="text-[10px] text-[var(--color-text-muted)] tabular-nums flex-shrink-0">
                          {groupProjects.length}
                        </span>
                      )}
                    </div>

                    {!isCollapsed && (
                      <div className="ml-3" data-reorder-zone={zoneId}>
                        {groupProjects.map((p, idx) => (
                          <div key={p.id}>
                            {reorderZone === zoneId && reorderDropIndex === idx && (
                              <div className="h-[2px] bg-[var(--color-accent)] mx-2" />
                            )}
                            {renderProjectRow(p, zoneId, `[data-reorder-zone='${zoneId}']`)}
                          </div>
                        ))}
                        {reorderZone === zoneId && reorderDropIndex === groupProjects.length && (
                          <div className="h-[2px] bg-[var(--color-accent)] mx-2" />
                        )}
                        {groupProjects.length === 0 && (
                          <div
                            className={`px-2 py-2 text-center text-[10px] text-[var(--color-text-muted)] italic transition-colors ${
                              isDragOver ? 'bg-[var(--color-accent)]/5' : ''
                            }`}
                          >
                            Drop workspaces here
                          </div>
                        )}
                      </div>
                    )}
                    {showGroupDropAfter && <div className="h-[2px] bg-[var(--color-accent)] mx-2 mt-0.5" />}
                  </div>
                )
              })}
              </div>

              {/* Ungrouped workspaces */}
              {ungroupedProjects.length > 0 && (
                <div className="mt-1">
                  <div
                    data-focus-group-id="__ungrouped__"
                    className={`flex items-center gap-1.5 px-2 py-1 text-[11px] font-medium select-none transition-all duration-150 ${
                      dragOverGroupId === '__ungrouped__'
                        ? 'text-[var(--color-accent)] bg-[var(--color-accent)]/15 ring-1 ring-inset ring-[var(--color-accent)] scale-[1.02]'
                        : 'text-[var(--color-text-muted)]'
                    }`}
                  >
                    Ungrouped
                  </div>
                  <div className="ml-1" data-reorder-zone="ungrouped">
                    {ungroupedProjects.map((p, idx) => (
                      <div key={p.id}>
                        {reorderZone === 'ungrouped' && reorderDropIndex === idx && (
                          <div className="h-[2px] bg-[var(--color-accent)] mx-2" />
                        )}
                        {renderProjectRow(p, 'ungrouped', "[data-reorder-zone='ungrouped']")}
                      </div>
                    ))}
                    {reorderZone === 'ungrouped' && reorderDropIndex === ungroupedProjects.length && (
                      <div className="h-[2px] bg-[var(--color-accent)] mx-2" />
                    )}
                  </div>
                </div>
              )}

              {/* Add new group */}
              <div className="mt-2 px-1">
                <div className="flex items-center gap-1">
                  <input
                    type="text"
                    value={newGroupName}
                    onChange={(e) => setNewGroupName(e.target.value)}
                    onKeyDown={(e) => { if (e.key === 'Enter') handleCreateGroup() }}
                    placeholder="+ New group"
                    className="flex-1 px-2 py-1 text-[11px] bg-transparent border border-transparent text-[var(--color-text-muted)] outline-none focus:border-[var(--color-border)] focus:text-[var(--color-text-primary)] no-drag"
                  />
                </div>
              </div>
            </>
          ) : (
            /* Simple flat list when focus groups disabled */
            <div className="space-y-0.5">
              <div className="px-2 pt-1 pb-1">
                <span className="text-[10px] font-semibold text-[var(--color-text-muted)] uppercase tracking-wider">
                  Workspaces
                </span>
              </div>
              <div data-reorder-zone="flat">
                {projects.filter((p) => !p.pinned).map((p, idx) => (
                  <div key={p.id}>
                    {reorderZone === 'flat' && reorderDropIndex === idx && (
                      <div className="h-[2px] bg-[var(--color-accent)] mx-2" />
                    )}
                    {renderProjectRow(p, 'flat', "[data-reorder-zone='flat']")}
                  </div>
                ))}
              </div>
              {projects.length === 0 && (
                <div className="px-2 py-6 text-center">
                  <span className="text-xs text-[var(--color-text-muted)]">No workspaces</span>
                </div>
              )}
            </div>
          )}
        </div>

        {/* + New Workspace button */}
        <div className="px-2 py-2 border-t border-[var(--color-border)]">
          <button
            className="w-full flex items-center justify-center gap-1.5 px-2 py-1.5 text-xs text-[var(--color-text-secondary)] hover:text-[var(--color-text-primary)] bg-white/[0.04] hover:bg-white/[0.08] transition-colors no-drag cursor-pointer"
            onClick={async () => {
              // Host-aware: local → native OS dialog; remote → in-app
              // RemoteFolderPicker over the host's fs. Single chokepoint
              // (pick-workspace-folder) so every "add workspace" entry point
              // stays host-aware. The chosen path flows through addProject.
              const folderPath = await pickWorkspaceFolder()
              if (folderPath) {
                await useProjectsStore.getState().addProject(folderPath)
                await fetchProjects()
              }
            }}
          >
            <svg className="w-3 h-3" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2}>
              <path strokeLinecap="round" strokeLinejoin="round" d="M12 4v16m8-8H4" />
            </svg>
            New Workspace
          </button>
        </div>
      </div>

      {/* ── Right panel: selected workspace settings ──
          overflow-hidden (not y-auto) so only ProjectDetail's tab panel
          scrolls — avoids a nested scrollbar that overpaints content. */}
      <div className="flex-1 overflow-hidden p-6 min-h-0 relative flex flex-col">
        {selectedProject ? (
          <ProjectDetail
            project={selectedProject}
            focusGroups={focusGroups}
            focusGroupsEnabled={focusGroupsEnabled}
            removeProject={removeProject}
            assignProjectToGroup={assignProjectToGroup}
            fetchProjects={fetchProjects}
          />
        ) : (
          <div className="flex items-center justify-center h-full">
            <span className="text-xs text-[var(--color-text-muted)]">
              Select a workspace to view its settings
            </span>
          </div>
        )}
      </div>

    </div>
  )
}

// ── Worktree Folders on Disk ─────────────────────────────────────────
function WorktreeFoldersOnDisk({
  project,
  fetchProjects
}: {
  project: ReturnType<typeof useProjectsStore.getState>['projects'][number]
  fetchProjects: () => Promise<void>
}): React.JSX.Element {
  const [diskWorktrees, setDiskWorktrees] = useState<
    Array<{ path: string; branch: string; isMain: boolean; isBare: boolean }>
  >([])
  const [loading, setLoading] = useState(true)
  const [reopening, setReopening] = useState<string | null>(null)

  useEffect(() => {
    let cancelled = false
    setLoading(true)
    daemonCliGet<any[]>('git/worktrees', { path: project.path })
      .then((wts) => {
        if (!cancelled) {
          setDiskWorktrees(wts)
          setLoading(false)
        }
      })
      .catch(() => {
        if (!cancelled) setLoading(false)
      })
    return () => {
      cancelled = true
    }
  }, [project.path, project.workspaces])

  // Determine which disk worktrees are active (have a workspace record)
  const activeWorktreePaths = new Set(
    project.workspaces
      .filter((ws) => ws.worktreePath)
      .map((ws) => ws.worktreePath!)
  )
  // Also consider the main project path as active if a branch workspace points to it
  const mainWorkspaceExists = project.workspaces.some((ws) => ws.type === 'branch')

  const handleReopen = async (wt: { path: string; branch: string }): Promise<void> => {
    setReopening(wt.path)
    try {
      // POST body is camelCase: the daemon's ReopenWorktreeBody reads
      // projectPath/worktreePath/branch. (The pre-migration invoke passed a
      // `projectId` key that the Tauri command — which expects
      // `project_path` — never consumed; the handler only needs the
      // worktree path + branch and echoes project_path back, so we now send
      // the correct project.path.)
      await daemonCliPost('git/reopen-worktree', {
        projectPath: project.path,
        worktreePath: wt.path,
        branch: wt.branch
      })
      await fetchProjects()
    } catch (err) {
      console.error('Reopen worktree failed:', err)
    } finally {
      setReopening(null)
    }
  }

  if (loading) {
    return (
      <div className="space-y-2">
        <h3 className="text-[10px] font-semibold text-[var(--color-text-muted)] uppercase tracking-wider">
          Worktree Folders on Disk
        </h3>
        <p className="text-[10px] text-[var(--color-text-muted)]">Loading...</p>
      </div>
    )
  }

  // Filter out bare worktrees
  const nonBare = diskWorktrees.filter((wt) => !wt.isBare)
  if (nonBare.length === 0) return <></>

  return (
    <div className="space-y-2">
      <h3 className="text-[10px] font-semibold text-[var(--color-text-muted)] uppercase tracking-wider flex items-center gap-1.5">
        Worktree Folders on Disk
        <span className="text-[9px] tabular-nums font-medium px-1.5 py-0.5 bg-[var(--color-wash-1)] text-[var(--color-text-muted)]">{nonBare.length}</span>
      </h3>
      <div className="border border-[var(--color-border)]">
        {nonBare.map((wt, i) => {
          const isActive = wt.isMain
            ? mainWorkspaceExists
            : activeWorktreePaths.has(wt.path)
          const isClosed = !isActive

          return (
            <div
              key={wt.path}
              className={`flex items-center gap-2 px-3 py-1.5 ${
                i < nonBare.length - 1 ? 'border-b border-[var(--color-border)]' : ''
              }`}
            >
              <svg
                className="w-3 h-3 flex-shrink-0 text-[var(--color-text-muted)]"
                fill="none"
                viewBox="0 0 24 24"
                stroke="currentColor"
                strokeWidth={2}
              >
                {wt.isMain ? (
                  <path strokeLinecap="round" strokeLinejoin="round" d="M13 10V3L4 14h7v7l9-11h-7z" />
                ) : (
                  <path strokeLinecap="round" strokeLinejoin="round" d="M7 7h.01M7 3h5c.512 0 1.024.195 1.414.586l7 7a2 2 0 010 2.828l-7 7a2 2 0 01-2.828 0l-7-7A2 2 0 013 12V7a4 4 0 014-4z" />
                )}
              </svg>
              <div className="flex-1 min-w-0">
                <span className="text-xs text-[var(--color-text-primary)] truncate block">
                  {wt.branch}
                </span>
                <span className="text-[10px] text-[var(--color-text-muted)] truncate block" title={wt.path}>
                  {wt.path.length > 50 ? '...' + wt.path.slice(-47) : wt.path}
                </span>
              </div>
              {isActive ? (
                <span className="text-[10px] text-[var(--color-status-ok-soft)] flex-shrink-0">(active)</span>
              ) : (
                <button
                  onClick={() => handleReopen(wt)}
                  disabled={reopening === wt.path}
                  className="px-2 py-0.5 text-[10px] text-[var(--color-accent)] border border-[var(--color-accent)]/30 hover:bg-[var(--color-accent)]/10 no-drag cursor-pointer disabled:opacity-50 disabled:cursor-not-allowed transition-colors flex-shrink-0"
                >
                  {reopening === wt.path ? 'Reopening...' : 'Reopen'}
                </button>
              )}
            </div>
          )
        })}
      </div>
    </div>
  )
}

// Per-workspace detail tabs (replaces the long two-column stack).
type WorkspaceSettingsTab =
  | 'agent'
  | 'context'
  | 'connections'
  | 'skills'
  | 'schedule'
  | 'worktrees'
  | 'import'
  | 'api'

const WORKSPACE_SETTINGS_TABS: WorkspaceSettingsTab[] = [
  'agent',
  'context',
  'connections',
  'skills',
  'schedule',
  'worktrees',
  'import',
  'api',
]

function parseWorkspaceSettingsTab(value: string | null): WorkspaceSettingsTab | null {
  if (value && (WORKSPACE_SETTINGS_TABS as string[]).includes(value)) {
    return value as WorkspaceSettingsTab
  }
  return null
}

// ── Workspace Detail (right panel content) ─────────────────────────────
function ProjectDetail({
  project,
  focusGroups,
  focusGroupsEnabled,
  removeProject,
  assignProjectToGroup,
  fetchProjects
}: {
  project: ReturnType<typeof useProjectsStore.getState>['projects'][number]
  focusGroups: ReturnType<typeof useFocusGroupsStore.getState>['focusGroups']
  focusGroupsEnabled: boolean
  removeProject: (id: string) => Promise<void>
  assignProjectToGroup: (projectId: string, groupId: string | null) => Promise<void>
  fetchProjects: () => Promise<void>
}): React.JSX.Element {
  const [iconLoading, setIconLoading] = useState(false)
  const [cropImage, setCropImage] = useState<string | null>(null)
  const [agentEditorOpen, setAgentEditorOpen] = useState(false)
  const [agentEditorName, setAgentEditorName] = useState('')
  /** Generic stack-layer edit (arbitrary .md path via AIFileEditor). */
  const [contextFileEdit, setContextFileEdit] = useState<{ path: string; label: string } | null>(null)
  // The WakeupEditor lives alongside the other "agent editor"
  // takeovers (ClaudeMdEditor, AgentPersonaEditor, ProjectContextEditor)
  // so it fills the Settings content area without colliding with the
  // workspaces sidebar's stacking context. State is lifted from
  // HeartbeatsPanel so the editor can render at this level.
  const [wakeupEditingHb, setWakeupEditingHb] = useState<HeartbeatRow | null>(null)
  // Heartbeat refresh nonce — incremented when the wakeup editor
  // closes so the panel's k2so_heartbeat_list query re-runs and
  // picks up any edits the agent made to the row.
  const [hbRefreshNonce, setHbRefreshNonce] = useState(0)
  // When agentMode is 'off' and there are no historical fires for this
  // workspace, there's no audit to show — hide History so we don't leave
  // an empty frame. A hidden mount still tracks onEmptyChange.
  const [historyEmpty, setHistoryEmpty] = useState(false)
  // 0.39.0f Phase 2.1: the workspace's primary agent identity, resolved
  // via the daemon-first `k2so_workspace_agent_display_name` helper
  // (which falls back to AGENT.md `name:` then folder basename). The
  // pre-unification code keyed off `mode` and hard-coded `__lead__` /
  // `k2so-agent` / `slug(project.name)`; that triplet drifted from
  // reality whenever the user renamed the agent or the workspace
  // identity changed. Resolving via the daemon keeps the Wakeup editor,
  // the HeartbeatsPanel header, and the SystemHeartbeatRow all in sync.
  // Empty string while loading — children handle the loading state by
  // suspending their reads until the name is known.
  const [primaryAgentName, setPrimaryAgentName] = useState('')
  // Canonical Agent Flow (canonical-agents PRD §9.2 / §9.3). The modal mode
  // (setup vs manage/undo) and the per-harness detected state drive the
  // canonical button label + which seed the modal launches with.
  const [canonicalModalMode, setCanonicalModalMode] = useState<'setup' | 'manage' | null>(null)
  const [canonicalProbes, setCanonicalProbes] = useState<HarnessProbe[]>([])
  const fileInputRef = useRef<HTMLInputElement>(null)
  // Consume the deep-link on first paint so manage → Heartbeats does not
  // flash Agent, and so a remount still sees the requested tab.
  const initialWorkspaceTab = useSettingsStore((s) => s.initialWorkspaceTab)
  const [settingsTab, setSettingsTab] = useState<WorkspaceSettingsTab>(
    () => parseWorkspaceSettingsTab(useSettingsStore.getState().initialWorkspaceTab) ?? 'agent',
  )
  useEffect(() => {
    const parsed = parseWorkspaceSettingsTab(initialWorkspaceTab)
    if (parsed) setSettingsTab(parsed)
  }, [initialWorkspaceTab])
  const selectSettingsTab = (tab: WorkspaceSettingsTab): void => {
    useSettingsStore.setState({ initialWorkspaceTab: null })
    setSettingsTab(tab)
  }

  // Close takeovers when the selected workspace changes — keep settingsTab
  // so the user stays on Context / Heartbeats / etc. while flipping agents.
  useEffect(() => {
    setAgentEditorOpen(false)
    setAgentEditorName('')
    setContextFileEdit(null)
    setWakeupEditingHb(null)
    setCanonicalModalMode(null)
    setHistoryEmpty(false)
  }, [project.id])

  const openContextEdit = useCallback((target: ContextEditTarget) => {
    if (target.kind === 'agent') {
      const name = primaryAgentName || project.name.toLowerCase().replace(/\s+/g, '-')
      setAgentEditorName(name)
      setAgentEditorOpen(true)
      return
    }
    if (target.kind === 'project') {
      setAgentEditorName('__claude_md__')
      setAgentEditorOpen(true)
      return
    }
    setContextFileEdit({ path: target.absPath, label: target.label })
  }, [primaryAgentName, project.name])

  // Detect per-harness canonical state (PRD §5.2) so the canonical button
  // reads "Set up …" vs "Manage / Undo". Re-runs when the modal closes so a
  // just-completed setup flips the label. Best-effort — a failure leaves the
  // button in its default "Set up …" state.
  useEffect(() => {
    let cancelled = false
    daemonCliPost<HarnessProbe[]>('canonical/detect-state', { project_path: project.path })
      .then((p) => { if (!cancelled) setCanonicalProbes(p) })
      .catch((err) => { if (!cancelled) console.warn('[canonical-state] detect failed:', err) })
    return () => { cancelled = true }
  }, [project.path, canonicalModalMode])

  // Resolve the workspace's primary agent display name once per
  // project. Used by the WakeupEditor takeover (agentName prop) and
  // the HeartbeatsPanel (agentName prop). The fetch is a single
  // daemon call; the resolver is total so we always end up with a
  // non-empty string for non-`off` workspaces.
  useEffect(() => {
    let cancelled = false
    if ((project.agentMode || 'off') === 'off') {
      setPrimaryAgentName('')
      return () => { cancelled = true }
    }
    agentDisplayName(project.path)
      .then((n) => { if (!cancelled) setPrimaryAgentName(n) })
      .catch((err) => {
        if (!cancelled) {
          console.error('[primary-agent-name] resolve failed:', err)
          setPrimaryAgentName('')
        }
      })
    return () => { cancelled = true }
  }, [project.path, project.agentMode])


  const handleDetectIcon = async (): Promise<void> => {
    setIconLoading(true)
    try {
      await daemonCliPost('projects/detect-icon', { projectId: project.id })
      emitProjectsChanged()
      await fetchProjects()
    } catch (err) {
      console.error('Icon detection failed:', err)
    } finally {
      setIconLoading(false)
    }
  }

  const handleUploadClick = (): void => {
    // Host-aware: on a remote host the native input would browse THIS
    // machine's disk — use the remote file picker instead.
    void pickIconImage({
      clickNativeInput: () => fileInputRef.current?.click(),
      setCropImage,
    })
  }

  const handleFileSelected = (e: React.ChangeEvent<HTMLInputElement>): void => {
    const file = e.target.files?.[0]
    if (!file) return

    const reader = new FileReader()
    reader.onload = () => {
      setCropImage(reader.result as string)
    }
    reader.readAsDataURL(file)

    // Reset input so the same file can be re-selected
    e.target.value = ''
  }

  const handleCropConfirm = async (croppedDataUrl: string): Promise<void> => {
    setCropImage(null)
    setIconLoading(true)
    try {
      await daemonCliPost('projects/update', { id: project.id, iconUrl: croppedDataUrl })
      emitProjectsChanged()
      await fetchProjects()
    } catch (err) {
      console.error('Icon save failed:', err)
    } finally {
      setIconLoading(false)
    }
  }

  const handleClearIcon = async (): Promise<void> => {
    setIconLoading(true)
    try {
      await daemonCliPost('projects/clear-icon', { projectId: project.id })
      emitProjectsChanged()
      await fetchProjects()
    } catch (err) {
      console.error('Icon clear failed:', err)
    } finally {
      setIconLoading(false)
    }
  }

  const firstLetter = project.name.charAt(0).toUpperCase()

  // Full-screen agent / context-file editor takeover (same pattern as CustomThemeCreator)
  if (contextFileEdit) {
    return (
      <SectionErrorBoundary>
        <div className="absolute inset-0 overflow-hidden bg-[var(--color-bg)]">
          <ContextLayerFileEditor
            filePath={contextFileEdit.path}
            label={contextFileEdit.label}
            projectPath={project.path}
            projectName={project.name}
            onClose={() => setContextFileEdit(null)}
          />
        </div>
      </SectionErrorBoundary>
    )
  }

  if (agentEditorOpen && agentEditorName) {
    return (
      <SectionErrorBoundary>
        <div className="absolute inset-0 overflow-hidden bg-[var(--color-bg)]">
          {agentEditorName === '__claude_md__' ? (
            <ClaudeMdEditor
              projectPath={project.path}
              projectName={project.name}
              onClose={() => setAgentEditorOpen(false)}
            />
          ) : agentEditorName === '__workspace_manager__' ? (
            <RoleSkillEditor
              role="workspace-manager"
              projectPath={project.path}
              projectName={project.name}
              onClose={() => setAgentEditorOpen(false)}
            />
          ) : agentEditorName === '__k2_agent__' ? (
            <RoleSkillEditor
              role="k2-agent"
              projectPath={project.path}
              projectName={project.name}
              onClose={() => setAgentEditorOpen(false)}
            />
          ) : (
            <AgentPersonaEditor
              agentName={agentEditorName}
              projectPath={project.path}
              onClose={() => setAgentEditorOpen(false)}
            />
          )}
        </div>
      </SectionErrorBoundary>
    )
  }

  // K2 Canonical Agent ceremony takeover (canonical-agents PRD §9.2). A
  // distinct full-area takeover (NOT routed through agentEditorName) because
  // it is a modal with the agent running + a structured plan/manifest
  // renderer, not the single-file AIFileEditor the persona editors use.
  if (canonicalModalMode) {
    return (
      <SectionErrorBoundary>
        <div className="absolute inset-0 overflow-hidden bg-[var(--color-bg)]">
          <CanonicalAgentModal
            projectPath={project.path}
            projectName={project.name}
            mode={canonicalModalMode}
            onClose={() => setCanonicalModalMode(null)}
          />
        </div>
      </SectionErrorBoundary>
    )
  }

  // Heartbeat WAKEUP.md takeover — same pattern as ClaudeMdEditor.
  // The state lives at this level (not in HeartbeatsPanel) so the
  // editor fills the Settings content area cleanly instead of being
  // squeezed inside the right-rail aside or fighting the workspaces
  // sidebar's stacking context with a fixed overlay.
  if (wakeupEditingHb) {
    // 0.39.0f Phase 2.1: agentName resolves through the daemon's
    // `k2so_workspace_agent_display_name` helper (see the
    // primaryAgentName useEffect above). The legacy mode→literal
    // mapping (`manager`→`__lead__`, `agent`→`k2so-agent`,
    // `custom`→slug(project.name)) drifted from AGENT.md whenever the
    // user renamed the agent; the single resolver keeps the editor
    // pointed at the actual on-disk identity. Empty string only
    // while the resolver is in-flight — fall back to the slug then.
    const wakeupAgentName = primaryAgentName
      || project.name.toLowerCase().replace(/\s+/g, '-')
    return (
      <SectionErrorBoundary>
        <div className="absolute inset-0 overflow-hidden bg-[var(--color-bg)]">
          <WakeupEditor
            projectPath={project.path}
            agentName={wakeupAgentName}
            heartbeat={wakeupEditingHb}
            otherHeartbeats={[]}
            onClose={() => {
              setWakeupEditingHb(null)
              setHbRefreshNonce((n) => n + 1)
            }}
          />
        </div>
      </SectionErrorBoundary>
    )
  }

  const agentMode = project.agentMode || 'off'
  const isManagerMode = agentMode === 'manager' || agentMode === 'coordinator' || agentMode === 'pod'

  const workspaceTabs: Array<{ id: WorkspaceSettingsTab; label: string }> = [
    { id: 'agent', label: 'Agent' },
    // Context always visible — holds PROJECT.md; Agent Settings nested inside.
    { id: 'context', label: 'Context' },
    { id: 'connections', label: 'Connections' },
    { id: 'skills', label: 'Skills' },
    { id: 'schedule', label: 'Heartbeats' },
    { id: 'worktrees', label: 'Worktrees' },
    { id: 'import', label: 'Import' },
    { id: 'api', label: 'API' },
  ]
  const visibleTabs = workspaceTabs
  // Header + tab strip always span the full settings column.
  // Tab body width is per-tab: wide for stack/lists, constrained for forms.
  const fullWidthTabContent =
    settingsTab === 'context' ||
    settingsTab === 'schedule' ||
    settingsTab === 'worktrees'

  return (
    <>
    {cropImage && (
      <IconCropDialog
        imageDataUrl={cropImage}
        onConfirm={handleCropConfirm}
        onCancel={() => setCropImage(null)}
      />
    )}
    <div className="flex flex-col h-full min-h-0 w-full">
      {/* ── Sticky header + tabs (full width of the settings content column) ── */}
      <div className="flex-shrink-0 space-y-4 pb-3 pr-1 w-full">
        <div className="flex items-start justify-between gap-3 w-full">
          <div className="min-w-0">
            <h2 className="text-base font-medium text-[var(--color-text-primary)]">{project.name}</h2>
            <p className="text-[11px] text-[var(--color-text-muted)] mt-1 break-all">{project.path}</p>
          </div>
          <button
            onClick={() => {
              const defaultWs = project.workspaces?.[0]
              if (defaultWs) {
                useProjectsStore.getState().setActiveWorkspace(project.id, defaultWs.id)
              }
              useSettingsStore.getState().closeSettings()
            }}
            className="flex-shrink-0 px-3 py-1.5 text-[11px] text-[var(--color-text-secondary)] border border-[var(--color-border)] hover:text-[var(--color-text-primary)] hover:border-[var(--color-text-muted)] transition-colors no-drag cursor-pointer"
          >
            Open Workspace
          </button>
        </div>

        <div
          role="tablist"
          aria-label="Workspace settings"
          className="flex flex-wrap gap-0.5 border-b border-[var(--color-border)] w-full"
        >
          {visibleTabs.map((tab) => {
            const active = settingsTab === tab.id
            return (
              <button
                key={tab.id}
                role="tab"
                type="button"
                aria-selected={active}
                onClick={() => selectSettingsTab(tab.id)}
                className={`px-3 py-2 text-[11px] font-medium transition-colors no-drag cursor-pointer border-b-2 -mb-px ${
                  active
                    ? 'border-[var(--color-accent)] text-[var(--color-text-primary)]'
                    : 'border-transparent text-[var(--color-text-muted)] hover:text-[var(--color-text-secondary)]'
                }`}
              >
                {tab.label}
              </button>
            )
          })}
        </div>
      </div>

      {/* ── Tab panels ──
          Context + Heartbeats use height-filling left/right splits.
          Other tabs scroll normally; narrow tabs keep max-w-3xl. */}
      <div
        className={
          settingsTab === 'context' || settingsTab === 'schedule'
            ? 'flex-1 min-h-0 overflow-hidden flex flex-col w-full pt-3 pr-3 pb-3'
            : `flex-1 overflow-y-auto min-h-0 pt-4 space-y-6 pb-8 pr-3 [scrollbar-gutter:stable] ${
                fullWidthTabContent ? 'w-full max-w-none' : 'w-full max-w-3xl'
              }`
        }
      >
        {settingsTab === 'agent' && (
          <>
            <SettingsGroup title="Identity">
              {/* Icon */}
              <div className="flex items-center gap-4 py-2">
                <div
                  className="flex-shrink-0 flex items-center justify-center overflow-hidden"
                  style={{
                    width: 48,
                    height: 48,
                    backgroundColor: project.iconUrl ? 'transparent' : project.color,
                    border: project.iconUrl ? `2px solid ${project.color}` : 'none'
                  }}
                >
                  {project.iconUrl ? (
                    <img
                      src={project.iconUrl}
                      alt={project.name}
                      style={{ width: '100%', height: '100%', objectFit: 'cover', objectPosition: 'center', display: 'block' }}
                    />
                  ) : (
                    <span
                      className="text-[var(--color-on-accent)] font-bold"
                      style={{ fontSize: 22, lineHeight: 1 }}
                    >
                      {firstLetter}
                    </span>
                  )}
                </div>
                <div className="flex items-center gap-2">
                  <button
                    onClick={handleDetectIcon}
                    disabled={iconLoading}
                    className="px-2.5 py-1 text-xs text-[var(--color-text-secondary)] border border-[var(--color-border)] hover:bg-[var(--color-bg-elevated)] hover:text-[var(--color-text-primary)] no-drag cursor-pointer disabled:opacity-50 disabled:cursor-not-allowed transition-colors"
                  >
                    {iconLoading ? 'Working...' : 'Detect'}
                  </button>
                  <button
                    onClick={handleUploadClick}
                    disabled={iconLoading}
                    className="px-2.5 py-1 text-xs text-[var(--color-text-secondary)] border border-[var(--color-border)] hover:bg-[var(--color-bg-elevated)] hover:text-[var(--color-text-primary)] no-drag cursor-pointer disabled:opacity-50 disabled:cursor-not-allowed transition-colors"
                  >
                    Upload
                  </button>
                  <input
                    ref={fileInputRef}
                    type="file"
                    accept="image/png,image/jpeg,image/svg+xml,image/x-icon"
                    className="hidden"
                    onChange={handleFileSelected}
                  />
                  {project.iconUrl && (
                    <button
                      onClick={handleClearIcon}
                      disabled={iconLoading}
                      className="px-2.5 py-1 text-xs text-[var(--color-status-error-soft)] border border-[color-mix(in_srgb,var(--color-status-error)_30%,transparent)] hover:bg-[color-mix(in_srgb,var(--color-status-error)_10%,transparent)] no-drag cursor-pointer disabled:opacity-50 disabled:cursor-not-allowed transition-colors"
                    >
                      Remove
                    </button>
                  )}
                </div>
              </div>

              {/* Color */}
              <div className="flex items-center justify-between py-2 border-t border-[var(--color-border)]">
                <span className="text-xs text-[var(--color-text-secondary)]">Color</span>
                <div className="flex items-center gap-1.5">
                  {['#3b82f6', '#ef4444', '#22c55e', '#f59e0b', '#a855f7', '#ec4899', '#06b6d4', '#64748b'].map((color) => (
                    <button
                      key={color}
                      onClick={() => {
                        // Optimistic store path: paints immediately, POSTs color,
                        // rolls back on failure — no success-path N+1 refetch.
                        void useProjectsStore.getState().setProjectColor(project.id, color)
                      }}
                      className={`w-4 h-4 flex-shrink-0 no-drag cursor-pointer transition-transform ${
                        project.color === color ? 'scale-125 ring-1 ring-white/50' : 'hover:scale-110'
                      }`}
                      style={{ backgroundColor: color }}
                    />
                  ))}
                </div>
              </div>

              {/* Agent display name — restored on the Agent tab Identity group
                  (below icon + color). Was only reachable via manager/custom
                  persona helpers after the settings-tab redesign; users lost
                  the obvious place to rename the agent. Saves via
                  setAgentDisplayName → AGENT.md display_name frontmatter. */}
              <div className="py-2 border-t border-[var(--color-border)]">
                <AgentDisplayNameField
                  projectPath={project.path}
                  helpText="Shown in the nav and Workspace tab. Does not change the handle or federated address."
                />
                <AgentHandleField projectPath={project.path} projectHandle={project.handle} />
              </div>

              {/* Focus Group */}
              {focusGroupsEnabled && (
                <div className="flex items-center justify-between py-2 border-t border-[var(--color-border)]">
                  <span className="text-xs text-[var(--color-text-secondary)]">Focus Group</span>
                  <SettingDropdown
                    value={project.focusGroupId ?? ''}
                    options={[
                      { value: '', label: 'No Group' },
                      ...focusGroups.map((g) => ({ value: g.id, label: g.name })),
                    ]}
                    onChange={async (v) => {
                      await assignProjectToGroup(project.id, v || null)
                      await fetchProjects()
                    }}
                  />
                </div>
              )}

              <DefaultAgentSelector projectId={project.id} currentDefaultAgent={project.defaultAgent} />
              <DefaultModelControls project={project} />
              <WorkspaceCompletionSoundToggle project={project} />
            </SettingsGroup>

            <SettingsGroup title="Remote Access">
              <RemoteInstructToggle project={project} fetchProjects={fetchProjects} />
            </SettingsGroup>
            <SettingsGroup title="DNS">
              <DnsManageToggle project={project} fetchProjects={fetchProjects} />
            </SettingsGroup>

            <div className="pt-2 border-t border-[var(--color-border)]">
              <button
                onClick={() => removeProject(project.id)}
                className="px-3 py-1 text-xs text-[var(--color-status-error-soft)] border border-[color-mix(in_srgb,var(--color-status-error)_30%,transparent)] hover:bg-[color-mix(in_srgb,var(--color-status-error)_10%,transparent)] no-drag cursor-pointer"
              >
                Remove Workspace
              </button>
            </div>
          </>
        )}

        {settingsTab === 'context' && (
          /* Full tab body: left stack + Canonical Agent | right FileViewer */
          <div className="h-full min-h-0 w-full">
            <ContextStackEditor
              projectPath={project.path}
              onEdit={openContextEdit}
              canonicalSlot={
                <CanonicalAgentButton
                  probes={canonicalProbes}
                  projectPath={project.path}
                  onOpen={(mode) => setCanonicalModalMode(mode)}
                />
              }
            />
          </div>
        )}

        {settingsTab === 'connections' && (
          <>
            <SettingsGroup title="Connections">
              <AgentsCreateConnectionsToggle project={project} fetchProjects={fetchProjects} />
            </SettingsGroup>
            <SettingsGroup title="Connected Workspaces">
              <ConnectedWorkspacesPanel projectId={project.id} />
            </SettingsGroup>
          </>
        )}

        {settingsTab === 'skills' && (
          <SettingsGroup title="Skills">
            <ProjectSkillsPanel projectPath={project.path} onOpenEditor={(name) => { setAgentEditorName(name); setAgentEditorOpen(true) }} />
          </SettingsGroup>
        )}

        {settingsTab === 'schedule' && (
          <div className="flex flex-col h-full min-h-0">
            {/* Left: heartbeats roster · Right: run history — independent scroll */}
            <div className="flex-1 min-h-0 flex flex-row border border-[var(--color-border)]">
              <div className="flex-1 min-w-0 min-h-0 flex flex-col border-r border-[var(--color-border)]">
                <div className="flex-1 min-h-0 overflow-y-auto p-3 [scrollbar-gutter:stable]">
                  <HeartbeatsPanel
                    key={`hb-${hbRefreshNonce}`}
                    projectPath={project.path}
                    agentMode={project.agentMode || 'custom'}
                    agentName={primaryAgentName
                      || project.name.toLowerCase().replace(/\s+/g, '-')}
                    onConfigureWakeup={(row) => setWakeupEditingHb(row)}
                  />
                </div>
                <div className="flex-shrink-0 border-t border-[var(--color-border)] px-3 py-2">
                  <ShowHeartbeatSessionsToggle projectPath={project.path} />
                </div>
              </div>
              <div className="w-[min(44%,26rem)] min-w-[16rem] max-w-md flex-shrink-0 min-h-0 flex flex-col p-3">
                <HistoryPanel
                  projectPath={project.path}
                  onEmptyChange={setHistoryEmpty}
                  fillHeight
                />
              </div>
            </div>
          </div>
        )}

        {settingsTab === 'worktrees' && (
          <SettingsGroup title="Worktrees">
            <div className={project.workspaces.length > 0 ? '' : 'hidden'}>
              <div className="border border-[var(--color-border)]">
                {project.workspaces.map((ws, i) => (
                  <div
                    key={ws.id}
                    className={`flex items-center gap-2 px-3 py-1.5 ${
                      i < project.workspaces.length - 1 ? 'border-b border-[var(--color-border)]' : ''
                    }`}
                  >
                    <svg className="w-3 h-3 flex-shrink-0 text-[var(--color-text-muted)]" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2}>
                      <path strokeLinecap="round" strokeLinejoin="round" d="M13 10V3L4 14h7v7l9-11h-7z" />
                    </svg>
                    <span className="text-xs text-[var(--color-text-primary)] flex-1 truncate">{ws.name}</span>
                    {ws.branch && (
                      <span className="text-[10px] text-[var(--color-text-muted)] truncate max-w-[120px]">{ws.branch}</span>
                    )}
                    <span className="text-[10px] text-[var(--color-text-muted)]">{ws.type}</span>
                  </div>
                ))}
              </div>
            </div>
            {project.workspaces.length === 0 && (
              <p className="text-[10px] text-[var(--color-text-muted)] py-1">No registered worktrees for this workspace.</p>
            )}
            <WorktreeFoldersOnDisk project={project} fetchProjects={fetchProjects} />
          </SettingsGroup>
        )}

        {settingsTab === 'import' && (
          <SettingsGroup title="Chat Migrations">
            <CursorMigrationPanel projectPath={project.path} />
          </SettingsGroup>
        )}
        {settingsTab === 'api' && (
          <div className="space-y-6">
            <SettingsGroup title="Sessions">
              <HideApiSessionsToggle project={project} />
            </SettingsGroup>
            <WorkspaceHostSessionCapPanel projectPath={project.path} />
            <WorkspaceApiKeysPanel workspaceSlug={workspaceGrantSlug(project)} />
          </div>
        )}
      </div>
    </div>
    </>
  )
}

// ── Host-session concurrent cap (workspace API tab) ─────────────────
//
// Per-workspace ceiling for concurrent live /v1 host-sessions (Scout etc.).
// Default 15 (daemon env K2_SANDBOX_WORKSPACE_CELL_CAP); max 512.
// CLI: `k2 workspace api-host-session-cap get|set <ws>`.

function WorkspaceHostSessionCapPanel({ projectPath }: { projectPath: string }): React.JSX.Element {
  const [raw, setRaw] = useState<number | null | undefined>(undefined) // undefined=loading, null=inherit
  const [draft, setDraft] = useState('')
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [hint, setHint] = useState<string | null>(null)

  const load = useCallback(async () => {
    setError(null)
    try {
      const s = await daemonCliGet<Record<string, unknown>>('settings', { project: projectPath })
      const v = s.hostSessionCellCap ?? s.host_session_cell_cap
      if (v === null || v === undefined) {
        setRaw(null)
        setDraft('')
      } else {
        const n = typeof v === 'number' ? v : parseInt(String(v), 10)
        setRaw(Number.isFinite(n) ? n : null)
        setDraft(Number.isFinite(n) ? String(n) : '')
      }
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
      setRaw(null)
    }
  }, [projectPath])

  useEffect(() => {
    void load()
  }, [load])

  const save = async (value: string): Promise<void> => {
    setBusy(true)
    setError(null)
    setHint(null)
    try {
      await daemonCliPost('workspace/set', {
        project: projectPath,
        fields: { host_session_cell_cap: value },
      })
      setHint(value === 'default' ? 'Cleared → inherit daemon default (15 or env).' : `Set to ${value}.`)
      await load()
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    } finally {
      setBusy(false)
    }
  }

  return (
    <div className="space-y-3 max-w-3xl" data-settings-id="projects.host-session-cell-cap">
      <div>
        <h3 className="text-sm font-medium text-[var(--color-text-primary)]">
          Concurrent host sessions
        </h3>
        <p className="text-[11px] text-[var(--color-text-muted)] mt-1">
          Max simultaneous live <code className="text-[10px]">/v1</code> host-session cells for this
          workspace (not total historical list rows). Default{' '}
          <strong className="font-medium text-[var(--color-text-secondary)]">15</strong> (or{' '}
          <code className="text-[10px]">K2_SANDBOX_WORKSPACE_CELL_CAP</code>). Max 512.
        </p>
      </div>
      <div className="flex flex-wrap items-center gap-2">
        <input
          type="number"
          min={1}
          max={512}
          placeholder="default (15)"
          value={draft}
          disabled={busy || raw === undefined}
          onChange={(e) => setDraft(e.target.value)}
          className="w-28 px-2 py-1 text-[12px] border border-[var(--color-border)] bg-[var(--color-bg)] text-[var(--color-text-primary)]"
        />
        <button
          type="button"
          disabled={busy || raw === undefined || !draft.trim()}
          onClick={() => void save(draft.trim())}
          className="px-2 py-1 text-[11px] border border-[var(--color-border)] cursor-pointer disabled:opacity-50"
        >
          Save
        </button>
        <button
          type="button"
          disabled={busy || raw === undefined || raw === null}
          onClick={() => void save('default')}
          className="px-2 py-1 text-[11px] border border-[var(--color-border)] cursor-pointer disabled:opacity-50"
        >
          Use default
        </button>
      </div>
      <p className="text-[10px] text-[var(--color-text-muted)]">
        Current:{' '}
        {raw === undefined
          ? '…'
          : raw === null
            ? 'inherit daemon default (15 or env)'
            : `${raw} concurrent`}
        {' · '}
        CLI: <code className="text-[10px]">k2 workspace api-host-session-cap get|set</code>
      </p>
      {error && <p className="text-[11px] text-[var(--color-status-error-soft)]">{error}</p>}
      {hint && !error && <p className="text-[11px] text-[var(--color-status-ok-soft)]">{hint}</p>}
    </div>
  )
}

// ── Show Heartbeat Sessions Toggle ──────────────────────────────────
//
// Per-workspace flag controlling whether heartbeat fires open a tab in
// the Tauri window. Default OFF (silent autonomous run, the v2-headless
// vision default). When ON, each fire opens a background tab (no focus
// steal); the user closes it when done auditing. State lives in
// `projects.show_heartbeat_sessions` (migration 0034).

function ShowHeartbeatSessionsToggle({ projectPath }: { projectPath: string }): React.JSX.Element {
  const [enabled, setEnabled] = useState<boolean | null>(null) // null = loading
  const [busy, setBusy] = useState(false)

  useEffect(() => {
    let cancelled = false
    invoke<boolean>('k2so_workspace_get_show_heartbeat_sessions', { projectPath })
      .then((v) => { if (!cancelled) setEnabled(v) })
      .catch((err) => {
        // Surface the error rather than silently defaulting — a missing
        // row here means the project_id resolution broke and the toggle
        // would otherwise lie to the user about its state.
        console.error('[show-heartbeat-sessions] read failed', err)
      })
    return () => { cancelled = true }
  }, [projectPath])

  const toggle = async (): Promise<void> => {
    if (enabled === null || busy) return
    const next = !enabled
    setBusy(true)
    setEnabled(next) // optimistic
    try {
      await daemonCliPost('heartbeat/set-show-sessions', {
        project_path: projectPath,
        enabled: next,
      })
    } catch (err) {
      console.error('[show-heartbeat-sessions] write failed', err)
      setEnabled(!next) // revert on failure
    } finally {
      setBusy(false)
    }
  }

  if (enabled === null) {
    return (
      <div className="text-[10px] text-[var(--color-text-muted)]">Loading…</div>
    )
  }

  return (
    <div className="border border-[var(--color-border)] p-3">
      <div className="flex items-start gap-3">
        <button
          onClick={toggle}
          role="switch"
          aria-checked={enabled}
          disabled={busy}
          className={`mt-0.5 w-7 h-3.5 flex items-center transition-colors no-drag cursor-pointer flex-shrink-0 disabled:opacity-50 ${
            enabled ? 'bg-[var(--color-accent)]' : 'bg-[var(--color-border)]'
          }`}
          title={enabled ? 'Heartbeat fires open background tabs' : 'Heartbeat fires run silently'}
        >
          <span
            className={`w-2.5 h-2.5 bg-[var(--color-on-accent)] block transition-transform ${
              enabled ? 'translate-x-3.5' : 'translate-x-0.5'
            }`}
          />
        </button>
        <div className="flex-1 min-w-0">
          <div className="text-xs font-medium text-[var(--color-text-primary)]">
            Show heartbeat sessions in tabs
          </div>
          <div className="text-[10px] text-[var(--color-text-muted)] mt-1 leading-relaxed">
            {enabled
              ? 'Each heartbeat fire opens a background tab in this window. Tabs persist until you close them. Audit the agent\'s work as it happens.'
              : 'Heartbeat fires run silently in the daemon (recommended). Audit them on demand from the sidebar Heartbeats panel.'}
          </div>
        </div>
      </div>
    </div>
  )
}

// ── #67 Per-workspace remote-instruct opt-in ───────────────────────
// Lets CONNECT-USERS (role >= Member, signed into this host over K2
// Connect) message THIS workspace's agent via the composer. The OWNER is
// always allowed regardless. DEFAULTS OFF / fail-closed: the composer
// instructs an agent running --dangerously-skip-permissions (= full shell
// + filesystem access), so a workspace must be explicitly opted in. The
// daemon ENFORCES this server-side per-workspace; this toggle only records
// the opt-in (and drives the composer-hide). Reads the current state from
// the projects store (`allowRemoteInstruct`); writes via /cli/remote-instruct.
function RemoteInstructToggle({
  project,
  fetchProjects,
}: {
  project: ProjectWithWorkspaces
  fetchProjects: () => Promise<void>
}): React.JSX.Element {
  const [busy, setBusy] = useState(false)
  const enabled = (project.allowRemoteInstruct ?? 0) === 1
  // The app-level master (K2 Connect → "Let remote users message agents")
  // OR-overrides this per-workspace flag server-side
  // (`remote_instruct_allowed_for_path`): with the master ON, remote
  // messages land here even while this toggle shows OFF. Surface that so
  // an OFF toggle never reads as a deny it doesn't enforce.
  const globalAllow = useSettingsStore((s) => s.allowRemoteInstruct)

  const toggle = async (): Promise<void> => {
    if (busy) return
    const next = !enabled
    setBusy(true)
    try {
      // Path-scoped GET write (mirrors /cli/worktree). The daemon still
      // enforces the gate server-side; this only records the opt-in.
      await daemonCliGet('remote-instruct', {
        project: project.path,
        enable: next ? '1' : '0',
      })
      await fetchProjects()
    } catch (err) {
      console.error('[remote-instruct] write failed', err)
    } finally {
      setBusy(false)
    }
  }

  return (
    <div className="border border-[var(--color-border)] p-3">
      <div className="flex items-start gap-3">
        <button
          onClick={toggle}
          role="switch"
          aria-checked={enabled}
          disabled={busy}
          data-settings-id="projects.allow-remote-instruct"
          className={`mt-0.5 w-7 h-3.5 flex items-center transition-colors no-drag cursor-pointer flex-shrink-0 disabled:opacity-50 ${
            enabled ? 'bg-[var(--color-accent)]' : 'bg-[var(--color-border)]'
          }`}
          title={enabled ? 'Remote users can message this workspace\'s agent' : 'Only the owner can message this workspace\'s agent'}
        >
          <span
            className={`w-2.5 h-2.5 bg-[var(--color-on-accent)] block transition-transform ${
              enabled ? 'translate-x-3.5' : 'translate-x-0.5'
            }`}
          />
        </button>
        <div className="flex-1 min-w-0">
          <div className="text-xs font-medium text-[var(--color-text-primary)]">
            Let remote users message this workspace
          </div>
          <div className="text-[10px] text-[var(--color-text-muted)] mt-1 leading-relaxed">
            {enabled
              ? 'People signed into this host over K2 Connect can send messages to this workspace\'s agent via the composer. You (the owner) can always message agents.'
              : 'Off (recommended): only you can message this workspace\'s agent. Turn on to let K2 Connect users instruct the agent here — it runs with full shell and filesystem access.'}
          </div>
          {!enabled && globalAllow && (
            <div className="text-[10px] text-[color-mix(in_srgb,var(--color-status-warn-amber-soft)_80%,transparent)] mt-1 leading-relaxed">
              Currently allowed anyway: the global &ldquo;Let remote users message
              agents&rdquo; switch (Settings → K2 Connect) opts in every workspace,
              overriding this toggle.
            </div>
          )}
        </div>
      </div>
    </div>
  )
}

// ── DNS K1 Per-workspace DNS-manage opt-in ─────────────────────────
// Lets agents create/update/delete DNS records for THIS workspace.
// DEFAULTS OFF / fail-closed. The daemon ENFORCES this server-side via
// `dns_manage_allowed_for_path` (app master OR per-workspace). Reads
// from the projects store (`dnsManageEnabled`); writes via /cli/dns-manage.
function DnsManageToggle({
  project,
  fetchProjects,
}: {
  project: ProjectWithWorkspaces
  fetchProjects: () => Promise<void>
}): React.JSX.Element {
  const [busy, setBusy] = useState(false)
  const enabled = (project.dnsManageEnabled ?? 0) === 1
  // The app-level master (K2 Connect → "Allow agents to manage DNS records")
  // OR-overrides this per-workspace flag server-side
  // (`dns_manage_allowed_for_path`): with the master ON, DNS manage is
  // allowed here even while this toggle shows OFF. Surface that so an
  // OFF toggle never reads as a deny it doesn't enforce.
  const globalAllow = useSettingsStore((s) => s.dnsManageEnabled)

  const toggle = async (): Promise<void> => {
    if (busy) return
    const next = !enabled
    setBusy(true)
    try {
      await daemonCliGet('dns-manage', {
        project: project.path,
        enable: next ? '1' : '0',
      })
      await fetchProjects()
    } catch (err) {
      console.error('[dns-manage] write failed', err)
    } finally {
      setBusy(false)
    }
  }

  return (
    <div className="border border-[var(--color-border)] p-3">
      <div className="flex items-start gap-3">
        <button
          onClick={toggle}
          role="switch"
          aria-checked={enabled}
          disabled={busy}
          data-settings-id="projects.dns-manage-enabled"
          className={`mt-0.5 w-7 h-3.5 flex items-center transition-colors no-drag cursor-pointer flex-shrink-0 disabled:opacity-50 ${
            enabled ? 'bg-[var(--color-accent)]' : 'bg-[var(--color-border)]'
          }`}
          title={enabled ? 'Agents may manage DNS for this workspace' : 'Agents cannot manage DNS for this workspace'}
        >
          <span
            className={`w-2.5 h-2.5 bg-[var(--color-on-accent)] block transition-transform ${
              enabled ? 'translate-x-3.5' : 'translate-x-0.5'
            }`}
          />
        </button>
        <div className="flex-1 min-w-0">
          <div className="text-xs font-medium text-[var(--color-text-primary)]">
            Allow agents to manage DNS for this workspace
          </div>
          <div className="text-[10px] text-[var(--color-text-muted)] mt-1 leading-relaxed">
            {enabled
              ? 'Agents in this workspace may create, update, and delete DNS records.'
              : 'Off (recommended): agents in this workspace cannot mutate DNS. Turn on to grant DNS management for this workspace only.'}
          </div>
          {!enabled && globalAllow && (
            <div className="text-[10px] text-[color-mix(in_srgb,var(--color-status-warn-amber-soft)_80%,transparent)] mt-1 leading-relaxed">
              Currently allowed anyway: the global &ldquo;Allow agents to manage DNS
              records&rdquo; switch (Settings → K2 Connect) opts in every workspace,
              overriding this toggle.
            </div>
          )}
        </div>
      </div>
    </div>
  )
}

// ── C1 Per-workspace agents-may-create-connections opt-in ──────────
// Lets agents add/remove workspace connections for THIS workspace.
// DEFAULTS OFF / fail-closed. Owner always may manage connections.
// The daemon ENFORCES this server-side via
// `agents_can_create_connections_for_path` (app master OR per-workspace).
// Reads from the projects store (`agentsCanCreateConnections`); writes
// via /cli/agents-create-connections.
function AgentsCreateConnectionsToggle({
  project,
  fetchProjects,
}: {
  project: ProjectWithWorkspaces
  fetchProjects: () => Promise<void>
}): React.JSX.Element {
  const [busy, setBusy] = useState(false)
  const enabled = (project.agentsCanCreateConnections ?? 0) === 1
  const globalAllow = useSettingsStore((s) => s.agentsCanCreateConnections)

  const toggle = async (): Promise<void> => {
    if (busy) return
    const next = !enabled
    setBusy(true)
    try {
      await daemonCliGet('agents-create-connections', {
        project: project.path,
        enable: next ? '1' : '0',
      })
      await fetchProjects()
    } catch (err) {
      console.error('[agents-create-connections] write failed', err)
    } finally {
      setBusy(false)
    }
  }

  return (
    <div className="border border-[var(--color-border)] p-3">
      <div className="flex items-start gap-3">
        <button
          onClick={toggle}
          role="switch"
          aria-checked={enabled}
          disabled={busy}
          data-settings-id="projects.agents-can-create-connections"
          className={`mt-0.5 w-7 h-3.5 flex items-center transition-colors no-drag cursor-pointer flex-shrink-0 disabled:opacity-50 ${
            enabled ? 'bg-[var(--color-accent)]' : 'bg-[var(--color-border)]'
          }`}
          title={
            enabled
              ? 'Agents may add/remove connections for this workspace'
              : 'Agents cannot add/remove connections for this workspace'
          }
        >
          <span
            className={`w-2.5 h-2.5 bg-[var(--color-on-accent)] block transition-transform ${
              enabled ? 'translate-x-3.5' : 'translate-x-0.5'
            }`}
          />
        </button>
        <div className="flex-1 min-w-0">
          <div className="text-xs font-medium text-[var(--color-text-primary)]">
            Allow agents to create connections for this workspace
          </div>
          <div className="text-[10px] text-[var(--color-text-muted)] mt-1 leading-relaxed">
            {enabled
              ? "Agents in this workspace may add and remove connections to other workspaces. You (the owner) can always manage connections."
              : "Off (recommended): agents in this workspace cannot add or remove connections. Turn on to grant connection management for this workspace only. You (the owner) can always manage connections."}
          </div>
          {!enabled && globalAllow && (
            <div className="text-[10px] text-[color-mix(in_srgb,var(--color-status-warn-amber-soft)_80%,transparent)] mt-1 leading-relaxed">
              Currently allowed anyway: the global &ldquo;Allow agents to create
              connections&rdquo; switch (Settings → K2 Connect) opts in every
              workspace, overriding this toggle.
            </div>
          )}
        </div>
      </div>
    </div>
  )
}

// ── K2 Agents Panel ───────────────────────────────────────────────

interface K2soAgentInfo {
  name: string
  role: string
  inboxCount: number
  activeCount: number
  doneCount: number
  isCoordinator: boolean // legacy field name from backend; true = manager agent
}

function AgentKebabMenu({ onSettings, onDelete }: { onSettings: () => void; onDelete?: () => void }): React.JSX.Element {
  const [open, setOpen] = useState(false)
  const menuRef = useRef<HTMLDivElement>(null)

  useEffect(() => {
    if (!open) return
    const handleClick = (e: MouseEvent) => {
      if (menuRef.current && !menuRef.current.contains(e.target as Node)) {
        setOpen(false)
      }
    }
    document.addEventListener('mousedown', handleClick)
    return () => document.removeEventListener('mousedown', handleClick)
  }, [open])

  return (
    <div className="relative" ref={menuRef}>
      <button
        onClick={() => setOpen(!open)}
        className="px-1 py-0.5 text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)] transition-colors no-drag cursor-pointer"
        title="More options"
      >
        <svg width="12" height="12" viewBox="0 0 16 16" fill="currentColor">
          <circle cx="8" cy="3" r="1.5" />
          <circle cx="8" cy="8" r="1.5" />
          <circle cx="8" cy="13" r="1.5" />
        </svg>
      </button>
      {open && (
        <div className="absolute right-0 top-full mt-1 z-50 bg-[var(--color-bg-elevated)] border border-[var(--color-border)] shadow-lg min-w-[140px]">
          <button
            onClick={() => { setOpen(false); onSettings() }}
            className="w-full text-left px-3 py-1.5 text-[11px] text-[var(--color-text-secondary)] hover:bg-[var(--color-bg-hover)] hover:text-[var(--color-text-primary)] transition-colors no-drag cursor-pointer"
          >
            Settings
          </button>
          {onDelete && (
            <button
              onClick={() => { setOpen(false); onDelete() }}
              className="w-full text-left px-3 py-1.5 text-[11px] text-[var(--color-status-error-soft)] hover:bg-[color-mix(in_srgb,var(--color-status-error)_10%,transparent)] hover:text-[var(--color-status-error-bright)] transition-colors no-drag cursor-pointer"
            >
              Delete Agent
            </button>
          )}
        </div>
      )}
    </div>
  )
}

// ── Default Agent Selector (per-workspace dropdown, migration 0063) ──────
//
// Agent de-generalization Slice 1: the per-workspace default-agent
// override (`projects.default_agent`). Value semantics: an agent_presets
// preset id (UUID); a stored LEGACY command token like "claude" is
// tolerated read-side by matching the preset command's first token
// (Slice 0's tolerant matching). ''/null = inherit the global
// Settings → Editors & Agents default at resolve time.
//
// Local component with optimistic store update. POST `projects/update`
// (empty string clears the column back to NULL).

function DefaultAgentSelector({ projectId, currentDefaultAgent }: { projectId: string; currentDefaultAgent?: string | null }): React.JSX.Element {
  const presets = usePresetsStore((s) => s.presets)
  const globalDefaultAgent = useSettingsStore((s) => s.defaultAgent)
  const [selected, setSelected] = useState(currentDefaultAgent || '')

  useEffect(() => {
    setSelected(currentDefaultAgent || '')
  }, [currentDefaultAgent])

  // Tolerant resolve: preset id first (canonical), then legacy command
  // token (what the global setting historically stored).
  const resolvePreset = (val: string) =>
    presets.find((p) => p.id === val) ?? presets.find((p) => p.command.split(/\s+/)[0] === val)

  const globalPreset = resolvePreset(globalDefaultAgent)
  const enabledPresets = presets.filter((p) => p.enabled !== 0)
  // A stored legacy token normalizes to its preset id so the dropdown
  // highlights the matching row; a value that matches NO preset falls
  // through to SettingDropdown's placeholder (shown muted) instead of
  // silently reading as "Inherit" or as the first option.
  const selectedPreset = selected ? resolvePreset(selected) : undefined
  const value = selected ? (selectedPreset?.id ?? selected) : ''

  const handleChange = async (presetId: string): Promise<void> => {
    setSelected(presetId)
    try {
      await daemonCliPost('projects/update', { id: projectId, defaultAgent: presetId || '' })
      emitProjectsChanged()
      const store = useProjectsStore.getState()
      const updated = store.projects.map((p) =>
        p.id === projectId ? { ...p, defaultAgent: presetId || null } : p
      )
      useProjectsStore.setState({ projects: updated })
    } catch (err) {
      console.error('[default-agent-selector] Update failed:', err)
    }
  }

  return (
    <div className="flex items-center justify-between py-2 border-t border-[var(--color-border)]">
      <div>
        <span className="text-xs text-[var(--color-text-secondary)]">Default Agent</span>
        <p className="text-[9px] text-[var(--color-text-muted)] mt-0.5">
          The agent ⇧⌘T launches for this workspace. Edit-with-AI uses Settings → Default AI Agent.
        </p>
      </div>
      <SettingDropdown
        value={value}
        options={[
          { value: '', label: `Inherit global (${globalPreset?.label ?? globalDefaultAgent})` },
          ...enabledPresets.map((p) => ({ value: p.id, label: p.label })),
        ]}
        onChange={handleChange}
        placeholder={selected || undefined}
      />
    </div>
  )
}

function binaryTokenFromCommand(command: string): string {
  const first = command.trim().split(/\s+/)[0] ?? ''
  const base = first.split(/[/\\]/).pop() ?? first
  return base.replace(/\.exe$/i, '')
}

function modelSuggestionsForAgent(command: string): string[] {
  switch (binaryTokenFromCommand(command)) {
    case 'claude':
      return ['opus', 'sonnet', 'haiku']
    case 'codex':
      return ['gpt-5.3-codex', 'o3', 'gpt-4.1']
    case 'grok':
      return ['grok-4', 'grok-3-mini']
    case 'gemini':
      return ['gemini-2.5-pro', 'gemini-2.5-flash']
    case 'cursor-agent':
    case 'agent':
      return ['composer-1']
    default:
      return []
  }
}

function DefaultModelControls({
  project,
}: {
  project: ProjectWithWorkspaces
}): React.JSX.Element {
  const presets = usePresetsStore((s) => s.presets)
  const globalDefaultAgent = useSettingsStore((s) => s.defaultAgent)
  const [model, setModel] = useState(project.defaultModel ?? '')
  const [force, setForce] = useState((project.forceModelOnResume ?? 0) !== 0)
  const [custom, setCustom] = useState('')

  useEffect(() => {
    setModel(project.defaultModel ?? '')
  }, [project.defaultModel])
  useEffect(() => {
    setForce((project.forceModelOnResume ?? 0) !== 0)
  }, [project.forceModelOnResume])

  const resolvePreset = (val: string) =>
    presets.find((p) => p.id === val) ?? presets.find((p) => p.command.split(/\s+/)[0] === val)

  const selectedAgent = project.defaultAgent || globalDefaultAgent
  const agentPreset = selectedAgent ? resolvePreset(selectedAgent) : undefined
  const suggestions = modelSuggestionsForAgent(agentPreset?.command ?? selectedAgent ?? '')

  const persist = async (nextModel: string, nextForce: boolean): Promise<void> => {
    const stored = nextModel.trim()
    const forceVal = stored && nextForce ? 1 : 0
    try {
      await daemonCliPost('projects/update', {
        id: project.id,
        defaultModel: stored,
        forceModelOnResume: forceVal,
      })
      emitProjectsChanged()
      const store = useProjectsStore.getState()
      const updated = store.projects.map((p) =>
        p.id === project.id
          ? { ...p, defaultModel: stored || null, forceModelOnResume: forceVal }
          : p,
      )
      useProjectsStore.setState({ projects: updated })
    } catch (err) {
      console.error('[default-model] Update failed:', err)
    }
  }

  const handleChip = (id: string): void => {
    const next = model === id ? '' : id
    setModel(next)
    setCustom('')
    if (!next) setForce(false)
    void persist(next, next ? force : false)
  }

  const handleClear = (): void => {
    setModel('')
    setCustom('')
    setForce(false)
    void persist('', false)
  }

  const handleCustomSubmit = (): void => {
    const next = custom.trim()
    if (!next) return
    setModel(next)
    setCustom('')
    void persist(next, force)
  }

  const handleForce = (next: boolean): void => {
    if (!model.trim()) return
    setForce(next)
    void persist(model, next)
  }

  const empty = !model.trim()

  return (
    <div className="py-2 border-t border-[var(--color-border)]" data-settings-id="projects.default-model">
      <div className="flex items-start justify-between gap-3">
        <div className="min-w-0">
          <span className="text-xs text-[var(--color-text-secondary)]">Default model</span>
          <p className="text-[9px] text-[var(--color-text-muted)] mt-0.5 leading-relaxed">
            New sessions of this workspace’s agent use this model. Check the box to also pass it on resume (new process). A live chat cannot change model. API `model` on a host-session overrides this for that call, including resume.
          </p>
        </div>
        {model ? (
          <button
            type="button"
            onClick={handleClear}
            className="text-[10px] text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)] no-drag cursor-pointer flex-shrink-0"
          >
            Clear
          </button>
        ) : null}
      </div>
      <div className="flex flex-wrap items-center gap-1 mt-2">
        {suggestions.map((id) => (
          <button
            key={id}
            type="button"
            onClick={() => handleChip(id)}
            className={`px-2 py-0.5 text-[10px] no-drag cursor-pointer border ${
              model === id
                ? 'bg-[var(--color-accent)] text-[var(--color-on-accent)] border-[var(--color-accent)]'
                : 'text-[var(--color-text-secondary)] border-[var(--color-border)] hover:text-[var(--color-text-primary)]'
            }`}
          >
            {id}
          </button>
        ))}
        <input
          type="text"
          value={suggestions.includes(model) ? custom : (custom || model)}
          onChange={(e) => {
            setCustom(e.target.value)
            if (!suggestions.includes(e.target.value)) setModel(e.target.value)
          }}
          onBlur={() => {
            if (custom.trim() && custom.trim() !== (project.defaultModel ?? '')) {
              handleCustomSubmit()
            }
          }}
          onKeyDown={(e) => {
            if (e.key === 'Enter') handleCustomSubmit()
          }}
          placeholder="custom id"
          className="px-2 py-0.5 text-[10px] w-28 bg-[var(--color-bg-elevated)] border border-[var(--color-border)] text-[var(--color-text-primary)] no-drag"
        />
      </div>
      <label
        className={`flex items-center gap-2 mt-2 text-[10px] no-drag ${
          empty ? 'text-[var(--color-text-muted)] opacity-60' : 'text-[var(--color-text-secondary)] cursor-pointer'
        }`}
        data-settings-id="projects.force-model-on-resume"
      >
        <input
          type="checkbox"
          checked={force && !empty}
          disabled={empty}
          onChange={(e) => handleForce(e.target.checked)}
          className="no-drag"
        />
        Force this model when resuming a session
      </label>
    </div>
  )
}

// ── Workspace Wake-up Editor — REMOVED in 0.32.6 ──────
// `.k2so/WAKEUP.md` retired; manager wake content now lives in the
// per-row `triage` heartbeat's WAKEUP.md (see Heartbeats panel).

// ── Workspace Knowledge editor (PROJECT.md — the source) ─────────────
// PROJECT.md is the single source for workspace knowledge. K2's
// regen pipeline compiles it (plus each agent's AGENT.md) into the
// per-agent SKILL.md files, then symlinks/marker-injects those into
// every CLI harness file (Claude/OpenCode/Pi/Codex/Gemini/Cursor/etc.).
// Editing PROJECT.md is the right surface; editing the compiled
// SKILL.md (the prior shape of this editor) led to silent overwrites
// because the regen pipeline owns the output.

/** AI File Editor for an arbitrary context-stack markdown path. */
function ContextLayerFileEditor({
  filePath,
  label,
  projectPath,
  projectName,
  onClose,
}: {
  filePath: string
  label: string
  projectPath: string
  projectName: string
  onClose: () => void
}): React.JSX.Element {
  const [content, setContent] = useState('')
  const [previewMode, setPreviewMode] = useState<'preview' | 'edit'>('preview')
  const [previewScale, setPreviewScale] = useState(100)
  const cssScale = Math.round(previewScale * 0.7)
  const agentCommand = useResolvedAgentCommand(undefined, { projectPath, scope: 'global' })
  const watchDir = filePath.includes('/')
    ? filePath.slice(0, filePath.lastIndexOf('/'))
    : projectPath

  useEffect(() => {
    daemonCliGet<{ content: string }>('fs/read-file', { path: filePath })
      .then((r) => setContent(r.content))
      .catch(() => setContent(''))
  }, [filePath])

  const handleClose = useCallback(async () => {
    try {
      await daemonCliPost('agents/regenerate-workspace-skill', { project_path: projectPath })
    } catch (err) {
      console.warn('[context-layer] regen on close failed:', err)
    }
    onClose()
  }, [projectPath, onClose])

  const systemPrompt = useMemo(() => [
    `You're helping the user edit a context-stack file for workspace "${projectName}".`,
    ``,
    `File label: ${label}`,
    `Path: ${filePath}`,
    ``,
    `This file is included in the always-on AGENTS.md stack when enabled.`,
    `Keep it high-signal. Prefer links/pointers over dumping whole docs.`,
    ``,
    `Current contents:`,
    content,
  ].join('\n'), [projectName, label, filePath, content])

  const terminalArgs = useMemo(() => {
    if (!agentCommand) return undefined
    return buildEditorAgentArgs({
      command: agentCommand.command,
      baseArgs: agentCommand.args,
      systemBrief: systemPrompt,
      userMessage: `Read ${filePath}. Help the user refine this context-stack file (${label}).`,
    })
  }, [agentCommand, systemPrompt, filePath, label])

  return (
    <AIFileEditor
      filePath={filePath}
      watchDir={watchDir}
      cwd={projectPath}
      command={agentCommand?.command}
      args={terminalArgs}
      title={`${label}`}
      instructions={`Editing ${filePath} — part of the always-on AGENTS.md context stack. Regen runs when you close this editor.`}
      warningText="This file is included in always-on context when its stack layer is enabled."
      onFileChange={setContent}
      onClose={() => void handleClose()}
      preview={
        <div className="h-full flex flex-col">
          <div className="flex items-center justify-between px-4 py-2 border-b border-[var(--color-border)] flex-shrink-0">
            <div className="text-xs text-[var(--color-text-muted)] truncate">
              <span className="font-medium text-[var(--color-text-primary)]">{label}</span>
              <span className="mx-2">&middot;</span>
              <span className="font-mono text-[10px]">{filePath}</span>
            </div>
            <div className="flex items-center gap-2 flex-shrink-0">
              {previewMode === 'preview' && (
                <div className="flex items-center gap-0.5">
                  <button
                    onClick={() => setPreviewScale((s) => Math.max(50, s - 10))}
                    className="w-5 h-5 flex items-center justify-center text-[11px] text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)] bg-[var(--color-bg-elevated)] border border-[var(--color-border)] no-drag cursor-pointer"
                  >
                    −
                  </button>
                  <span className="text-[9px] tabular-nums text-[var(--color-text-muted)] w-7 text-center">{previewScale}%</span>
                  <button
                    onClick={() => setPreviewScale((s) => Math.min(200, s + 10))}
                    className="w-5 h-5 flex items-center justify-center text-[11px] text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)] bg-[var(--color-bg-elevated)] border border-[var(--color-border)] no-drag cursor-pointer"
                  >
                    +
                  </button>
                </div>
              )}
              <div className="flex border border-[var(--color-border)]">
                <button
                  onClick={() => setPreviewMode('preview')}
                  className={`px-2 py-0.5 text-[10px] no-drag cursor-pointer ${
                    previewMode === 'preview'
                      ? 'bg-[var(--color-accent)] text-[var(--color-on-accent)]'
                      : 'text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)]'
                  }`}
                >
                  Preview
                </button>
                <button
                  onClick={() => setPreviewMode('edit')}
                  className={`px-2 py-0.5 text-[10px] no-drag cursor-pointer ${
                    previewMode === 'edit'
                      ? 'bg-[var(--color-accent)] text-[var(--color-on-accent)]'
                      : 'text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)]'
                  }`}
                >
                  Source
                </button>
              </div>
            </div>
          </div>
          <div className="flex-1 overflow-auto p-4">
            {previewMode === 'preview' ? (
              <div style={{ zoom: cssScale / 100 }} className="prose prose-invert prose-sm max-w-none">
                <Markdown remarkPlugins={[remarkGfm]}>{content || '*Empty file.*'}</Markdown>
              </div>
            ) : (
              <CodeEditor
                code={content}
                filePath={filePath}
                onSave={async (c) => {
                  try {
                    await daemonCliPost('fs/write-file', { path: filePath, content: c })
                  } catch {
                    /* write failed — leave buffer for retry */
                  }
                }}
                onChange={(c) => setContent(c)}
              />
            )}
          </div>
        </div>
      }
    />
  )
}

function ClaudeMdEditor({ projectPath, projectName, onClose }: { projectPath: string; projectName: string; onClose: () => void }): React.JSX.Element {
  const [content, setContent] = useState('')
  const [previewMode, setPreviewMode] = useState<'preview' | 'edit'>('preview')
  const [previewScale, setPreviewScale] = useState(100)
  const cssScale = Math.round(previewScale * 0.7)

  const filePath = `${projectPath}/.k2/PROJECT.md`
  const watchDir = `${projectPath}/.k2`

  // Default agent resolved through the one seam (id-first, legacy-token
  // tolerant, first-enabled fallback), scoped to this project.
  const agentCommand = useResolvedAgentCommand(undefined, { projectPath, scope: 'global' })

  useEffect(() => {
    daemonCliGet<{ content: string }>('fs/read-file', { path: filePath })
      .then((r) => setContent(r.content))
      .catch(() => setContent(''))
  }, [filePath])

  const handleFileChange = useCallback((c: string) => setContent(c), [])

  // Trigger workspace SKILL.md regen on close so PROJECT.md edits
  // propagate to every harness file before the user closes the editor.
  const handleClose = useCallback(async () => {
    try {
      await daemonCliPost('agents/regenerate-workspace-skill', { project_path: projectPath })
    } catch (err) {
      console.warn('[workspace-knowledge] regen on close failed:', err)
    }
    onClose()
  }, [projectPath, onClose])

  const systemPrompt = useMemo(() => [
    `You're helping the user edit the workspace knowledge for "${projectName}".`,
    ``,
    `File: .k2so/PROJECT.md (source)`,
    `Path: ${filePath}`,
    ``,
    `This is the SOURCE. K2 compiles it (plus the agent's ROLE.md) into the`,
    `canonical .k2so/skills/<name>/SKILL.md on save. Mirroring this content out`,
    `into the CLI harness files (CLAUDE.md, GEMINI.md, .cursor/rules, AGENTS.md,`,
    `etc.) is OPT-IN per workspace — it only happens if harness fan-out is`,
    `enabled or the user runs the K2 Canonical Agent. Edit here once regardless.`,
    ``,
    `Good content for this file:`,
    `• Project overview — what this codebase does`,
    `• Tech stack — languages, frameworks, key dependencies`,
    `• Key directories — important paths and what lives in them`,
    `• Conventions — code style, commit format, branch naming, PR process`,
    `• Build & test — how to build, run tests, deploy`,
    `• Important notes — gotchas, known issues, things to watch out for`,
    ``,
    `Do NOT include agent-specific role/persona content — that lives in the`,
    `agent's ROLE.md (.k2/agent/ROLE.md); edit it from Settings → Workspaces`,
    `→ Context → Agent layer → Edit.`,
    ``,
    `Current contents:`,
    content,
  ].join('\n'), [projectName, filePath, content])

  const terminalCommand = agentCommand?.command
  const terminalArgs = useMemo(() => {
    if (!agentCommand) return undefined
    return buildEditorAgentArgs({
      command: agentCommand.command,
      baseArgs: agentCommand.args,
      systemBrief: systemPrompt,
      userMessage: `Read ${filePath}. Help the user define their workspace knowledge for "${projectName}". Start by asking about their tech stack and project structure.`,
    })
  }, [agentCommand, systemPrompt, projectName, filePath])

  return (
    <AIFileEditor
      filePath={filePath}
      watchDir={watchDir}
      cwd={projectPath}
      command={terminalCommand}
      args={terminalArgs}
      title={`Workspace Knowledge: ${projectName}`}
      instructions="Editing .k2so/PROJECT.md — the source for workspace knowledge. K2 compiles this into the canonical SKILL.md. When harness fan-out is enabled (opt-in), it also mirrors out to CLAUDE.md, AGENTS.md, GEMINI.md, .cursor/rules, .goosehints, etc. Regen runs automatically when you close this editor."
      warningText="This is the source file for the workspace's shared knowledge. When harness fan-out is enabled, edits mirror into every chosen CLI LLM harness on save."
      onFileChange={handleFileChange}
      onClose={handleClose}
      preview={
        <div className="h-full flex flex-col">
          <div className="flex items-center justify-between px-4 py-2 border-b border-[var(--color-border)] flex-shrink-0">
            <div className="text-xs text-[var(--color-text-muted)]">
              <span className="font-medium text-[var(--color-text-primary)]">PROJECT.md</span>
              <span className="mx-2">&middot;</span>
              <span>Source — compiled into every agent's SKILL.md</span>
            </div>
            <div className="flex items-center gap-2 flex-shrink-0">
              {previewMode === 'preview' && (
                <div className="flex items-center gap-0.5">
                  <button
                    onClick={() => setPreviewScale((s) => Math.max(50, s - 10))}
                    className="w-5 h-5 flex items-center justify-center text-[11px] text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)] bg-[var(--color-bg-elevated)] border border-[var(--color-border)] no-drag cursor-pointer"
                  >
                    −
                  </button>
                  <span className="text-[9px] tabular-nums text-[var(--color-text-muted)] w-7 text-center">{previewScale}%</span>
                  <button
                    onClick={() => setPreviewScale((s) => Math.min(200, s + 10))}
                    className="w-5 h-5 flex items-center justify-center text-[11px] text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)] bg-[var(--color-bg-elevated)] border border-[var(--color-border)] no-drag cursor-pointer"
                  >
                    +
                  </button>
                </div>
              )}
              <div className="flex gap-0.5">
                {(['preview', 'edit'] as const).map((mode) => (
                  <button
                    key={mode}
                    onClick={() => setPreviewMode(mode)}
                    className={`px-2 py-1 text-[10px] font-medium transition-colors no-drag cursor-pointer ${
                      previewMode === mode
                        ? 'bg-[var(--color-accent)] text-[var(--color-on-accent)]'
                        : 'bg-[var(--color-bg-elevated)] text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)] border border-[var(--color-border)]'
                    }`}
                  >
                    {mode === 'preview' ? 'Preview' : 'Edit'}
                  </button>
                ))}
              </div>
            </div>
          </div>
          {previewMode === 'preview' ? (
            <div className="flex-1 overflow-auto p-4">
              <div className="markdown-content" style={{ fontSize: `${cssScale}%` }}>
                <Markdown remarkPlugins={[remarkGfm]}>
                  {content || '*No SKILL.md yet. Use the AI assistant to set up your workspace knowledge, or click Edit to write it manually.*'}
                </Markdown>
              </div>
            </div>
          ) : (
            <div className="flex-1 overflow-hidden">
              <CodeEditor
                code={content}
                filePath={filePath}
                onSave={async (c) => {
                  try { await daemonCliPost('fs/write-file', { path: filePath, content: c }) } catch {}
                }}
                onChange={(c) => setContent(c)}
              />
            </div>
          )}
        </div>
      }
    />
  )
}

// ── Agent Display Name Input (0.37.4) ────────────────────────────────
//
// A small inline editor that reads the workspace's primary agent's
// display name (AGENT.md `display_name:` → `name:` → projects.name)
// and writes new values via `k2so_workspace_set_agent_display_name`.
//
// Why a separate field from the technical agent name? Because in
// 0.37.4 the technical name (the agent's AGENT.md `name:` /
// directory basename — historically the `__lead__` sentinel for
// manager workspaces, removed in 0.39.0f Phase 2.1) still keys
// infrastructure layers — v2_session_map,
// `workspace_sessions.terminal_id`, pending_live queue dirs. Editing
// the technical name would cascade through all of them and risk
// dropping the live PTY. The display name decouples the
// human-facing label from those keys; the user gets a friendly
// "what to call this agent" without the rename gymnastics.
//
// A future 0.38.0 ships the full `agent-display-name.md` PRD: drop
// the technical name from infrastructure, collapse `display_name:`
// and `name:` back into one field. Until then, this is the cheap
// stepping stone that fixes the user-visible "I renamed my agent
// and the inbox tab still says the wrong name" complaint.

function AgentDisplayNameField({
  projectPath,
  helpText,
  trailing,
}: {
  projectPath: string
  helpText?: string
  /** Optional element rendered to the right of the Save button —
   *  meant for siblings like a Manage Persona button so the row
   *  reads input | save | persona on one line. */
  trailing?: React.ReactNode
}): React.JSX.Element {
  const [ready, setReady] = useState(false)
  const [draft, setDraft] = useState('')
  const [saved, setSaved] = useState('')
  const [error, setError] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)
  const [flash, setFlash] = useState(false)

  useEffect(() => {
    let cancelled = false
    agentDisplayName(projectPath)
      .then((n) => { if (!cancelled) { setDraft(n); setSaved(n); setReady(true) } })
      .catch((e) => { if (!cancelled) { console.error('[display-name] read failed:', e); setReady(true) } })
    return () => { cancelled = true }
  }, [projectPath])

  const dirty = ready && draft !== saved

  // Mirrors k2-core's `validate_display_name` (0.40.24 S3 loosened
  // contract): display names are human labels — spaces + mixed case
  // are allowed ("QA Bot", "K2 - Marketing Manager"). Still banned:
  // empty, >64 chars, control characters, leading/trailing
  // whitespace, and "/" (the name seeds retire's archive folder
  // label).
  const validate = (n: string): string | null => {
    if (n.length === 0) return 'Display name must not be empty.'
    if (n.length > 64) return 'Display name must be at most 64 characters.'
    if (n !== n.trim()) return 'Display name must not start or end with whitespace.'
    if (n.includes('/')) return "Display name must not contain '/'."
    if (n.includes(':')) return "Display name must not contain ':' (federated addresses use name::host)."
    // eslint-disable-next-line no-control-regex
    if (/[\u0000-\u001f\u007f-\u009f]/.test(n)) return 'Display name must not contain control characters.'
    return null
  }

  const handleSave = async (): Promise<void> => {
    // 0.40.24 S3: names are case-preserving now (mixed case is part
    // of the label), so the 0.37.9 save-time lowercasing is retired
    // along with the slug-shaped rule.
    const candidate = draft
    const err = validate(candidate)
    if (err) { setError(err); return }
    setError(null)
    setBusy(true)
    try {
      await setAgentDisplayName(projectPath, candidate)
      setDraft(candidate)
      setSaved(candidate)
      setFlash(true)
      setTimeout(() => setFlash(false), 1200)
    } catch (e) {
      setError(String(e))
    } finally {
      setBusy(false)
    }
  }

  return (
    <div>
      <div className="flex items-center gap-2">
        <div className="flex-1 min-w-0">
          <label className="block text-[9px] uppercase tracking-wider text-[var(--color-text-muted)] mb-1">
            Agent Name
          </label>
          <input
            type="text"
            value={draft}
            // 0.37.9 — DON'T transform the value on every keystroke.
            // Pre-fix this read `e.target.value.toLowerCase()`, which
            // mutates the DOM input's value mid-composition during
            // Apple Dictation. Dictation manages the input value
            // internally during the composition phase; if we mutate
            // it underneath, dictation's state desyncs and the
            // engagement hangs/aborts. (0.40.24 S3: lowercase
            // enforcement is gone entirely — names are case-
            // preserving; `validate()` at save time carries the
            // remaining rules.)
            onChange={(e) => { setDraft(e.target.value); setError(null) }}
            disabled={!ready || busy}
            placeholder="agent"
            spellCheck={false}
            autoComplete="off"
            autoCorrect="off"
            autoCapitalize="off"
            onKeyDown={(e) => { if (e.key === 'Enter' && dirty && !busy) handleSave() }}
            className="w-full px-2 py-1 text-xs bg-[var(--color-bg-elevated)] border border-[var(--color-border)] text-[var(--color-text-primary)] focus:outline-none focus:border-[var(--color-accent)] disabled:opacity-60"
          />
        </div>
        <button
          onClick={handleSave}
          disabled={!ready || busy || !dirty}
          className="px-3 py-1.5 text-[10px] font-medium text-[var(--color-accent)] bg-[var(--color-accent)]/10 hover:bg-[var(--color-accent)]/20 border border-[var(--color-accent)]/30 transition-colors no-drag cursor-pointer disabled:opacity-50 flex-shrink-0 self-end"
        >
          {busy ? 'Saving…' : flash ? 'Saved' : 'Save'}
        </button>
        {trailing}
      </div>
      {error && (
        <p className="text-[10px] text-[var(--color-status-error-soft)] mt-1">{error}</p>
      )}
      {!error && helpText && (
        <p className="text-[9px] text-[var(--color-text-muted)] mt-1">{helpText}</p>
      )}
    </div>
  )
}

function federatedHostFromTunnel(status: {
  running?: boolean
  public_url?: string | null
  subdomain?: string | null
} | null): string | null {
  if (!status?.running) return null
  const url = status.public_url?.trim()
  if (url) {
    try {
      const host = new URL(url.includes('://') ? url : `https://${url}`).hostname
      if (host) return host
    } catch {
      /* fall through to subdomain */
    }
  }
  const sub = status.subdomain?.trim()
  if (sub) return sub.includes('.') ? sub : `${sub}.k2.dev`
  return null
}

function AgentHandleField({
  projectPath,
  projectHandle,
}: {
  projectPath: string
  projectHandle?: string
}): React.JSX.Element {
  const [handle, setHandle] = useState(projectHandle ?? '')
  const [draft, setDraft] = useState(projectHandle ?? '')
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const confirm = useConfirmDialogStore((s) => s.confirm)
  const { status } = useTunnelUrls()
  const tunnelHost = federatedHostFromTunnel(status)

  useEffect(() => {
    let cancelled = false
    agentHandle(projectPath)
      .then((h) => {
        if (!cancelled) {
          setHandle(h)
          setDraft(h)
        }
      })
      .catch(() => {})
    return () => { cancelled = true }
  }, [projectPath, projectHandle])

  const dirty = draft.trim() !== handle.trim() && draft.trim().length > 0

  const onChange = async (): Promise<void> => {
    const next = draft.trim()
    if (!next || next === handle) return
    const ok = await confirm({
      title: 'Change handle?',
      message:
        `Changing the handle changes this agent's address (${handle || 'old'}::${tunnelHost ?? 'host'} → ${next}::${tunnelHost ?? 'host'}). Existing federated connections will break until the other side reconnects or updates the handle.`,
      confirmLabel: 'Change handle',
      destructive: true,
    })
    if (!ok) return
    setBusy(true)
    setError(null)
    try {
      const stored = await setAgentHandle(projectPath, next)
      setHandle(stored)
      setDraft(stored)
      void emit('sync:projects').catch(() => {})
    } catch (e) {
      setError(String(e))
    } finally {
      setBusy(false)
    }
  }

  return (
    <div className="mt-3">
      <label className="block text-[9px] uppercase tracking-wider text-[var(--color-text-muted)] mb-1">
        Handle
      </label>
      <div className="flex items-center gap-2">
        <input
          type="text"
          value={draft}
          onChange={(e) => { setDraft(e.target.value); setError(null) }}
          disabled={busy}
          spellCheck={false}
          autoComplete="off"
          autoCorrect="off"
          autoCapitalize="off"
          className="flex-1 min-w-0 px-2 py-1 text-xs font-mono bg-[var(--color-bg-elevated)] border border-[var(--color-border)] text-[var(--color-text-primary)] focus:outline-none focus:border-[var(--color-accent)] disabled:opacity-60"
        />
        <button
          onClick={() => { void onChange() }}
          disabled={busy || !dirty}
          className="px-3 py-1.5 text-[10px] font-medium text-[var(--color-accent)] bg-[var(--color-accent)]/10 hover:bg-[var(--color-accent)]/20 border border-[var(--color-accent)]/30 transition-colors no-drag cursor-pointer disabled:opacity-50 flex-shrink-0"
        >
          {busy ? 'Saving…' : 'Change handle…'}
        </button>
      </div>
      {handle && tunnelHost && (
        <p className="text-[9px] font-mono text-[var(--color-text-muted)] mt-1">{handle}::{tunnelHost}</p>
      )}
      {handle && !tunnelHost && (
        <p className="text-[9px] text-[var(--color-text-muted)] mt-1 leading-relaxed">
          Get a subdomain so this agent can talk to other people&apos;s agents on K2.{' '}
          <button
            type="button"
            className="text-[var(--color-accent)] hover:underline bg-transparent border-none p-0 cursor-pointer no-drag"
            onClick={() => useSettingsStore.getState().setSection('k2-connect')}
          >
            Open K2 Connect
          </button>
        </p>
      )}
      {error && (
        <p className="text-[10px] text-[var(--color-status-error-soft)] mt-1">{error}</p>
      )}
    </div>
  )
}

// ── Custom Agent Persona Button ──────────────────────────────────────

function CustomAgentPersonaButton({ projectPath, projectName, onOpenEditor }: { projectPath: string; projectName: string; onOpenEditor: (agentName: string) => void }): React.JSX.Element {
  const derived = projectName.toLowerCase().replace(/\s+/g, '-').replace(/[^a-z0-9-]/g, '')
  const [ready, setReady] = useState(false)
  // Technical agent name from AGENT.md `name:` (the directory-side
  // identifier). Post-0.37.0 unification, every workspace's primary
  // agent lives at `.k2so/agent/AGENT.md`; we just need its `name:`
  // for the Manage Persona open-editor call. Friendly label editing
  // goes through AgentDisplayNameField (`display_name:` frontmatter).
  const [techName, setTechName] = useState<string>(derived)

  useEffect(() => {
    let cancelled = false
    invoke<(K2soAgentInfo & { agentType?: string })[]>('k2so_agents_list', { projectPath })
      .then((agents) => {
        if (cancelled) return
        // Post-0.37.0 there's at most one agent per workspace. Pick
        // whichever one comes back; if the workspace mode is custom
        // we expect exactly one with `type: custom` (or whatever
        // AGENT.md was scaffolded with), but the picker is mode-blind
        // because the technical name is what we need either way.
        const primary = agents[0]
        setTechName(primary?.name ?? derived)
        setReady(true)
      })
      .catch((e) => {
        if (cancelled) return
        console.error('[custom-agent] list failed:', e)
        setReady(true)
      })
    return () => { cancelled = true }
  }, [projectPath, derived])

  // Refresh the technical name on cross-window sync (e.g. someone
  // edited AGENT.md `name:` via the persona editor in another tab).
  useEffect(() => {
    let unlisten: (() => void) | null = null
    let cancelled = false
    listen('sync:projects', () => {
      invoke<(K2soAgentInfo & { agentType?: string })[]>('k2so_agents_list', { projectPath })
        .then((agents) => { if (agents[0]?.name) setTechName(agents[0].name) })
        .catch(() => {})
    }).then((u) => { if (cancelled) u(); else unlisten = u })
    return () => { cancelled = true; unlisten?.() }
  }, [projectPath])

  const handleOpenPersona = async (): Promise<void> => {
    if (!ready) return
    // First-time setup: scaffold a custom agent with the
    // workspace-derived technical name. Idempotent — if AGENT.md
    // already exists, k2so_agents_create returns the existing agent's
    // info without overwriting (post-0.37.0 unification behavior).
    try {
      await daemonCliGet('agents/create', {
        project: projectPath,
        name: techName,
        role: 'Custom agent — customize via the persona editor',
        agent_type: 'custom',
      })
    } catch (e) {
      console.warn('[custom-agent] create returned error (may be benign if already exists):', e)
    }
    onOpenEditor(techName)
  }

  return (
    <AgentDisplayNameField
      projectPath={projectPath}
      helpText="The friendly label used in the inbox tab, chat tab, and persona prompts. Edit anytime — does not affect the agent's technical name or live session."
      trailing={
        <button
          onClick={handleOpenPersona}
          disabled={!ready}
          className="px-3 py-1.5 text-[10px] font-medium text-[var(--color-accent)] bg-[var(--color-accent)]/10 hover:bg-[var(--color-accent)]/20 border border-[var(--color-accent)]/30 transition-colors no-drag cursor-pointer disabled:opacity-50 flex-shrink-0 self-end whitespace-nowrap"
        >
          Manage Persona
        </button>
      }
    />
  )
}

function K2SOAgentPersonaButton({ projectPath, projectName, onOpenEditor }: { projectPath: string; projectName: string; onOpenEditor: (agentName: string) => void }): React.JSX.Element {
  const [ready, setReady] = useState(false)
  const [agentName, setAgentName] = useState('k2so-agent')

  // Ensure the K2 agent exists for this workspace
  useEffect(() => {
    const ensure = async () => {
      try {
        const agents = await invoke<(K2soAgentInfo & { agentType?: string })[]>('k2so_agents_list', { projectPath })
        // Stage A dual-read: `k2` and legacy `k2so` are the same builtin type.
        const existing = agents.find((a: any) => isBuiltinAgentType(a.agentType))
        if (existing) {
          setAgentName(existing.name)
        } else {
          await daemonCliGet('agents/create', {
            project: projectPath,
            name: 'k2so-agent',
            role: 'K2 planner — builds PRDs, milestones, and technical plans',
            agent_type: 'k2so',
          })
        }
        setReady(true)
      } catch (e) {
        console.error('[k2so-agent] Init failed:', e)
        setReady(true)
      }
    }
    ensure()
  }, [projectPath, projectName])

  return (
    <AgentDisplayNameField
      projectPath={projectPath}
      helpText="The friendly label used in the inbox tab. Edit anytime — does not affect routing or the live session."
      trailing={
        <button
          onClick={() => onOpenEditor(agentName)}
          disabled={!ready}
          className="px-3 py-1.5 text-[10px] font-medium text-[var(--color-accent)] bg-[var(--color-accent)]/10 hover:bg-[var(--color-accent)]/20 border border-[var(--color-accent)]/30 transition-colors no-drag cursor-pointer disabled:opacity-50 flex-shrink-0 self-end whitespace-nowrap"
        >
          Manage Persona
        </button>
      }
    />
  )
}

// ── Connected Workspaces Panel ──────────────────────────────────────

interface WorkspaceRelation {
  id: string
  sourceProjectId: string
  targetProjectId: string
  relationType: string
  createdAt: string
}

function ConnectedWorkspacesPanel({ projectId }: { projectId: string }): React.JSX.Element {
  const [relations, setRelations] = useState<WorkspaceRelation[]>([])
  const [incoming, setIncoming] = useState<WorkspaceRelation[]>([])
  const [loading, setLoading] = useState(true)
  const [adding, setAdding] = useState(false)
  const [search, setSearch] = useState('')
  const [showLocalAdd, setShowLocalAdd] = useState(false)
  const [showFedAdd, setShowFedAdd] = useState(false)
  // Federated add wizard: pick trusted server → pick agent from their roster
  const [fedPeers, setFedPeers] = useState<FederationPeer[]>([])
  const [fedPeersAvailable, setFedPeersAvailable] = useState(true)
  const [selectedPeerFp, setSelectedPeerFp] = useState('')
  const [rosterAgents, setRosterAgents] = useState<RosterAgent[]>([])
  const [rosterLoading, setRosterLoading] = useState(false)
  const [remoteConns, setRemoteConns] = useState<RemoteConnectionEntry[]>([])
  const [remoteError, setRemoteError] = useState<string | null>(null)
  const [localError, setLocalError] = useState<string | null>(null)
  const projects = useProjectsStore((s) => s.projects)

  const sourcePath = useMemo(
    () => projects.find((p) => p.id === projectId)?.path ?? '',
    [projects, projectId],
  )

  const fetchRelations = useCallback(async () => {
    try {
      const [outgoing, inc, remote] = await Promise.all([
        daemonCliGet<WorkspaceRelation[]>('relations/list', { project_id: projectId }),
        daemonCliGet<WorkspaceRelation[]>('relations/list-incoming', { project_id: projectId }),
        listRemoteConnections(sourcePath),
      ])
      setRelations(outgoing)
      setIncoming(inc)
      setRemoteConns(remote)
    } catch {
      setRelations([])
      setIncoming([])
      setRemoteConns([])
    } finally {
      setLoading(false)
    }
  }, [projectId, sourcePath])

  useEffect(() => {
    fetchRelations()
  }, [fetchRelations])

  // Load trusted federation peers when the federated-add panel opens
  useEffect(() => {
    if (!showFedAdd) return
    let cancelled = false
    void (async () => {
      const res = await listFederationPeers()
      if (cancelled) return
      setFedPeersAvailable(res.available)
      setFedPeers(res.available ? trustedPeers(res.data) : [])
    })()
    return () => {
      cancelled = true
    }
  }, [showFedAdd])

  // When a peer server is selected, fetch its agent roster (permission-filtered server-side)
  useEffect(() => {
    if (!selectedPeerFp) {
      setRosterAgents([])
      return
    }
    let cancelled = false
    setRosterLoading(true)
    void (async () => {
      const res = await fetchPeerRoster(selectedPeerFp)
      if (cancelled) return
      setRosterAgents(res.available ? res.data : [])
      setRosterLoading(false)
    })()
    return () => {
      cancelled = true
    }
  }, [selectedPeerFp])

  const selectedPeer = useMemo(
    () => fedPeers.find((p) => p.fingerprint === selectedPeerFp) ?? null,
    [fedPeers, selectedPeerFp],
  )

  const connectedIds = useMemo(() => new Set(relations.map((r) => r.targetProjectId)), [relations])
  const availableProjects = useMemo(
    () =>
      projects
        .filter((p) => p.id !== projectId && !connectedIds.has(p.id))
        .sort((a, b) => a.name.localeCompare(b.name)),
    [projects, projectId, connectedIds],
  )
  const filteredProjects = useMemo(
    () =>
      search.trim()
        ? availableProjects.filter((p) => p.name.toLowerCase().includes(search.toLowerCase()))
        : availableProjects,
    [availableProjects, search],
  )

  // Agents already linked as federated connections (by agent::host)
  const linkedRemoteAddrs = useMemo(
    () => new Set(remoteConns.map((r) => r.address.toLowerCase())),
    [remoteConns],
  )

  const availableRosterAgents = useMemo(() => {
    if (!selectedPeer) return []
    const host = selectedPeer.subdomain
      ? `${selectedPeer.subdomain}.k2.dev`
      : ''
    if (!host) return rosterAgents
    return rosterAgents.filter((a) => {
      const addr = formatAgentHost(a.agent, host).toLowerCase()
      return !linkedRemoteAddrs.has(addr)
    })
  }, [rosterAgents, selectedPeer, linkedRemoteAddrs])

  const handleAddLocal = useCallback(
    async (targetProjectId: string) => {
      setAdding(true)
      setLocalError(null)
      try {
        await daemonCliPost('relations/create', {
          source_project_id: projectId,
          target_project_id: targetProjectId,
        })
        setShowLocalAdd(false)
        setSearch('')
        await fetchRelations()
      } catch (e) {
        setLocalError(e instanceof Error ? e.message : String(e))
      } finally {
        setAdding(false)
      }
    },
    [projectId, fetchRelations],
  )

  const handleRemoveLocal = useCallback(
    async (id: string) => {
      setLocalError(null)
      try {
        await daemonCliPost('relations/delete', { id })
        await fetchRelations()
      } catch (e) {
        setLocalError(e instanceof Error ? e.message : String(e))
      }
    },
    [fetchRelations],
  )

  const handleAddFederated = useCallback(
    async (agent: RosterAgent) => {
      if (!sourcePath || !selectedPeer?.subdomain) {
        setRemoteError('Missing workspace path or peer subdomain — cannot add federated connection.')
        return
      }
      const target = formatAgentHost(agent.agent, `${selectedPeer.subdomain}.k2.dev`)
      setAdding(true)
      setRemoteError(null)
      try {
        const { reverseWarning } = await addRemoteConnection(sourcePath, target)
        setShowFedAdd(false)
        setSelectedPeerFp('')
        setRosterAgents([])
        await fetchRelations()
        if (reverseWarning) setRemoteError(reverseWarning)
      } catch (e) {
        setRemoteError(e instanceof Error ? e.message : String(e))
      } finally {
        setAdding(false)
      }
    },
    [sourcePath, selectedPeer, fetchRelations],
  )

  const handleRemoveRemote = useCallback(
    async (address: string) => {
      if (!sourcePath) {
        setRemoteError(
          'This workspace has no resolved path on the active server — cannot remove the remote connection.',
        )
        return
      }
      setRemoteError(null)
      try {
        await removeRemoteConnection(sourcePath, address)
        await fetchRelations()
      } catch (e) {
        const msg = e instanceof Error ? e.message : String(e)
        console.error('[connected-workspaces] Remote disconnect failed:', e)
        setRemoteError(msg || 'Failed to remove remote connection')
      }
    },
    [sourcePath, fetchRelations],
  )

  const projectsById = useMemo(() => {
    const map = new Map<string, (typeof projects)[number]>()
    for (const p of projects) map.set(p.id, p)
    return map
  }, [projects])

  const peerLabel = (p: FederationPeer) => p.label || p.subdomain || p.fingerprint.slice(0, 12)

  return (
    <div className="space-y-5">
      {/* ── Local Connections ─────────────────────────────────────── */}
      <div>
        <div className="flex items-center justify-between mb-2">
          <div>
            <h3 className="text-xs font-medium text-[var(--color-text-primary)]">Local Connections</h3>
            <p className="text-[10px] text-[var(--color-text-muted)] mt-0.5">
              Workspaces on this server that this agent can message.
            </p>
          </div>
          <button
            type="button"
            onClick={() => {
              setShowLocalAdd(!showLocalAdd)
              setShowFedAdd(false)
              setSearch('')
              setLocalError(null)
            }}
            title="Add local connection"
            className="w-6 h-6 flex items-center justify-center text-sm leading-none bg-[var(--color-accent)] text-[var(--color-on-accent)] cursor-pointer no-drag"
          >
            +
          </button>
        </div>

        {showLocalAdd && (
          <div className="border border-[var(--color-border)] bg-[var(--color-bg-elevated)] mb-2">
            <div className="px-3 py-1.5 border-b border-[var(--color-border)]">
              <input
                type="text"
                value={search}
                onChange={(e) => {
                  setSearch(e.target.value)
                  setLocalError(null)
                }}
                placeholder="Search local workspaces…"
                autoFocus
                className="w-full bg-transparent text-xs text-[var(--color-text-primary)] placeholder-[var(--color-text-muted)] outline-none"
              />
            </div>
            <div className="max-h-[200px] overflow-y-auto">
              {filteredProjects.length === 0 ? (
                <div className="px-3 py-2 text-[10px] text-[var(--color-text-muted)]">
                  {search.trim() ? 'No matching workspaces.' : 'No more local workspaces to connect.'}
                </div>
              ) : (
                filteredProjects.map((p) => (
                  <button
                    key={p.id}
                    type="button"
                    onClick={() => void handleAddLocal(p.id)}
                    disabled={adding}
                    className="w-full flex items-center gap-2 px-3 py-1.5 text-left hover:bg-white/[0.06] transition-colors no-drag cursor-pointer disabled:opacity-50 border-b border-[var(--color-border)] last:border-b-0"
                  >
                    <span
                      className="w-2 h-2 flex-shrink-0 rounded-full"
                      style={{ backgroundColor: p.color || 'var(--color-neutral)' }}
                    />
                    <span className="text-xs text-[var(--color-text-primary)] truncate">{p.name}</span>
                  </button>
                ))
              )}
            </div>
          </div>
        )}

        {loading ? (
          <div className="text-[10px] text-[var(--color-text-muted)]">Loading…</div>
        ) : relations.length === 0 ? (
          <div className="text-[10px] text-[var(--color-text-muted)] px-3 py-2 border border-[var(--color-border)]">
            No local connections yet.
          </div>
        ) : (
          <div className="border border-[var(--color-border)]">
            {relations.map((rel) => {
              const target = projectsById.get(rel.targetProjectId)
              return (
                <div
                  key={rel.id}
                  className="flex items-center gap-2 px-3 py-1.5 border-b border-[var(--color-border)] last:border-b-0"
                >
                  <span
                    className="w-2 h-2 flex-shrink-0 rounded-full"
                    style={{ backgroundColor: target?.color || 'var(--color-neutral)' }}
                  />
                  <span className="text-xs text-[var(--color-text-primary)] flex-1 truncate">
                    {target?.name || 'Unknown workspace'}
                  </span>
                  <button
                    type="button"
                    onClick={() => void handleRemoveLocal(rel.id)}
                    className="w-5 h-5 flex items-center justify-center text-[var(--color-text-muted)] hover:text-[var(--color-status-error-soft)] transition-colors no-drag cursor-pointer flex-shrink-0"
                    title="Remove local connection"
                  >
                    <svg width="8" height="8" viewBox="0 0 8 8" fill="none" stroke="currentColor" strokeWidth="1.5">
                      <line x1="1" y1="1" x2="7" y2="7" />
                      <line x1="7" y1="1" x2="1" y2="7" />
                    </svg>
                  </button>
                </div>
              )
            })}
          </div>
        )}
        {localError && (
          <div className="mt-1 px-3 py-2 text-[10px] text-[var(--color-status-error-soft)] border border-[var(--color-border)]">
            {localError}
          </div>
        )}

        {!loading && incoming.length > 0 && (
          <>
            <h4 className="text-[10px] font-semibold text-[var(--color-text-muted)] uppercase tracking-wider mt-3">
              Connected to this workspace
            </h4>
            <div className="border border-[var(--color-border)]">
              {incoming.map((rel) => {
                const source = projectsById.get(rel.sourceProjectId)
                return (
                  <div
                    key={rel.id}
                    className="flex items-center gap-2 px-3 py-1.5 border-b border-[var(--color-border)] last:border-b-0"
                  >
                    <span
                      className="w-2 h-2 flex-shrink-0 rounded-full"
                      style={{ backgroundColor: source?.color || 'var(--color-neutral)' }}
                    />
                    <span className="text-xs text-[var(--color-text-primary)] flex-1 truncate">
                      {source?.name || 'Unknown workspace'}
                    </span>
                  </div>
                )
              })}
            </div>
          </>
        )}
      </div>

      {/* ── Federated Connections ────────────────────────────────── */}
      <div>
        <div className="flex items-center justify-between mb-2">
          <div>
            <h3 className="text-xs font-medium text-[var(--color-text-primary)]">Federated Connections</h3>
            <p className="text-[10px] text-[var(--color-text-muted)] mt-0.5">
              Agents on paired servers. Pick a federated server, then an agent they expose.
            </p>
          </div>
          <button
            type="button"
            onClick={() => {
              setShowFedAdd(!showFedAdd)
              setShowLocalAdd(false)
              setSelectedPeerFp('')
              setRosterAgents([])
              setRemoteError(null)
            }}
            title="Add federated connection"
            className="w-6 h-6 flex items-center justify-center text-sm leading-none bg-[var(--color-accent)] text-[var(--color-on-accent)] cursor-pointer no-drag"
          >
            +
          </button>
        </div>

        {showFedAdd && (
          <div className="border border-[var(--color-border)] bg-[var(--color-bg-elevated)] mb-2">
            <div className="px-3 py-2 border-b border-[var(--color-border)] space-y-2">
              <label className="block text-[10px] text-[var(--color-text-muted)] uppercase tracking-wider">
                Federated server
              </label>
              {!fedPeersAvailable ? (
                <p className="text-[10px] text-[var(--color-text-muted)]">
                  Federation is off or unavailable. Enable it under Settings → K2 Connect.
                </p>
              ) : fedPeers.length === 0 ? (
                <p className="text-[10px] text-[var(--color-text-muted)]">
                  No trusted federated servers yet. Under Settings → Connections, open a saved
                  server and click <span className="text-[var(--color-text-secondary)]">Pair as federated peer</span>
                  {' '}(enable federation on both sides first).
                </p>
              ) : (
                <SettingDropdown
                  value={selectedPeerFp}
                  placeholder="Select a server…"
                  menuAlign="left"
                  fullWidth
                  options={fedPeers.map((p) => ({
                    value: p.fingerprint,
                    label: p.subdomain
                      ? `${peerLabel(p)} (${p.subdomain}.k2.dev)`
                      : peerLabel(p),
                  }))}
                  onChange={(fp) => {
                    setSelectedPeerFp(fp)
                    setRemoteError(null)
                  }}
                />
              )}
            </div>

            {selectedPeerFp && (
              <div className="max-h-[220px] overflow-y-auto">
                <div className="px-3 py-1.5 text-[10px] text-[var(--color-text-muted)] border-b border-[var(--color-border)]">
                  Agents on {selectedPeer ? peerLabel(selectedPeer) : 'server'} you can connect to
                  {selectedPeer?.subdomain ? ` · ${selectedPeer.subdomain}.k2.dev` : ''}
                </div>
                {rosterLoading ? (
                  <div className="px-3 py-2 text-[10px] text-[var(--color-text-muted)]">Loading agents…</div>
                ) : availableRosterAgents.length === 0 ? (
                  <div className="px-3 py-2 text-[10px] text-[var(--color-text-muted)]">
                    No connectable agents on this server. On their side: turn on &quot;Let remote users message
                    agents&quot;, or per-workspace Remote Access / allow agents to create connections.
                  </div>
                ) : (
                  availableRosterAgents.map((a) => (
                    <button
                      key={`${a.workspace_id}:${a.agent}`}
                      type="button"
                      disabled={adding || !selectedPeer?.subdomain}
                      onClick={() => void handleAddFederated(a)}
                      className="w-full flex items-center gap-2 px-3 py-1.5 text-left hover:bg-white/[0.06] transition-colors no-drag cursor-pointer disabled:opacity-50 border-b border-[var(--color-border)] last:border-b-0"
                    >
                      <span className="w-2 h-2 flex-shrink-0 rounded-full bg-[var(--color-accent)]" />
                      <span className="text-xs text-[var(--color-text-primary)] truncate">{a.agent}</span>
                      <span className="text-[10px] text-[var(--color-text-muted)] truncate ml-auto">
                        {a.workspace_name}
                      </span>
                    </button>
                  ))
                )}
              </div>
            )}
            {remoteError && showFedAdd && (
              <div className="px-3 py-2 text-[10px] text-[var(--color-status-error-soft)] border-t border-[var(--color-border)]">
                {remoteError}
              </div>
            )}
          </div>
        )}

        {loading ? null : remoteConns.length === 0 ? (
          <div className="text-[10px] text-[var(--color-text-muted)] px-3 py-2 border border-[var(--color-border)]">
            No federated connections yet.
          </div>
        ) : (
          <div className="border border-[var(--color-border)]">
            {remoteConns.map((rc) => (
              <div
                key={rc.address}
                className="flex items-center gap-2 px-3 py-1.5 border-b border-[var(--color-border)] last:border-b-0"
              >
                <span className="w-2 h-2 flex-shrink-0 rounded-full bg-[var(--color-accent)]" />
                <span className="text-xs text-[var(--color-text-primary)] flex-1 truncate" title={rc.address}>
                  {rc.address}
                </span>
                <span className="text-[9px] text-[var(--color-text-muted)] uppercase tracking-wider">
                  federated
                </span>
                <button
                  type="button"
                  onClick={() => void handleRemoveRemote(rc.address)}
                  className="w-5 h-5 flex items-center justify-center text-[var(--color-text-muted)] hover:text-[var(--color-status-error-soft)] transition-colors no-drag cursor-pointer flex-shrink-0"
                  title="Remove federated connection"
                >
                  <svg width="8" height="8" viewBox="0 0 8 8" fill="none" stroke="currentColor" strokeWidth="1.5">
                    <line x1="1" y1="1" x2="7" y2="7" />
                    <line x1="7" y1="1" x2="1" y2="7" />
                  </svg>
                </button>
              </div>
            ))}
          </div>
        )}
        {!showFedAdd && remoteError && (
          <div className="mt-1 px-3 py-2 text-[10px] text-[var(--color-status-error-soft)] border border-[var(--color-border)]">
            {remoteError}
          </div>
        )}
      </div>
    </div>
  )
}

/// Compact summary row returned by the new `k2so_skills_list` Tauri
/// verb. Shape matches `k2so_core::skills::crud::SkillSummary` — see
/// `crates/k2so-core/src/skills/crud.rs`.
interface SkillSummary {
  name: string
  title: string | null
  lastModified: number
}

function ProjectAgentsPanel({ projectPath, onOpenEditor }: { projectPath: string; onOpenEditor: (agentName: string) => void }): React.JSX.Element {
  // `agents` drives the Workspace Manager subsection — it carries the
  // `isCoordinator`/role info the manager block needs. Phase 2.5 polish
  // (0.39.0h): the Skills list got promoted out of this panel into its
  // own top-level `ProjectSkillsPanel` SettingsGroup, so this component
  // is now just Manager + Project Context.
  const [agents, setAgents] = useState<K2soAgentInfo[]>([])
  const [wsInboxCount, setWsInboxCount] = useState(0)

  const fetchAgents = useCallback(async () => {
    try {
      const result = await invoke<K2soAgentInfo[]>('k2so_agents_list', { projectPath })
      setAgents(result)
    } catch {
      setAgents([])
    }
  }, [projectPath])

  const fetchWsInbox = useCallback(async () => {
    // Phase 2.1c Item 2 — workspace inbox primitive count endpoint.
    try {
      const count = await invoke<number>('k2so_inbox_count', { projectPath })
      setWsInboxCount(count)
    } catch {
      setWsInboxCount(0)
    }
  }, [projectPath])

  useEffect(() => {
    fetchAgents()
    fetchWsInbox()
  }, [fetchAgents, fetchWsInbox])

  const manager = agents.find((a) => a.isCoordinator)

  return (
    <div className="space-y-3">
      {/* Manager section */}
      {manager && (
        <div>
          <h3 className="text-[10px] font-semibold text-[var(--color-accent)] uppercase tracking-wider mb-1">
            Workspace Manager
          </h3>
          <div className="border border-[var(--color-accent)]/30">
            <div className="px-3 py-2 space-y-2">
              <div className="flex items-center">
                <span className="text-xs font-medium text-[var(--color-text-primary)] flex-shrink-0">{manager.name}</span>
                <span className="text-[9px] font-medium text-[var(--color-accent)] bg-[var(--color-accent)]/10 px-1.5 py-0.5 ml-1.5 flex-shrink-0">
                  MANAGER
                </span>
                {/* Phase 2.5b followup: only the workspace inbox count
                    survives. `delegated` / `done` came from the
                    per-agent `.k2so/agents/<name>/work/{active,done}/`
                    surface — that's gone under the workspace==agent
                    invariant + Phase 2.5b skill consolidation. Work
                    now lives on the workspace inbox primitive, not
                    per-agent work queues. */}
                <div className="flex items-center justify-end gap-1.5 text-[10px] flex-1 ml-2">
                  {wsInboxCount > 0 && (
                    <span className="text-[var(--color-accent)]" title="Items in workspace inbox">{wsInboxCount} inbox</span>
                  )}
                </div>
              </div>
              <p className="text-[10px] text-[var(--color-text-muted)] truncate">{manager.role}</p>
              {/* 0.37.4: friendly display label, separate from the
                  internal agent identity (directory basename / AGENT.md
                  `name:`) so the inbox tab can show something more
                  human. Pre-0.39.0f Phase 2.1 the manager identity was
                  the `__lead__` sentinel — removed; the identity is now
                  the workspace's primary agent name. Manage Persona
                  rides as the trailing slot so input | save | persona
                  all sit on one row. */}
              <AgentDisplayNameField
                projectPath={projectPath}
                helpText="Shown on the inbox tab — what you call this manager. The internal agent identity (folder basename) is unchanged."
                trailing={
                  <button
                    onClick={() => onOpenEditor(manager.name)}
                    className="px-3 py-1.5 text-[10px] font-medium text-[var(--color-accent)] bg-[var(--color-accent)]/10 hover:bg-[var(--color-accent)]/20 border border-[var(--color-accent)]/30 transition-colors no-drag cursor-pointer flex-shrink-0 self-end whitespace-nowrap"
                    title="Manage workspace manager persona"
                  >
                    Manage Persona
                  </button>
                }
              />
            </div>
          </div>
        </div>
      )}

      {/* Project Context section removed — it edited the same .k2so/PROJECT.md
          as "Workspace Knowledge" (workspace settings, always-on). One editor. */}

      {/* Workspace Wake-up retired in 0.32.6. Its content migrated to the
          per-workspace `triage` heartbeat row (edit via the Heartbeats
          list above — each row has its own WAKEUP.md now). See the
          migrate_or_scaffold_lead_heartbeat startup pass. */}

      {/* Skills section was promoted out of this panel in 0.39.0h
          Phase 2.5 polish — it now renders as its own top-level
          SettingsGroup (`ProjectSkillsPanel`) above Worktrees. The
          workspace==agent invariant (Phase 2.1) means primary-agent +
          skill-profiles are siblings, not parent-child; the visual
          hierarchy matches that. */}
    </div>
  )
}

// ── Project Skills Panel ────────────────────────────────────────────
//
// Promoted out of ProjectAgentsPanel in 0.39.0h. Renders the Skills
// list as its own top-level Settings group. Always visible — every
// workspace has skill profiles (Phase 2.5b). Fetches its own skills
// + agents (the latter only to filter the workspace manager out of the
// per-row list so it isn't duplicated next to the Workspace Manager
// block in Agent Settings).

function ProjectSkillsPanel({ projectPath, onOpenEditor }: { projectPath: string; onOpenEditor: (agentName: string) => void }): React.JSX.Element {
  const [skills, setSkills] = useState<SkillSummary[]>([])
  const [agents, setAgents] = useState<K2soAgentInfo[]>([])
  const [loading, setLoading] = useState(true)
  const [showCreate, setShowCreate] = useState(false)
  const [newName, setNewName] = useState('')
  const [newSeed, setNewSeed] = useState('')
  const [creating, setCreating] = useState(false)
  const nameInputRef = useRef<HTMLInputElement>(null)

  const fetchSkills = useCallback(async () => {
    // Phase 2.5b followup: dedicated `.k2so/skills/` enumerator. Returns
    // every skill folder regardless of workspace mode, so the Skills
    // section works under every mode (Off / K2 Agent / Manager /
    // Custom).
    try {
      const result = await invoke<SkillSummary[]>('k2so_skills_list', { projectPath })
      setSkills(result)
    } catch {
      setSkills([])
    } finally {
      setLoading(false)
    }
  }, [projectPath])

  const fetchAgents = useCallback(async () => {
    try {
      const result = await invoke<K2soAgentInfo[]>('k2so_agents_list', { projectPath })
      setAgents(result)
    } catch {
      setAgents([])
    }
  }, [projectPath])

  useEffect(() => {
    fetchSkills()
    fetchAgents()
  }, [fetchSkills, fetchAgents])

  useEffect(() => {
    if (showCreate) {
      requestAnimationFrame(() => nameInputRef.current?.focus())
    }
  }, [showCreate])

  const handleCreate = useCallback(async () => {
    if (!newName.trim()) return
    setCreating(true)
    try {
      await daemonCliPost('skills/create', {
        project_path: projectPath,
        name: newName.trim().toLowerCase().replace(/\s+/g, '-'),
        from_skill: newSeed.trim() ? newSeed.trim() : null,
      })
      setNewName('')
      setNewSeed('')
      setShowCreate(false)
      await fetchSkills()
      await fetchAgents()
    } catch (e) {
      console.error('[skills] Create failed:', e)
    } finally {
      setCreating(false)
    }
  }, [projectPath, newName, newSeed, fetchSkills, fetchAgents])

  const handleDelete = useCallback(async (name: string) => {
    const confirmed = await useConfirmDialogStore.getState().confirm({
      title: `Remove Skill "${name}"?`,
      message: 'The skill folder (.k2so/skills/' + name + '/) will be moved to the recycle bin. You can restore it within ~30 days if you change your mind.',
      confirmLabel: 'Remove',
      destructive: true,
    })
    if (!confirmed) return
    try {
      await daemonCliPost('skills/remove', { project_path: projectPath, name })
      await fetchSkills()
      await fetchAgents()
    } catch (e) {
      console.error('[skills] Remove failed:', e)
    }
  }, [projectPath, fetchSkills, fetchAgents])

  const manager = agents.find((a) => a.isCoordinator)
  // Filter the manager out of the per-row skill list so it isn't
  // rendered twice (once in the Workspace Manager block inside Agent
  // Settings, once here). Other skills — including the K2 planner
  // agent + every sub-agent template — render in the list.
  const skillRows = skills.filter((s) => !manager || s.name !== manager.name)

  const SkillListItem = ({ skill }: { skill: SkillSummary }): React.JSX.Element => (
    <div className="px-3 py-2 border-b border-[var(--color-border)] last:border-b-0">
      <div className="flex items-center justify-between">
        <div className="flex-1 min-w-0 mr-3">
          <div className="flex items-center">
            <span className="text-xs font-medium text-[var(--color-text-primary)] flex-shrink-0">{skill.name}</span>
          </div>
          {skill.title && skill.title !== skill.name && (
            <p className="text-[10px] text-[var(--color-text-muted)] truncate mt-0.5">{skill.title}</p>
          )}
        </div>
        <div className="flex items-center gap-1 flex-shrink-0">
          <button
            onClick={() => onOpenEditor(skill.name)}
            className="px-2 py-0.5 text-[10px] font-medium text-[var(--color-accent)] bg-[var(--color-accent)]/10 hover:bg-[var(--color-accent)]/20 border border-[var(--color-accent)]/30 transition-colors no-drag cursor-pointer"
            title="Edit .k2so/skills/{skill.name}/SKILL.md"
          >
            Edit
          </button>
          <button
            onClick={() => handleDelete(skill.name)}
            className="w-5 h-5 flex items-center justify-center text-[var(--color-text-muted)] hover:text-[var(--color-status-error-soft)] transition-colors no-drag cursor-pointer"
            title="Remove skill (recoverable via recycle bin)"
          >
            <svg width="8" height="8" viewBox="0 0 8 8" fill="none" stroke="currentColor" strokeWidth="1.5">
              <line x1="1" y1="1" x2="7" y2="7" />
              <line x1="7" y1="1" x2="1" y2="7" />
            </svg>
          </button>
        </div>
      </div>
    </div>
  )

  return (
    <div>
      <div className="flex items-center justify-between mb-1">
        <span className="flex items-center gap-1.5">
          {skillRows.length > 0 && (
            <span className="text-[9px] tabular-nums font-medium px-1.5 py-0.5 bg-[var(--color-wash-1)] text-[var(--color-text-muted)]">{skillRows.length}</span>
          )}
        </span>
        <button
          onClick={() => setShowCreate(!showCreate)}
          className="px-2 py-0.5 text-[10px] text-[var(--color-text-muted)] border border-[var(--color-border)] hover:text-[var(--color-text-primary)] hover:border-[var(--color-text-muted)] transition-colors no-drag cursor-pointer"
        >
          {showCreate ? 'Cancel' : '+ New Skill'}
        </button>
      </div>

      {/* Create form — A19 "any skill can seed any other": the
          optional seed dropdown lets the user start a new skill from
          an existing one's SKILL.md body. Defaults to blank scaffold
          when no seed is picked. */}
      {showCreate && (
        <div className="border border-[var(--color-border)] p-3 space-y-2 mb-2">
          <input
            ref={nameInputRef}
            type="text"
            placeholder="Skill name (e.g. backend-eng)"
            value={newName}
            onChange={(e) => setNewName(e.target.value)}
            className="w-full bg-[var(--color-bg-elevated)] border border-[var(--color-border)] text-xs text-[var(--color-text-primary)] px-2 py-1.5 outline-none focus:border-[var(--color-accent)] placeholder:text-[var(--color-text-muted)]"
            onKeyDown={(e) => e.key === 'Enter' && handleCreate()}
          />
          {skills.length > 0 && (
            <select
              value={newSeed}
              onChange={(e) => setNewSeed(e.target.value)}
              className="w-full bg-[var(--color-bg-elevated)] border border-[var(--color-border)] text-xs text-[var(--color-text-primary)] px-2 py-1.5 outline-none focus:border-[var(--color-accent)]"
            >
              <option value="">Blank skill (no seed)</option>
              {skills.map((s) => (
                <option key={s.name} value={s.name}>Seed from: {s.name}</option>
              ))}
            </select>
          )}
          <button
            onClick={handleCreate}
            disabled={creating || !newName.trim()}
            className="px-3 py-1 text-xs font-medium bg-[var(--color-accent)] text-[var(--color-on-accent)] hover:bg-[var(--color-accent)]/90 transition-colors no-drag cursor-pointer disabled:opacity-50"
          >
            {creating ? 'Creating...' : 'Create Skill'}
          </button>
        </div>
      )}

      {/* Skill list. Counts dropped from per-row display — the
          inbox/active/done numbers came from the legacy multi-agent
          work surface and don't map onto the post-Phase-2.5b skills
          primitive (a skill is a doc profile, not a work queue). The
          Workspace Manager subsection (inside Agent Settings) still
          shows the workspace inbox count, which is the relevant unit
          of work. */}
      {loading ? (
        <p className="text-[10px] text-[var(--color-text-muted)]">Loading skills...</p>
      ) : skillRows.length === 0 && !showCreate ? (
        <p className="text-[10px] text-[var(--color-text-muted)]">
          No skills yet. Create one to give your agent a specialized capability profile.
        </p>
      ) : (
        <div className="border border-[var(--color-border)]">
          {skillRows.map((skill) => (
            <SkillListItem key={skill.name} skill={skill} />
          ))}
        </div>
      )}
    </div>
  )
}

// ── Cursor IDE Chat Migration Panel ─────────────────────────────────

interface CursorIdeSession {
  composerId: string
  name: string
  createdAt: number
  lastUpdatedAt: number
  mode: string
  alreadyMigrated: boolean
  migratable: boolean
}

function CursorMigrationPanel({ projectPath }: { projectPath: string }): React.JSX.Element | null {
  const [sessions, setSessions] = useState<CursorIdeSession[]>([])
  const [loading, setLoading] = useState(true)
  const [migrating, setMigrating] = useState(false)
  const [migratingIds, setMigratingIds] = useState<Set<string>>(new Set())
  const [justMigratedIds, setJustMigratedIds] = useState<Set<string>>(new Set())
  const [error, setError] = useState<string | null>(null)

  const fetchIdeSessions = useCallback(async () => {
    try {
      const result = await daemonCliGet<CursorIdeSession[]>('chat/discover-ide', { project_path: projectPath })
      setSessions(result)
    } catch {
      setSessions([])
    } finally {
      setLoading(false)
    }
  }, [projectPath])

  useEffect(() => {
    fetchIdeSessions()
  }, [fetchIdeSessions])

  const unmigratedSessions = sessions.filter((s) => !s.alreadyMigrated && !justMigratedIds.has(s.composerId) && s.migratable)
  const migratedSessions = sessions.filter((s) => s.alreadyMigrated || justMigratedIds.has(s.composerId))
  const nonMigratableSessions = sessions.filter((s) => !s.migratable && !s.alreadyMigrated)

  const handleMigrateAll = useCallback(async () => {
    if (unmigratedSessions.length === 0) return
    setMigrating(true)
    setError(null)

    let succeeded = 0
    let failed = 0

    // Migrate one at a time so the UI updates per-session
    for (const session of unmigratedSessions) {
      setMigratingIds(new Set([session.composerId]))
      try {
        const r = await daemonCliPost<{ migrated: number }>('chat/migrate-ide', {
          project_path: projectPath,
          composer_ids: [session.composerId],
        })
        const count = r.migrated
        if (count > 0) {
          succeeded++
          setJustMigratedIds((prev) => new Set([...prev, session.composerId]))
        } else {
          failed++
        }
      } catch {
        failed++
      }
    }

    if (failed > 0) {
      setError(`${succeeded} migrated, ${failed} failed (missing conversation data)`)
    }
    setMigrating(false)
    setMigratingIds(new Set())
    await fetchIdeSessions()
  }, [unmigratedSessions, projectPath, fetchIdeSessions])

  if (loading) return null
  if (sessions.length === 0) return null

  return (
    <div className="space-y-2">
      <h3 className="text-[10px] font-semibold text-[var(--color-text-muted)] uppercase tracking-wider flex items-center gap-1.5">
        Cursor IDE Conversations
        <span className="text-[9px] tabular-nums font-medium px-1.5 py-0.5 bg-[var(--color-wash-1)] text-[var(--color-text-muted)]">{sessions.length}</span>
      </h3>

      <p className="text-[10px] text-[var(--color-text-muted)]">
        Migrate conversations from the Cursor IDE to CLI format so they can be resumed in K2 terminals.
      </p>

      {/* Session list */}
      <div className="border border-[var(--color-border)] max-h-[250px] overflow-y-auto">
        {sessions.map((session, i) => {
          const isMigrated = session.alreadyMigrated || justMigratedIds.has(session.composerId)
          const isCurrentlyMigrating = migratingIds.has(session.composerId)
          const date = new Date(session.lastUpdatedAt || session.createdAt)
          const dateStr = date.toLocaleDateString('en-US', { month: 'short', day: 'numeric' })

          return (
            <div
              key={session.composerId}
              className={`flex items-center gap-2 px-3 py-1.5 text-xs ${
                i < sessions.length - 1 ? 'border-b border-[var(--color-border)]' : ''
              }`}
            >
              <AgentIcon agent="Cursor Agent" size={12} />
              <span className={`flex-1 truncate ${isMigrated ? 'text-[var(--color-text-muted)]' : 'text-[var(--color-text-primary)]'}`}>
                {session.name || 'Untitled'}
              </span>
              <span className="text-[10px] text-[var(--color-text-muted)] flex-shrink-0">
                {dateStr}
              </span>
              {isCurrentlyMigrating ? (
                <span className="text-[10px] text-[var(--color-accent)] flex-shrink-0 animate-pulse">
                  migrating...
                </span>
              ) : isMigrated ? (
                <span className="text-[10px] text-[var(--color-status-ok-soft)] flex-shrink-0">
                  migrated
                </span>
              ) : !session.migratable ? (
                <span className="text-[10px] text-[var(--color-text-muted)] flex-shrink-0 opacity-50">
                  chat only
                </span>
              ) : (
                <span className="text-[10px] text-[var(--color-text-muted)] flex-shrink-0">
                  pending
                </span>
              )}
            </div>
          )
        })}
      </div>

      {/* Error */}
      {error && (
        <p className="text-[10px] text-[var(--color-status-error-soft)]">{error}</p>
      )}

      {/* Status + button */}
      <div className="flex items-center justify-between">
        <span className="text-[10px] text-[var(--color-text-muted)]">
          {migratedSessions.length > 0 && (
            <span className="text-[var(--color-status-ok-soft)]">{migratedSessions.length} migrated</span>
          )}
          {migratedSessions.length > 0 && unmigratedSessions.length > 0 && ' · '}
          {unmigratedSessions.length > 0 && (
            <span>{unmigratedSessions.length} pending</span>
          )}
          {nonMigratableSessions.length > 0 && (
            <span> · {nonMigratableSessions.length} chat-only</span>
          )}
          {migratedSessions.length > 0 && unmigratedSessions.length === 0 && nonMigratableSessions.length === 0 && (
            <span> — all conversations available in Chat History</span>
          )}
        </span>

        {unmigratedSessions.length > 0 && (
          <button
            onClick={handleMigrateAll}
            disabled={migrating}
            className="px-3 py-1.5 text-xs font-medium bg-[var(--color-accent)] text-[var(--color-on-accent)] hover:opacity-90 transition-opacity cursor-pointer disabled:opacity-50 no-drag"
          >
            {migrating
              ? `Migrating ${migratingIds.size}...`
              : `Migrate ${unmigratedSessions.length}`}
          </button>
        )}
      </div>
    </div>
  )
}


