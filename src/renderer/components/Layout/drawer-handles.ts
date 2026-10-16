// THE SWITCHES for the drawer-controls experiment (0.46.0 UI). Each can be
// turned off on its own.
//
// - DRAWER_HANDLE_PAGES: pages whose room shell shows the edge grips
//   (`DrawerHandle.tsx`) instead of the top bar's drawer buttons. Empty it
//   to bring the top bar buttons back.
// - DRAWER_TABBAR_TOGGLE_PAGES: pages whose tab bars carry the » / «
//   drawer toggles (`DrawerTabBarToggle.tsx`): in the room's tab bar while
//   a drawer is closed, in the drawer's own tab strip while it is open.
//
// Agents only while Rosson tries it; add 'home' (same shell) to extend.

import { createContext, useContext } from 'react'
import type { AppPage } from '@/stores/page-view'
import { usePanelsStore } from '@/stores/panels'

export const DRAWER_HANDLE_PAGES: ReadonlySet<AppPage> = new Set<AppPage>(['agents'])

export function drawerHandlesOn(page: AppPage): boolean {
  return DRAWER_HANDLE_PAGES.has(page)
}

export const DRAWER_TABBAR_TOGGLE_PAGES: ReadonlySet<AppPage> = new Set<AppPage>(['agents'])

export function drawerTabBarTogglesOn(page: AppPage): boolean {
  return DRAWER_TABBAR_TOGGLE_PAGES.has(page)
}

/** Set by the Agents/Home `Layout` only, so Focus windows and any other
 *  shell that reuses TabBar / TabbedPanel never show the tab bar toggles. */
export const DrawerTabBarTogglesContext = createContext(false)

export function useDrawerTabBarToggles(): boolean {
  return useContext(DrawerTabBarTogglesContext)
}

/** True when the room tab bar should show this side's "show drawer" slot:
 *  toggles on, that drawer closed, and it has tabs to show. */
export function useRoomTabBarDrawerToggle(side: 'left' | 'right'): boolean {
  const on = useDrawerTabBarToggles()
  const open = usePanelsStore((s) => (side === 'left' ? s.leftPanelOpen : s.rightPanelOpen))
  const hasTabs = usePanelsStore((s) => (side === 'left' ? s.leftPanelTabs : s.rightPanelTabs).length > 0)
  return on && !open && hasTabs
}
