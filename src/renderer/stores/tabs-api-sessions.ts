// API-session tab matching for the tabs store (moved out of tabs.ts): the
// de-dupe and reaper checks for API-spawned and host-session cells.

import type { Tab, TerminalItemData, WorkspaceTabSnapshot } from './tabs'

/** True when a terminal item attached to (or spawned as) `agentName`, or
 *  carrying `sessionId`, is ALREADY surfaced in any tab across all groups.
 *  The de-dupe guard for API-spawned adoption: the window that already adopted
 *  this cell (or a re-delivered event) must NOT create a second tab. Mirrors
 *  the AgentChatPane remount-guard's identity check (session_id) + the
 *  `isPaneGroupSurfaced` "already represented" check, matched on the attach
 *  agent_name since the API cell has no `tab-`-shaped paneGroupId. */
export function tabMatchesApiSession(tab: Tab, agentName: string, sessionId: string): boolean {
  for (const pg of tab.paneGroups.values()) {
    for (const item of pg.items) {
      if (item.type !== 'terminal') continue
      const d = item.data as TerminalItemData
      if (d.attachAgentName === agentName) return true
      if (sessionId && d.sessionId === sessionId) return true
    }
  }
  return false
}

export function isApiSandboxSessionSurfaced(
  state: {
    tabs: Tab[]
    extraGroups: Array<{ tabs: Tab[]; activeTabId: string | null }>
    backgroundWorkspaces?: Record<string, WorkspaceTabSnapshot>
  },
  agentName: string,
  sessionId: string,
): boolean {
  for (const tab of state.tabs) {
    if (tabMatchesApiSession(tab, agentName, sessionId)) return true
  }
  for (const group of state.extraGroups) {
    for (const tab of group.tabs) {
      if (tabMatchesApiSession(tab, agentName, sessionId)) return true
    }
  }
  // Also de-dupe against parked (non-active workspace) strips — host-session
  // adopt may place cells there so the focused workspace is not polluted.
  if (state.backgroundWorkspaces) {
    for (const snap of Object.values(state.backgroundWorkspaces)) {
      for (const tab of snap.tabs) {
        if (tabMatchesApiSession(tab, agentName, sessionId)) return true
      }
      for (const group of snap.extraGroups) {
        for (const tab of group.tabs) {
          if (tabMatchesApiSession(tab, agentName, sessionId)) return true
        }
      }
    }
  }
  return false
}

/** True when removing the tab is safe under the API-session reaper path.
 *  Skip pinned/system tabs and any tab that isn't a pure attach to this
 *  `api-…` agent (splits / agent panes / file viewers keep their own
 *  lifecycle — same invariant as `tabIsDropCandidateForSessionRemoval`). */
export function tabIsDropCandidateForApiSessionRemoval(tab: Tab, agentName: string): boolean {
  if (tab.isSystemAgent) return false
  if (tab.paneGroups.size !== 1) return false
  const pg = [...tab.paneGroups.values()][0]
  if (!pg) return false
  if (pg.items.length === 0) return false
  for (const item of pg.items) {
    if (item.type !== 'terminal') return false
    const d = item.data as TerminalItemData
    if (d.attachAgentName !== agentName) return false
  }
  return true
}
