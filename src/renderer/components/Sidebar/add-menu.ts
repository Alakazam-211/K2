// The nav's add menu (the footer's Add and the rail's +): Create an agent
// (prd-agent-templates-v1 §6.1) beside Add workspace. The app's shared menu
// (ContextMenu: Glass's menu material, Terminal's square box), each row with
// its icon. It opens under the button that opened it, or above it when the
// button sits at the window's foot (the nav footer), so it never covers the
// button, from a click or the keyboard.

import { AgentsIcon, FolderIcon } from '@/components/icons/K2Icons'
import { useContextMenuStore, type ContextMenuItemDef } from '@/stores/context-menu'
import { openCreateAgentSheet } from '@/stores/agent-templates'

export const ADD_MENU_CREATE_AGENT = 'add-menu-create-agent'
export const ADD_MENU_ADD_WORKSPACE = 'add-menu-add-workspace'

export const ADD_MENU_LABEL = 'Add'
export const ADD_MENU_TITLE = 'Create an agent or add a workspace'

export const ADD_MENU_ITEMS: ContextMenuItemDef[] = [
  { id: ADD_MENU_CREATE_AGENT, label: 'Create an agent', icon: AgentsIcon({ size: 14 }) },
  { id: ADD_MENU_ADD_WORKSPACE, label: 'Add workspace', icon: FolderIcon({ size: 14 }) },
]

/** The menu's height: two 27px rows, 4px padding top and bottom, and its
 *  1px edge (ContextMenu's row and panel). */
export const ADD_MENU_HEIGHT = 2 * 27 + 2 * 4 + 2
/** The gap between the button and the menu. */
const GAP = 4

/** Where the menu's top-left goes for a button at `r` in a window `vh` tall:
 *  under the button, or above it when there's no room below. */
export function addMenuPosition(r: { left: number; top: number; bottom: number }, vh: number): { x: number; y: number } {
  const below = r.bottom + GAP
  if (below + ADD_MENU_HEIGHT <= vh) return { x: r.left, y: below }
  return { x: r.left, y: Math.max(GAP, r.top - GAP - ADD_MENU_HEIGHT) }
}

/** Show the menu at `anchor` and run the pick. */
export async function showAddMenu(anchor: HTMLElement | null, addWorkspace: () => void): Promise<void> {
  const r = anchor?.getBoundingClientRect()
  const vh = typeof window === 'undefined' ? Number.POSITIVE_INFINITY : window.innerHeight
  const { x, y } = r ? addMenuPosition(r, vh) : { x: 0, y: 0 }
  const picked = await useContextMenuStore.getState().show(x, y, ADD_MENU_ITEMS)
  if (picked === ADD_MENU_CREATE_AGENT) openCreateAgentSheet()
  else if (picked === ADD_MENU_ADD_WORKSPACE) addWorkspace()
}
