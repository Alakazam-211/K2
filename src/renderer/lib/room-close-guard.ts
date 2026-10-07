// "An agent is still working — close anyway?" for a tab or pane (Home M5).
//
// The window's own room asks the window's active-agents store, as before. A
// Home room on another server asks its server's activity rows
// (`room.activityView`, prd-daemon-activity-and-thread-working-v1 S5); the
// window's store never sees its terminals, so asking it would close a
// working agent on B with no prompt.

import { useActiveAgentsStore, type ActiveAgent } from '@/stores/active-agents'
import { isBusyDisplay, terminalDisplay, type ActivityDisplay } from '@/stores/activity'
import type { Room } from '@/stores/room'
import type { Tab, TerminalItemData } from '@/stores/tabs'

/** The daemon's display for the pane's session when it is mid-turn
 *  (working or waiting on the human), else null. */
function isBusy(room: Room, data: TerminalItemData): ActivityDisplay | null {
  const display = terminalDisplay(room.activityView.getState(), data)
  return isBusyDisplay(display) ? display : null
}

function asAgent(tab: Pick<Tab, 'id' | 'title'>, groupIndex: number, data: TerminalItemData, display: ActivityDisplay): ActiveAgent {
  return {
    terminalId: data.terminalId,
    command: data.commandHint ?? data.command ?? 'agent',
    tabId: tab.id,
    tabTitle: tab.title,
    groupIndex,
    status: 'active',
    display,
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
      const display = isBusy(room, data)
      if (display) out.push(asAgent(tab, groupIndex, data, display))
    }
  }
  return out
}

/** The agent still working in one terminal pane of `room`, or null. */
export function workingAgentInPane(room: Room, tab: Pick<Tab, 'id' | 'title'>, data: TerminalItemData): ActiveAgent | null {
  if (room.isPrimary) return useActiveAgentsStore.getState().agents.get(data.terminalId) ?? null
  const display = isBusy(room, data)
  return display ? asAgent(tab, 0, data, display) : null
}
