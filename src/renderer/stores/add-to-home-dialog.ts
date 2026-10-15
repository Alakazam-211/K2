// Agents sidebar right-click → "Add to Home", when this device has more
// than one Home: the pick-a-Home dialog's open state. The agent and its
// server's host key are captured at the click (lib/add-to-home.ts), so the
// row keeps the right host even if the window switches servers meanwhile.

import { create } from 'zustand'
import type { ContextMenuItemDef } from './context-menu'
import type { ListedWorkspace } from '@/lib/add-to-home'

/** Context-menu id + label for the sidebar row (directly under Pin/Unpin). */
export const ADD_TO_HOME_MENU_ID = 'add-to-home'
export const ADD_TO_HOME_MENU_LABEL = 'Add to Home'

export function addToHomeMenuItem(): ContextMenuItemDef {
  return { id: ADD_TO_HOME_MENU_ID, label: ADD_TO_HOME_MENU_LABEL }
}

interface AddToHomeDialogState {
  isOpen: boolean
  hostKey: string | null
  workspace: ListedWorkspace | null
  open: (args: { hostKey: string; workspace: ListedWorkspace }) => void
  close: () => void
}

export const useAddToHomeDialogStore = create<AddToHomeDialogState>((set) => ({
  isOpen: false,
  hostKey: null,
  workspace: null,
  open: ({ hostKey, workspace }) => set({ isOpen: true, hostKey, workspace }),
  close: () => set({ isOpen: false, hostKey: null, workspace: null }),
}))
