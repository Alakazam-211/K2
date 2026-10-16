// Drawer toggles (0.46.0 UI), on every page and window with drawers: the
// » / « buttons (`DrawerTabBarToggle.tsx`) replace the top bar's drawer
// buttons. A closed drawer's toggle takes its own slot at the far edge of
// the room-side strip; an open drawer's sits in the drawer's own tab strip
// or header at the end nearest the room.

import { usePanelsStore } from '@/stores/panels'

/** True when the room tab bar should show this side's "show drawer" slot:
 *  that workspace drawer is closed and has tabs to show. */
export function useRoomTabBarDrawerToggle(side: 'left' | 'right'): boolean {
  const open = usePanelsStore((s) => (side === 'left' ? s.leftPanelOpen : s.rightPanelOpen))
  const hasTabs = usePanelsStore((s) => (side === 'left' ? s.leftPanelTabs : s.rightPanelTabs).length > 0)
  return !open && hasTabs
}
