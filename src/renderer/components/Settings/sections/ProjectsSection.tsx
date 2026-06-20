import React from 'react'
import { useEffect, useState, useCallback, useRef, useMemo } from 'react'
import { listen, emit } from '@tauri-apps/api/event'
import { invoke } from '@tauri-apps/api/core'
import { daemonCliGet, daemonCliPost } from '@/lib/daemon-cli'
import { agentDisplayName, setAgentDisplayName } from '@/lib/workspace-agent'
import { useSettingsStore } from '@/stores/settings'
import { useProjectsStore, type ProjectWithWorkspaces } from '@/stores/projects'
import { useFocusGroupsStore } from '@/stores/focus-groups'
import { usePresetsStore, parseCommand } from '@/stores/presets'
import { useTabsStore } from '@/stores/tabs'
import { useConfirmDialogStore } from '@/stores/confirm-dialog'
import { pickWorkspaceFolder } from '@/lib/pick-workspace-folder'
import IconCropDialog from '../IconCropDialog'
import ProjectAvatar from '@/components/Sidebar/ProjectAvatar'
import AgentIcon from '@/components/AgentIcon/AgentIcon'
import { AgentPersonaEditor } from '@/components/AgentPersonaEditor/AgentPersonaEditor'
import { AIFileEditor } from '@/components/AIFileEditor/AIFileEditor'
import Markdown from '@/components/Markdown/Markdown'
import remarkGfm from 'remark-gfm'
import { CodeEditor } from '@/components/FileViewerPane/CodeEditor'
import { CustomThemeCreator } from '../CustomThemeCreator'
import { SettingsGroup, SettingDropdown } from '../controls/SettingControls'
import { CAP_LABELS, CAP_COLORS, CAPABILITIES, type StateData } from '@shared/constants/capabilities'
import { showContextMenu } from '@/lib/context-menu'
import { SectionErrorBoundary } from '../SectionErrorBoundary'
import type { SettingEntry } from '../searchManifest'
import { HeartbeatsPanel, HistoryPanel, WakeupEditor, type HeartbeatRow } from './HeartbeatsSection'
import { ContextLayersPreview } from './ContextLayersPreview'
import { RoleSkillEditor } from './RoleSkillEditor'
import { CanonicalAgentModal } from './CanonicalAgentModal'
import { type HarnessProbe } from './canonicalState'
import { RoleSkillButton, CanonicalAgentButton } from './CanonicalAgentButtons'

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
  { id: 'projects.workspace-knowledge', section: 'projects', label: 'Workspace Knowledge', description: 'Shared .k2so/PROJECT.md injected into every agent', keywords: ['workspace knowledge', 'project context', 'project.md', 'shared'] },
  { id: 'projects.heartbeat', section: 'projects', label: 'Heartbeat Schedule', description: 'Scheduled / hourly / off per-project heartbeat mode', keywords: ['heartbeat', 'schedule', 'cron', 'hourly', 'scheduled'] },
  { id: 'projects.agents', section: 'projects', label: 'Project Agents', description: 'Custom agent personas + wake-up files per workspace', keywords: ['agent', 'persona', 'wakeup', 'create'] },
  { id: 'projects.worktrees', section: 'projects', label: 'Worktree Folders', description: 'Enable/disable per-agent git worktrees', keywords: ['worktree', 'git', 'branch'] },
  { id: 'projects.relations', section: 'projects', label: 'Connected Workspaces', description: 'Workspace relations for cross-project messaging', keywords: ['relations', 'connected', 'cross-workspace', 'links'] },
  { id: 'projects.cursor-migrate', section: 'projects', label: 'Cursor Session Migration', description: 'Port Cursor IDE sessions into K2', keywords: ['cursor', 'migrate', 'session', 'import'] },
]

