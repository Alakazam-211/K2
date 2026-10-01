// "An agent is still working — close anyway?" for a tab or pane (Home M5).
//
// The window's own room asks the window's active-agents store, as before. A
// Home room on another server has its own activity (`room.activityView`: the
// panes' title feed merged with that server's `session_activity_changed`);
// the window's store never sees its terminals, so asking it would close a
// working agent on B with no prompt.

import { mergePaneStatus, useActiveAgentsStore, type ActiveAgent } from '@/stores/active-agents'
import type { Room } from '@/stores/room'
import type { Tab, TerminalItemData } from '@/stores/tabs'

function isBusy(room: Room, terminalId: string): ReturnType<typeof mergePaneStatus> | null {
  const v = room.activityView.getState()
  const status = mergePaneStatus(v.paneStatuses.get(terminalId), v.daemonPaneStatuses.get(terminalId))
  return status === 'working' || status === 'permission' ? status : null
}

function asAgent(tab: Pick<Tab, 'id' | 'title'>, groupIndex: number, data: TerminalItemData, status: ActiveAgent['hookStatus']): ActiveAgent {
  return {
    terminalId: data.terminalId,
    command: data.commandHint ?? data.command ?? 'agent',
    tabId: tab.id,
    tabTitle: tab.title,
    groupIndex,
    status: 'active',
    hookStatus: status,
  }
}

/** Agents still working in `tabId` (column `groupIndex`) of `room`. */
export function workingAgentsInTab(room: Room, tabId: string, groupIndex: number): ActiveAgent[] {
  if (room.isPrimary) return useActiveAgentsStore.getState().getAgentsInTab(tabId)
  const st = room.tabs.getState()
  const column = groupIndex === 0 ? st.tabs : st.extraGroups[groupIndex - 1]?.tabs ?? []
  const tab = column.find((t) => t.id === tabId)
  if (!tab) return []
  const out: ActiveAgent[] = []
  for (const pg of tab.paneGroups.values()) {
    for (const item of pg.items) {
      if (item.type !== 'terminal') continue
      const data = item.data as TerminalItemData
      const status = isBusy(room, data.terminalId)
      if (status) out.push(asAgent(tab, groupIndex, data, status))
    }
  }
  return out
}

/** The agent still working in one terminal pane of `room`, or null. */
export function workingAgentInPane(room: Room, tab: Pick<Tab, 'id' | 'title'>, data: TerminalItemData): ActiveAgent | null {
  if (room.isPrimary) return useActiveAgentsStore.getState().agents.get(data.terminalId) ?? null
  const status = isBusy(room, data.terminalId)
  return status ? asAgent(tab, 0, data, status) : null
}
