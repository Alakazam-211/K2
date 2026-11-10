import { PrimaryRoom } from '@/components/Room/PrimaryRoom'
import { floatMaterial } from '@/components/ui/float-material'
import React from 'react'
import { useEffect, useLayoutEffect, useState, useCallback, useRef, useMemo } from 'react'
import { listen, emit } from '@tauri-apps/api/event'
import { invoke } from '@tauri-apps/api/core'
import { daemonCliGet, daemonCliPost, daemonCliPostQuery } from '@/lib/daemon-cli'
import { harnessLabel, harnessOfCommand, movePinnedChat, movePinnedChatMessage } from '@/lib/pinned-harness'
import { asArray } from '@/lib/as-array'
import { isBuiltinAgentType } from '@/lib/agent-type'
import {
  addRemoteConnection,
  removeRemoteConnection,
  reAddGoneRemoteConnection,
  canReAddGoneConnection,
  listRemoteConnections,
  type RemoteConnectionEntry,
} from '@/lib/federation'
import { agentDisplayName } from '@/lib/workspace-agent'
import { AddConnectionPicker } from './AddConnectionPicker'
import {
  AgentColorPicker,
  AgentDisplayNameField,
  AgentHandleField,
  AgentPictureControls,
} from '@/components/AgentIdentity/AgentIdentityFields'
import { onAgentIdentityChanged } from '@/stores/session-events'
import { agentIdentityDisplayName, agentIdentityMatchesPath } from '@/lib/agent-identity'
import { useSettingsStore } from '@/stores/settings'
import { useProjectsStore, noteOptimisticProjectsMutationSuccess, type ProjectWithWorkspaces } from '@/stores/projects'
import { useFocusGroupsStore } from '@/stores/focus-groups'
import { usePresetsStore, parseCommand } from '@/stores/presets'
import { useResolvedAgentCommand } from '@/hooks/useResolvedAgentCommand'
import { useTabsStore } from '@/stores/tabs'
import { useConfirmDialogStore } from '@/stores/confirm-dialog'
import { pickWorkspaceFolder } from '@/lib/pick-workspace-folder'
import ProjectAvatar from '@/components/Sidebar/ProjectAvatar'
import AgentIcon from '@/components/AgentIcon/AgentIcon'
import { AgentPersonaEditor } from '@/components/AgentPersonaEditor/AgentPersonaEditor'
import { AIFileEditor } from '@/components/AIFileEditor/AIFileEditor'
import { buildEditorAgentArgs } from '@/lib/editor-agent-args'
import Markdown from '@/components/Markdown/Markdown'
import remarkGfm from 'remark-gfm'
import { CodeEditor } from '@/components/FileViewerPane/CodeEditor'
import { CustomThemeCreator } from '../CustomThemeCreator'
import { AVATAR_ROUND, SETTING_FIELD_WIDTH, SETTING_ROW_HINT, SETTING_ROW_LABEL, SettingDropdown, SettingRow, SettingSwitchRow, SettingsGroup } from '../controls/SettingControls'
import { RemindersList } from '@/components/HeartbeatsPanel/RemindersList'
import { MailWatchPanel } from './MailWatchPanel'
import { showContextMenu } from '@/lib/context-menu'
import { SectionErrorBoundary } from '../SectionErrorBoundary'
import type { SettingEntry } from '../searchManifest'
import { useRemoveWorkspaceDialogStore } from '@/stores/remove-workspace-dialog'
import { HeartbeatsPanel, HistoryPanel, WakeupEditor, type HeartbeatRow } from './HeartbeatsSection'
import { ContextStackEditor, type ContextEditTarget } from './ContextStackEditor'
import { RoleSkillEditor } from './RoleSkillEditor'
import { CanonicalAgentModal } from './CanonicalAgentModal'
import { type HarnessProbe } from './canonicalState'
import { RoleSkillButton, CanonicalAgentButton } from './CanonicalAgentButtons'
import { WorkspaceApiKeysPanel } from './ApiTokensSection'
import { FederatedConnectionRow } from './FederatedConnectionRow'
import { fetchViewerRole } from '@/components/Presence/PresenceKickButton'
import { HideApiSessionsToggle } from '@/components/WorkspacePanel/HideApiSessionsToggle'
import { WorkspaceCompletionSoundToggle } from '@/components/WorkspacePanel/WorkspaceCompletionSoundToggle'
import { WorkspaceTabsSettings } from '@/components/Settings/PinnedTabSettingRows'
import { FolderTrustRow } from './FolderTrustRow'
import { AgentEmailNoticesRow } from './AgentEmailNoticesRow'
import { WorkspacePhonePushToggle } from '@/components/WorkspacePanel/WorkspacePhonePushToggle'
import { workspaceGrantSlug } from './api-keys-api'
import { setSqlDbAgentAccess, setWorkspaceDbUpgrade } from './data-api'
import { primaryScope } from '@/kessel/server-scope'
import { RemoteInstructToggle } from '../shared/RemoteInstructToggle'
import { Button, Callout, Input, OverflowMenu, Surface, Toggle } from '@/components/ui'
import { WorkspaceLlmTokens } from '../shared/LlmLoginPicker'
import { computeErrorMessage, setWorkspaceComputeManage, setWorkspaceComputeOperate, setWorkspaceComputeSwitch } from './compute-api'
import { metaChordLabel } from '@/lib/shortcut-label'
import { reorderedZoneIds } from '@/components/Sidebar/row-drag-reorder'
import { AgentHandoffAcceptFrom } from './HandoffAcceptFrom'
import { AgentAppsAccess } from './AgentAppsAccess'
import { AgentPermissionCardsRow, AskedChip, PermissionAsksProvider } from './PermissionCardsSection'
import { useGardenAccessSwitchStore } from '@/lib/zen/garden-access-api'
import { WorkspaceSettingsPage } from './WorkspaceSettingsPage'
import {
  WORKSPACE_SETTINGS_TABS,
  advancedSectionFor,
  lastOpenedProject,
  parseWorkspaceSettingsTab,
  rememberOpenedProject,
  type WorkspaceSettingsTab,
} from './workspace-tabs'

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
  { id: 'projects.default-model', section: 'projects', label: 'Default model', description: 'The model new sessions in this workspace use', keywords: ['default model', 'opus', 'sonnet', 'workspace model'] },
  { id: 'projects.llm-logins', section: 'projects', label: 'LLM tokens', description: 'Use a specific Claude / Codex / Grok / Gemini subscription or API token for this workspace\'s agents instead of the server default', keywords: ['token', 'tokens', 'subscription', 'api token', 'api key', 'server default', 'login', 'account', 'pin'] },
  { id: 'projects.tabs', section: 'projects', label: 'Tabs: Show the Design tab, Show the Mobile tab', description: 'Keep the Design and Mobile tabs pinned next to Chat and Inbox for this workspace. You can always open them from the + menu', keywords: ['tabs', 'pinned', 'design', 'mobile', 'phone', 'simulator', 'design mode', 'remove', 'bring back', 'show'] },
  { id: 'projects.force-model-on-resume', section: 'projects', label: 'Force model on resume', description: 'Also use the default model when a session resumes', keywords: ['resume', 'model', 'force'] },
  { id: 'projects.db-agent-create', section: 'projects', label: 'Allow this agent to create databases', description: 'Lets this agent create new databases (k2 db create). It can always use the ones it has', keywords: ['db_agent_access', 'create', 'passport', 'agent', 'write', 'database', 'sql'] },
  { id: 'projects.agents-can-manage-skin', section: 'projects', label: 'Build and run Apps', description: 'Apps (beta) for this agent: Build and run Apps, Manage its Apps end to end (budgets, quotas, egress hosts, box services, keys), Let this agent manage its Apps\' users (App users, test guests, passwords, rooms), and what its Apps may use (Database, Files and rooms, Thread, Mail, Chat / Voice Hosting). The owner turns them on; an admin only off. Loosening the wall always asks you', keywords: ['agents_can_manage_skin', 'agents_can_manage_apps_full', 'app_cores', 'cores', 'apps', 'app', 'build', 'run', 'manage', 'end to end', 'database', 'files', 'rooms', 'thread', 'mail', 'sew', 'passport', 'agent', 'guests', 'roles', 'k2 app access'] },
  { id: 'projects.agents-can-manage-app-users', section: 'projects', label: 'Let this agent manage its Apps\u2019 users', description: 'Its own Apps only: add and remove App users and test guests, passwords, email, names, unlock, roles and its own room, invites, the guests-rooms grant in its own room, App keys that manage users. Users it makes are tied to it. Never a box login. The owner turns it on and off', keywords: ['agents_can_manage_app_users', 'app_users', 'app users', 'test guests', 'users', 'guests', 'passwords', 'invites', 'guests-rooms', 'k2 app access --users'] },
  { id: 'projects.agents-can-manage-agents', section: 'projects', label: 'Allow hiring and managing agents', description: 'Lets this workspace\'s main agent open and stop sidecars (k2 sidecar); later also hire and retire agents', keywords: ['agents_can_manage_agents', 'sidecar', 'hire', 'agent', 'passport', 'manage agents'] },
  { id: 'projects.handoff-accept-from', section: 'projects', label: 'Secrets from other agents', description: 'Agents whose secrets go straight into this agent\'s vault; everything else waits for you', keywords: ['handoff', 'accept from', 'vault', 'secret', 'send secret', 'agent to agent'] },
  { id: 'projects.agents-can-manage-vault', section: 'projects', label: 'Allow managing server-wide secrets', description: 'Lets this workspace\'s main agent share server secrets with other workspaces and look after them (k2 vault); never with itself, and never loosen a secret it uses', keywords: ['vault_manage', 'vault', 'secret', 'secrets', 'passport', 'agent', 'it agent'] },
  { id: 'projects.agents-can-use-compute', section: 'projects', label: 'Allow compute nodes', description: 'Lets this workspace\'s agents run builds and tests on the machines switched on in Settings → Compute (k2 compute run); on grants every machine, and ones added later', keywords: ['agents_can_use_compute', 'compute', 'compute nodes', 'node', 'machines', 'build', 'tests', 'k2 compute', 'passport', 'agent'] },
  { id: 'projects.agents-can-manage-compute', section: 'projects', label: 'Allow managing compute nodes', description: 'Lets this workspace\'s agent add and confirm compute machines, rename, pause, drain, resume, retire and route them, and grant or revoke them for any workspace (k2 compute node, k2 compute grant); every act is logged and adding or confirming a machine sends you an inbox note. Turning the preview on or off and removing a machine stay yours. Only the owner can switch it', keywords: ['agents_can_manage_compute', 'compute_manage', 'manage', 'compute', 'compute nodes', 'node', 'machines', 'enroll', 'confirm', 'grant', 'revoke', 'retire', 'k2 compute', 'passport', 'agent'] },
  { id: 'projects.agents-can-upgrade-postgres', section: 'projects', label: 'Allow this agent to manage Postgres versions', description: 'Move this agent\'s databases (and its Apps\' databases) to a newer Postgres, create on a chosen version, and roll back (k2 db upgrade, k2 db create --major); live moves (k2 db upgrade --live) come to agents once proven; every act is logged. Deleting the old copy stays with you. The owner turns it on; the owner or an Admin may turn it off', keywords: ['agents_can_upgrade_postgres', 'db_upgrade', 'postgres', 'postgresql', 'pg', 'pg 17', 'version', 'major', 'upgrade', 'database', 'rollback', 'k2 db upgrade', 'live', 'dual-run', 'agent'] },
  { id: 'projects.agents-can-request-permissions', section: 'projects', label: 'Can ask for permission cards', description: 'Beta: lets this agent ask you or an admin for a setting change with a card in its Thread (k2 permission request); nothing changes until someone allows it. Shown while Permission cards is on for the server (Settings → Access → Permission Cards). The owner or an admin switches it', keywords: ['agents_can_request_permissions', 'permission_cards', 'permission cards', 'permission', 'card', 'ask', 'request', 'approve', 'beta', 'k2 permission', 'agent'] },
  { id: 'projects.agents-can-refresh-chat', section: 'projects', label: 'Can refresh its own pinned chat', description: 'Lets this agent start a new pinned chat with the same agent when its chat goes stale, with a note for itself (k2 agent refresh). The old chat stays in Chat history and the Thread carries on. Off: it asks you on a card. The owner or an admin switches it', keywords: ['agents_can_refresh_chat', 'chat_refresh', 'refresh', 'pinned chat', 'new chat', 'k2 agent refresh', 'chat.refresh', 'stale'] },
  { id: 'projects.agents-can-operate-compute', section: 'projects', label: 'Allow this agent to operate compute nodes', description: 'Lets this workspace\'s agent pause, drain, resume, narrow, update and restart services on your compute machines without asking each time; root actions still wait for you (k2 compute operate)', keywords: ['agents_can_operate_compute', 'compute_operate', 'operate', 'compute', 'compute nodes', 'node', 'machines', 'pause', 'update', 'k2 compute', 'passport', 'agent'] },
  { id: 'projects.agents-can-manage-projects', section: 'projects', label: 'Manage K2 projects', description: 'Lets this workspace\'s agent create, rename and delete K2 projects and add or remove their members (k2 project); on by default. Off: it can still read projects and post in its project chat', keywords: ['agents_can_manage_projects', 'projects_manage', 'project', 'projects', 'k2 project', 'members', 'project chat', 'passport', 'agent'] },
  { id: 'projects.agents-can-manage-gardens', section: 'projects', label: 'Manage Gardens and data sources', description: 'Lets this workspace\'s agent approve data sources, bind secrets it may already use, pause, pin, enable triggers and edit Gardens and widgets with no confirm (k2 sources, k2 zen); every act is logged. Off: it proposes and you approve', keywords: ['agents_can_manage_gardens', 'gardens_manage', 'gardens', 'garden', 'zen', 'widgets', 'data sources', 'sources', 'triggers', 'approve', 'k2 sources', 'k2 zen', 'passport', 'agent'] },
  { id: 'projects.mail-agent-send', section: 'projects', label: 'Let this agent send email', description: 'Per-workspace agent send mode (off, approval, on) for hosted and linked inboxes; same as k2 mail sending', keywords: ['mail_agent_send', 'agent send', 'sending', 'send', 'email', 'mail', 'gmail', 'linked', 'gated', 'approval', 'outbound'] },
  { id: 'projects.mail-manage-enabled', section: 'projects', label: 'Allow agents to manage hosted mail on this server', description: 'Lets this workspace’s agents run k2 hostmail enable/disable and domain list/add/check. Uninstall, domain remove and OAuth stay with the owner', keywords: ['mail_manage_enabled', 'hostmail', 'mail', 'passport', 'agent', 'domain'] },
]

/** Settings pane width (px) from which the workspace list and the selected
 *  workspace's settings sit side by side. Narrower than this, it is one page
 *  at a time: the list, then a workspace with All workspaces to go back. The
 *  list column is 18rem; what's left keeps a settings row (label and hint,
 *  then a 15rem field) readable. The same in every theme. */
export const WORKSPACES_TWO_COLUMN_MIN_PX = 840

/** The element's width, kept current as the Settings pane resizes; 0 until
 *  it has been measured (and in test DOMs, which have no layout). Measured
 *  before paint, so the first frame already has the right layout. Pass the
 *  element itself (a callback ref's state): the root changes with the layout. */
function useElementWidth(el: HTMLElement | null): number {
  const [width, setWidth] = useState(0)
  useLayoutEffect(() => {
    if (!el) return
    const read = (): void => setWidth(el.clientWidth)
    read()
    if (typeof ResizeObserver === 'undefined') return
    const ro = new ResizeObserver(read)
    ro.observe(el)
    return () => ro.disconnect()
  }, [el])
  return width
}

