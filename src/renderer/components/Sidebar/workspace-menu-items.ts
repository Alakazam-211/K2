// The Agents sidebar's right-click placement block: Pin/Unpin, then
// "Add to Home" directly beneath it, then the Active Bar rows. A pure
// function so the order is testable without rendering the sidebar.

import type { ContextMenuItemDef } from '@/stores/context-menu'
import { addToHomeMenuItem } from '@/stores/add-to-home-dialog'

export function workspacePlacementMenuItems(project: {
  pinned: number
  manuallyActive: number
}): ContextMenuItemDef[] {
  return [
    { id: 'separator-pin', label: '', type: 'separator' },
    { id: 'toggle-pin', label: project.pinned ? 'Unpin' : 'Pin to Top' },
    addToHomeMenuItem(),
    { id: 'toggle-active', label: project.manuallyActive ? 'Remove from Active Bar' : 'Add to Active Bar' },
    ...(!project.manuallyActive ? [{ id: 'active-24h', label: 'Active for 24hrs' }] : []),
  ]
}
