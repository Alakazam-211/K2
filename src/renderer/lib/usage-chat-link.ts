// Usage v2 — an Activity row links to its chat (§2.2 `session_id`). The
// row names the workspace and the conversation; the link selects that
// workspace on the window's server, leaves Settings for the Agents page,
// and focuses the tab that already holds the conversation, or resumes it
// in a new tab the way Chat history does (same resume grammar, preset
// flags in front).

import { PROVIDER_CONFIG } from '@/components/ChatHistory/ContinueNewChatDialog'
import { samePath } from '@/components/Settings/sections/workspaceUsageLabel'
import { findChatSessionInTab } from '@/lib/chat-session-tab'
import { usePageViewStore } from '@/stores/page-view'
import { usePresetsStore } from '@/stores/presets'
import { useProjectsStore } from '@/stores/projects'
import { useSettingsStore } from '@/stores/settings'
import { useTabsStore } from '@/stores/tabs'

export interface UsageChatWorkspace {
  projectId: string
  workspaceId: string
  cwd: string
}

interface ProjectLike {
  id: string
  path: string
  workspaces: ReadonlyArray<{ id: string; worktreePath?: string | null }>
}

/** The window-server workspace a ledger path counts for: a worktree first,
 *  then a project's own path (its first workspace). Null when none. */
export function findUsageWorkspace(projects: readonly ProjectLike[], path: string | null | undefined): UsageChatWorkspace | null {
  if (!path) return null
  for (const project of projects) {
    for (const ws of project.workspaces) {
      if (ws.worktreePath && samePath(ws.worktreePath, path)) {
        return { projectId: project.id, workspaceId: ws.id, cwd: ws.worktreePath }
      }
    }
  }
  for (const project of projects) {
    if (samePath(project.path, path) && project.workspaces[0]) {
      return { projectId: project.id, workspaceId: project.workspaces[0].id, cwd: project.path }
    }
  }
  return null
}

/** True when the row can link to a chat: a known session in a known
 *  workspace, and a harness K2 knows how to resume. */
export function canOpenUsageChat(
  projects: readonly ProjectLike[],
  row: { workspace?: string | null; session_id?: string; harness: string },
): boolean {
  return Boolean(row.session_id) && PROVIDER_CONFIG[row.harness] !== undefined && findUsageWorkspace(projects, row.workspace) !== null
}

const SESSION_FLAGS_TO_STRIP = new Set(['--resume', '-r', '--continue', '-c', '--session'])

function presetArgs(command: string): string[] {
  const preset = usePresetsStore.getState().presets.find((p) => p.command.split(/\s+/)[0] === command && p.enabled)
  if (!preset) return []
  const parts = preset.command.match(/(?:[^\s"']+|"[^"]*"|'[^']*')+/g) ?? []
  return parts
    .map((p: string) => p.replace(/^["']|["']$/g, ''))
    .slice(1)
    .filter((a: string) => !SESSION_FLAGS_TO_STRIP.has(a))
}

/** The argv that resumes `sessionId` in its harness, or null when K2 can't. */
export function usageResumeLaunch(harness: string, sessionId: string): { command: string; args: string[] } | null {
  const config = PROVIDER_CONFIG[harness]
  if (!config) return null
  const front = presetArgs(config.command)
  if (config.resumeSubcommand) return { command: config.command, args: [...front, config.resumeSubcommand, sessionId] }
  return { command: config.command, args: [...front, config.resumeFlag ?? '--resume', sessionId] }
}

function focusOpenChat(sessionId: string): boolean {
  const tabs = useTabsStore.getState()
  const ids = new Set([sessionId])
  const strips = [{ tabs: tabs.tabs, idx: 0 }, ...tabs.extraGroups.map((g, i) => ({ tabs: g.tabs, idx: i + 1 }))]
  for (const { tabs: strip, idx } of strips) {
    for (const tab of strip) {
      if (!findChatSessionInTab(tab, ids)) continue
      if (idx === 0) tabs.setActiveTab(tab.id)
      else tabs.setActiveTabInGroup(idx, tab.id)
      return true
    }
  }
  return false
}

/** Resolve once the tab strip shows `key`'s tabs, or after `timeoutMs`. */
function whenWorkspaceShown(key: string, timeoutMs: number): Promise<void> {
  if (useTabsStore.getState().activeWorkspaceKey === key) return Promise.resolve()
  return new Promise((resolve) => {
    const done = (): void => {
      clearTimeout(timer)
      unsub()
      resolve()
    }
    const timer = setTimeout(done, timeoutMs)
    const unsub = useTabsStore.subscribe((s) => {
      if (s.activeWorkspaceKey === key) done()
    })
  })
}

export type OpenUsageChatResult = 'focused' | 'opened' | 'not-found'

/** Open the chat a ledger row came from (window's server only). */
export async function openUsageChat(row: {
  workspace?: string | null
  session_id?: string
  harness: string
}): Promise<OpenUsageChatResult> {
  const sessionId = row.session_id?.trim()
  if (!sessionId) return 'not-found'
  const ws = findUsageWorkspace(useProjectsStore.getState().projects, row.workspace)
  const launch = usageResumeLaunch(row.harness, sessionId)
  if (!ws || !launch) return 'not-found'

  useSettingsStore.getState().closeSettings()
  usePageViewStore.getState().setPage('agents')
  const key = `${ws.projectId}:${ws.workspaceId}`
  const projects = useProjectsStore.getState()
  if (projects.activeProjectId !== ws.projectId || projects.activeWorkspaceId !== ws.workspaceId) {
    projects.setActiveWorkspace(ws.projectId, ws.workspaceId)
    await whenWorkspaceShown(key, 5_000)
  }
  if (focusOpenChat(sessionId)) return 'focused'
  const tabs = useTabsStore.getState()
  const group = tabs.splitCount > 1 ? tabs.splitCount - 1 : 0
  tabs.addTabToGroup(group, ws.cwd, {
    title: `${PROVIDER_CONFIG[row.harness]?.label ?? row.harness} ${sessionId.slice(0, 8)}`,
    command: launch.command,
    args: launch.args,
    locked: true,
    conversationId: sessionId,
  })
  return 'opened'
}