export function ProjectsSection(): React.JSX.Element {
  const [rootEl, rootRef] = useState<HTMLDivElement | null>(null)
  const paneWidth = useElementWidth(rootEl)
  const projects = useProjectsStore((s) => s.projects)
  const fetchProjects = useProjectsStore((s) => s.fetchProjects)

  const focusGroups = useFocusGroupsStore((s) => s.focusGroups)
  const focusGroupsEnabled = useFocusGroupsStore((s) => s.focusGroupsEnabled)
  const setFocusGroupsEnabled = useFocusGroupsStore((s) => s.setFocusGroupsEnabled)
  const createFocusGroup = useFocusGroupsStore((s) => s.createFocusGroup)
  const deleteFocusGroup = useFocusGroupsStore((s) => s.deleteFocusGroup)
  const renameFocusGroup = useFocusGroupsStore((s) => s.renameFocusGroup)
  const assignProjectToGroup = useFocusGroupsStore((s) => s.assignProjectToGroup)

  // Wide: the list on the left and the selected workspace (or the settings
  // for every workspace) on the right. Narrow: one page at a time. A deep
  // link (`initialProjectId`) selects or opens a workspace directly.
  const initialProjectId = useSettingsStore((s) => s.initialProjectId)
  const [selectedProjectId, setSelectedProjectId] = useState<string | null>(initialProjectId ?? null)

  // "Workspace settings" (0.46.5, moved from General → Workspaces): the page
  // for every workspace. `projectsPage` is the one-shot a deep link
  // (`agent-skills`) or a search result sets.
  const projectsPage = useSettingsStore((s) => s.projectsPage)
  const [showWorkspaceSettings, setShowWorkspaceSettings] = useState<boolean>(
    () => projectsPage === 'workspace-settings',
  )
  useEffect(() => {
    if (!projectsPage) return
    setShowWorkspaceSettings(projectsPage === 'workspace-settings')
    if (projectsPage === 'list') setSelectedProjectId(null)
    useSettingsStore.setState({ projectsPage: null })
  }, [projectsPage])
  const selectProject = useCallback((id: string) => {
    setShowWorkspaceSettings(false)
    setSelectedProjectId(id)
    rememberOpenedProject(id)
    // Picking another workspace by hand retires a consumed deep link, so the
    // same link can select its workspace again later.
    const linked = useSettingsStore.getState().initialProjectId
    if (linked && linked !== id) useSettingsStore.setState({ initialProjectId: null })
  }, [])
  const backToList = useCallback(() => {
    setShowWorkspaceSettings(false)
    setSelectedProjectId(null)
    // Clear the deep link so the same workspace can be deep-linked again.
    useSettingsStore.setState({ initialProjectId: null, initialWorkspaceTab: null })
  }, [])

  // When initialProjectId changes (e.g. right-click a different project), update selection
  useEffect(() => {
    if (initialProjectId) {
      selectProject(initialProjectId)
    }
  }, [initialProjectId, selectProject])

  // The tab stays while you move between workspaces.
  const lastTabRef = useRef<WorkspaceSettingsTab>('agent')

  // Nothing to list yet: is that "still loading", "really none", or "the list
  // couldn't be read"? The store only logs a failed read.
  const [listState, setListState] = useState<'loading' | 'ready' | { error: string }>('loading')
  const checkList = useCallback(async () => {
    setListState('loading')
    try {
      await daemonCliGet(primaryScope(), 'projects/list')
      setListState('ready')
    } catch (err) {
      setListState({ error: err instanceof Error ? err.message : String(err) })
    }
  }, [])
  useEffect(() => {
    if (projects.length === 0) void checkList()
    else setListState('ready')
  }, [projects.length, checkList])

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

  // Both ways to delete a group (its menu, right-click) ask first. Workspaces
  // aren't touched: the column is ON DELETE SET NULL, so they become ungrouped.
  const confirmDeleteGroup = useCallback(async (group: { id: string; name: string }) => {
    const count = useProjectsStore.getState().projects.filter((p) => p.focusGroupId === group.id).length
    const ok = await useConfirmDialogStore.getState().confirm({
      title: `Delete the focus group \u201c${group.name}\u201d?`,
      message:
        count === 0
          ? 'It has no workspaces in it.'
          : count === 1
            ? 'Its 1 workspace isn\u2019t removed. It moves to Ungrouped.'
            : `Its ${count} workspaces aren\u2019t removed. They move to Ungrouped.`,
      confirmLabel: 'Delete group',
      destructive: true,
    })
    if (!ok) return
    await deleteFocusGroup(group.id)
    await fetchProjects()
  }, [deleteFocusGroup, fetchProjects])

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
      await confirmDeleteGroup(group)
    }
  }, [focusGroups, confirmDeleteGroup])

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
          // Within-zone reorder. The zone is the rows as painted (search
          // and focus-group filters included) — the drop gap counts those.
          // Send the full order (the Agents nav does the same) so no two
          // workspaces end up on one tab order.
          const container = document.querySelector(containerSelector)
          const zoneIds = container
            ? Array.from(container.querySelectorAll<HTMLElement>('[data-settings-project-id]'))
                .map((el) => el.dataset.settingsProjectId ?? '')
            : []
          const di = reorderDropRef.current
          const allIds = useProjectsStore.getState().projects.map((p) => p.id)
          const ids = di === null ? null : reorderedZoneIds(allIds, zoneIds, projectId, di)
          if (ids) void reorderProjects(ids)
        }
      }

      setReorderDragId(null)
      setReorderZone(null)
      setReorderDropIndex(null)
      setDragOverGroupId(null)
      reorderDropRef.current = null
    }

    document.addEventListener('mousemove', handleMouseMove)
    document.addEventListener('mouseup', handleMouseUp)
  }, [reorderProjects, assignProjectToGroup])


  // The list in the order it is painted: pinned, then each focus group, then
  // the rest. `match` is the search filter (or none).
  const listedOrder = useCallback((match: (p: typeof projects[number]) => boolean): typeof projects => {
    const result: typeof projects = projects.filter((p) => p.pinned && match(p))
    if (focusGroupsEnabled) {
      for (const group of focusGroups) {
        result.push(...projects.filter((p) => p.focusGroupId === group.id && !p.pinned && match(p)))
      }
      result.push(...projects.filter((p) => !p.focusGroupId && !p.pinned && match(p)))
    } else {
      result.push(...projects.filter((p) => !p.pinned && match(p)))
    }
    return result
  }, [projects, focusGroups, focusGroupsEnabled])

  // Build flat list of all visible projects for keyboard navigation
  const allVisibleProjects = useMemo(() => listedOrder(matchesSearch), [listedOrder, matchesSearch])

  // Side by side when the pane is wide enough and there is a workspace to
  // show; otherwise the one-page-at-a-time layout (which also holds the
  // empty, loading and error states).
  const twoColumn = paneWidth >= WORKSPACES_TWO_COLUMN_MIN_PX && projects.length > 0

  // Side by side, a workspace is always shown: the one picked, else the one
  // last viewed, else the first in the list (search doesn't move it).
  let shownProject: typeof projects[number] | null = null
  if (!showWorkspaceSettings) {
    shownProject = selectedProject
    if (!shownProject && twoColumn) {
      const last = lastOpenedProject()
      shownProject = projects.find((p) => p.id === last) ?? listedOrder(() => true)[0] ?? null
    }
  }
  const shownProjectId = shownProject?.id ?? null
  useEffect(() => {
    if (shownProjectId) rememberOpenedProject(shownProjectId)
  }, [shownProjectId])

  // Keep the selected row in view as the selection moves.
  useEffect(() => {
    if (!twoColumn || !shownProjectId) return
    const el = document.querySelector<HTMLElement>(`[data-settings-project-id="${shownProjectId}"]`)
    el?.scrollIntoView?.({ block: 'nearest' })
  }, [twoColumn, shownProjectId])

  // Reset keyboard index when search changes
  useEffect(() => { setKeyboardIndex(-1) }, [searchQuery])

  // Keyboard navigation in search. Side by side, ↑/↓ move the selection
  // itself; on one page they move a highlight and Enter opens it.
  const handleSearchKeyDown = useCallback((e: React.KeyboardEvent) => {
    if (twoColumn) {
      if (e.key !== 'ArrowDown' && e.key !== 'ArrowUp') return
      e.preventDefault()
      if (allVisibleProjects.length === 0) return
      const at = allVisibleProjects.findIndex((p) => p.id === shownProjectId)
      const next = at < 0
        ? 0
        : Math.max(0, Math.min(allVisibleProjects.length - 1, at + (e.key === 'ArrowDown' ? 1 : -1)))
      selectProject(allVisibleProjects[next].id)
      return
    }
    if (e.key === 'ArrowDown') {
      e.preventDefault()
      setKeyboardIndex((prev) => Math.min(prev + 1, allVisibleProjects.length - 1))
    } else if (e.key === 'ArrowUp') {
      e.preventDefault()
      setKeyboardIndex((prev) => Math.max(prev - 1, 0))
    } else if (e.key === 'Enter' && keyboardIndex >= 0 && keyboardIndex < allVisibleProjects.length) {
      e.preventDefault()
      selectProject(allVisibleProjects[keyboardIndex].id)
    }
  }, [twoColumn, allVisibleProjects, shownProjectId, keyboardIndex, selectProject])

  // ↑/↓ on a focused row move to the next row (and, side by side, select it).
  const moveFromRow = useCallback((id: string, delta: 1 | -1) => {
    const at = allVisibleProjects.findIndex((p) => p.id === id)
    const next = at < 0 ? undefined : allVisibleProjects[at + delta]
    if (!next) return
    if (twoColumn) selectProject(next.id)
    document.querySelector<HTMLElement>(`[data-settings-project-id="${next.id}"]`)?.focus()
  }, [allVisibleProjects, twoColumn, selectProject])

  // Scroll keyboard-selected item into view
  useEffect(() => {
    if (keyboardIndex >= 0 && allVisibleProjects[keyboardIndex]) {
      const el = document.querySelector(`[data-settings-project-id="${allVisibleProjects[keyboardIndex].id}"]`)
      el?.scrollIntoView?.({ block: 'nearest' })
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

  const togglePinned = async (p: typeof projects[number]): Promise<void> => {
    const newPinned = p.pinned ? 0 : 1
    await daemonCliPost(primaryScope(), 'projects/update', { id: p.id, pinned: newPinned })
    emitProjectsChanged()
    const store = useProjectsStore.getState()
    useProjectsStore.setState({
      projects: store.projects.map((proj) => (proj.id === p.id ? { ...proj, pinned: newPinned } : proj)),
    })
  }

  // Workspace row renderer (called as function, NOT as <Component/>, to avoid unmount/remount flicker).
  // Side by side the row is narrow: the name over its path, the selected row
  // marked, no chevron (nothing opens on another page).
  const renderProjectRow = (p: typeof projects[number], zone: string, containerSelector: string) => {
    const isDragged = reorderDragId === p.id
    const kbIdx = allVisibleProjects.findIndex((vp) => vp.id === p.id)
    const isKeyboardHighlighted = !twoColumn && kbIdx >= 0 && kbIdx === keyboardIndex
    const isSelected = twoColumn && shownProjectId === p.id
    return (
      <div
        data-settings-project-id={p.id}
        data-selected={isSelected ? '' : undefined}
        role="button"
        tabIndex={0}
        aria-label={twoColumn ? `Show settings for ${p.name}` : `Open ${p.name}`}
        aria-current={isSelected ? 'true' : undefined}
        onClick={() => selectProject(p.id)}
        onKeyDown={(e) => {
          // Keys on the pin button are the button's own.
          if (e.target !== e.currentTarget) return
          if (e.key === 'Enter' || e.key === ' ') {
            e.preventDefault()
            selectProject(p.id)
          } else if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
            e.preventDefault()
            moveFromRow(p.id, e.key === 'ArrowDown' ? 1 : -1)
          }
        }}
        onContextMenu={(e) => handleProjectContextMenu(e, p)}
        onMouseDown={(e) => { if (e.button === 0) handleReorderMouseDown(e, p.id, zone, containerSelector) }}
        className={`relative flex items-center gap-3 ${twoColumn ? 'px-3 py-2' : 'px-4 py-2.5'} transition-colors no-drag cursor-pointer group select-none outline-none focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-[var(--color-accent)] active:cursor-grabbing ${
          isSelected
            ? 'bg-[var(--color-accent)]/15'
            : isKeyboardHighlighted ? 'bg-[var(--color-wash-1)]' : 'hover:bg-[var(--color-wash-1)]'
        } ${isDragged ? 'opacity-30' : ''}`}
      >
        {isSelected && (
          <span aria-hidden="true" className="absolute left-0 top-0 bottom-0 w-[3px] bg-[var(--color-accent)]" />
        )}
        <span className={AVATAR_ROUND}>
          <ProjectAvatar
            projectPath={p.path}
            projectName={p.name}
            projectColor={p.color}
            projectId={p.id}
            iconUrl={p.iconUrl}
            size={24}
          />
        </span>
        {twoColumn ? (
          <span className="min-w-0 flex-1">
            <span className="block type-name text-[var(--color-text-primary)] truncate">{p.name}</span>
            {/* Cut from the start: the end of a path is the part that differs. */}
            <span className="block type-hint text-[var(--color-text-secondary)] truncate [direction:rtl] text-left" title={p.path}>
              <bdi>{p.path}</bdi>
            </span>
          </span>
        ) : (
          <>
            <span className="type-name text-[var(--color-text-primary)] truncate min-w-0 flex-shrink-0 max-w-[45%]">{p.name}</span>
            <span className="type-hint text-[var(--color-text-secondary)] truncate min-w-0 flex-1" title={p.path}>{p.path}</span>
          </>
        )}
        <button
          type="button"
          onClick={(e) => {
            e.stopPropagation()
            void togglePinned(p)
          }}
          onMouseDown={(e) => e.stopPropagation()}
          className={`w-6 h-6 flex-shrink-0 flex items-center justify-center transition-colors no-drag cursor-pointer outline-none focus-visible:outline-2 focus-visible:outline-[var(--color-accent)] ${
            p.pinned
              ? 'text-[var(--color-accent)]'
              : 'text-transparent group-hover:text-[var(--color-text-secondary)] group-focus-within:text-[var(--color-text-secondary)] focus-visible:text-[var(--color-text-secondary)] hover:!text-[var(--color-accent)]'
          }`}
          title={p.pinned ? 'Unpin' : 'Pin to top'}
          aria-label={p.pinned ? `Unpin ${p.name}` : `Pin ${p.name} to top`}
          aria-pressed={Boolean(p.pinned)}
        >
          <svg width="12" height="12" viewBox="0 0 16 16" fill="currentColor" aria-hidden="true">
            <path d="M9.828.722a.5.5 0 0 1 .354.146l4.95 4.95a.5.5 0 0 1-.707.707l-.71-.71-3.18 3.18a3.5 3.5 0 0 1-.4.3L11 11.106V14.5a.5.5 0 0 1-.854.354L7.5 12.207 4.854 14.854a.5.5 0 0 1-.708-.708L6.793 11.5 4.146 8.854A.5.5 0 0 1 4.5 8h3.394a3.5 3.5 0 0 0 .3-.4l3.18-3.18-.71-.71a.5.5 0 0 1 .354-.854z" />
          </svg>
        </button>
        {!twoColumn && (
          <svg className="w-3 h-3 flex-shrink-0 text-[var(--color-text-secondary)]" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2} aria-hidden="true">
            <path strokeLinecap="round" strokeLinejoin="round" d="M9 5l7 7-7 7" />
          </svg>
        )}
      </div>
    )
  }

  const addWorkspace = async (): Promise<void> => {
    // Host-aware: local → native OS dialog; remote → in-app
    // RemoteFolderPicker over the host's fs. Single chokepoint
    // (pick-workspace-folder) so every "add workspace" entry point
    // stays host-aware. The chosen path flows through addProject.
    const folderPath = await pickWorkspaceFolder()
    if (folderPath) {
      await useProjectsStore.getState().addProject(folderPath)
      await fetchProjects()
    }
  }

  /** One zone of rows in a card, with the drop line between rows. */
  const renderZone = (rows: typeof projects, zone: string, selector: string): React.JSX.Element => (
    // overflow-hidden: a selected or hovered row keeps to the card's corners.
    <Surface chamfer material="tile" className="overflow-hidden">
      <div data-reorder-zone={zone} className="divide-y divide-[var(--color-border)]">
        {rows.map((p, idx) => (
          <div key={p.id}>
            {reorderZone === zone && reorderDropIndex === idx && <div className="h-[2px] bg-[var(--color-accent)]" />}
            {renderProjectRow(p, zone, selector)}
          </div>
        ))}
        {reorderZone === zone && reorderDropIndex === rows.length && <div className="h-[2px] bg-[var(--color-accent)]" />}
      </div>
    </Surface>
  )

  const searching = searchQuery.trim().length > 0
  const noMatches = projects.length > 0 && allVisibleProjects.length === 0

  const searchField = (
    <Input
      ref={searchInputRef}
      value={searchQuery}
      onChange={(e) => setSearchQuery(e.target.value)}
      onKeyDown={handleSearchKeyDown}
      placeholder="Search workspaces"
      aria-label="Search workspaces"
      className={`${twoColumn ? 'w-full' : 'flex-1'} min-w-0 type-control`}
    />
  )

  const focusGroupsSwitch = (
    <div className="flex items-center gap-2 flex-shrink-0" data-settings-id="projects.focus-groups">
      <span className="type-row text-[var(--color-text-secondary)]">Focus Groups</span>
      <Toggle
        checked={focusGroupsEnabled}
        aria-label="Focus Groups"
        onChange={() => setFocusGroupsEnabled(!focusGroupsEnabled)}
      />
    </div>
  )

  const sortMenu = (
    <OverflowMenu
      label="Sort workspaces"
      items={[
        {
          id: 'sort-groups',
          label: 'Sort focus groups A–Z',
          disabled: !focusGroupsEnabled,
          hint: focusGroupsEnabled ? undefined : 'Turn on Focus Groups first.',
          onSelect: async () => {
            const sorted = [...focusGroups].sort((a, b) => a.name.localeCompare(b.name))
            await reorderFocusGroups(sorted.map((g) => g.id))
          },
        },
        {
          id: 'sort-workspaces',
          label: 'Sort workspaces A–Z',
          onSelect: async () => {
            const sorted = [...projects].sort((a, b) => a.name.localeCompare(b.name))
            await reorderProjects(sorted.map((p) => p.id))
          },
        },
      ]}
    />
  )

  /** The groups: no-match note, Pinned, each focus group, Ungrouped, Add group. */
  const listGroups = (
    <>
      {noMatches && (
        <Surface chamfer material="tile">
          <div className={`px-4 py-5 flex ${twoColumn ? 'flex-col items-start gap-3' : 'items-center justify-between gap-4'}`} data-testid="workspaces-no-match">
            <div className="type-row text-[var(--color-text-primary)] min-w-0 break-words">
              No workspaces match &ldquo;{searchQuery.trim()}&rdquo;
            </div>
            <Button variant="quiet" onClick={() => setSearchQuery('')}>Clear search</Button>
          </div>
        </Surface>
      )}

      {/* Pinned first, then each focus group, then the rest. */}
      {pinnedProjects.length > 0 && (
        <section>
          <ListHeading>Pinned</ListHeading>
          {renderZone(pinnedProjects, 'pinned', "[data-reorder-zone='pinned']")}
        </section>
      )}

      {focusGroupsEnabled ? (
        <>
          <div data-focus-group-reorder-container className="space-y-5">
            {focusGroups.map((group, groupIdx) => {
              const groupProjects = projects.filter((p) => p.focusGroupId === group.id && !p.pinned && matchesSearch(p))
              const isCollapsed = collapsedGroups.has(group.id)
              const isDragOver = dragOverGroupId === group.id
              const zoneId = `group:${group.id}`
              const isGroupDragged = groupDragId === group.id
              const showGroupDropBefore = groupDropIndex === groupIdx
              const showGroupDropAfter = groupDropIndex === focusGroups.length && groupIdx === focusGroups.length - 1

              // Hide empty focus groups when searching
              if (searching && groupProjects.length === 0) return null

              return (
                <section key={group.id} className={isGroupDragged ? 'opacity-30' : ''} data-focus-group-reorder-id={group.id}>
                  {showGroupDropBefore && <div className="h-[2px] bg-[var(--color-accent)] mb-1" />}
                  <div
                    data-focus-group-id={group.id}
                    className={`flex items-center gap-2 h-7 mb-1 cursor-pointer no-drag select-none transition-colors ${
                      isDragOver ? 'bg-[var(--color-accent)]/15 ring-1 ring-inset ring-[var(--color-accent)]' : ''
                    }`}
                    onClick={() => { if (renamingGroupId !== group.id) toggleGroupCollapse(group.id) }}
                    onMouseDown={(e) => handleGroupReorderMouseDown(e, group.id)}
                    onContextMenu={(e) => handleGroupContextMenu(e, group.id)}
                  >
                    <svg
                      className={`w-3 h-3 text-[var(--color-text-secondary)] transition-transform flex-shrink-0 ${isCollapsed ? '' : 'rotate-90'}`}
                      fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2} aria-hidden="true"
                    >
                      <path strokeLinecap="round" strokeLinejoin="round" d="M9 5l7 7-7 7" />
                    </svg>
                    {group.color && (
                      <span className="w-2 h-2 flex-shrink-0 rounded-[var(--radius-badge)]" style={{ backgroundColor: group.color }} aria-hidden="true" />
                    )}
                    {renamingGroupId === group.id ? (
                      <Input
                        ref={renameGroupInputRef}
                        value={renamingGroupName}
                        aria-label="Group name"
                        onChange={(e) => setRenamingGroupName(e.target.value)}
                        onKeyDown={(e) => {
                          if (e.key === 'Enter') handleGroupRenameConfirm()
                          else if (e.key === 'Escape') { setRenamingGroupId(null); setRenamingGroupName('') }
                        }}
                        onBlur={handleGroupRenameConfirm}
                        onClick={(e) => e.stopPropagation()}
                        className="flex-1 min-w-0 type-control"
                      />
                    ) : (
                      <span className="type-heading text-[var(--color-text-primary)] flex-1 min-w-0 truncate">{group.name}</span>
                    )}
                    {isDragOver ? (
                      <span className="type-hint text-[var(--color-accent)] flex-shrink-0">Drop here</span>
                    ) : (
                      <span className="type-hint text-[var(--color-text-secondary)] tabular-nums flex-shrink-0">
                        {groupProjects.length}
                      </span>
                    )}
                    <div onClick={(e) => e.stopPropagation()} onMouseDown={(e) => e.stopPropagation()}>
                      <OverflowMenu
                        label={`More actions for ${group.name}`}
                        items={[
                          {
                            id: 'rename',
                            label: 'Rename',
                            onSelect: () => {
                              setRenamingGroupId(group.id)
                              setRenamingGroupName(group.name)
                              requestAnimationFrame(() => renameGroupInputRef.current?.focus())
                            },
                          },
                          {
                            id: 'delete',
                            label: 'Delete group',
                            danger: true,
                            group: true,
                            hint: 'Its workspaces move to Ungrouped.',
                            onSelect: () => {
                              void confirmDeleteGroup(group)
                            },
                          },
                        ]}
                      />
                    </div>
                  </div>
                  {!isCollapsed && (
                    groupProjects.length > 0 ? (
                      renderZone(groupProjects, zoneId, `[data-reorder-zone='${zoneId}']`)
                    ) : (
                      <Surface chamfer material="tile">
                        <div className="px-4 py-3 type-hint text-[var(--color-text-secondary)]">Drop workspaces here</div>
                      </Surface>
                    )
                  )}
                  {showGroupDropAfter && <div className="h-[2px] bg-[var(--color-accent)] mt-1" />}
                </section>
              )
            })}
          </div>

          {ungroupedProjects.length > 0 && (
            <section>
              <ListHeading
                dataFocusGroupId="__ungrouped__"
                highlight={dragOverGroupId === '__ungrouped__'}
              >
                Ungrouped
              </ListHeading>
              {renderZone(ungroupedProjects, 'ungrouped', "[data-reorder-zone='ungrouped']")}
            </section>
          )}

          <div className="flex items-center gap-2">
            <Input
              value={newGroupName}
              onChange={(e) => setNewGroupName(e.target.value)}
              onKeyDown={(e) => { if (e.key === 'Enter') handleCreateGroup() }}
              placeholder="New focus group name"
              aria-label="New focus group name"
              className={`${twoColumn ? 'flex-1 min-w-0' : SETTING_FIELD_WIDTH} type-control`}
            />
            <Button variant="quiet" className="flex-shrink-0" disabled={!newGroupName.trim()} onClick={() => void handleCreateGroup()}>
              Add group
            </Button>
          </div>
        </>
      ) : (
        projects.some((p) => !p.pinned && matchesSearch(p)) && (
          <section>
            <ListHeading>Workspaces</ListHeading>
            {renderZone(projects.filter((p) => !p.pinned && matchesSearch(p)), 'flat', "[data-reorder-zone='flat']")}
          </section>
        )
      )}
    </>
  )

  // ── Side by side: the list column, and the selected workspace ──────────
  if (twoColumn) {
    return (
      <div ref={rootRef} className="h-full min-h-0 flex" data-workspaces-layout="two-column">
        <aside
          aria-label="Workspaces"
          data-settings-id="projects.list"
          className="w-72 flex-shrink-0 min-h-0 flex flex-col border-r border-[var(--color-border)]"
        >
          <div className="flex-shrink-0 px-4 pt-6 pb-4 space-y-3">
            {/* Settings for every workspace: selectable like a workspace. */}
            <button
              type="button"
              data-testid="workspace-settings-entry"
              aria-current={showWorkspaceSettings ? 'true' : undefined}
              onClick={() => setShowWorkspaceSettings(true)}
              data-k2-chamfer="control"
              className={`relative w-full flex items-center gap-3 px-3 py-2 rounded-[var(--radius-selector)] text-left type-control no-drag cursor-pointer transition-colors outline-none focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-[var(--color-accent)] ${
                showWorkspaceSettings
                  ? 'bg-[var(--color-accent)]/15 text-[var(--color-text-primary)]'
                  : 'text-[var(--color-text-secondary)] hover:text-[var(--color-text-primary)] hover:bg-[var(--color-wash-1)]'
              }`}
            >
              <svg className="w-4 h-4 flex-shrink-0" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={1.75} aria-hidden="true">
                <path strokeLinecap="round" strokeLinejoin="round" d="M10.325 4.317c.426-1.756 2.924-1.756 3.35 0a1.724 1.724 0 002.573 1.066c1.543-.94 3.31.826 2.37 2.37a1.724 1.724 0 001.065 2.572c1.756.426 1.756 2.924 0 3.35a1.724 1.724 0 00-1.066 2.573c.94 1.543-.826 3.31-2.37 2.37a1.724 1.724 0 00-2.572 1.065c-.426 1.756-2.924 1.756-3.35 0a1.724 1.724 0 00-2.573-1.066c-1.543.94-3.31-.826-2.37-2.37a1.724 1.724 0 00-1.065-2.572c-1.756-.426-1.756-2.924 0-3.35a1.724 1.724 0 001.066-2.573c-.94-1.543.826-3.31 2.37-2.37.996.608 2.296.07 2.572-1.065z" />
                <path strokeLinecap="round" strokeLinejoin="round" d="M15 12a3 3 0 11-6 0 3 3 0 016 0z" />
              </svg>
              <span className="flex-1 truncate">Workspace settings</span>
            </button>
            {searchField}
            <div className="flex items-center justify-between gap-2">
              {focusGroupsSwitch}
              {sortMenu}
            </div>
          </div>
          <div className="flex-1 min-h-0 overflow-y-auto px-4 pb-4 space-y-5 [scrollbar-gutter:stable]">
            {listGroups}
          </div>
          <div className="flex-shrink-0 px-4 py-3 border-t border-[var(--color-border)]">
            <Button variant="quiet" className="w-full justify-center" data-settings-id="projects.add" onClick={() => void addWorkspace()}>
              New workspace
            </Button>
          </div>
        </aside>

        {showWorkspaceSettings ? (
          <div className="flex-1 min-w-0 min-h-0 overflow-y-auto p-6 [scrollbar-gutter:stable]">
            <div className="w-full max-w-[52rem]">
              <WorkspaceSettingsPage />
            </div>
          </div>
        ) : shownProject ? (
          // overflow-hidden: only the tab body scrolls. `relative` holds the
          // editors that take over this column.
          <div className="flex-1 min-w-0 min-h-0 overflow-hidden p-6 relative flex flex-col">
            <ProjectDetail
              project={shownProject}
              projects={projects}
              focusGroups={focusGroups}
              focusGroupsEnabled={focusGroupsEnabled}
              assignProjectToGroup={assignProjectToGroup}
              fetchProjects={fetchProjects}
              onSwitchProject={selectProject}
              lastTabRef={lastTabRef}
            />
          </div>
        ) : null}
      </div>
    )
  }

  // ── One page at a time (narrow, or nothing to list yet) ───────────────
  if (showWorkspaceSettings) {
    return (
      <div ref={rootRef} className="h-full min-h-0 overflow-y-auto p-6 [scrollbar-gutter:stable]" data-workspaces-layout="single">
        <div className="w-full max-w-[52rem]">
          <BackToWorkspaces onBack={backToList} />
          <WorkspaceSettingsPage />
        </div>
      </div>
    )
  }

  if (selectedProject) {
    return (
      <div ref={rootRef} className="h-full min-h-0 overflow-hidden p-6 relative flex flex-col" data-workspaces-layout="single">
        <ProjectDetail
          project={selectedProject}
          projects={projects}
          focusGroups={focusGroups}
          focusGroupsEnabled={focusGroupsEnabled}
          assignProjectToGroup={assignProjectToGroup}
          fetchProjects={fetchProjects}
          onBack={backToList}
          onSwitchProject={selectProject}
          lastTabRef={lastTabRef}
        />
      </div>
    )
  }

  const listError = typeof listState === 'object' ? listState.error : null
  const nothingYet = projects.length === 0

  return (
    <div ref={rootRef} className="h-full min-h-0 overflow-y-auto p-6 [scrollbar-gutter:stable]" data-settings-id="projects.list" data-workspaces-layout="single">
      <div className="w-full max-w-[52rem] space-y-6 pb-6">
        <header className="flex items-start justify-between gap-4">
          <div className="min-w-0">
            <h2 className="type-title text-[var(--color-text-primary)]">Workspaces / Agents</h2>
            <p className="type-hint text-[var(--color-text-secondary)] mt-1">
              Pick a workspace to change how its agent is set up.
            </p>
          </div>
          <div className="flex items-center gap-2 flex-shrink-0">
            <Button
              variant="quiet"
              data-testid="workspace-settings-entry"
              onClick={() => setShowWorkspaceSettings(true)}
            >
              Workspace settings
            </Button>
            {/* The empty state has its own; one primary action per screen. */}
            {!(nothingYet && listState === 'ready') && (
              <Button variant="accent" data-settings-id="projects.add" onClick={() => void addWorkspace()}>
                New workspace
              </Button>
            )}
          </div>
        </header>

        {nothingYet ? (
          <Surface chamfer material="tile">
            {listError ? (
              <div className="px-4 py-5 flex items-center justify-between gap-4" data-testid="workspaces-load-error">
                <div className="min-w-0">
                  <div className="type-row text-[var(--color-text-primary)]">Couldn&rsquo;t load your workspaces</div>
                  <div className={SETTING_ROW_HINT} title={listError}>
                    K2 couldn&rsquo;t read the workspace list from this server, so nothing is shown here. Try again in a moment. If it keeps failing, restart K2.
                  </div>
                </div>
                <Button variant="quiet" onClick={() => { void fetchProjects(); void checkList() }}>Try again</Button>
              </div>
            ) : listState === 'loading' ? (
              <div className="px-4 py-5 type-row text-[var(--color-text-secondary)]" data-testid="workspaces-loading">
                Loading your workspaces…
              </div>
            ) : (
              <div className="px-4 py-5 flex items-center justify-between gap-4" data-testid="workspaces-empty">
                <div className="min-w-0">
                  <div className="type-row text-[var(--color-text-primary)]">No workspaces yet</div>
                  <div className={SETTING_ROW_HINT}>Add a folder to give it an agent you can set up here.</div>
                </div>
                <Button variant="accent" data-settings-id="projects.add" onClick={() => void addWorkspace()}>New workspace</Button>
              </div>
            )}
          </Surface>
        ) : (
          <>
            {/* Find it first. */}
            <div className="flex items-center gap-3">
              {searchField}
              {focusGroupsSwitch}
              {sortMenu}
            </div>
            {listGroups}
          </>
        )}
      </div>
    </div>
  )
}