export function ProjectsSection(): React.JSX.Element {
  const projects = useProjectsStore((s) => s.projects)
  const removeProject = useProjectsStore((s) => s.removeProject)
  const fetchProjects = useProjectsStore((s) => s.fetchProjects)
  const projectSettings = useSettingsStore((s) => s.projectSettings)
  const updateProjectSetting = useSettingsStore((s) => s.updateProjectSetting)

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
  const editors = ['Cursor', 'VS Code', 'Zed', 'Other']

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

  const settingsAgenticEnabled = useSettingsStore((s) => s.agenticSystemsEnabled)

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
          await fetchProjects()
        } else if (hoveredGroupId === '__ungrouped__') {
          await assignProjectToGroup(projectId, null)
          await fetchProjects()
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
  }, [reorderProjects, assignProjectToGroup, fetchProjects])


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
      await daemonCliPost('projects/update', { id: p.id, pinned: p.pinned ? 0 : 1 })
      emitProjectsChanged()
      await fetchProjects()
    } else if (clickedId.startsWith('move:')) {
      const groupId = clickedId.replace('move:', '')
      await assignProjectToGroup(p.id, groupId === '__none__' ? null : groupId)
      await fetchProjects()
    }
  }, [focusGroupsEnabled, focusGroups, fetchProjects, assignProjectToGroup])

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
              className={`w-2.5 h-2.5 bg-white block transition-transform ${
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

      {/* ── Right panel: selected workspace settings ── */}
      <div className="flex-1 overflow-y-auto p-6 min-h-0 relative">
        {selectedProject ? (
          <ProjectDetail
            project={selectedProject}
            editors={editors}
            focusGroups={focusGroups}
            focusGroupsEnabled={focusGroupsEnabled}
            projectSettings={projectSettings}
            updateProjectSetting={updateProjectSetting}
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
        <span className="text-[9px] tabular-nums font-medium px-1.5 py-0.5 bg-white/5 text-[var(--color-text-muted)]">{nonBare.length}</span>
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
                <span className="text-[10px] text-green-400 flex-shrink-0">(active)</span>
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

// ── Workspace Detail (right panel content) ─────────────────────────────
function ProjectDetail({
  project,
  editors,
  focusGroups,
  focusGroupsEnabled,
  projectSettings,
  updateProjectSetting,
  removeProject,
  assignProjectToGroup,
  fetchProjects
}: {
  project: ReturnType<typeof useProjectsStore.getState>['projects'][number]
  editors: string[]
  focusGroups: ReturnType<typeof useFocusGroupsStore.getState>['focusGroups']
  focusGroupsEnabled: boolean
  projectSettings: Record<string, Record<string, any>>
  updateProjectSetting: (projectId: string, key: string, value: string) => void
  removeProject: (id: string) => Promise<void>
  assignProjectToGroup: (projectId: string, groupId: string | null) => Promise<void>
  fetchProjects: () => Promise<void>
}): React.JSX.Element {
  const [iconLoading, setIconLoading] = useState(false)
  const [cropImage, setCropImage] = useState<string | null>(null)
  const [agentEditorOpen, setAgentEditorOpen] = useState(false)
  const [agentEditorName, setAgentEditorName] = useState('')
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
  // workspace, there's no audit to show — collapse the right column so
  // we don't leave an empty frame next to every Off workspace.
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

  // Close editor when project changes (user navigated away without using back button)
  useEffect(() => {
    setAgentEditorOpen(false)
    setAgentEditorName('')
    setWakeupEditingHb(null)
    setCanonicalModalMode(null)
  }, [project.id])

  // Detect per-harness canonical state (PRD §5.2) so the canonical button
  // reads "Set up …" vs "Manage / Undo". Re-runs when the modal closes so a
  // just-completed setup flips the label. Best-effort — a failure leaves the
  // button in its default "Set up …" state.
  useEffect(() => {
    let cancelled = false
    invoke<HarnessProbe[]>('k2so_detect_canonical_state', { projectPath: project.path })
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
    fileInputRef.current?.click()
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

  // Full-screen agent editor takeover (same pattern as CustomThemeCreator)
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

  return (
    <>
    {cropImage && (
      <IconCropDialog
        imageDataUrl={cropImage}
        onConfirm={handleCropConfirm}
        onCancel={() => setCropImage(null)}
      />
    )}
    <div className={`grid gap-8 ${
      (project.agentMode || 'off') === 'off' && historyEmpty
        ? 'grid-cols-[minmax(0,42rem)]'
        : 'grid-cols-[minmax(0,42rem)_minmax(0,1fr)]'
    }`}>
    <div className="space-y-6 min-w-0">
      {/* ── Header ── */}
      <div className="flex items-start justify-between gap-3">
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

      {/* ── Group 1: Workspace — Icon, Color, Focus Group ── */}
      <SettingsGroup title="Workspace">
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
                className="text-white font-bold"
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
                className="px-2.5 py-1 text-xs text-red-400 border border-red-500/30 hover:bg-red-500/10 no-drag cursor-pointer disabled:opacity-50 disabled:cursor-not-allowed transition-colors"
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
                onClick={async () => {
                  await daemonCliPost('projects/update', { id: project.id, color })
                  emitProjectsChanged()
                  await fetchProjects()
                }}
                className={`w-4 h-4 flex-shrink-0 no-drag cursor-pointer transition-transform ${
                  project.color === color ? 'scale-125 ring-1 ring-white/50' : 'hover:scale-110'
                }`}
                style={{ backgroundColor: color }}
              />
            ))}
          </div>
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

        {/* Workspace Knowledge — edits the canonical .k2so/PROJECT.md
            (shared project knowledge injected into every agent at launch).
            Regenerates the workspace SKILL.md on close. This is the single
            PROJECT.md editor (the redundant "Project Context" block was
            removed). */}
        <div className="pt-3 border-t border-[var(--color-border)]">
          <div className="flex items-center justify-between">
            <div>
              <span className="text-xs text-[var(--color-text-secondary)]">Workspace Knowledge</span>
              <p className="text-[9px] text-[var(--color-text-muted)] mt-0.5">
                Edits <span className="font-mono">.k2so/PROJECT.md</span> — shared project knowledge (tech stack, conventions, key directories) injected into every agent at launch.
              </p>
            </div>
            <button
              onClick={() => { setAgentEditorName('__claude_md__'); setAgentEditorOpen(true) }}
              className="px-5 py-1.5 text-[11px] font-medium text-[var(--color-accent)] bg-[var(--color-accent)]/10 hover:bg-[var(--color-accent)]/20 border border-[var(--color-accent)]/30 transition-colors no-drag cursor-pointer flex-shrink-0 whitespace-nowrap"
            >
              Manage Knowledge
            </button>
          </div>
        </div>
      </SettingsGroup>

      {/* ── Group 2: Agent Settings — Mode tabs, Heartbeat, Agents list ── */}
      {useSettingsStore.getState().agenticSystemsEnabled && <SettingsGroup
        title="Agent Settings"
        badge={
          <span
            className="text-[8px] uppercase tracking-wider font-semibold px-1.5 py-0.5 bg-[var(--color-accent)]/15 text-[var(--color-accent)]"
            title="Agentic systems are in beta — interface and behavior may change"
          >
            beta
          </span>
        }
      >
        <div className="space-y-2">
          {/* Mode selector */}
          <div className="flex gap-1">
            {(['off', 'agent', 'manager', 'custom'] as const).map((mode) => {
              const isActive = (project.agentMode || 'off') === mode || (mode === 'manager' && (project.agentMode === 'coordinator' || project.agentMode === 'pod'))
              const labels = { off: 'Off', custom: 'Custom Agent', agent: 'K2 Agent', manager: 'Workspace Manager' }
              return (
                <button
                  key={mode}
                  onClick={async () => {
                    const currentMode = project.agentMode || 'off'
                    if (currentMode === mode) return

                    // Confirm before modifying CLAUDE.md — explain what will happen
                    const fromLabel = currentMode === 'off' ? null : labels[currentMode as keyof typeof labels]
                    const toLabel = labels[mode]

                    if (mode === 'off') {
                      const confirmed = await useConfirmDialogStore.getState().confirm({
                        title: `Disable ${fromLabel} Mode`,
                        message: [
                          'This will:',
                          '',
                          '• Move CLAUDE.md to .k2so/CLAUDE.md.disabled',
                          '• Your content is preserved and restored if you re-enable',
                          '• The heartbeat will be turned off if active',
                        ].join('\n'),
                        confirmLabel: 'Disable',
                      })
                      if (!confirmed) return
                    } else if (mode === 'custom') {
                      const lines = [
                        'Train a single agent to operate any software via the heartbeat.',
                        '',
                        'What happens:',
                        '• No CLAUDE.md is generated — the agent runs from its persona only',
                        '• Use "Manage Persona" to define its behavior with the AI editor',
                        '• Worktrees are disabled in this mode',
                      ]
                      if (currentMode !== 'off') {
                        lines.push('', `Switching from ${fromLabel}:`, '• The current CLAUDE.md will be moved to .k2so/CLAUDE.md.disabled')
                      }
                      const confirmed = await useConfirmDialogStore.getState().confirm({
                        title: `Enable ${toLabel} Mode`,
                        message: lines.join('\n'),
                        confirmLabel: `Enable ${toLabel} Mode`,
                      })
                      if (!confirmed) return
                    } else if (mode === 'agent') {
                      const lines = [
                        'A K2 planner agent that helps you build PRDs, milestones, and technical plans.',
                        '',
                        'What happens:',
                        '• Generates a CLAUDE.md with K2 planner instructions',
                        '• If a user-written CLAUDE.md exists, it won\'t be overwritten',
                        '  (the generated version is saved to .k2so/CLAUDE.md.generated)',
                      ]
                      if (currentMode !== 'off') {
                        lines.push('', `Switching from ${fromLabel}:`, '• The current CLAUDE.md will be moved to .k2so/CLAUDE.md.disabled')
                      }
                      const confirmed = await useConfirmDialogStore.getState().confirm({
                        title: `Enable ${toLabel} Mode`,
                        message: lines.join('\n'),
                        confirmLabel: `Enable ${toLabel} Mode`,
                      })
                      if (!confirmed) return
                    } else if (mode === 'manager') {
                      const lines = [
                        'A workspace manager delegates work to agent templates that execute in parallel worktrees.',
                        '',
                        'What happens:',
                        '• Generates a CLAUDE.md with manager instructions',
                        '• A manager agent is created automatically',
                        '• If a user-written CLAUDE.md exists, it won\'t be overwritten',
                        '  (the generated version is saved to .k2so/CLAUDE.md.generated)',
                      ]
                      if (currentMode !== 'off') {
                        lines.push('', `Switching from ${fromLabel}:`, '• The current CLAUDE.md will be moved to .k2so/CLAUDE.md.disabled')
                      }
                      const confirmed = await useConfirmDialogStore.getState().confirm({
                        title: `Enable ${toLabel} Mode`,
                        message: lines.join('\n'),
                        confirmLabel: `Enable ${toLabel} Mode`,
                      })
                      if (!confirmed) return
                    }

                    if (currentMode !== 'off') {
                      await daemonCliPost('agents/disable-workspace-claude-md', {
                        project_path: project.path,
                      }).catch(console.error)
                    }

                    await daemonCliPost('projects/update', { id: project.id, agentMode: mode })
                    emitProjectsChanged()

                    if (mode === 'agent' || mode === 'manager') {
                      await daemonCliPost('agents/regenerate-workspace-skill', {
                        project_path: project.path,
                      }).catch(console.error)
                    }

                    if (mode === 'off' && project.heartbeatEnabled) {
                      await daemonCliPost('projects/update', { id: project.id, heartbeatEnabled: 0 })
                      emitProjectsChanged()
                    }

                    await fetchProjects()
                  }}
                  className={`flex-1 px-2 py-1.5 text-[10px] font-medium transition-colors no-drag cursor-pointer ${
                    isActive
                      ? 'bg-[var(--color-accent)] text-white'
                      : 'bg-[var(--color-bg-elevated)] text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)] border border-[var(--color-border)]'
                  }`}
                >
                  {labels[mode]}
                </button>
              )
            })}
          </div>

          <p className="text-[10px] text-[var(--color-text-muted)]">
            {(project.agentMode || 'off') === 'off' && 'No agent features enabled for this workspace.'}
            {(project.agentMode || 'off') === 'custom' && 'Custom Agent — train agents to operate any software via the heartbeat. Customize each agent\'s behavior with the AI persona editor.'}
            {(project.agentMode || 'off') === 'agent' && 'K2 Agent — a planner that helps you build PRDs, milestones, and technical plans for this workspace.'}
            {((project.agentMode || 'off') === 'manager' || project.agentMode === 'coordinator' || project.agentMode === 'pod') && 'Workspace Manager — delegates work to agent templates that execute in parallel worktrees.'}
          </p>

          {/* Agent identity — Persona + name FIRST (above State) so the
              reading order is identity → lifecycle-state → schedule. */}
          {(project.agentMode || 'off') === 'custom' && (
            <div className="pt-2 border-t border-[var(--color-border)]">
              <CustomAgentPersonaButton projectPath={project.path} projectName={project.name} onOpenEditor={(name) => { setAgentEditorName(name); setAgentEditorOpen(true) }} />
            </div>
          )}
          {(project.agentMode || 'off') === 'agent' && (
            <div className="pt-2 border-t border-[var(--color-border)]">
              <K2SOAgentPersonaButton projectPath={project.path} projectName={project.name} onOpenEditor={(name) => { setAgentEditorName(name); setAgentEditorOpen(true) }} />
            </div>
          )}

          {/* ── Three opt-in agent-setup skills (canonical-agents PRD §9.3) ──
              Role buttons gate on agent role; the canonical button is
              ALWAYS shown — every mode incl. custom AND off — so it must
              render OUTSIDE the ProjectAgentsPanel `!== 'off'` gate below. */}
          {/* Workspace Manager — role = manager (manager|coordinator|pod). */}
          {((project.agentMode || 'off') === 'manager' || project.agentMode === 'coordinator' || project.agentMode === 'pod') && (
            <div className="pt-2 border-t border-[var(--color-border)]">
              <RoleSkillButton
                role="workspace-manager"
                projectPath={project.path}
                onOpen={() => { setAgentEditorName('__workspace_manager__'); setAgentEditorOpen(true) }}
              />
            </div>
          )}
          {/* K2 Agent — role = agent. */}
          {(project.agentMode || 'off') === 'agent' && (
            <div className="pt-2 border-t border-[var(--color-border)]">
              <RoleSkillButton
                role="k2-agent"
                projectPath={project.path}
                onOpen={() => { setAgentEditorName('__k2_agent__'); setAgentEditorOpen(true) }}
              />
            </div>
          )}
          {/* K2 Canonical Agent — ALWAYS, including custom and off. */}
          <div className="pt-2 border-t border-[var(--color-border)]">
            <CanonicalAgentButton
              probes={canonicalProbes}
              projectPath={project.path}
              onOpen={(mode) => setCanonicalModalMode(mode)}
            />
          </div>

          {/* State selector — only when a mode is active */}
          {(project.agentMode || 'off') !== 'off' && (
            <StateSelector projectId={project.id} currentStateId={project.stateId} />
          )}

          {/* Heartbeats moved to the right column (next to History +
              Context Layers) so the wake-related surfaces live together
              and the main settings column stays focused on workspace
              identity/mode/worktree setup. See the aside below. */}

          {/* Workspace Manager block — gated to manager modes only.
              ProjectAgentsPanel's only remaining content is the Manager
              section (Skills was promoted out in 0.39.0h; the Project
              Context block was removed — it duplicated the always-on
              "Workspace Knowledge" editor above). So render the panel AND
              its top divider only for manager workspaces; other modes
              would otherwise show an empty panel under an orphaned line. */}
          {((project.agentMode || 'off') === 'manager' || project.agentMode === 'coordinator' || project.agentMode === 'pod') && (
            <div className="pt-2">
              <ProjectAgentsPanel projectPath={project.path} onOpenEditor={(name) => { setAgentEditorName(name); setAgentEditorOpen(true) }} />
            </div>
          )}

          {/* Connected Workspaces — for manager, coordinator, and custom modes */}
          {((project.agentMode || 'off') === 'manager' || project.agentMode === 'coordinator' || (project.agentMode || 'off') === 'custom') && (
            <div className="pt-2 border-t border-[var(--color-border)]">
              <ConnectedWorkspacesPanel projectId={project.id} />
            </div>
          )}
        </div>
      </SettingsGroup>}

      {/* ── Group 2.5: Skills — promoted out of Agent Settings in 0.39.0h.
          Phase 2.1 invariant: workspace has ONE primary agent + N skill
          profiles. Primary-agent and skills are sibling concepts, so
          the visual hierarchy renders them as sibling SettingsGroups
          rather than parent (Agent Settings) → child (Skills). Always
          visible regardless of `agenticSystemsEnabled` / `agentMode`
          per the Phase 2.5b followup — every workspace can manage its
          skill profiles. */}
      <SettingsGroup title="Skills">
        <ProjectSkillsPanel projectPath={project.path} onOpenEditor={(name) => { setAgentEditorName(name); setAgentEditorOpen(true) }} />
      </SettingsGroup>

      {/* ── Group 3: Worktree Management ── */}
      <SettingsGroup title="Worktrees">
        {/* Worktrees table */}
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

        {/* Worktree Folders on Disk */}
        <WorktreeFoldersOnDisk project={project} fetchProjects={fetchProjects} />
      </SettingsGroup>

      {/* ── Group 4: Chat Migrations ── */}
      <SettingsGroup title="Chat Migrations">
        <CursorMigrationPanel projectPath={project.path} />
      </SettingsGroup>

      {/* ── Danger zone ── */}
      <div className="pt-4 border-t border-[var(--color-border)]">
        <button
          onClick={() => removeProject(project.id)}
          className="px-3 py-1 text-xs text-red-400 border border-red-500/30 hover:bg-red-500/10 no-drag cursor-pointer"
        >
          Remove Workspace
        </button>
      </div>
    </div>

    {/* Right column — wake-related surfaces grouped together: Heartbeats
        (the schedule), Context Layers (what ships in each wake), and the
        fire History (what actually happened). Sticks to the top while
        the left column scrolls so the wake picture stays visible next to
        whatever setting you're editing. Hidden entirely when the
        workspace is Off AND has no historical fire rows, so an Off
        workspace that was never an agent doesn't get a dead frame. */}
    {!((project.agentMode || 'off') === 'off' && historyEmpty) && (
      <aside className="min-w-0 sticky top-0 self-start space-y-4">
        {(project.agentMode || 'off') !== 'off' && (
          <>
            <HeartbeatsPanel
              key={`hb-${hbRefreshNonce}`}
              projectPath={project.path}
              agentMode={project.agentMode || null}
              // 0.39.0f Phase 2.1: single daemon-resolved primary
              // agent identity, no mode→literal mapping. See
              // primaryAgentName useEffect above for the resolver.
              agentName={primaryAgentName
                || project.name.toLowerCase().replace(/\s+/g, '-')}
              onConfigureWakeup={(row) => setWakeupEditingHb(row)}
            />
            <ShowHeartbeatSessionsToggle projectPath={project.path} />
          </>
        )}
        {(project.agentMode || 'off') !== 'off' && (
          <ContextLayersPreview
            projectPath={project.path}
            agentMode={project.agentMode || null}
            onOpenSettings={() => {
              // Deep-link to Settings → Agent Skills. The layer stack is
              // read-only; edits go through the Agent Skills section.
              useSettingsStore.getState().setSection('agent-skills')
            }}
          />
        )}
        <HistoryPanel projectPath={project.path} onEmptyChange={setHistoryEmpty} />
      </aside>
    )}
    </div>
    </>
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
            className={`w-2.5 h-2.5 bg-white block transition-transform ${
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
              className="w-full text-left px-3 py-1.5 text-[11px] text-red-400 hover:bg-red-500/10 hover:text-red-300 transition-colors no-drag cursor-pointer"
            >
              Delete Agent
            </button>
          )}
        </div>
      )}
    </div>
  )
}

