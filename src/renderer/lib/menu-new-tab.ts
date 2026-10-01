import { focusedRoom } from '@/stores/window-room'

/** cwd File-menu actions use: the focused room's workspace (active
 *  workspace, else project path, else '~' in the primary room). Null when
 *  no room is focused — the menu item then does nothing (MS17). */
export function workspaceCwdForMenu(): string | null {
  const room = focusedRoom()
  return room ? room.cwd() : null
}

/** File → New Tab, in the focused room only (MS18). Direct terminal
 *  create. Does not open the tab-bar plus menu. */
export function menuNewTab(): void {
  const room = focusedRoom()
  if (!room) return
  room.tabs.getState().addTab(room.cwd())
}
