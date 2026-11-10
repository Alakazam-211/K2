// Settings → Workspaces / Agents: which tab of a workspace holds a setting.
//
// Search and deep links land on a tab, so the tab list and the setting → tab
// map live here, apart from the page, and `searchManifest` can read them
// without importing the section.

export type WorkspaceSettingsTab =
  | 'agent'
  | 'permissions'
  | 'context'
  | 'connections'
  | 'skills'
  | 'schedule'
  | 'mail'
  | 'advanced'

export const WORKSPACE_SETTINGS_TABS: WorkspaceSettingsTab[] = [
  'agent',
  'permissions',
  'context',
  'connections',
  'skills',
  'schedule',
  'mail',
  'advanced',
]

/** Old tab names some links still pass, and the Advanced section each became. */
const ADVANCED_SECTION: Record<string, string> = {
  worktrees: 'projects.worktrees',
  import: 'projects.cursor-migrate',
  api: 'projects.host-session-cell-cap',
}

export function parseWorkspaceSettingsTab(value: string | null): WorkspaceSettingsTab | null {
  if (!value) return null
  if ((WORKSPACE_SETTINGS_TABS as string[]).includes(value)) return value as WorkspaceSettingsTab
  return value in ADVANCED_SECTION ? 'advanced' : null
}

/** The Advanced section a legacy link (`worktrees`, `import`, `api`) lands on. */
export function advancedSectionFor(value: string | null): string | null {
  return value ? (ADVANCED_SECTION[value] ?? null) : null
}

/** Settings shown on the list page, not inside one workspace. */
const LIST_SETTINGS = new Set(['projects.list', 'projects.add', 'projects.focus-groups'])

const PERMISSION_SETTINGS = new Set([
  'projects.agents-can-manage-agents',
  'projects.agents-can-manage-vault',
  'projects.handoff-accept-from',
  'projects.handoff-send-allow',
  'projects.agents-can-use-compute',
  'projects.agents-can-operate-compute',
  'projects.agents-can-manage-compute',
  'projects.agents-can-upgrade-postgres',
  'projects.db-agent-create',
  'projects.agents-can-manage-projects',
  'projects.agents-can-manage-gardens',
  'projects.agents-can-manage-org-chart',
  'projects.agents-can-request-permissions',
  'projects.agents-can-refresh-chat',
  'projects.mail-agent-send',
  'projects.agent-email-notices',
  'projects.mail-manage-enabled',
  'projects.agents-can-manage-skin',
  'projects.agents-can-manage-app-users',
  'projects.allow-remote-instruct',
  'projects.dns-manage-enabled',
])

const OTHER_TABS: Record<string, WorkspaceSettingsTab> = {
  'projects.context-stack': 'context',
  'projects.relations': 'connections',
  'projects.agents-can-create-connections': 'connections',
  'projects.agents': 'skills',
  'projects.heartbeat': 'schedule',
  'projects.worktrees': 'advanced',
  'projects.cursor-migrate': 'advanced',
  'projects.host-session-cell-cap': 'advanced',
}

/** Where a `projects.*` search result lives: on the list page, or on a tab of
 *  a workspace. Everything else per-workspace is on the Agent tab. */
export function workspaceTabForSetting(id: string): WorkspaceSettingsTab | 'list' {
  if (LIST_SETTINGS.has(id)) return 'list'
  if (PERMISSION_SETTINGS.has(id)) return 'permissions'
  return OTHER_TABS[id] ?? 'agent'
}

let lastOpenedProjectId: string | null = null

/** Remembered so a search result opens the workspace you were just in. */
export function rememberOpenedProject(id: string | null): void {
  if (id) lastOpenedProjectId = id
}

export function lastOpenedProject(): string | null {
  return lastOpenedProjectId
}
