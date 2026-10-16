// THE SWITCH for the drawer-handles experiment (0.46.0 UI): pages whose
// room shell shows edge handles (`DrawerHandle.tsx`) instead of the top
// bar's drawer buttons. Agents only while Rosson tries it; add 'home'
// (same shell) to extend it, or empty the set to bring the top bar
// buttons back.

import type { AppPage } from '@/stores/page-view'

export const DRAWER_HANDLE_PAGES: ReadonlySet<AppPage> = new Set<AppPage>(['agents'])

export function drawerHandlesOn(page: AppPage): boolean {
  return DRAWER_HANDLE_PAGES.has(page)
}
