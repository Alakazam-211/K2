// When a Design tab's notes tray goes away (prd-browser-design-mode-v1 C2):
// notes are per viewer and survive the page reloading, a workspace switch
// (the tab is parked in the background snapshot) and a layout re-apply
// (same item id). They are dropped when the tab is CLOSED: the item is in
// none of the room's strips, live or parked.

import type { Tab, WorkspaceTabSnapshot } from '@/stores/tabs'

interface RoomTabsView {
  tabs: Tab[]
  extraGroups: Array<{ tabs: Tab[] }>
  backgroundWorkspaces: Record<string, Pick<WorkspaceTabSnapshot, 'tabs' | 'extraGroups'>>
}

function stripsHoldItem(strips: Tab[][], itemId: string): boolean {
  for (const tabs of strips) {
    for (const tab of tabs) {
      for (const pg of tab.paneGroups.values()) {
        if (pg.items.some((item) => item.id === itemId)) return true
      }
    }
  }
  return false
}

/** True while the room still has the item somewhere (open or parked). */
export function designItemStillOpen(state: RoomTabsView, itemId: string): boolean {
  if (stripsHoldItem([state.tabs, ...state.extraGroups.map((g) => g.tabs)], itemId)) return true
  for (const snap of Object.values(state.backgroundWorkspaces)) {
    if (stripsHoldItem([snap.tabs, ...snap.extraGroups.map((g) => g.tabs)], itemId)) return true
  }
  return false
}
