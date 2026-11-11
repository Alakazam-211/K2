// Open a workspace's Chat tab in the Thread view, on the window's server:
// Kessel right after it's made, and Kessel again when the person starts hiring
// their Chief of Staff (prd-setup-checklist-v1 §10, `kessel-first-run-contract.md`).
//
// The Thread is asked for explicitly (`stores/session-view-request.ts`), so
// this client's "New agents open in" choice (`k2.newAgentView`) doesn't
// apply here. Settings closes, the Agents page selects the workspace, and
// once its tabs show, the pinned Chat tab is made the active tab.

import { usePageViewStore } from '@/stores/page-view'
import { openOnAgentsPage } from '@/lib/page-selection'
import { useProjectsStore } from '@/stores/projects'
import { requestSessionView } from '@/stores/session-view-request'
import { useSettingsStore } from '@/stores/settings'
import { pinnedTabOf, useTabsStore, type AgentItemData, type Tab } from '@/stores/tabs'

/** How long to wait for the workspace's tabs before giving up on picking
 *  the Chat tab (the workspace still opens; the view ask still stands). */
export const CHAT_TAB_WAIT_MS = 5_000

function chatTabOf(tabs: Tab[]): Tab | null {
  const tab = pinnedTabOf(tabs)
  const item = tab ? Array.from(tab.paneGroups.values())[0]?.items[0] : undefined
  return item?.type === 'agent' && (item.data as AgentItemData).section === 'chat' ? tab : null
}

/** Make the Chat tab active once `projectId`'s tabs show, or give up after
 *  `timeoutMs`. Resolves true when it did. */
function focusChatTabWhenShown(projectId: string, timeoutMs: number): Promise<boolean> {
  const prefix = `${projectId}:`
  const tryFocus = (): boolean => {
    const s = useTabsStore.getState()
    if (!s.activeWorkspaceKey?.startsWith(prefix)) return false
    const chat = chatTabOf(s.tabs)
    if (!chat) return false
    if (s.activeTabId !== chat.id) s.setActiveTab(chat.id)
    return true
  }
  if (tryFocus()) return Promise.resolve(true)
  return new Promise((resolve) => {
    let done = false
    const finish = (ok: boolean): void => {
      if (done) return
      done = true
      clearTimeout(timer)
      unsub()
      resolve(ok)
    }
    const timer = setTimeout(() => finish(false), timeoutMs)
    const unsub = useTabsStore.subscribe(() => {
      if (tryFocus()) finish(true)
    })
  })
}

/** Open `projectId` (a `projects.id`) on the Agents page with its Chat tab
 *  in the Thread view. Resolves false when this window's server doesn't
 *  list that workspace even after a fresh read. */
export async function openWorkspaceThread(projectId: string): Promise<boolean> {
  if (!projectId) return false
  requestSessionView(projectId, 'thread')
  useSettingsStore.getState().closeSettings()
  const find = () => useProjectsStore.getState().projects.find((p) => p.id === projectId)
  let project = find()
  if (!project) {
    await useProjectsStore.getState().fetchProjects()
    project = find()
  }
  if (!project) return false
  if (usePageViewStore.getState().page !== 'agents' || useProjectsStore.getState().activeProjectId !== project.id) {
    openOnAgentsPage(project.id, project.workspaces[0]?.id ?? null)
  }
  await focusChatTabWhenShown(project.id, CHAT_TAB_WAIT_MS)
  return true
}
