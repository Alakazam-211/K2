// The workspace's pinned Chat item: the agent whose Thread a Design tab and
// a Mobile tab show in their left column (prd-browser-design-mode-v1,
// prd-mobile-dev-v1 M-A1).

import type { AgentItemData, Tab } from '@/stores/tabs'

/** The workspace's pinned Chat (the Chat system tab's agent item). */
export function pinnedChatItem(tabs: Tab[]): AgentItemData | null {
  for (const tab of tabs) {
    if (!tab.isSystemAgent) continue
    const item = Array.from(tab.paneGroups.values())[0]?.items[0]
    if (item?.type !== 'agent') continue
    const d = item.data as AgentItemData
    if (d.section === 'chat') return d
  }
  return null
}