function ListHeading({
  children,
  dataFocusGroupId,
  highlight = false,
}: {
  children: React.ReactNode
  dataFocusGroupId?: string
  highlight?: boolean
}): React.JSX.Element {
  return (
    <div
      data-focus-group-id={dataFocusGroupId}
      className={`flex items-center h-7 mb-1 type-heading transition-colors ${
        highlight ? 'text-[var(--color-accent)] bg-[var(--color-accent)]/15 ring-1 ring-inset ring-[var(--color-accent)]' : 'text-[var(--color-text-primary)]'
      }`}
    >
      {children}
    </div>
  )
}

function BackToWorkspaces({ onBack }: { onBack: () => void }): React.JSX.Element {
  return (
    <div className="mb-3">
      <Button variant="quiet" onClick={onBack} data-testid="workspaces-back">
        &larr; All workspaces
      </Button>
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
    daemonCliGet<any[]>(primaryScope(), 'git/worktrees', { path: project.path })
      .then((wts) => {
        if (!cancelled) {
          setDiskWorktrees(asArray(wts))
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
      await daemonCliPost(primaryScope(), 'git/reopen-worktree', {
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
      <SettingsGroup title="Worktree folders on disk" card>
        <p className="px-4 py-3 type-row text-[var(--color-text-secondary)]">Loading…</p>
      </SettingsGroup>
    )
  }

  // Filter out bare worktrees
  const nonBare = diskWorktrees.filter((wt) => !wt.isBare)
  if (nonBare.length === 0) return <></>

  return (
    <SettingsGroup
      title="Worktree folders on disk"
      card
      badge={<span className="type-badge tabular-nums text-[var(--color-text-secondary)]">{nonBare.length}</span>}
    >
      {nonBare.map((wt) => {
        const isActive = wt.isMain
          ? mainWorkspaceExists
          : activeWorktreePaths.has(wt.path)

        return (
          <div key={wt.path} className="flex items-center gap-3 px-4 py-2.5">
            <div className="flex-1 min-w-0">
              <span className="type-row text-[var(--color-text-primary)] truncate block">{wt.branch}</span>
              <span className="type-hint text-[var(--color-text-secondary)] truncate block" title={wt.path}>
                {wt.path.length > 50 ? '...' + wt.path.slice(-47) : wt.path}
              </span>
            </div>
            {isActive ? (
              <span className="type-hint text-[var(--color-status-ok-soft)] flex-shrink-0">(active)</span>
            ) : (
              <Button variant="quiet" className="flex-shrink-0" onClick={() => handleReopen(wt)} disabled={reopening === wt.path}>
                {reopening === wt.path ? 'Reopening...' : 'Reopen'}
              </Button>
            )}
          </div>
        )
      })}
    </SettingsGroup>
  )
}

// ── Workspace Detail (right panel content) ─────────────────────────────
function ProjectDetail({
  project,
  projects,
  focusGroups,
  focusGroupsEnabled,
  assignProjectToGroup,
  fetchProjects,
  onBack,
  onSwitchProject,
  lastTabRef,
}: {
  project: ReturnType<typeof useProjectsStore.getState>['projects'][number]
  projects: ReturnType<typeof useProjectsStore.getState>['projects']
  /** One page at a time: back to the list. Side by side there is none. */
  onBack?: () => void
  onSwitchProject: (id: string) => void
  lastTabRef: React.MutableRefObject<WorkspaceSettingsTab>
  focusGroups: ReturnType<typeof useFocusGroupsStore.getState>['focusGroups']
  focusGroupsEnabled: boolean
  assignProjectToGroup: (projectId: string, groupId: string | null) => Promise<void>
  fetchProjects: () => Promise<void>
}): React.JSX.Element {
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
  // Consume the deep-link on first paint so manage → Heartbeats does not
  // flash Agent, and so a remount still sees the requested tab.
  const initialWorkspaceTab = useSettingsStore((s) => s.initialWorkspaceTab)
  const [settingsTab, setSettingsTab] = useState<WorkspaceSettingsTab>(
    () => parseWorkspaceSettingsTab(useSettingsStore.getState().initialWorkspaceTab) ?? lastTabRef.current,
  )
  useEffect(() => {
    const parsed = parseWorkspaceSettingsTab(initialWorkspaceTab)
    if (parsed) setSettingsTab(parsed)
    // A link to the old Worktrees / Import / API tab: land on that section.
    const section = advancedSectionFor(initialWorkspaceTab)
    if (section) {
      requestAnimationFrame(() => {
        document.querySelector(`[data-settings-id="${section}"]`)?.scrollIntoView({ block: 'start' })
      })
    }
  }, [initialWorkspaceTab])
  useEffect(() => {
    lastTabRef.current = settingsTab
  }, [settingsTab, lastTabRef])
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
    daemonCliPost<HarnessProbe[]>(primaryScope(), 'canonical/detect-state', { project_path: project.path })
      .then((p) => { if (!cancelled) setCanonicalProbes(asArray<HarnessProbe>(p)) })
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
    agentDisplayName(primaryScope(), project.path)
      .then((n) => { if (!cancelled) setPrimaryAgentName(n) })
      .catch((err) => {
        if (!cancelled) {
          console.error('[primary-agent-name] resolve failed:', err)
          setPrimaryAgentName('')
        }
      })
    // Agent rename live update: a rename anywhere repaints this page.
    const offIdentity = onAgentIdentityChanged(primaryScope(), (e) => {
      if (cancelled || !agentIdentityMatchesPath(e, project.path)) return
      const n = agentIdentityDisplayName(e)
      if (n) setPrimaryAgentName(n)
    })
    return () => { cancelled = true; offIdentity() }
  }, [project.path, project.agentMode])


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

  const tabLabel: Record<WorkspaceSettingsTab, string> = {
    agent: 'Agent',
    permissions: 'Permissions',
    context: 'Context',
    connections: 'Connections',
    skills: 'Skills',
    schedule: 'Heartbeats',
    mail: 'Mail',
    advanced: 'Advanced',
  }
  // Header + tab strip span the settings column. Forms keep the shared
  // reading width; Context and Heartbeats fill the height.
  const fillsHeight = settingsTab === 'context' || settingsTab === 'schedule'

  const openWorkspace = (): void => {
    const defaultWs = project.workspaces?.[0]
    if (defaultWs) {
      useProjectsStore.getState().setActiveWorkspace(project.id, defaultWs.id)
    }
    useSettingsStore.getState().closeSettings()
  }

  const workspaceActions = (
    <div className="flex items-center gap-2 flex-shrink-0">
      <SettingDropdown
        value={project.id}
        ariaLabel="Switch workspace"
        triggerLabel="Switch workspace"
        options={projects.map((p) => ({ value: p.id, label: p.name }))}
        onChange={(id) => onSwitchProject(id)}
      />
      <Button variant="quiet" onClick={openWorkspace}>
        Open workspace
      </Button>
    </div>
  )

  return (
    <div className="flex flex-col h-full min-h-0 w-full">
      <div className={`flex-shrink-0 w-full space-y-4 ${fillsHeight ? '' : 'max-w-[52rem]'}`}>
        {onBack && (
          <div className="flex items-center justify-between gap-3">
            <Button variant="quiet" onClick={onBack} data-testid="workspaces-back">
              &larr; All workspaces
            </Button>
            {workspaceActions}
          </div>
        )}

        <div className="flex items-center gap-3 min-w-0">
          <span className={AVATAR_ROUND}>
            <ProjectAvatar
              projectPath={project.path}
              projectName={project.name}
              projectColor={project.color}
              projectId={project.id}
              iconUrl={project.iconUrl}
              size={36}
            />
          </span>
          <div className="min-w-0 flex-1">
            <h2 className="type-title text-[var(--color-text-primary)] truncate">{project.name}</h2>
            <p className="type-hint text-[var(--color-text-secondary)] mt-0.5 truncate" title={project.path}>{project.path}</p>
          </div>
          {/* Side by side the actions share the name's line. */}
          {!onBack && workspaceActions}
        </div>

        <div
          role="tablist"
          aria-label="Workspace settings"
          className="flex flex-wrap gap-0.5 border-b border-[var(--color-border)] w-full"
        >
          {WORKSPACE_SETTINGS_TABS.map((id) => {
            const active = settingsTab === id
            return (
              <button
                key={id}
                role="tab"
                type="button"
                aria-selected={active}
                onClick={() => selectSettingsTab(id)}
                className={`px-3 py-2 type-control transition-colors no-drag cursor-pointer border-b-2 -mb-px outline-none focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-[var(--color-accent)] ${
                  active
                    ? 'border-[var(--color-accent)] text-[var(--color-text-primary)]'
                    : 'border-transparent text-[var(--color-text-secondary)] hover:text-[var(--color-text-primary)]'
                }`}
              >
                {tabLabel[id]}
              </button>
            )
          })}
        </div>
      </div>

      <div
        className={
          fillsHeight
            ? 'flex-1 min-h-0 overflow-hidden flex flex-col w-full pt-3 pb-3'
            : 'flex-1 overflow-y-auto min-h-0 pt-5 pb-8 w-full [scrollbar-gutter:stable]'
        }
      >
        {!fillsHeight && (
          <div className="w-full max-w-[52rem] space-y-6">
            {settingsTab === 'agent' && (
              <>
                <SettingsGroup title="Identity" card>
                  {/* The shared identity controls (`components/AgentIdentity`),
                      the same ones the room's Agent drawer tab shows. */}
                  <AgentPictureControls scope={primaryScope()} project={project} card />
                  <AgentDisplayNameField
                    scope={primaryScope()}
                    projectPath={project.path}
                    helpText="The name K2 shows for this agent. Doesn't change the handle or address."
                    card
                  />
                  <AgentHandleField scope={primaryScope()} projectPath={project.path} projectHandle={project.handle} card />
                  <AgentColorPicker project={project} card />
                </SettingsGroup>

                <SettingsGroup title="Model" card>
                  <DefaultAgentSelector projectId={project.id} currentDefaultAgent={project.defaultAgent} />
                  <DefaultModelControls project={project} />
                </SettingsGroup>

                <SettingsGroup title="LLM tokens" card>
                  <WorkspaceLlmTokens scope={primaryScope()} projectId={project.id} />
                </SettingsGroup>

                <SettingsGroup title="Notifications" card>
                  <WorkspaceCompletionSoundToggle project={project} />
                  <WorkspacePhonePushToggle project={project} />
                </SettingsGroup>

                <WorkspaceTabsSettings project={project} scope={primaryScope()} />

                <SettingsGroup title="Workspace" card>
                  {focusGroupsEnabled && (
                    <SettingRow label="Focus group" field>
                      <SettingDropdown
                        fullWidth
                        value={project.focusGroupId ?? ''}
                        options={[
                          { value: '', label: 'No group' },
                          ...focusGroups.map((g) => ({ value: g.id, label: g.name })),
                        ]}
                        onChange={async (v) => {
                          await assignProjectToGroup(project.id, v || null)
                          await fetchProjects()
                        }}
                      />
                    </SettingRow>
                  )}
                  <FolderTrustRow scope={primaryScope()} projectPath={project.path} />
                </SettingsGroup>

                <SettingsGroup title="Remove" card>
                  <SettingRow
                    label="Remove workspace"
                    hint="Asks what to do with its files and Apps first."
                  >
                    {/* 0.46.7: the same dialog as the sidebar's Remove (mode,
                        then a workspace's Apps and what they'd lose), never
                        a silent failure. */}
                    <Button
                      variant="quiet"
                      onClick={() =>
                        useRemoveWorkspaceDialogStore.getState().open({
                          projectId: project.id,
                          projectName: project.name,
                          projectPath: project.path,
                        })
                      }
                    >
                      Remove workspace
                    </Button>
                  </SettingRow>
                </SettingsGroup>
              </>
            )}

            {settingsTab === 'permissions' && (
              <PermissionAsksProvider project={project}>
                <SettingsGroup title="Agents and secrets" card>
                  <AgentsManageAgentsToggle project={project} />
                  <AgentsManageVaultToggle project={project} />
                  <AgentHandoffAcceptFrom project={project} />
                  <AgentPermissionCardsRow project={project} />
                  <AgentsRefreshChatToggle project={project} />
                </SettingsGroup>
                <SettingsGroup title="Compute" card>
                  <AgentsUseComputeToggle project={project} />
                  <AgentsOperateComputeToggle project={project} />
                  <AgentsManageComputeToggle project={project} />
                </SettingsGroup>
                <SettingsGroup title="Database" card>
                  <DbAgentCreateToggle project={project} />
                  <AgentsManagePostgresToggle project={project} />
                </SettingsGroup>
                <SettingsGroup title="Projects and Gardens" card>
                  <AgentsManageProjectsToggle project={project} />
                  <AgentsManageGardensToggle project={project} />
                </SettingsGroup>
                <SettingsGroup title="Email sending" card>
                  <MailAgentSendControl project={project} />
                  {/* Agent email notices (N10): hides itself for a Member and on a
                      server without them. */}
                  <AgentEmailNoticesRow scope={primaryScope()} projectId={project.id} />
                </SettingsGroup>
                <SettingsGroup title="Hosted mail" card>
                  <MailManageToggle project={project} />
                </SettingsGroup>
                <SettingsGroup title="Apps" card>
                  <div className="px-4 pt-3 empty:hidden"><AskedChip row="projects.agents-can-manage-skin" /></div>
                  <AgentAppsAccess project={project} />
                </SettingsGroup>
                <SettingsGroup title="Agents on other servers" card>
                  <RemoteInstructToggle project={project} fetchProjects={fetchProjects} />
                </SettingsGroup>
                <SettingsGroup title="DNS" card>
                  <DnsManageToggle project={project} fetchProjects={fetchProjects} />
                </SettingsGroup>
              </PermissionAsksProvider>
            )}

            {settingsTab === 'connections' && (
              <PermissionAsksProvider project={project}>
                <SettingsGroup title="Connection rules" card settingId="projects.relations">
                  <AgentsCreateConnectionsToggle project={project} fetchProjects={fetchProjects} />
                </SettingsGroup>
                <ConnectedWorkspacesPanel projectId={project.id} />
              </PermissionAsksProvider>
            )}

            {settingsTab === 'skills' && (
              <ProjectSkillsPanel projectPath={project.path} onOpenEditor={(name) => { setAgentEditorName(name); setAgentEditorOpen(true) }} />
            )}

            {settingsTab === 'mail' && <MailWatchPanel projectPath={project.path} />}

            {settingsTab === 'advanced' && (
              <>
                <SettingsGroup title="Worktrees" card settingId="projects.worktrees">
                  {project.workspaces.map((ws) => (
                    <div key={ws.id} className="flex items-center gap-3 px-4 py-2.5">
                      <svg className="w-3 h-3 flex-shrink-0 text-[var(--color-text-secondary)]" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2} aria-hidden="true">
                        <path strokeLinecap="round" strokeLinejoin="round" d="M13 10V3L4 14h7v7l9-11h-7z" />
                      </svg>
                      <span className="type-row text-[var(--color-text-primary)] flex-1 truncate">{ws.name}</span>
                      {ws.branch && (
                        <span className="type-hint text-[var(--color-text-secondary)] truncate max-w-[12rem]">{ws.branch}</span>
                      )}
                      <span className="type-hint text-[var(--color-text-secondary)]">{ws.type}</span>
                    </div>
                  ))}
                  {project.workspaces.length === 0 && (
                    <p className="px-4 py-3 type-row text-[var(--color-text-secondary)]">No registered worktrees for this workspace.</p>
                  )}
                </SettingsGroup>
                <WorktreeFoldersOnDisk project={project} fetchProjects={fetchProjects} />

                <div data-settings-id="projects.cursor-migrate">
                  <SettingsGroup title="Chat migrations" card>
                    <CursorMigrationPanel projectPath={project.path} />
                  </SettingsGroup>
                </div>

                <SettingsGroup title="API" card>
                  <PrimaryRoom>
                    <HideApiSessionsToggle project={project} />
                  </PrimaryRoom>
                  <WorkspaceHostSessionCapPanel projectPath={project.path} />
                </SettingsGroup>
                <WorkspaceApiKeysPanel workspaceSlug={workspaceGrantSlug(project)} />
              </>
            )}
          </div>
        )}

        {settingsTab === 'context' && (
          /* Full tab body: left stack + Canonical Agent | right FileViewer */
          <div className="h-full min-h-0 w-full" data-settings-id="projects.context-stack">
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

        {settingsTab === 'schedule' && (
          <div className="flex flex-col h-full min-h-0" data-settings-id="projects.heartbeat">
            {/* Left: heartbeats roster · Right: run history — independent scroll */}
            <div className="flex-1 min-h-0 flex flex-row border border-[var(--color-border)]">
              <div className="flex-1 min-w-0 min-h-0 flex flex-col border-r border-[var(--color-border)]">
                <div className="flex-1 min-h-0 overflow-y-auto p-3 [scrollbar-gutter:stable]">
                  <div className="pb-3">
                    <RemindersList scope={primaryScope()} projectPath={project.path} manage />
                  </div>
                  <PrimaryRoom>
                    <HeartbeatsPanel
                      key={`hb-${hbRefreshNonce}`}
                      projectPath={project.path}
                      agentMode={project.agentMode || 'custom'}
                      agentName={primaryAgentName
                        || project.name.toLowerCase().replace(/\s+/g, '-')}
                      onConfigureWakeup={(row) => setWakeupEditingHb(row)}
                    />
                  </PrimaryRoom>
                </div>
                <div className="flex-shrink-0 border-t border-[var(--color-border)]">
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
      </div>
    </div>
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
      const s = await daemonCliGet<Record<string, unknown>>(primaryScope(), 'settings', { project: projectPath })
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
      await daemonCliPost(primaryScope(), 'workspace/set', {
        project: projectPath,
        fields: { host_session_cell_cap: value },
      })
      setHint(value === 'default' ? 'Using the server default.' : `Set to ${value}.`)
      await load()
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    } finally {
      setBusy(false)
    }
  }

  return (
    <SettingRow
      settingId="projects.host-session-cell-cap"
      label="Concurrent API sessions"
      hint={
        <>
          How many API sessions can run at once in this workspace, from 1 to 512. Default 15, unless this
          server sets its own. Now:{' '}
          {raw === undefined ? '…' : raw === null ? 'server default' : `${raw} at once`}.
        </>
      }
      below={
        error ? (
          <p className="type-hint text-[var(--color-status-error-soft)]">{error}</p>
        ) : hint ? (
          <p className="type-hint text-[var(--color-status-ok-soft)]">{hint}</p>
        ) : undefined
      }
    >
      <Input
        type="number"
        min={1}
        max={512}
        placeholder="Default (15)"
        value={draft}
        disabled={busy || raw === undefined}
        onChange={(e) => setDraft(e.target.value)}
        aria-label="Concurrent API sessions"
        className="w-28 type-control"
      />
      <Button variant="quiet" disabled={busy || raw === undefined || !draft.trim()} onClick={() => void save(draft.trim())}>
        Save
      </Button>
      <Button variant="quiet" disabled={busy || raw === undefined || raw === null} onClick={() => void save('default')}>
        Use default
      </Button>
    </SettingRow>
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
      await daemonCliPost(primaryScope(), 'heartbeat/set-show-sessions', {
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
      <div className="type-hint text-[var(--color-text-muted)]">Loading…</div>
    )
  }

  return (
    <SettingSwitchRow
      control={
        <Toggle
          checked={enabled}
          onChange={toggle}
          disabled={busy}
          title={enabled ? 'Heartbeats open background tabs' : 'Heartbeats run in the background'}
        />
      }
    >
          <div className={SETTING_ROW_LABEL}>
            Show heartbeat sessions in tabs
          </div>
          <div className="type-hint text-[var(--color-text-secondary)] mt-0.5 leading-relaxed">
            {enabled
              ? 'Each heartbeat opens a background tab in this window until you close it, so you can follow the agent\'s work.'
              : 'Heartbeats run in the background (recommended). See them any time in the sidebar Heartbeats panel.'}
          </div>
    </SettingSwitchRow>
  )
}

// ── Agent permission rows ──────────────────────────────────────────
// The per-agent permission switches below render in two places: here
// (Settings → Workspaces → Agent, a bordered box with the full
// description) and in the room's Agent drawer tab (`compact`: the label
// and the switch only, so they fit a narrow drawer). One component each,
// one writer each; the drawer never forks its own form.
function permissionDescClass(compact: boolean): string {
  return compact ? 'hidden' : SETTING_ROW_HINT
}

// ── DNS K1 Per-workspace DNS-manage opt-in ─────────────────────────
// Lets agents create/update/delete DNS records for THIS workspace.
// DEFAULTS OFF / fail-closed. The daemon ENFORCES this server-side via
// `dns_manage_allowed_for_path` (app master OR per-workspace). Reads
// from the projects store (`dnsManageEnabled`); writes via /cli/dns-manage.
export function DnsManageToggle({
  project,
  compact = false,
  readOnly = false,
  fetchProjects,
}: {
  project: ProjectWithWorkspaces
  /** Drawer Agent tab: the label and switch only (no description box). */
  compact?: boolean
  /** Shown but not flippable: a room on another server, or a role below the route floor. */
  readOnly?: boolean
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
    if (busy || readOnly) return
    const next = !enabled
    setBusy(true)
    try {
      await daemonCliPostQuery(primaryScope(), 'dns-manage', {
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
    <SettingSwitchRow
      compact={compact}
      control={
        <Toggle
          size={compact ? 'sm' : 'md'}
          checked={enabled}
          onChange={toggle}
          disabled={busy || readOnly}
          data-settings-id="projects.dns-manage-enabled"
          title={enabled ? 'Agents may manage DNS for this workspace' : 'Agents cannot manage DNS for this workspace'}
        />
      }
    >
          <div className={SETTING_ROW_LABEL}>
            Allow agents to manage DNS for this workspace
            <AskedChip row="projects.dns-manage-enabled" />
          </div>
          <div className={permissionDescClass(compact)}>
            {enabled
              ? 'Agents in this workspace may create, update and delete DNS records.'
              : 'Off (recommended): agents in this workspace can\'t change DNS.'}
          </div>
          {!enabled && globalAllow && !readOnly && (
            <div className="type-hint text-[var(--color-status-warn-text)] mt-1 leading-relaxed">
              Currently allowed anyway: the global &ldquo;Allow agents to manage DNS
              records&rdquo; (Settings → Tunnel) is on for every workspace.
            </div>
          )}
    </SettingSwitchRow>
  )
}

// k2 sidecar v1 — "Allow hiring and managing agents". DEFAULTS OFF.
// Lets this workspace's main agent open and stop sidecars (later also
// hire and retire agents). Writes POST /cli/agent-access/set
// `{workspace, toggle: 'agents_manage', value}` (Owner/Admin). Optimistic
// store patch; do not fetchProjects() on this path.
export function AgentsManageAgentsToggle({
  project,
  compact = false,
  readOnly = false,
}: {
  project: ProjectWithWorkspaces
  /** Drawer Agent tab: the label and switch only (no description box). */
  compact?: boolean
  /** Shown but not flippable: a room on another server, or a role below the route floor. */
  readOnly?: boolean
}): React.JSX.Element {
  const [busy, setBusy] = useState(false)
  const enabled = (project.agentsCanManageAgents ?? 0) === 1

  const toggle = async (): Promise<void> => {
    if (busy || readOnly) return
    const next = !enabled
    const before = project.agentsCanManageAgents
    setBusy(true)
    useProjectsStore.setState((s) => ({
      projects: s.projects.map((p) =>
        p.id === project.id ? { ...p, agentsCanManageAgents: next ? 1 : 0 } : p,
      ),
    }))
    try {
      await daemonCliPost(primaryScope(), 'agent-access/set', {
        workspace: project.path,
        toggle: 'agents_manage',
        value: next,
      })
      noteOptimisticProjectsMutationSuccess()
      emitProjectsChanged()
    } catch (err) {
      useProjectsStore.setState((s) => ({
        projects: s.projects.map((p) =>
          p.id === project.id ? { ...p, agentsCanManageAgents: before } : p,
        ),
      }))
      console.error('[agent-access] write failed', err)
    } finally {
      setBusy(false)
    }
  }

  return (
    <SettingSwitchRow
      compact={compact}
      control={
        <Toggle
          size={compact ? 'sm' : 'md'}
          checked={enabled}
          onChange={toggle}
          disabled={busy || readOnly}
          data-settings-id="projects.agents-can-manage-agents"
          title={
            enabled
              ? 'This workspace\'s main agent may open and stop sidecars'
              : 'This workspace\'s agent cannot open or stop sidecars'
          }
        />
      }
    >
          <div className={SETTING_ROW_LABEL}>
            Allow hiring and managing agents
            <AskedChip row="projects.agents-can-manage-agents" />
          </div>
          <div className={permissionDescClass(compact)}>
            Lets this workspace&rsquo;s main agent open and stop sidecars. Sidecars themselves can&rsquo;t.
          </div>
    </SettingSwitchRow>
  )
}

// K2 vault (prd-k2-vault-v1 §16.1, KV39) — "Allow managing server-wide
// secrets". DEFAULTS OFF. Same single writer as the row above:
// POST /cli/agent-access/set `{workspace, toggle: 'vault_manage', value}`
// (Owner/Admin). Optimistic store patch; no projects refetch here.
export function AgentsManageVaultToggle({
  project,
  compact = false,
  readOnly = false,
}: {
  project: ProjectWithWorkspaces
  /** Drawer Agent tab: the label and switch only (no description box). */
  compact?: boolean
  /** Shown but not flippable: a room on another server, or a role below the route floor. */
  readOnly?: boolean
}): React.JSX.Element {
  const [busy, setBusy] = useState(false)
  const enabled = (project.agentsCanManageVault ?? 0) === 1

  const toggle = async (): Promise<void> => {
    if (busy || readOnly) return
    const next = !enabled
    const before = project.agentsCanManageVault
    setBusy(true)
    useProjectsStore.setState((s) => ({
      projects: s.projects.map((p) =>
        p.id === project.id ? { ...p, agentsCanManageVault: next ? 1 : 0 } : p,
      ),
    }))
    try {
      await daemonCliPost(primaryScope(), 'agent-access/set', {
        workspace: project.path,
        toggle: 'vault_manage',
        value: next,
      })
      noteOptimisticProjectsMutationSuccess()
      emitProjectsChanged()
    } catch (err) {
      useProjectsStore.setState((s) => ({
        projects: s.projects.map((p) =>
          p.id === project.id ? { ...p, agentsCanManageVault: before } : p,
        ),
      }))
      console.error('[agent-access] vault_manage write failed', err)
    } finally {
      setBusy(false)
    }
  }

  return (
    <SettingSwitchRow
      compact={compact}
      control={
        <Toggle
          size={compact ? 'sm' : 'md'}
          checked={enabled}
          onChange={toggle}
          disabled={busy || readOnly}
          data-settings-id="projects.agents-can-manage-vault"
          title={
            enabled
              ? 'This workspace\'s main agent may manage server-wide secrets'
              : 'This workspace\'s agent cannot manage server-wide secrets'
          }
        />
      }
    >
          <div className={SETTING_ROW_LABEL}>
            Allow managing server-wide secrets
            <AskedChip row="projects.agents-can-manage-vault" />
          </div>
          <div className={permissionDescClass(compact)}>
            Lets its main agent share server secrets with other workspaces, never with itself. Seeing a value still needs you.
          </div>
    </SettingSwitchRow>
  )
}

// K2 compute nodes — "Allow compute nodes". DEFAULTS OFF. The single
// control for an agent's compute (Rosson 2026-10-11: it lives here with the
// other agent permissions, not in Settings → Compute). Writes POST
// /cli/agent-access/set `{workspace, toggle: 'compute', value}` (Owner/Admin)
// via `setWorkspaceComputeSwitch`; the daemon then grants every machine
// switched on (default limits) and grants machines added later; off removes
// every grant. Optimistic store patch; no projects refetch here.
export function AgentsUseComputeToggle({
  project,
  compact = false,
  readOnly = false,
}: {
  project: ProjectWithWorkspaces
  /** Drawer Agent tab: the label and switch only (no description box). */
  compact?: boolean
  /** Shown but not flippable: a room on another server, or a role below the route floor. */
  readOnly?: boolean
}): React.JSX.Element {
  const [busy, setBusy] = useState(false)
  const [result, setResult] = useState<string | null>(null)
  const [error, setError] = useState<string | null>(null)
  const enabled = (project.agentsCanUseCompute ?? 0) === 1

  const toggle = async (): Promise<void> => {
    if (busy || readOnly) return
    const next = !enabled
    const before = project.agentsCanUseCompute
    setBusy(true)
    setError(null)
    useProjectsStore.setState((s) => ({
      projects: s.projects.map((p) =>
        p.id === project.id ? { ...p, agentsCanUseCompute: next ? 1 : 0 } : p,
      ),
    }))
    try {
      const out = await setWorkspaceComputeSwitch(project.path, next)
      noteOptimisticProjectsMutationSuccess()
      emitProjectsChanged()
      if (next) {
        const machines = [...(out.granted ?? []), ...(out.kept ?? [])]
        setResult(
          machines.length === 0
            ? 'No machines switched on yet. Ones you add in Settings → Compute are granted automatically.'
            : `Can use ${machines.length === 1 ? '1 machine' : `${machines.length} machines`}: ${machines.join(', ')}.`,
        )
      } else {
        setResult(null)
      }
    } catch (err) {
      useProjectsStore.setState((s) => ({
        projects: s.projects.map((p) =>
          p.id === project.id ? { ...p, agentsCanUseCompute: before } : p,
        ),
      }))
      setError(computeErrorMessage(err))
      console.error('[agent-access] compute write failed', err)
    } finally {
      setBusy(false)
    }
  }

  return (
    <SettingSwitchRow
      compact={compact}
      control={
        <Toggle
          size={compact ? 'sm' : 'md'}
          checked={enabled}
          onChange={toggle}
          aria-label="Allow compute nodes"
          disabled={busy || readOnly}
          data-settings-id="projects.agents-can-use-compute"
          title={
            enabled
              ? 'This workspace\'s agents may run jobs on your compute machines'
              : 'This workspace\'s agents cannot use compute machines'
          }
        />
      }
    >
          <div className={SETTING_ROW_LABEL}>
            Allow compute nodes
            <AskedChip row="projects.agents-can-use-compute" />
          </div>
          <div className={permissionDescClass(compact)}>
            Allow this agent to use compute nodes: run builds and tests on the machines you&rsquo;ve switched on, and on ones you add later.
          </div>
          {result ? (
            <div className="type-hint text-[var(--color-text-secondary)] mt-1" data-testid="agents-compute-result">
              {result}
            </div>
          ) : null}
          {error ? (
            <div className="type-hint text-[var(--color-status-error-soft)] mt-1" data-testid="agents-compute-error">
              {error}
            </div>
          ) : null}
    </SettingSwitchRow>
  )
}

// Node admin (prd-k2-node-admin-v1 Q14, 0156) — "Allow this agent to
// operate compute nodes". DEFAULTS OFF. The owner's standing approval for
// the operate class only (narrow, pause/drain/resume, update to a newer
// signed build, roll back, restart a service); root actions still wait for
// the owner every time. Writes POST /cli/agent-access/set `{workspace,
// toggle: 'compute_operate', value}` (Owner/Admin) via
// `setWorkspaceComputeOperate`, the same route as `k2 compute operate`.
// Owner/Admin only: a Member never sees it; until whoami answers it shows
// but can't be flipped. Optimistic store patch; no projects refetch here.
export type OperateComputeRole = 'owner' | 'admin' | 'member'

export function operateComputeVisibility(role: OperateComputeRole | null): { show: boolean; canFlip: boolean } {
  if (role === 'member') return { show: false, canFlip: false }
  return { show: true, canFlip: role === 'owner' || role === 'admin' }
}

export function AgentsOperateComputeToggle({
  project,
  compact = false,
  readOnly = false,
}: {
  project: ProjectWithWorkspaces
  /** Drawer Agent tab: the label and switch only (no description box). */
  compact?: boolean
  /** Shown but not flippable: a room on another server, or a role below the route floor. */
  readOnly?: boolean
}): React.JSX.Element | null {
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [role, setRole] = useState<OperateComputeRole | null>(null)
  const enabled = (project.agentsCanOperateCompute ?? 0) === 1

  useEffect(() => {
    let cancelled = false
    void fetchViewerRole().then((r) => {
      if (!cancelled) setRole(r)
    })
    return () => {
      cancelled = true
    }
  }, [])

  const { show, canFlip } = operateComputeVisibility(role)
  if (!show) return null

  const toggle = async (): Promise<void> => {
    if (busy || !canFlip || readOnly) return
    const next = !enabled
    const before = project.agentsCanOperateCompute
    setBusy(true)
    setError(null)
    useProjectsStore.setState((s) => ({
      projects: s.projects.map((p) =>
        p.id === project.id ? { ...p, agentsCanOperateCompute: next ? 1 : 0 } : p,
      ),
    }))
    try {
      await setWorkspaceComputeOperate(project.path, next)
      noteOptimisticProjectsMutationSuccess()
      emitProjectsChanged()
    } catch (err) {
      useProjectsStore.setState((s) => ({
        projects: s.projects.map((p) =>
          p.id === project.id ? { ...p, agentsCanOperateCompute: before } : p,
        ),
      }))
      setError(computeErrorMessage(err))
      console.error('[agent-access] compute_operate write failed', err)
    } finally {
      setBusy(false)
    }
  }

  return (
    <SettingSwitchRow
      compact={compact}
      control={
        <Toggle
          size={compact ? 'sm' : 'md'}
          checked={enabled}
          onChange={toggle}
          aria-label="Allow this agent to operate compute nodes"
          disabled={busy || !canFlip || readOnly}
          data-settings-id="projects.agents-can-operate-compute"
          title={
            !canFlip
              ? 'Checking that you are the owner or an admin of this server'
              : enabled
                ? 'This workspace\'s agent may operate your compute machines; root actions still wait for you'
                : 'This workspace\'s agent cannot operate compute machines'
          }
        />
      }
    >
          <div className={SETTING_ROW_LABEL}>
            Allow this agent to operate compute nodes{' '}
            <span className="type-hint text-[var(--color-text-secondary)]">Beta</span>
            <AskedChip row="projects.agents-can-operate-compute" />
          </div>
          <div className={permissionDescClass(compact)}>
            Pause, drain, resume, update and restart your compute machines without asking. Root actions wait for your approval.
          </div>
          {error ? (
            <div className="type-hint text-[var(--color-status-error-soft)] mt-1" data-testid="agents-compute-operate-error">
              {error}
            </div>
          ) : null}
    </SettingSwitchRow>
  )
}

// Agents manage compute nodes (0194, Rosson 2026-10-18) — "Allow managing
// compute nodes". DEFAULTS OFF. On: a local agent from this workspace adds
// and confirms machines, renames, pauses/drains/resumes, retires and routes
// them, and grants or revokes them for any workspace, each act an Access
// Audit line (add and confirm also drop an owner inbox note). The preview
// flag, removing a machine and this switch stay the owner's. Writes POST
// /cli/agent-access/set `{workspace, toggle: 'compute_manage', value}` via
// `setWorkspaceComputeManage` (same route as `k2 compute manage`). The
// daemon lets only the owner flip it: an Admin sees it read-only, a Member
// never sees it; until whoami answers it shows but can't be flipped. The
// check lives in the daemon; this only shows the switch.
export function manageComputeVisibility(role: OperateComputeRole | null): { show: boolean; canFlip: boolean } {
  if (role === 'member') return { show: false, canFlip: false }
  return { show: true, canFlip: role === 'owner' }
}

export function AgentsManageComputeToggle({
  project,
  compact = false,
  readOnly = false,
}: {
  project: ProjectWithWorkspaces
  /** Drawer Agent tab: the label and switch only (no description box). */
  compact?: boolean
  /** Shown but not flippable: a room on another server. */
  readOnly?: boolean
}): React.JSX.Element | null {
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [role, setRole] = useState<OperateComputeRole | null>(null)
  const enabled = (project.agentsCanManageCompute ?? 0) === 1

  useEffect(() => {
    let cancelled = false
    void fetchViewerRole().then((r) => {
      if (!cancelled) setRole(r)
    })
    return () => {
      cancelled = true
    }
  }, [])

  const { show, canFlip } = manageComputeVisibility(role)
  if (!show) return null

  const toggle = async (): Promise<void> => {
    if (busy || !canFlip || readOnly) return
    const next = !enabled
    const before = project.agentsCanManageCompute
    setBusy(true)
    setError(null)
    useProjectsStore.setState((s) => ({
      projects: s.projects.map((p) =>
        p.id === project.id ? { ...p, agentsCanManageCompute: next ? 1 : 0 } : p,
      ),
    }))
    try {
      await setWorkspaceComputeManage(project.path, next)
      noteOptimisticProjectsMutationSuccess()
      emitProjectsChanged()
    } catch (err) {
      useProjectsStore.setState((s) => ({
        projects: s.projects.map((p) =>
          p.id === project.id ? { ...p, agentsCanManageCompute: before } : p,
        ),
      }))
      setError(computeErrorMessage(err))
      console.error('[agent-access] compute_manage write failed', err)
    } finally {
      setBusy(false)
    }
  }

  return (
    <SettingSwitchRow
      compact={compact}
      control={
        <Toggle
          size={compact ? 'sm' : 'md'}
          checked={enabled}
          onChange={toggle}
          aria-label="Allow managing compute nodes"
          disabled={busy || !canFlip || readOnly}
          data-settings-id="projects.agents-can-manage-compute"
          title={
            role === null
              ? 'Checking that you are the owner of this server'
              : !canFlip
                ? 'Only the server\'s owner can change this'
                : enabled
                  ? 'This workspace\'s agent manages your compute machines (logged)'
                  : 'This workspace\'s agent cannot manage your compute machines'
          }
        />
      }
    >
          <div className={SETTING_ROW_LABEL}>
            Allow managing compute nodes{' '}
            <span className="type-hint text-[var(--color-text-secondary)]">Beta</span>
            <AskedChip row="projects.agents-can-manage-compute" />
          </div>
          <div className={permissionDescClass(compact)}>
            Add, rename, pause, retire and route compute machines, and grant them to any workspace, without asking. Turning compute on or off and removing a machine stay yours.
          </div>
          {error ? (
            <div className="type-hint text-[var(--color-status-error-soft)] mt-1" data-testid="agents-compute-manage-error">
              {error}
            </div>
          ) : null}
    </SettingSwitchRow>
  )
}

// Postgres versions (0201, prd-pg17-and-db-upgrade-v1 §12) — "Allow this
// agent to manage Postgres versions" (`db_upgrade`). DEFAULTS OFF. On: a
// local agent from this workspace may check, move and roll back its own and
// its Apps' databases to a newer Postgres, create on a chosen version and
// tidy empty clusters, each act audited. Deleting the old copy, other
// workspaces' databases and `--all` stay the owner's. Writes POST
// /cli/agent-access/set `{workspace, toggle: 'db_upgrade', value}`. The
// daemon lets the owner raise it and the owner or an Admin lower it (the
// Apps-switch floor): an Admin sees it flippable only while it is on, a
// Member never sees it. The check lives in the daemon.
export function dbUpgradeVisibility(
  role: OperateComputeRole | null,
  enabled: boolean,
): { show: boolean; canFlip: boolean } {
  if (role === 'member') return { show: false, canFlip: false }
  if (role === 'owner') return { show: true, canFlip: true }
  if (role === 'admin') return { show: true, canFlip: enabled }
  return { show: true, canFlip: false }
}

export function AgentsManagePostgresToggle({
  project,
  compact = false,
  readOnly = false,
}: {
  project: ProjectWithWorkspaces
  compact?: boolean
  readOnly?: boolean
}): React.JSX.Element | null {
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [role, setRole] = useState<OperateComputeRole | null>(null)
  const enabled = (project.agentsCanUpgradePostgres ?? 0) === 1

  useEffect(() => {
    let cancelled = false
    void fetchViewerRole().then((r) => {
      if (!cancelled) setRole(r)
    })
    return () => {
      cancelled = true
    }
  }, [])

  const { show, canFlip } = dbUpgradeVisibility(role, enabled)
  if (!show) return null

  const toggle = async (): Promise<void> => {
    if (busy || !canFlip || readOnly) return
    const next = !enabled
    const before = project.agentsCanUpgradePostgres
    setBusy(true)
    setError(null)
    useProjectsStore.setState((s) => ({
      projects: s.projects.map((p) =>
        p.id === project.id ? { ...p, agentsCanUpgradePostgres: next ? 1 : 0 } : p,
      ),
    }))
    try {
      await setWorkspaceDbUpgrade(project.path, next)
      noteOptimisticProjectsMutationSuccess()
      emitProjectsChanged()
    } catch (err) {
      useProjectsStore.setState((s) => ({
        projects: s.projects.map((p) =>
          p.id === project.id ? { ...p, agentsCanUpgradePostgres: before } : p,
        ),
      }))
      setError(computeErrorMessage(err))
      console.error('[agent-access] db_upgrade write failed', err)
    } finally {
      setBusy(false)
    }
  }

  return (
    <SettingSwitchRow
      compact={compact}
      control={
        <Toggle
          size={compact ? 'sm' : 'md'}
          checked={enabled}
          onChange={toggle}
          aria-label="Allow this agent to manage Postgres versions"
          disabled={busy || !canFlip || readOnly}
          data-settings-id="projects.agents-can-upgrade-postgres"
          title={
            role === null
              ? 'Checking your role on this server'
              : !canFlip
                ? 'Only the server\'s owner can turn this on (an Admin may turn it off)'
                : enabled
                  ? 'This workspace\'s agent manages its databases\' Postgres versions (logged)'
                  : 'This workspace\'s agent cannot change Postgres versions'
          }
        />
      }
    >
          <div className={SETTING_ROW_LABEL}>
            Allow this agent to manage Postgres versions
          </div>
          <div className={permissionDescClass(compact)}>
            Create databases on a chosen Postgres version, move them to a newer one, and roll back. Deleting the old copy stays with you.
          </div>
          {error ? (
            <div className="type-hint text-[var(--color-status-error-soft)] mt-1" data-testid="agents-db-upgrade-error">
              {error}
            </div>
          ) : null}
    </SettingSwitchRow>
  )
}

// K2 projects (0162, Rosson 2026-10-14) — "Manage K2 projects". DEFAULTS
// ON (projects are open by default). Off: this workspace's agent gets 403
// `agent_permission_off` on every project mutation (create, rename, delete,
// members, settings); reading projects and posting in its project chat
// stay allowed. Same single writer as the rows above: POST
// /cli/agent-access/set `{workspace, toggle: 'projects_manage', value}`
// (Owner/Admin; an agent can never flip it). Optimistic store patch; no
// projects refetch here.
export function AgentsManageProjectsToggle({
  project,
  compact = false,
  readOnly = false,
}: {
  project: ProjectWithWorkspaces
  /** Drawer Agent tab: the label and switch only (no description box). */
  compact?: boolean
  /** Shown but not flippable: a room on another server, or a role below the route floor. */
  readOnly?: boolean
}): React.JSX.Element {
  const [busy, setBusy] = useState(false)
  // Absent (an older daemon) reads as on: that is the column default.
  const enabled = (project.agentsCanManageProjects ?? 1) === 1

  const toggle = async (): Promise<void> => {
    if (busy || readOnly) return
    const next = !enabled
    const before = project.agentsCanManageProjects
    setBusy(true)
    useProjectsStore.setState((s) => ({
      projects: s.projects.map((p) =>
        p.id === project.id ? { ...p, agentsCanManageProjects: next ? 1 : 0 } : p,
      ),
    }))
    try {
      await daemonCliPost(primaryScope(), 'agent-access/set', {
        workspace: project.path,
        toggle: 'projects_manage',
        value: next,
      })
      noteOptimisticProjectsMutationSuccess()
      emitProjectsChanged()
    } catch (err) {
      useProjectsStore.setState((s) => ({
        projects: s.projects.map((p) =>
          p.id === project.id ? { ...p, agentsCanManageProjects: before } : p,
        ),
      }))
      console.error('[agent-access] projects_manage write failed', err)
    } finally {
      setBusy(false)
    }
  }

  return (
    <SettingSwitchRow
      compact={compact}
      control={
        <Toggle
          size={compact ? 'sm' : 'md'}
          checked={enabled}
          onChange={toggle}
          aria-label="Manage K2 projects"
          disabled={busy || readOnly}
          data-settings-id="projects.agents-can-manage-projects"
          title={
            enabled
              ? 'This workspace\'s agent may create and change K2 projects and their members'
              : 'This workspace\'s agent can read projects and post in its project chat, but not change them'
          }
        />
      }
    >
          <div className={SETTING_ROW_LABEL}>
            Manage K2 projects
            <AskedChip row="projects.agents-can-manage-projects" />
          </div>
          <div className={permissionDescClass(compact)}>
            Lets this agent create, rename and delete K2 projects and change their members. Off: it can still read them and post in its project chat.
          </div>
    </SettingSwitchRow>
  )
}

// Agents run Gardens end to end (0181, Rosson 2026-10-16) — "Manage Gardens
// and data sources". DEFAULTS OFF. On: this workspace's agent approves data
// sources (its own proposals and others'), binds a secret it may already
// use, pauses, pins, enables triggers and edits Gardens and widgets with no
// confirm; every act is an Access Audit line. Off: it proposes and a person
// approves. Same single writer as the rows above: POST
// /cli/agent-access/set `{workspace, toggle: 'gardens_manage', value}`
// (Owner/Admin; an agent can never flip it). Optimistic store patch; no
// projects refetch here.
//
// prd-garden-access-v1 §2 (as proposed): with Garden access on (this
// computer's switch, and the window on this computer), the row keeps its
// data-source powers only and reads "Manage data sources"; who edits a
// Garden is its list in Settings → Gardens. Off: as 0.46.3.
/** "Can refresh its own pinned chat" (0263, 0.46.8): the owner or an admin
 *  (the route's floor; a Member's write is refused and rolls back here). */
export function AgentsRefreshChatToggle({
  project,
}: {
  project: ProjectWithWorkspaces
}): React.JSX.Element {
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const enabled = (project.agentsCanRefreshChat ?? 0) === 1

  const toggle = async (): Promise<void> => {
    if (busy) return
    const next = !enabled
    const before = project.agentsCanRefreshChat
    setBusy(true)
    setError(null)
    useProjectsStore.setState((s) => ({
      projects: s.projects.map((p) => (p.id === project.id ? { ...p, agentsCanRefreshChat: next ? 1 : 0 } : p)),
    }))
    try {
      await daemonCliPost(primaryScope(), 'agent-access/set', {
        workspace: project.path,
        toggle: 'chat_refresh',
        value: next,
      })
      noteOptimisticProjectsMutationSuccess()
      emitProjectsChanged()
    } catch (err) {
      useProjectsStore.setState((s) => ({
        projects: s.projects.map((p) => (p.id === project.id ? { ...p, agentsCanRefreshChat: before } : p)),
      }))
      setError(err instanceof Error ? err.message : String(err))
      console.error('[agent-access] chat_refresh write failed', err)
    } finally {
      setBusy(false)
    }
  }

  return (
    <SettingSwitchRow
      control={
        <Toggle
          checked={enabled}
          onChange={toggle}
          aria-label="Can refresh its own pinned chat"
          disabled={busy}
          data-settings-id="projects.agents-can-refresh-chat"
          title={enabled ? 'This agent starts a new pinned chat itself' : 'This agent asks before it starts a new pinned chat'}
        />
      }
    >
      <div className={SETTING_ROW_LABEL}>Can refresh its own pinned chat</div>
      <div className={SETTING_ROW_HINT}>
            Lets this agent start a fresh pinned chat when its chat goes stale. What it&apos;s doing then stops; the old chat stays in Chat history.
          </div>
      {error && <p className="type-hint text-[var(--color-status-error-soft)] mt-1">{error}</p>}
    </SettingSwitchRow>
  )
}

export function AgentsManageGardensToggle({
  project,
}: {
  project: ProjectWithWorkspaces
}): React.JSX.Element {
  const [busy, setBusy] = useState(false)
  const enabled = (project.agentsCanManageGardens ?? 0) === 1
  const accessOn = useGardenAccessSwitchStore((s) => s.on) === true && !primaryScope().isRemote
  const label = accessOn ? 'Manage data sources' : 'Manage Gardens and data sources'

  const toggle = async (): Promise<void> => {
    if (busy) return
    const next = !enabled
    const before = project.agentsCanManageGardens
    setBusy(true)
    useProjectsStore.setState((s) => ({
      projects: s.projects.map((p) =>
        p.id === project.id ? { ...p, agentsCanManageGardens: next ? 1 : 0 } : p,
      ),
    }))
    try {
      await daemonCliPost(primaryScope(), 'agent-access/set', {
        workspace: project.path,
        toggle: 'gardens_manage',
        value: next,
      })
      noteOptimisticProjectsMutationSuccess()
      emitProjectsChanged()
    } catch (err) {
      useProjectsStore.setState((s) => ({
        projects: s.projects.map((p) =>
          p.id === project.id ? { ...p, agentsCanManageGardens: before } : p,
        ),
      }))
      console.error('[agent-access] gardens_manage write failed', err)
    } finally {
      setBusy(false)
    }
  }

  return (
    <SettingSwitchRow
      control={
        <Toggle
          checked={enabled}
          onChange={toggle}
          aria-label={label}
          disabled={busy}
          data-settings-id="projects.agents-can-manage-gardens"
          title={
            accessOn
              ? enabled
                ? 'This workspace\'s agent approves data sources itself (logged)'
                : 'This workspace\'s agent proposes data sources; you approve'
              : enabled
                ? 'This workspace\'s agent approves data sources and edits Gardens itself (logged)'
                : 'This workspace\'s agent proposes data sources and Garden changes; you approve'
          }
        />
      }
    >
          <div className={SETTING_ROW_LABEL}>
            {label}
            <AskedChip row="projects.agents-can-manage-gardens" />
          </div>
          {accessOn ? (
            <div className={SETTING_ROW_HINT} data-garden-access-copy="">
              Approves and pauses data sources and turns on triggers without asking you. Every act is logged in Access Audit.
            </div>
          ) : (
            <div className={SETTING_ROW_HINT}>
              Approves and pauses data sources, turns on triggers and edits your Gardens without asking you. Every act is logged in Access Audit. Locked Gardens stay yours.
            </div>
          )}
    </SettingSwitchRow>
  )
}

// Per-workspace agents-may-manage-hosted-mail. DEFAULTS OFF.
// No global master. Writes POST /cli/mail-manage `{project, enable: 0|1}`.
// Optimistic store patch; do not fetchProjects() on this path.
export function MailManageToggle({
  project,
  compact = false,
  readOnly = false,
}: {
  project: ProjectWithWorkspaces
  /** Drawer Agent tab: the label and switch only (no description box). */
  compact?: boolean
  /** Shown but not flippable: a room on another server, or a role below the route floor. */
  readOnly?: boolean
}): React.JSX.Element {
  const [busy, setBusy] = useState(false)
  const enabled = (project.mailManageEnabled ?? 0) === 1

  const toggle = async (): Promise<void> => {
    if (busy || readOnly) return
    const next = !enabled
    const before = project.mailManageEnabled
    setBusy(true)
    useProjectsStore.setState((s) => ({
      projects: s.projects.map((p) =>
        p.id === project.id ? { ...p, mailManageEnabled: next ? 1 : 0 } : p,
      ),
    }))
    try {
      await daemonCliPost(primaryScope(), 'mail-manage', {
        project: project.path,
        enable: next ? 1 : 0,
      })
      noteOptimisticProjectsMutationSuccess()
      emitProjectsChanged()
    } catch (err) {
      useProjectsStore.setState((s) => ({
        projects: s.projects.map((p) =>
          p.id === project.id ? { ...p, mailManageEnabled: before } : p,
        ),
      }))
      console.error('[mail-manage] write failed', err)
    } finally {
      setBusy(false)
    }
  }

  return (
    <SettingSwitchRow
      compact={compact}
      control={
        <Toggle
          size={compact ? 'sm' : 'md'}
          checked={enabled}
          onChange={toggle}
          disabled={busy || readOnly}
          data-settings-id="projects.mail-manage-enabled"
          title={
            enabled
              ? 'This agent may manage hosted mail on this server'
              : 'This agent cannot manage hosted mail'
          }
        />
      }
    >
          <div className={SETTING_ROW_LABEL}>
            Allow agents to manage hosted mail on this server
            <AskedChip row="projects.mail-manage-enabled" />
          </div>
          <div className={permissionDescClass(compact)}>
            Lets this agent run hosted mail on this server: turn it on or off and set up domains and delivery. Uninstalling, removing a domain and OAuth stay with you.
          </div>
    </SettingSwitchRow>
  )
}

// Create-only passport for `k2 db create`. DEFAULTS OFF / fail-closed.
// No global master (unlike DNS/connections). Writes POST /cli/workspace/set
// `{ fields: { db_agent_access: "write"|"off" } }`. Optimistic store patch;
// do not fetchProjects() on this path.
export function DbAgentCreateToggle({
  project,
  compact = false,
  readOnly = false,
}: {
  project: ProjectWithWorkspaces
  /** Drawer Agent tab: the label and switch only (no description box). */
  compact?: boolean
  /** Shown but not flippable: a room on another server, or a role below the route floor. */
  readOnly?: boolean
}): React.JSX.Element {
  const [busy, setBusy] = useState(false)
  const enabled = (project.dbAgentAccess ?? 'off') === 'write'

  const toggle = async (): Promise<void> => {
    if (busy || readOnly) return
    const next = !enabled
    const before = project.dbAgentAccess
    setBusy(true)
    useProjectsStore.setState((s) => ({
      projects: s.projects.map((p) =>
        p.id === project.id ? { ...p, dbAgentAccess: next ? 'write' : 'off' } : p,
      ),
    }))
    try {
      await setSqlDbAgentAccess(project.path, next)
      noteOptimisticProjectsMutationSuccess()
      emitProjectsChanged()
    } catch (err) {
      useProjectsStore.setState((s) => ({
        projects: s.projects.map((p) =>
          p.id === project.id ? { ...p, dbAgentAccess: before } : p,
        ),
      }))
      console.error('[db-agent-create] write failed', err)
    } finally {
      setBusy(false)
    }
  }

  return (
    <SettingSwitchRow
      compact={compact}
      control={
        <Toggle
          size={compact ? 'sm' : 'md'}
          checked={enabled}
          onChange={toggle}
          disabled={busy || readOnly}
          data-settings-id="projects.db-agent-create"
          title={
            enabled
              ? 'This agent may run k2 db create'
              : 'This agent cannot run k2 db create'
          }
        />
      }
    >
          <div className={SETTING_ROW_LABEL}>
            Allow this agent to create databases
            <AskedChip row="projects.db-agent-create" />
          </div>
          <div className={permissionDescClass(compact)}>
            Lets this agent create new databases. It can always use ones this workspace owns or has been granted.
          </div>
    </SettingSwitchRow>
  )
}

// ── C1 Per-workspace agents-may-create-connections opt-in ──────────
// Lets agents add/remove workspace connections for THIS workspace.
// DEFAULTS OFF / fail-closed. Owner always may manage connections.
// The daemon ENFORCES this server-side via
// `agents_can_create_connections_for_path` (app master OR per-workspace).
// Reads from the projects store (`agentsCanCreateConnections`); writes
// via /cli/agents-create-connections.
export function AgentsCreateConnectionsToggle({
  project,
  compact = false,
  readOnly = false,
  fetchProjects,
}: {
  project: ProjectWithWorkspaces
  /** Drawer Agent tab: the label and switch only (no description box). */
  compact?: boolean
  /** Shown but not flippable: a room on another server, or a role below the route floor. */
  readOnly?: boolean
  fetchProjects: () => Promise<void>
}): React.JSX.Element {
  const [busy, setBusy] = useState(false)
  const enabled = (project.agentsCanCreateConnections ?? 0) === 1
  const globalAllow = useSettingsStore((s) => s.agentsCanCreateConnections)

  const toggle = async (): Promise<void> => {
    if (busy || readOnly) return
    const next = !enabled
    setBusy(true)
    try {
      await daemonCliPostQuery(primaryScope(), 'agents-create-connections', {
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
    <SettingSwitchRow
      compact={compact}
      control={
        <Toggle
          size={compact ? 'sm' : 'md'}
          checked={enabled}
          onChange={toggle}
          disabled={busy || readOnly}
          data-settings-id="projects.agents-can-create-connections"
          title={
            enabled
              ? 'Agents may add/remove connections for this workspace'
              : 'Agents cannot add/remove connections for this workspace'
          }
        />
      }
    >
          <div className={SETTING_ROW_LABEL}>
            Allow agents to create connections for this workspace
            <AskedChip row="projects.agents-can-create-connections" />
          </div>
          <div className={permissionDescClass(compact)}>
            {enabled
              ? 'Agents in this workspace may add and remove connections to other workspaces.'
              : 'Off (recommended): agents in this workspace can\'t add or remove connections.'}
          </div>
          {!enabled && globalAllow && !readOnly && (
            <div className="type-hint text-[var(--color-status-warn-text)] mt-1 leading-relaxed">
              Currently allowed anyway: the global &ldquo;Allow agents to create
              connections&rdquo; (Settings → Tunnel) is on for every
              workspace.
            </div>
          )}
    </SettingSwitchRow>
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
        <div {...floatMaterial('menu')} className="absolute right-0 top-full mt-1 z-50 bg-[var(--color-bg)] border border-[var(--color-border)] shadow-lg min-w-[140px]">
          <button
            onClick={() => { setOpen(false); onSettings() }}
            className="w-full text-left px-3 py-1.5 type-control text-[var(--color-text-secondary)] hover:bg-[var(--color-bg-hover)] hover:text-[var(--color-text-primary)] transition-colors no-drag cursor-pointer"
          >
            Settings
          </button>
          {onDelete && (
            <button
              onClick={() => { setOpen(false); onDelete() }}
              className="w-full text-left px-3 py-1.5 type-control text-[var(--color-status-error-soft)] hover:bg-[color-mix(in_srgb,var(--color-status-error)_10%,transparent)] hover:text-[var(--color-status-error-bright)] transition-colors no-drag cursor-pointer"
            >
              Delete agent
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

  // 0.46.6: the default agent only changes NEW chats. After a change, offer
  // to move the pinned chat too (one tap; the daemon does the move).
  const [offerMove, setOfferMove] = useState<string | null>(null)
  const [moveNote, setMoveNote] = useState('')
  const [moving, setMoving] = useState(false)

  const handleChange = async (presetId: string): Promise<void> => {
    setSelected(presetId)
    setMoveNote('')
    try {
      await daemonCliPost(primaryScope(), 'projects/update', { id: projectId, defaultAgent: presetId || '' })
      emitProjectsChanged()
      const store = useProjectsStore.getState()
      const updated = store.projects.map((p) =>
        p.id === projectId ? { ...p, defaultAgent: presetId || null } : p
      )
      useProjectsStore.setState({ projects: updated })
      const chosen = presetId ? resolvePreset(presetId) : globalPreset
      setOfferMove(harnessOfCommand(chosen?.command))
    } catch (err) {
      console.error('[default-agent-selector] Update failed:', err)
    }
  }

  const handleMove = async (harness: string): Promise<void> => {
    setMoving(true)
    try {
      const r = await movePinnedChat(primaryScope(), projectId, harness)
      setMoveNote(movePinnedChatMessage(r))
      setOfferMove(null)
    } catch (err) {
      setMoveNote(`The pinned chat was not moved: ${err instanceof Error ? err.message : String(err)}`)
    } finally {
      setMoving(false)
    }
  }

  return (
    <SettingRow
      label="Default agent"
      hint={`${metaChordLabel('Shift+Meta+T') ? `The agent ${metaChordLabel('Shift+Meta+T')} launches` : 'The default agent'} for this workspace. Edit with AI uses Default AI Agent in Providers / Models.`}
      field
      below={
        offerMove || moveNote ? (
          <div className="space-y-2">
            {offerMove && (
              <div className="flex items-center justify-between gap-4" data-testid="default-agent-move-pinned">
                <span className="type-hint text-[var(--color-text-secondary)] leading-relaxed">
                  Also move the pinned chat to {harnessLabel(offerMove)}? New chats use it already; the pinned chat (heartbeats, Threads, messages) keeps its current agent until moved.
                </span>
                <div className="flex gap-2 flex-shrink-0">
                  <Button variant="accent" disabled={moving} onClick={() => { void handleMove(offerMove) }}>
                    {moving ? 'Moving…' : 'Move'}
                  </Button>
                  <Button variant="quiet" disabled={moving} onClick={() => setOfferMove(null)}>
                    Not now
                  </Button>
                </div>
              </div>
            )}
            {moveNote && (
              <p className="type-hint text-[var(--color-text-secondary)]" data-testid="default-agent-move-note">{moveNote}</p>
            )}
          </div>
        ) : undefined
      }
    >
      <SettingDropdown
        fullWidth
        value={value}
        ariaLabel="Default agent"
        options={[
          { value: '', label: `Inherit global (${globalPreset?.label ?? globalDefaultAgent})` },
          ...enabledPresets.map((p) => ({ value: p.id, label: p.label })),
        ]}
        onChange={handleChange}
        placeholder={selected || undefined}
      />
    </SettingRow>
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

/** Dropdown value that opens the custom ID field; never stored. */
const CUSTOM_MODEL = '__custom__'

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
      await daemonCliPost(primaryScope(), 'projects/update', {
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

  const suggested = suggestions.includes(model)
  const [customOpen, setCustomOpen] = useState(false)
  const showCustomField = customOpen || (model.trim() !== '' && !suggested)

  const choose = (value: string): void => {
    if (value === CUSTOM_MODEL) {
      setCustomOpen(true)
      setCustom(suggested ? '' : model)
      return
    }
    setCustomOpen(false)
    setCustom('')
    setModel(value)
    if (!value) setForce(false)
    void persist(value, value ? force : false)
  }

  const handleCustomSubmit = (): void => {
    const next = custom.trim()
    if (!next) return
    setModel(next)
    setCustom('')
    setCustomOpen(false)
    void persist(next, force)
  }

  const handleForce = (next: boolean): void => {
    if (!model.trim()) return
    setForce(next)
    void persist(model, next)
  }

  const empty = !model.trim()
  const options = [
    { value: '', label: "Agent's default" },
    ...suggestions.map((id) => ({ value: id, label: id })),
    ...(!empty && !suggested ? [{ value: model, label: model }] : []),
    { value: CUSTOM_MODEL, label: 'Custom ID…' },
  ]

  return (
    <>
      <SettingRow
        label="Default model"
        settingId="projects.default-model"
        hint="The model new sessions in this workspace use. A running chat can't switch models. An API call that sets its own model overrides this."
        field
        below={
          showCustomField ? (
            <div className="flex items-center justify-end gap-2">
              <span className="type-hint text-[var(--color-text-secondary)]">Custom model ID</span>
              <Input
                value={customOpen ? custom : custom || model}
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
                placeholder="Custom ID"
                aria-label="Custom model ID"
                className={`${SETTING_FIELD_WIDTH} type-control`}
              />
            </div>
          ) : undefined
        }
      >
        <SettingDropdown
          fullWidth
          value={customOpen ? CUSTOM_MODEL : model}
          ariaLabel="Default model"
          options={options}
          onChange={choose}
        />
      </SettingRow>
      <SettingRow
        label="Force this model when resuming a session"
        hint={empty ? 'Choose a default model first.' : 'Also uses the default model when a chat resumes.'}
        settingId="projects.force-model-on-resume"
      >
        <Toggle
          checked={force && !empty}
          disabled={empty}
          aria-label="Force this model when resuming a session"
          onChange={handleForce}
        />
      </SettingRow>
    </>
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
    daemonCliGet<{ content: string }>(primaryScope(), 'fs/read-file', { path: filePath })
      .then((r) => setContent(r.content))
      .catch(() => setContent(''))
  }, [filePath])

  const handleClose = useCallback(async () => {
    try {
      await daemonCliPost(primaryScope(), 'agents/regenerate-workspace-skill', { project_path: projectPath })
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
            <div className="type-hint text-[var(--color-text-muted)] truncate">
              <span className="font-medium text-[var(--color-text-primary)]">{label}</span>
              <span className="mx-2">&middot;</span>
              <span className="font-mono type-hint">{filePath}</span>
            </div>
            <div className="flex items-center gap-2 flex-shrink-0">
              {previewMode === 'preview' && (
                <div className="flex items-center gap-0.5">
                  <button
                    onClick={() => setPreviewScale((s) => Math.max(50, s - 10))}
                    className="w-5 h-5 flex items-center justify-center type-control text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)] bg-[var(--color-bg-elevated)] border border-[var(--color-border)] no-drag cursor-pointer"
                  >
                    −
                  </button>
                  <span className="type-hint tabular-nums text-[var(--color-text-muted)] w-7 text-center">{previewScale}%</span>
                  <button
                    onClick={() => setPreviewScale((s) => Math.min(200, s + 10))}
                    className="w-5 h-5 flex items-center justify-center type-control text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)] bg-[var(--color-bg-elevated)] border border-[var(--color-border)] no-drag cursor-pointer"
                  >
                    +
                  </button>
                </div>
              )}
              <div className="flex border border-[var(--color-border)]">
                <button
                  onClick={() => setPreviewMode('preview')}
                  className={`px-2 py-0.5 type-control no-drag cursor-pointer ${
                    previewMode === 'preview'
                      ? 'bg-[var(--color-accent)] text-[var(--color-on-accent)]'
                      : 'text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)]'
                  }`}
                >
                  Preview
                </button>
                <button
                  onClick={() => setPreviewMode('edit')}
                  className={`px-2 py-0.5 type-control no-drag cursor-pointer ${
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
                    await daemonCliPost(primaryScope(), 'fs/write-file', { path: filePath, content: c })
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
    daemonCliGet<{ content: string }>(primaryScope(), 'fs/read-file', { path: filePath })
      .then((r) => setContent(r.content))
      .catch(() => setContent(''))
  }, [filePath])

  const handleFileChange = useCallback((c: string) => setContent(c), [])

  // Trigger workspace SKILL.md regen on close so PROJECT.md edits
  // propagate to every harness file before the user closes the editor.
  const handleClose = useCallback(async () => {
    try {
      await daemonCliPost(primaryScope(), 'agents/regenerate-workspace-skill', { project_path: projectPath })
    } catch (err) {
      console.warn('[workspace-knowledge] regen on close failed:', err)
    }
    onClose()
  }, [projectPath, onClose])

  const systemPrompt = useMemo(() => [
    `You're helping the user edit the workspace knowledge for "${projectName}".`,
    ``,
    `File: .k2/PROJECT.md (source)`,
    `Path: ${filePath}`,
    ``,
    `This is the SOURCE. K2 compiles it (plus the agent's ROLE.md) into the`,
    `canonical .k2/skills/<name>/SKILL.md on save. Mirroring this content out`,
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
      instructions="Editing .k2/PROJECT.md — the source for workspace knowledge. K2 compiles this into the canonical SKILL.md. When harness fan-out is enabled (opt-in), it also mirrors out to CLAUDE.md, AGENTS.md, GEMINI.md, .cursor/rules, .goosehints, etc. Regen runs automatically when you close this editor."
      warningText="This is the source file for the workspace's shared knowledge. When harness fan-out is enabled, edits mirror into every chosen CLI LLM harness on save."
      onFileChange={handleFileChange}
      onClose={handleClose}
      preview={
        <div className="h-full flex flex-col">
          <div className="flex items-center justify-between px-4 py-2 border-b border-[var(--color-border)] flex-shrink-0">
            <div className="type-hint text-[var(--color-text-muted)]">
              <span className="font-medium text-[var(--color-text-primary)]">PROJECT.md</span>
              <span className="mx-2">&middot;</span>
              <span>Source — compiled into every agent's SKILL.md</span>
            </div>
            <div className="flex items-center gap-2 flex-shrink-0">
              {previewMode === 'preview' && (
                <div className="flex items-center gap-0.5">
                  <button
                    onClick={() => setPreviewScale((s) => Math.max(50, s - 10))}
                    className="w-5 h-5 flex items-center justify-center type-control text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)] bg-[var(--color-bg-elevated)] border border-[var(--color-border)] no-drag cursor-pointer"
                  >
                    −
                  </button>
                  <span className="type-hint tabular-nums text-[var(--color-text-muted)] w-7 text-center">{previewScale}%</span>
                  <button
                    onClick={() => setPreviewScale((s) => Math.min(200, s + 10))}
                    className="w-5 h-5 flex items-center justify-center type-control text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)] bg-[var(--color-bg-elevated)] border border-[var(--color-border)] no-drag cursor-pointer"
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
                    className={`px-2 py-1 type-control transition-colors no-drag cursor-pointer ${
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
                  try { await daemonCliPost(primaryScope(), 'fs/write-file', { path: filePath, content: c }) } catch {}
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
      await daemonCliPostQuery(primaryScope(), 'agents/create', {
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
      scope={primaryScope()}
      projectPath={projectPath}
      helpText="The friendly label used in the inbox tab, chat tab, and persona prompts. Edit anytime — does not affect the agent's technical name or live session."
      trailing={
        <button
          onClick={handleOpenPersona}
          disabled={!ready}
          className="px-3 py-1.5 type-control text-[var(--color-accent)] bg-[var(--color-accent)]/10 hover:bg-[var(--color-accent)]/20 border border-[var(--color-accent)]/30 transition-colors no-drag cursor-pointer disabled:opacity-50 flex-shrink-0 self-end whitespace-nowrap"
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
        const agents = asArray<K2soAgentInfo & { agentType?: string }>(await invoke('k2so_agents_list', { projectPath }))
        // Stage A dual-read: `k2` and legacy `k2so` are the same builtin type.
        const existing = agents.find((a: any) => isBuiltinAgentType(a.agentType))
        if (existing) {
          setAgentName(existing.name)
        } else {
          await daemonCliPostQuery(primaryScope(), 'agents/create', {
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
      scope={primaryScope()}
      projectPath={projectPath}
      helpText="The friendly label used in the inbox tab. Edit anytime — does not affect routing or the live session."
      trailing={
        <button
          onClick={() => onOpenEditor(agentName)}
          disabled={!ready}
          className="px-3 py-1.5 type-control text-[var(--color-accent)] bg-[var(--color-accent)]/10 hover:bg-[var(--color-accent)]/20 border border-[var(--color-accent)]/30 transition-colors no-drag cursor-pointer disabled:opacity-50 flex-shrink-0 self-end whitespace-nowrap"
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

export function ConnectedWorkspacesPanel({ projectId }: { projectId: string }): React.JSX.Element {
  const [relations, setRelations] = useState<WorkspaceRelation[]>([])
  const [incoming, setIncoming] = useState<WorkspaceRelation[]>([])
  const [loading, setLoading] = useState(true)
  // 0.46.1: one Add connection picker (This server + paired servers).
  const [showAdd, setShowAdd] = useState(false)
  const addButtonRef = useRef<HTMLButtonElement>(null)
  const [remoteConns, setRemoteConns] = useState<RemoteConnectionEntry[]>([])
  const [remoteError, setRemoteError] = useState<string | null>(null)
  const [localError, setLocalError] = useState<string | null>(null)
  // 0159: a gone connection's Re-add is owner/admin only (the daemon checks too).
  const [viewerRole, setViewerRole] = useState<string | null>(null)
  const [reAdding, setReAdding] = useState<string | null>(null)
  const projects = useProjectsStore((s) => s.projects)

  useEffect(() => {
    let cancelled = false
    void fetchViewerRole().then((role) => {
      if (!cancelled) setViewerRole(role)
    })
    return () => {
      cancelled = true
    }
  }, [])

  const sourcePath = useMemo(
    () => projects.find((p) => p.id === projectId)?.path ?? '',
    [projects, projectId],
  )

  const fetchRelations = useCallback(async () => {
    try {
      const [outgoing, inc, remote] = await Promise.all([
        daemonCliGet<WorkspaceRelation[]>(primaryScope(), 'relations/list', { project_id: projectId }),
        daemonCliGet<WorkspaceRelation[]>(primaryScope(), 'relations/list-incoming', { project_id: projectId }),
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

  const connectedIds = useMemo(() => new Set(relations.map((r) => r.targetProjectId)), [relations])

  // Agents already linked as federated connections (by agent::host)
  const linkedRemoteAddrs = useMemo(
    () => new Set(remoteConns.map((r) => r.address.toLowerCase())),
    [remoteConns],
  )

  // The picker stays open after an add (like Home's); it shows errors in
  // its own footer, so these throw instead of setting the panel errors.
  const handleAddLocal = useCallback(
    async (targetProjectId: string) => {
      setLocalError(null)
      await daemonCliPost(primaryScope(), 'relations/create', {
        source_project_id: projectId,
        target_project_id: targetProjectId,
      })
      await fetchRelations()
    },
    [projectId, fetchRelations],
  )

  const handleRemoveLocal = useCallback(
    async (id: string) => {
      setLocalError(null)
      try {
        await daemonCliPost(primaryScope(), 'relations/delete', { id })
        await fetchRelations()
      } catch (e) {
        setLocalError(e instanceof Error ? e.message : String(e))
      }
    },
    [fetchRelations],
  )

  const handleAddFederated = useCallback(
    async (target: string): Promise<string | null> => {
      if (!sourcePath) {
        throw new Error('K2 can’t find this workspace on this server. Switch to the server it’s on and try again.')
      }
      setRemoteError(null)
      const { reverseWarning } = await addRemoteConnection(sourcePath, target)
      await fetchRelations()
      return reverseWarning
    },
    [sourcePath, fetchRelations],
  )

  const handleRemoveRemote = useCallback(
    async (address: string) => {
      if (!sourcePath) {
        setRemoteError(
          'K2 can’t find this workspace on this server. Switch to the server it’s on and try again.',
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
        setRemoteError(msg || 'Couldn’t remove the connection. Try again.')
      }
    },
    [sourcePath, fetchRelations],
  )

  const handleReAddGone = useCallback(
    async (address: string) => {
      if (!sourcePath) {
        setRemoteError(
          'K2 can’t find this workspace on this server. Switch to the server it’s on and try again.',
        )
        return
      }
      setReAdding(address)
      setRemoteError(null)
      try {
        const { reverseWarning } = await reAddGoneRemoteConnection(sourcePath, address)
        await fetchRelations()
        if (reverseWarning) setRemoteError(reverseWarning)
      } catch (e) {
        setRemoteError(e instanceof Error ? e.message : String(e))
      } finally {
        setReAdding(null)
      }
    },
    [sourcePath, fetchRelations],
  )

  const projectsById = useMemo(() => {
    const map = new Map<string, (typeof projects)[number]>()
    for (const p of projects) map.set(p.id, p)
    return map
  }, [projects])

  return (
    <>
      <SettingsGroup
        title="On this server"
        card
        hint="Agents this agent can message: on this server and on paired servers."
        action={
          <Button
            ref={addButtonRef}
            variant="accent"
            title="Add connection"
            aria-expanded={showAdd}
            onClick={() => {
              setShowAdd(!showAdd)
              setLocalError(null)
              setRemoteError(null)
            }}
          >
            + Add connection
          </Button>
        }
      >
        {showAdd && (
          <AddConnectionPicker
            projectId={projectId}
            connectedLocalIds={connectedIds}
            connectedRemoteAddrs={linkedRemoteAddrs}
            onAddLocal={handleAddLocal}
            onAddRemote={handleAddFederated}
            onClose={() => setShowAdd(false)}
            anchorRef={addButtonRef}
          />
        )}
        {loading ? (
          <p className="px-4 py-3 type-row text-[var(--color-text-secondary)]">Loading…</p>
        ) : relations.length === 0 ? (
          <p className="px-4 py-3 type-row text-[var(--color-text-secondary)]">No connections on this server yet.</p>
        ) : (
          relations.map((rel) => {
            const target = projectsById.get(rel.targetProjectId)
            return (
              <div key={rel.id} className="flex items-center gap-3 px-4 py-2.5">
                <span
                  className="w-2.5 h-2.5 flex-shrink-0 rounded-[var(--radius-badge)]"
                  style={{ backgroundColor: target?.color || 'var(--color-neutral)' }}
                  aria-hidden="true"
                />
                <span className="type-row text-[var(--color-text-primary)] flex-1 truncate">
                  {target?.name || 'Unknown workspace'}
                </span>
                <Button variant="quiet" title="Remove connection" onClick={() => void handleRemoveLocal(rel.id)}>
                  Remove
                </Button>
              </div>
            )
          })
        )}
        {localError && (
          <div className="px-4 py-3">
            <Callout tone="error">{localError}</Callout>
          </div>
        )}
      </SettingsGroup>

      {!loading && incoming.length > 0 && (
        <SettingsGroup title="Connected to this workspace" card>
          {incoming.map((rel) => {
            const source = projectsById.get(rel.sourceProjectId)
            return (
              <div key={rel.id} className="flex items-center gap-3 px-4 py-2.5">
                <span
                  className="w-2.5 h-2.5 flex-shrink-0 rounded-[var(--radius-badge)]"
                  style={{ backgroundColor: source?.color || 'var(--color-neutral)' }}
                  aria-hidden="true"
                />
                <span className="type-row text-[var(--color-text-primary)] flex-1 truncate">
                  {source?.name || 'Unknown workspace'}
                </span>
              </div>
            )
          })}
        </SettingsGroup>
      )}

      <SettingsGroup title="On paired servers" card>
        {loading ? (
          <p className="px-4 py-3 type-row text-[var(--color-text-secondary)]">Loading…</p>
        ) : remoteConns.length === 0 ? (
          <p className="px-4 py-3 type-row text-[var(--color-text-secondary)]">No connections on paired servers yet.</p>
        ) : (
          remoteConns.map((rc) => (
            <FederatedConnectionRow
              key={rc.address}
              entry={rc}
              canReAdd={canReAddGoneConnection(viewerRole)}
              busy={reAdding === rc.address}
              onRemove={() => void handleRemoveRemote(rc.address)}
              onReAdd={() => void handleReAddGone(rc.address)}
            />
          ))
        )}
        {remoteError && (
          <div className="px-4 py-3">
            <Callout tone="error">{remoteError}</Callout>
          </div>
        )}
      </SettingsGroup>
    </>
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
      setAgents(asArray<K2soAgentInfo>(result))
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
          <h3 className="type-section text-[var(--color-accent)] uppercase tracking-wider mb-1">
            Workspace Manager
          </h3>
          <div className="border border-[var(--color-accent)]/30">
            <div className="px-3 py-2 space-y-2">
              <div className="flex items-center">
                <span className="type-name text-[var(--color-text-primary)] flex-shrink-0">{manager.name}</span>
                <span className="type-hint text-[var(--color-accent)] bg-[var(--color-accent)]/10 px-1.5 py-0.5 ml-1.5 flex-shrink-0">
                  MANAGER
                </span>
                {/* Phase 2.5b followup: only the workspace inbox count
                    survives. `delegated` / `done` came from the
                    per-agent `.k2so/agents/<name>/work/{active,done}/`
                    surface — that's gone under the workspace==agent
                    invariant + Phase 2.5b skill consolidation. Work
                    now lives on the workspace inbox primitive, not
                    per-agent work queues. */}
                <div className="flex items-center justify-end gap-1.5 type-hint flex-1 ml-2">
                  {wsInboxCount > 0 && (
                    <span className="text-[var(--color-accent)]" title="Items in workspace inbox">{wsInboxCount} inbox</span>
                  )}
                </div>
              </div>
              <p className="type-hint text-[var(--color-text-muted)] truncate">{manager.role}</p>
              {/* 0.37.4: friendly display label, separate from the
                  internal agent identity (directory basename / AGENT.md
                  `name:`) so the inbox tab can show something more
                  human. Pre-0.39.0f Phase 2.1 the manager identity was
                  the `__lead__` sentinel — removed; the identity is now
                  the workspace's primary agent name. Manage Persona
                  rides as the trailing slot so input | save | persona
                  all sit on one row. */}
              <AgentDisplayNameField
                scope={primaryScope()}
                projectPath={projectPath}
                helpText="Shown on the inbox tab — what you call this manager. The internal agent identity (folder basename) is unchanged."
                trailing={
                  <button
                    onClick={() => onOpenEditor(manager.name)}
                    className="px-3 py-1.5 type-control text-[var(--color-accent)] bg-[var(--color-accent)]/10 hover:bg-[var(--color-accent)]/20 border border-[var(--color-accent)]/30 transition-colors no-drag cursor-pointer flex-shrink-0 self-end whitespace-nowrap"
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
  const [loadError, setLoadError] = useState(false)
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
      setSkills(asArray<SkillSummary>(result))
      setLoadError(false)
    } catch (err) {
      console.error('[skills] list failed:', err)
      setSkills([])
      setLoadError(true)
    } finally {
      setLoading(false)
    }
  }, [projectPath])

  const fetchAgents = useCallback(async () => {
    try {
      const result = await invoke<K2soAgentInfo[]>('k2so_agents_list', { projectPath })
      setAgents(asArray<K2soAgentInfo>(result))
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
      await daemonCliPost(primaryScope(), 'skills/create', {
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
      message: 'The skill folder (.k2/skills/' + name + '/) will be moved to the recycle bin. You can restore it within ~30 days if you change your mind.',
      confirmLabel: 'Remove',
      destructive: true,
    })
    if (!confirmed) return
    try {
      await daemonCliPost(primaryScope(), 'skills/remove', { project_path: projectPath, name })
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
    <div className="flex items-center justify-between gap-3 px-4 py-2.5">
      <div className="flex-1 min-w-0">
        <div className="type-name text-[var(--color-text-primary)] truncate">{skill.name}</div>
        {skill.title && skill.title !== skill.name && (
          <p className="type-hint text-[var(--color-text-secondary)] truncate mt-0.5">{skill.title}</p>
        )}
      </div>
      <div className="flex items-center gap-2 flex-shrink-0">
        <Button variant="quiet" onClick={() => onOpenEditor(skill.name)} title={`Edit .k2/skills/${skill.name}/SKILL.md`}>
          Edit
        </Button>
        <Button variant="quiet" onClick={() => handleDelete(skill.name)} title="Remove skill (recoverable via recycle bin)">
          Remove
        </Button>
      </div>
    </div>
  )

  return (
    <SettingsGroup
      title="Skills"
      card
      settingId="projects.agents"
      badge={skillRows.length > 0 ? <span className="type-badge tabular-nums text-[var(--color-text-secondary)]">{skillRows.length}</span> : undefined}
      action={
        <Button variant="quiet" onClick={() => setShowCreate(!showCreate)}>
          {showCreate ? 'Cancel' : '+ New skill'}
        </Button>
      }
    >
      {/* Create form — A19 "any skill can seed any other": the
          optional seed dropdown lets the user start a new skill from
          an existing one's SKILL.md body. Defaults to blank scaffold
          when no seed is picked. */}
      {showCreate && (
        <div className="px-4 py-3 space-y-2">
          <Input
            ref={nameInputRef}
            placeholder="Skill name (e.g. backend-eng)"
            aria-label="Skill name"
            value={newName}
            onChange={(e) => setNewName(e.target.value)}
            className="w-full type-control"
            onKeyDown={(e) => e.key === 'Enter' && handleCreate()}
          />
          {skills.length > 0 && (
            <SettingDropdown
              fullWidth
              ariaLabel="Seed skill"
              value={newSeed}
              onChange={setNewSeed}
              options={[
                { value: '', label: 'Blank skill (no seed)' },
                ...skills.map((s) => ({ value: s.name, label: `Seed from: ${s.name}` })),
              ]}
            />
          )}
          <Button variant="accent" onClick={handleCreate} disabled={creating || !newName.trim()}>
            {creating ? 'Creating...' : 'Create skill'}
          </Button>
        </div>
      )}

      {/* Skill list. Counts dropped from per-row display — the
          inbox/active/done numbers came from the legacy multi-agent
          work surface and don't map onto the post-Phase-2.5b skills
          primitive (a skill is a doc profile, not a work queue). */}
      {loading ? (
        <p className="px-4 py-3 type-row text-[var(--color-text-secondary)]" data-testid="skills-loading">Loading skills…</p>
      ) : loadError ? (
        <div className="px-4 py-3 flex items-center justify-between gap-4" data-testid="skills-load-error">
          <div className="min-w-0">
            <div className="type-row text-[var(--color-text-primary)]">Couldn&rsquo;t load this workspace&rsquo;s skills</div>
            <div className={SETTING_ROW_HINT}>K2 couldn&rsquo;t read the skills folder. Try again in a moment.</div>
          </div>
          <Button variant="quiet" onClick={() => { setLoading(true); void fetchSkills() }}>Try again</Button>
        </div>
      ) : skillRows.length === 0 && !showCreate ? (
        <p className="px-4 py-3 type-row text-[var(--color-text-secondary)]">
          No skills yet. Create one to give your agent a specialized capability profile.
        </p>
      ) : (
        skillRows.map((skill) => <SkillListItem key={skill.name} skill={skill} />)
      )}
    </SettingsGroup>
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
      const result = await daemonCliGet<CursorIdeSession[]>(primaryScope(), 'chat/discover-ide', { project_path: projectPath })
      setSessions(asArray<CursorIdeSession>(result))
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
        const r = await daemonCliPost<{ migrated: number }>(primaryScope(), 'chat/migrate-ide', {
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

  if (loading) return <p className="px-4 py-3 type-row text-[var(--color-text-secondary)]">Looking for Cursor conversations…</p>
  if (sessions.length === 0) return <p className="px-4 py-3 type-row text-[var(--color-text-secondary)]">No Cursor IDE conversations found for this workspace.</p>

  return (
    <div className="px-4 py-3 space-y-3">
      <div>
        <div className={SETTING_ROW_LABEL}>
          Cursor IDE conversations{' '}
          <span className="type-badge tabular-nums text-[var(--color-text-secondary)]">{sessions.length}</span>
        </div>
        <div className={SETTING_ROW_HINT}>
          Migrate conversations from the Cursor IDE to CLI format so they can be resumed in K2 terminals.
        </div>
      </div>

      <div className="max-h-[250px] overflow-y-auto divide-y divide-[var(--color-border)] border-y border-[var(--color-border)]">
        {sessions.map((session) => {
          const isMigrated = session.alreadyMigrated || justMigratedIds.has(session.composerId)
          const isCurrentlyMigrating = migratingIds.has(session.composerId)
          const date = new Date(session.lastUpdatedAt || session.createdAt)
          const dateStr = date.toLocaleDateString('en-US', { month: 'short', day: 'numeric' })

          return (
            <div key={session.composerId} className="flex items-center gap-2 py-1.5 type-row">
              <AgentIcon agent="Cursor Agent" size={12} />
              <span className={`flex-1 truncate ${isMigrated ? 'text-[var(--color-text-secondary)]' : 'text-[var(--color-text-primary)]'}`}>
                {session.name || 'Untitled'}
              </span>
              <span className="type-hint text-[var(--color-text-secondary)] flex-shrink-0">{dateStr}</span>
              {isCurrentlyMigrating ? (
                <span className="type-hint text-[var(--color-accent)] flex-shrink-0 animate-pulse">migrating...</span>
              ) : isMigrated ? (
                <span className="type-hint text-[var(--color-status-ok-soft)] flex-shrink-0">migrated</span>
              ) : !session.migratable ? (
                <span className="type-hint text-[var(--color-text-secondary)] flex-shrink-0">chat only</span>
              ) : (
                <span className="type-hint text-[var(--color-text-secondary)] flex-shrink-0">pending</span>
              )}
            </div>
          )
        })}
      </div>

      {error && <Callout tone="error">{error}</Callout>}

      <div className="flex items-center justify-between gap-3">
        <span className="type-hint text-[var(--color-text-secondary)]">
          {migratedSessions.length > 0 && (
            <span className="text-[var(--color-status-ok-soft)]">{migratedSessions.length} migrated</span>
          )}
          {migratedSessions.length > 0 && unmigratedSessions.length > 0 && ' · '}
          {unmigratedSessions.length > 0 && <span>{unmigratedSessions.length} pending</span>}
          {nonMigratableSessions.length > 0 && <span> · {nonMigratableSessions.length} chat-only</span>}
          {migratedSessions.length > 0 && unmigratedSessions.length === 0 && nonMigratableSessions.length === 0 && (
            <span> — all conversations available in Chat History</span>
          )}
        </span>
        {unmigratedSessions.length > 0 && (
          <Button variant="accent" onClick={handleMigrateAll} disabled={migrating}>
            {migrating ? `Migrating ${migratingIds.size}...` : `Migrate ${unmigratedSessions.length}`}
          </Button>
        )}
      </div>
    </div>
  )
}

// Per-workspace agent-send mode (`projects.mail_agent_send`): may this
// workspace's agents SEND email, from hosted AND linked inboxes. Every OS
// (linked sends run on macOS too). The daemon owns the value: read via
// GET /cli/mail/config?workspace=, written via POST /cli/mail/config/set
// `{workspace, agentSend}` (Admin floor; the agent's own session is
// refused server-side). Same switch as `k2 mail sending <ws> <mode>`.
// Not on the projects row, so no fetchProjects() here.
type MailAgentSendMode = 'off' | 'approval' | 'on'

function MailAgentSendControl({
  project,
}: {
  project: ProjectWithWorkspaces
}): React.JSX.Element {
  const [mode, setMode] = useState<MailAgentSendMode | null>(null)
  const [inherited, setInherited] = useState(false)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)

  useEffect(() => {
    let cancelled = false
    daemonCliGet<{
      workspace?: { agentSend?: { effective?: string; override?: string | null } }
    }>(primaryScope(), 'mail/config', { workspace: project.path })
      .then((d) => {
        if (cancelled) return
        const eff = d.workspace?.agentSend?.effective
        setMode(eff === 'on' || eff === 'approval' ? eff : 'off')
        setInherited(d.workspace?.agentSend?.override == null)
      })
      .catch((err) => {
        if (!cancelled) setError(err instanceof Error ? err.message : String(err))
      })
    return () => {
      cancelled = true
    }
  }, [project.path])

  const change = async (next: string): Promise<void> => {
    if (busy || (next !== 'off' && next !== 'approval' && next !== 'on')) return
    const before = mode
    const beforeInherited = inherited
    setBusy(true)
    setError(null)
    setMode(next)
    setInherited(false)
    try {
      await daemonCliPost(primaryScope(), 'mail/config/set', {
        workspace: project.path,
        agentSend: next,
      })
    } catch (err) {
      setMode(before)
      setInherited(beforeInherited)
      setError(err instanceof Error ? err.message : String(err))
    } finally {
      setBusy(false)
    }
  }

  return (
    <div data-settings-id="projects.mail-agent-send">
    <SettingRow
      label={<>Let this agent send email<AskedChip row="projects.mail-agent-send" /></>}
      hint={
        <>
          Covers hosted K2 mailboxes and linked inboxes. Approval queues hosted mailboxes only; a linked inbox needs On.
          {inherited && mode !== null ? ' Using the server default.' : ''}
        </>
      }
      field
      below={
        <>
          <Callout tone="warn" data-testid="mail-agent-send-on-warning" className="type-hint leading-relaxed">
            With sending on, an agent that reads a malicious email can be talked into sending any mail it can read, with its attachments, and any workspace file that doesn&apos;t look like a secret, to any address: up to 20 messages an hour and 100 a day per mailbox, at 25 MiB each. Nobody is asked or told at send time; the outbox records it afterwards. The secret check catches mistakes, not intent. The DMARC check only slows forged senders, and an attacker&apos;s own domain passes it. Use approval for any agent that reads mail from outside.
          </Callout>
          {error && <div className="type-hint text-[var(--color-status-error-soft)] mt-2">{error}</div>}
        </>
      }
    >
      <SettingDropdown
        fullWidth
        ariaLabel="Agent email sending"
        value={mode ?? ''}
        placeholder="…"
        disabled={busy || mode === null}
        onChange={(v) => void change(v)}
        options={[
          { value: 'off', label: 'Off' },
          { value: 'approval', label: 'Approval' },
          { value: 'on', label: 'On' },
        ]}
      />
    </SettingRow>
    </div>
  )
}
