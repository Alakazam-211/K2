// The right-click menu on an agent: "Workspace Settings" is always the first
// item, with a separator under it. One builder for the
// agent menus (Agents nav, Home nav and its rail) so a new item can't push
// it down. Callers pass the rest of the menu; any settings item or leading
// separator in it is dropped here, so the head stays one item and one line.

import type { ContextMenuItemDef } from '@/stores/context-menu'

export const WORKSPACE_SETTINGS_MENU_LABEL = 'Workspace Settings'

/** `settingsId` is the caller's own id for the item (its click handler's
 *  key). `null` hides it (a viewer who can't open this agent's settings):
 *  the rest is returned as is. */
export function agentMenuWithSettingsFirst(
  settingsId: string | null,
  rest: ContextMenuItemDef[],
): ContextMenuItemDef[] {
  if (settingsId === null) return rest
  const body = rest.filter((i) => i.id !== settingsId)
  while (body.length > 0 && body[0].type === 'separator') body.shift()
  const head: ContextMenuItemDef[] = [{ id: settingsId, label: WORKSPACE_SETTINGS_MENU_LABEL }]
  if (body.length === 0) return head
  return [...head, { id: `${settingsId}-separator`, label: '', type: 'separator' }, ...body]
}
