// The nav's add menu (the footer's Add and the rail's +): Create an agent
// (prd-agent-templates-v1 §6.1) beside Add Workspace. Shown under the button
// that opened it, so it lands in the same place from a click or the keyboard.

import { useContextMenuStore, type ContextMenuItemDef } from '@/stores/context-menu'
import { openCreateAgentSheet } from '@/stores/agent-templates'

export const ADD_MENU_CREATE_AGENT = 'add-menu-create-agent'
export const ADD_MENU_ADD_WORKSPACE = 'add-menu-add-workspace'

export const ADD_MENU_LABEL = 'Add'
export const ADD_MENU_TITLE = 'Create an agent or add a workspace'

export const ADD_MENU_ITEMS: ContextMenuItemDef[] = [
  { id: ADD_MENU_CREATE_AGENT, label: 'Create an agent' },
  { id: ADD_MENU_ADD_WORKSPACE, label: 'Add Workspace' },
]

/** Show the menu under `anchor` and run the pick. */
export async function showAddMenu(anchor: HTMLElement | null, addWorkspace: () => void): Promise<void> {
  const r = anchor?.getBoundingClientRect()
  const x = r ? r.left : 0
  const y = r ? r.bottom : 0
  const picked = await useContextMenuStore.getState().show(x, y, ADD_MENU_ITEMS)
  if (picked === ADD_MENU_CREATE_AGENT) openCreateAgentSheet()
  else if (picked === ADD_MENU_ADD_WORKSPACE) addWorkspace()
}