// ── Adaptive Heartbeat Config (Phase indicator, interval, force-wake) ──

interface HeartbeatConfig {
  mode: string
  intervalSeconds: number
  phase: string
  maxIntervalSeconds: number
  minIntervalSeconds: number
  costBudget: string
  consecutiveNoOps: number
  autoBackoff: boolean
  lastWake: string | null
  nextWake: string | null
}

const PHASE_COLORS: Record<string, { dot: string; label: string }> = {
  setup: { dot: 'bg-blue-400', label: 'text-blue-400' },
  active: { dot: 'bg-green-400', label: 'text-green-400' },
  monitoring: { dot: 'bg-amber-400', label: 'text-amber-400' },
  idle: { dot: 'bg-gray-400', label: 'text-gray-400' },
  blocked: { dot: 'bg-red-400', label: 'text-red-400' },
}

function AdaptiveHeartbeatConfig({ projectPath }: { projectPath: string }): React.JSX.Element {
  const [config, setConfig] = useState<HeartbeatConfig | null>(null)
  const [agents, setAgents] = useState<{ name: string; type: string }[]>([])
  const [selectedAgent, setSelectedAgent] = useState<string>('')

  // Load custom agents for this project
  useEffect(() => {
    invoke<{ name: string; agentType: string }[]>('k2so_agents_list', { projectPath })
      .then((list) => {
        const customAgents = list.filter((a) => a.agentType === 'custom')
        setAgents(customAgents.map((a) => ({ name: a.name, type: a.agentType })))
        if (customAgents.length > 0 && !selectedAgent) {
          setSelectedAgent(customAgents[0].name)
        }
      })
      .catch(() => {})
  }, [projectPath])

  // Load heartbeat config for selected agent
  useEffect(() => {
    if (!selectedAgent) return
    invoke<HeartbeatConfig>('k2so_agents_get_heartbeat', { projectPath, agentName: selectedAgent })
      .then(setConfig)
      .catch(() => setConfig(null))
  }, [projectPath, selectedAgent])

  if (!config || agents.length === 0) return <></>

  const phaseStyle = PHASE_COLORS[config.phase] || PHASE_COLORS.monitoring
  const formatInterval = (s: number) => s >= 3600 ? `${Math.round(s / 3600)}h` : s >= 60 ? `${Math.round(s / 60)}m` : `${s}s`

  const handleUpdate = async (updates: { interval?: number; phase?: string }) => {
    try {
      const result = await invoke<HeartbeatConfig>('k2so_agents_set_heartbeat', {
        projectPath,
        agentName: selectedAgent,
        interval: updates.interval ?? null,
        phase: updates.phase ?? null,
        mode: null,
        costBudget: null,
        forceWake: null,
      })
      setConfig(result)
    } catch (err) {
      console.error('[heartbeat] Update failed:', err)
    }
  }

  const handleForceWake = async () => {
    try {
      // Set next_wake to now so the scheduler picks it up immediately
      const result = await invoke<HeartbeatConfig>('k2so_agents_set_heartbeat', {
        projectPath,
        agentName: selectedAgent,
        interval: null,
        phase: null,
        mode: null,
        costBudget: null,
        forceWake: true,
      })
      setConfig(result)
      // Trigger immediate triage
      await daemonCliGet('scheduler-tick', { project: projectPath })
    } catch (err) {
      console.error('[heartbeat] Force wake failed:', err)
    }
  }

  return (
    <div className="py-2 border-t border-[var(--color-border)]">
      <div className="flex items-center justify-between mb-2">
        <div className="flex items-center gap-2">
          {/* Phase indicator dot */}
          <span className={`w-2 h-2 rounded-full ${phaseStyle.dot}`} />
          <span className={`text-[10px] font-medium ${phaseStyle.label}`}>
            {config.phase}
          </span>
          <span className="text-[10px] text-[var(--color-text-muted)]">
            every {formatInterval(config.intervalSeconds)}
          </span>
          {config.consecutiveNoOps > 0 && (
            <span className="text-[9px] text-[var(--color-text-muted)] opacity-60">
              ({config.consecutiveNoOps} idle)
            </span>
          )}
        </div>
        <button
          onClick={handleForceWake}
          className="px-2 py-0.5 text-[9px] text-[var(--color-text-muted)] border border-[var(--color-border)] hover:text-[var(--color-text-primary)] hover:border-[var(--color-text-secondary)] transition-colors cursor-pointer no-drag"
        >
          Force Wake
        </button>
      </div>

      {/* Interval presets */}
      <div className="flex gap-1 mb-1.5">
        {[
          { label: '1m', seconds: 60, phase: 'active' },
          { label: '5m', seconds: 300, phase: 'monitoring' },
          { label: '15m', seconds: 900, phase: 'monitoring' },
          { label: '1h', seconds: 3600, phase: 'idle' },
        ].map((preset) => (
          <button
            key={preset.label}
            onClick={() => handleUpdate({ interval: preset.seconds, phase: preset.phase })}
            className={`px-2 py-0.5 text-[9px] border transition-colors cursor-pointer no-drag ${
              config.intervalSeconds === preset.seconds
                ? 'border-[var(--color-accent)] text-[var(--color-accent)] bg-[var(--color-accent)]/10'
                : 'border-[var(--color-border)] text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)]'
            }`}
          >
            {preset.label}
          </button>
        ))}
      </div>

      {/* Last/next wake info */}
      <div className="flex gap-3 text-[9px] text-[var(--color-text-muted)]">
        {config.lastWake && (
          <span>Last: {new Date(config.lastWake).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' })}</span>
        )}
        {config.nextWake && (
          <span>Next: {new Date(config.nextWake).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' })}</span>
        )}
        {config.autoBackoff && config.consecutiveNoOps >= 3 && (
          <span className="text-amber-400/80">auto-backoff active</span>
        )}
      </div>
    </div>
  )
}

// ── State Selector (per-workspace dropdown) ──────────────────────────────

function StateSelector({ projectId, currentStateId }: { projectId: string; currentStateId?: string | null }): React.JSX.Element {
  const [states, setStates] = useState<StateData[]>([])
  const [selectedId, setSelectedId] = useState(currentStateId || '')

  useEffect(() => {
    daemonCliGet<StateData[]>('states/list').then(setStates).catch(() => {})
  }, [])

  useEffect(() => {
    setSelectedId(currentStateId || '')
  }, [currentStateId])

  const handleChange = async (stateId: string) => {
    setSelectedId(stateId)
    try {
      await daemonCliPost('projects/update', { id: projectId, stateId: stateId || '' })
      emitProjectsChanged()
      const store = useProjectsStore.getState()
      const updated = store.projects.map((p) =>
        p.id === projectId ? { ...p, stateId: stateId || null } : p
      )
      useProjectsStore.setState({ projects: updated })
    } catch (err) {
      console.error('[state-selector] Update failed:', err)
    }
  }

  if (states.length === 0) return <></>

  const activeState = states.find((t) => t.id === selectedId)

  return (
    <div className="pt-3 pb-1 border-t border-[var(--color-border)]">
      <div className="flex items-center justify-between">
        <span className="text-xs text-[var(--color-text-primary)]">State</span>
        <SettingDropdown
          value={selectedId || ''}
          options={[
            { value: '', label: 'No state' },
            ...states.map((t) => ({ value: t.id, label: t.name })),
          ]}
          onChange={handleChange}
        />
      </div>
      {activeState?.description && (
        <p className="text-[10px] text-[var(--color-text-muted)] mt-1.5 leading-relaxed">{activeState.description}</p>
      )}
      {activeState && (
        <div className="flex flex-wrap gap-x-1.5 gap-y-1 mt-2">
          {CAPABILITIES.map((cap) => {
            const val = activeState[cap.key] as string
            return (
              <span
                key={cap.key}
                className={`inline-flex items-center gap-1 px-1.5 py-0.5 text-[9px] border border-[var(--color-border)] bg-[var(--color-bg)]`}
              >
                <span className="text-[var(--color-text-muted)]">{cap.label}</span>
                <span className={CAP_COLORS[val] || 'text-[var(--color-text-muted)]'}>{CAP_LABELS[val]}</span>
              </span>
            )
          })}
        </div>
      )}
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

function ClaudeMdEditor({ projectPath, projectName, onClose }: { projectPath: string; projectName: string; onClose: () => void }): React.JSX.Element {
  const [content, setContent] = useState('')
  const [previewMode, setPreviewMode] = useState<'preview' | 'edit'>('preview')
  const [previewScale, setPreviewScale] = useState(100)
  const cssScale = Math.round(previewScale * 0.7)

  const filePath = `${projectPath}/.k2/PROJECT.md`
  const watchDir = `${projectPath}/.k2`

  const defaultAgent = useSettingsStore((s) => s.defaultAgent)
  const presets = usePresetsStore((s) => s.presets)
  const agentCommand = useMemo(() => {
    const preset = presets.find((p) => p.id === defaultAgent) || presets.find((p) => p.enabled)
    if (!preset) return null
    return parseCommand(preset.command)
  }, [defaultAgent, presets])

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
    `This is the SOURCE. K2 compiles it (plus the agent's AGENT.md) into the`,
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
    `agent's AGENT.md (.k2so/agent/AGENT.md) and the user can edit it via`,
    `Settings → Workspaces → "Manage Persona".`,
    ``,
    `Current contents:`,
    content,
  ].join('\n'), [projectName, filePath, content])

  const terminalCommand = agentCommand?.command
  const terminalArgs = useMemo(() => {
    if (!agentCommand) return undefined
    const baseArgs = [...agentCommand.args]
    if (agentCommand.command === 'claude') {
      return [
        ...baseArgs,
        '--append-system-prompt', systemPrompt,
        `Read ${filePath}. Help the user define their workspace knowledge for "${projectName}". Start by asking about their tech stack and project structure.`,
      ]
    }
    return baseArgs
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
                        ? 'bg-[var(--color-accent)] text-white'
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

  const validate = (n: string): string | null => {
    if (n.length < 2) return 'Display name must be at least 2 characters.'
    if (n.length > 64) return 'Display name must be at most 64 characters.'
    if (!/^[a-z][a-z0-9-]*[a-z0-9]$/.test(n)) {
      return 'Lowercase letters, digits, hyphens only. Start with a letter, no trailing hyphen.'
    }
    return null
  }

  const handleSave = async (): Promise<void> => {
    // 0.37.9 — lowercase at save time, not on every keystroke. The
    // onChange handler used to lowercase eagerly, but that mutated
    // the DOM input value mid-composition during Apple Dictation,
    // hanging the dictation engagement. Lowercasing once on commit
    // keeps the validation contract intact and dictation working.
    const candidate = draft.toLowerCase()
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
            Agent display name
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
            // engagement hangs/aborts. Lowercase enforcement now
            // happens at save time via `validate()` instead.
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
        <p className="text-[10px] text-red-400 mt-1">{error}</p>
      )}
      {!error && helpText && (
        <p className="text-[9px] text-[var(--color-text-muted)] mt-1">{helpText}</p>
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
        const existing = agents.find((a: any) => a.agentType === 'k2so')
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
  const [showAdd, setShowAdd] = useState(false)
  const [adding, setAdding] = useState(false)
  const [search, setSearch] = useState('')
  const projects = useProjectsStore((s) => s.projects)

  const fetchRelations = useCallback(async () => {
    try {
      const [outgoing, inc] = await Promise.all([
        // Host-aware: read through the daemon so the "Connected Workspaces"
        // panel works when driving a remote K2 Connect host (the LOCAL
        // Tauri invoke misfired against a remote daemon). Mirrors the
        // host-aware relations/create + relations/delete writes.
        daemonCliGet<WorkspaceRelation[]>('relations/list', { project_id: projectId }),
        daemonCliGet<WorkspaceRelation[]>('relations/list-incoming', { project_id: projectId }),
      ])
      setRelations(outgoing)
      setIncoming(inc)
    } catch {
      setRelations([])
      setIncoming([])
    } finally {
      setLoading(false)
    }
  }, [projectId])

  useEffect(() => {
    fetchRelations()
  }, [fetchRelations])

  // Projects available for connecting (exclude self and already-connected, sorted alphabetically)
  const connectedIds = useMemo(() => new Set(relations.map((r) => r.targetProjectId)), [relations])
  const availableProjects = useMemo(
    () => projects
      .filter((p) => p.id !== projectId && !connectedIds.has(p.id))
      .sort((a, b) => a.name.localeCompare(b.name)),
    [projects, projectId, connectedIds]
  )
  const filteredProjects = useMemo(
    () => search.trim()
      ? availableProjects.filter((p) => p.name.toLowerCase().includes(search.toLowerCase()))
      : availableProjects,
    [availableProjects, search]
  )

  const handleAdd = useCallback(async (targetProjectId: string) => {
    setAdding(true)
    try {
      await daemonCliPost('relations/create', { source_project_id: projectId, target_project_id: targetProjectId })
      setShowAdd(false)
      await fetchRelations()
    } catch (e) {
      console.error('[connected-workspaces] Create failed:', e)
    } finally {
      setAdding(false)
    }
  }, [projectId, fetchRelations])

  const handleRemove = useCallback(async (id: string) => {
    try {
      await daemonCliPost('relations/delete', { id })
      await fetchRelations()
    } catch (e) {
      console.error('[connected-workspaces] Delete failed:', e)
    }
  }, [fetchRelations])

  // Resolve target project details
  const projectsById = useMemo(() => {
    const map = new Map<string, typeof projects[number]>()
    for (const p of projects) map.set(p.id, p)
    return map
  }, [projects])

  return (
    <div>
      <div className="flex items-center justify-between mb-2">
        <div>
          <h3 className="text-xs font-medium text-[var(--color-text-primary)]">Connected Workspaces</h3>
          <p className="text-[10px] text-[var(--color-text-muted)] mt-0.5">
            Connect other workspaces so this agent can oversee or interact with them.
          </p>
        </div>
        {availableProjects.length > 0 && (
          <button
            onClick={() => { setShowAdd(!showAdd); setSearch('') }}
            title="Add connection"
            className="w-6 h-6 flex items-center justify-center text-sm leading-none bg-[var(--color-accent)] text-white cursor-pointer no-drag"
          >
            +
          </button>
        )}
      </div>

      {/* Add connection dropdown with search */}
      {showAdd && (
        <div className="border border-[var(--color-border)] bg-[var(--color-bg-elevated)]">
          <div className="px-3 py-1.5 border-b border-[var(--color-border)]">
            <input
              type="text"
              value={search}
              onChange={(e) => setSearch(e.target.value)}
              placeholder="Search workspaces..."
              autoFocus
              className="w-full bg-transparent text-xs text-[var(--color-text-primary)] placeholder-[var(--color-text-muted)] outline-none"
            />
          </div>
          <div className="max-h-[200px] overflow-y-auto">
            {filteredProjects.length === 0 ? (
              <div className="px-3 py-2 text-[10px] text-[var(--color-text-muted)]">
                {search.trim() ? 'No matching workspaces.' : 'No more workspaces available to connect.'}
              </div>
            ) : (
              filteredProjects.map((p) => (
                <button
                  key={p.id}
                  onClick={() => { handleAdd(p.id); setSearch('') }}
                  disabled={adding}
                  className="w-full flex items-center gap-2 px-3 py-1.5 text-left hover:bg-white/[0.06] transition-colors no-drag cursor-pointer disabled:opacity-50 border-b border-[var(--color-border)] last:border-b-0"
                >
                  <span
                    className="w-2 h-2 flex-shrink-0 rounded-full"
                    style={{ backgroundColor: p.color || '#6b7280' }}
                  />
                  <span className="text-xs text-[var(--color-text-primary)] truncate">{p.name}</span>
                  {p.agentMode && p.agentMode !== 'off' && (
                    <span className="text-[9px] text-[var(--color-text-muted)] ml-auto flex-shrink-0">
                      {p.agentMode === 'custom' ? 'Custom' : p.agentMode === 'agent' ? 'K2' : 'Manager'}
                    </span>
                  )}
                </button>
              ))
            )}
          </div>
        </div>
      )}

      {/* Connected workspaces list */}
      {loading ? (
        <div className="text-[10px] text-[var(--color-text-muted)]">Loading...</div>
      ) : relations.length === 0 ? (
        <div className="text-[10px] text-[var(--color-text-muted)]">
          No connected workspaces yet.
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
                  style={{ backgroundColor: target?.color || '#6b7280' }}
                />
                <span className="text-xs text-[var(--color-text-primary)] flex-1 truncate">
                  {target?.name || 'Unknown workspace'}
                </span>
                {target?.agentMode && target.agentMode !== 'off' && (
                  <span className="text-[9px] text-[var(--color-text-muted)]">
                    {target.agentMode === 'custom' ? 'Custom' : target.agentMode === 'agent' ? 'K2' : 'Manager'}
                  </span>
                )}
                <button
                  onClick={() => handleRemove(rel.id)}
                  className="w-5 h-5 flex items-center justify-center text-[var(--color-text-muted)] hover:text-red-400 transition-colors no-drag cursor-pointer flex-shrink-0"
                  title="Remove connection"
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

      {/* Incoming connections (workspaces that connect TO this one) */}
      {!loading && incoming.length > 0 && (
        <>
          <h3 className="text-[10px] font-semibold text-[var(--color-text-muted)] uppercase tracking-wider mt-4">
            Connected Agents
          </h3>
          <p className="text-[10px] text-[var(--color-text-muted)]">
            These agent workspaces have access to communicate with this workspace.
          </p>
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
                    style={{ backgroundColor: source?.color || '#6b7280' }}
                  />
                  <span className="text-xs text-[var(--color-text-primary)] flex-1 truncate">
                    {source?.name || 'Unknown workspace'}
                  </span>
                  {source?.agentMode && source.agentMode !== 'off' && (
                    <span className="text-[9px] text-[var(--color-text-muted)]">
                      {source.agentMode === 'custom' ? 'Custom' : source.agentMode === 'agent' ? 'K2' : 'Manager'}
                    </span>
                  )}
                </div>
              )
            })}
          </div>
        </>
      )}
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
// list as its own top-level Settings group. Always visible (regardless
// of agent mode / `agenticSystemsEnabled`) — per the Phase 2.5b
// followup, every workspace has skill profiles. Fetches its own skills
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
            className="w-5 h-5 flex items-center justify-center text-[var(--color-text-muted)] hover:text-red-400 transition-colors no-drag cursor-pointer"
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
            <span className="text-[9px] tabular-nums font-medium px-1.5 py-0.5 bg-white/5 text-[var(--color-text-muted)]">{skillRows.length}</span>
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
            className="px-3 py-1 text-xs font-medium bg-[var(--color-accent)] text-white hover:bg-[var(--color-accent)]/90 transition-colors no-drag cursor-pointer disabled:opacity-50"
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
        <span className="text-[9px] tabular-nums font-medium px-1.5 py-0.5 bg-white/5 text-[var(--color-text-muted)]">{sessions.length}</span>
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
                <span className="text-[10px] text-green-400 flex-shrink-0">
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
        <p className="text-[10px] text-red-400">{error}</p>
      )}

      {/* Status + button */}
      <div className="flex items-center justify-between">
        <span className="text-[10px] text-[var(--color-text-muted)]">
          {migratedSessions.length > 0 && (
            <span className="text-green-400">{migratedSessions.length} migrated</span>
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
            className="px-3 py-1.5 text-xs font-medium bg-[var(--color-accent)] text-white hover:opacity-90 transition-opacity cursor-pointer disabled:opacity-50 no-drag"
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



