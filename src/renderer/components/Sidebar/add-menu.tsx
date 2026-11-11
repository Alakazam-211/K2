// The nav's add menu (the footer's Add and the rail's +): Create an agent
// (prd-agent-templates-v1 §6.1) beside Add Workspace, each named as the
// dialog it opens titles itself (the sheet's "Create an agent", "Add
// Workspace to K2"). It is the app's shared row menu (ui/OverflowMenu): the
// chamfered box in Terminal and Frost in Glass, each row with its icon,
// anchored to the button that opened it, flipping above it at the window's
// foot (the nav footer) so it never covers the button. Arrows move, Esc
// closes and gives focus back to the button.

import { OverflowMenu, type OverflowMenuItem, type OverflowMenuTriggerProps } from '@/components/ui'
import { AgentsIcon, FolderIcon } from '@/components/icons/K2Icons'
import { openCreateAgentSheet } from '@/stores/agent-templates'

export const ADD_MENU_CREATE_AGENT = 'add-menu-create-agent'
export const ADD_MENU_ADD_WORKSPACE = 'add-menu-add-workspace'

export const ADD_MENU_LABEL = 'Add'
export const ADD_MENU_TITLE = 'Create an agent or add a workspace'

/** The menu's rows, in order. */
export function addMenuItems(addWorkspace: () => void): OverflowMenuItem[] {
  return [
    { id: ADD_MENU_CREATE_AGENT, label: 'Create an agent', icon: <AgentsIcon size={14} />, onSelect: () => openCreateAgentSheet() },
    { id: ADD_MENU_ADD_WORKSPACE, label: 'Add Workspace', icon: <FolderIcon size={14} />, onSelect: addWorkspace },
  ]
}

/** The menu around the caller's own button (`trigger` gets the ref and the
 *  menu semantics to spread on it). */
export function AddMenu({
  addWorkspace,
  trigger,
}: {
  addWorkspace: () => void
  trigger: (t: OverflowMenuTriggerProps) => React.ReactNode
}): React.JSX.Element {
  return <OverflowMenu items={addMenuItems(addWorkspace)} label={ADD_MENU_TITLE} align="start" renderTrigger={trigger} />
}
