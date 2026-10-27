// "Mark all as read" on an agent row (thread-read-all-v1, 0.46.5).
//
// One menu row and one action, shared by every agent row that draws an
// unread badge: the agents list (Sidebar), Home's rows on this server and
// Home's rows on another server. The row shows only while the badge has
// unread to clear and the agent's server can do it (`markAllReadSupported`).
// Waiting cards are not unread: they stay until answered.

import type { ContextMenuItemDef } from '@/stores/context-menu'
import { useToastStore } from '@/stores/toast'
import { MARK_ALL_READ_MENU_ID, markAgentAllRead, markAllReadSupported, type MarkAllReadResult } from '@/stores/unread'
import type { ServerScope } from '@/kessel/server-scope'

export { MARK_ALL_READ_MENU_ID }

/** The menu row (plus a separator after it unless `separator: false`), or
 *  nothing. */
export function markAllReadMenuItems(
  scope: ServerScope,
  unread: number,
  opts: { separator?: boolean } = {},
): ContextMenuItemDef[] {
  if (!(unread > 0) || !markAllReadSupported(scope)) return []
  const row: ContextMenuItemDef = { id: MARK_ALL_READ_MENU_ID, label: 'Mark all as read' }
  if (opts.separator === false) return [row]
  return [row, { id: 'separator-mark-all-read', label: '', type: 'separator' }]
}

/** Run it for one agent; a failure is a toast (the row re-reads either way). */
export async function runMarkAllRead(
  scope: ServerScope,
  projectId: string,
  agentLabel: string,
): Promise<MarkAllReadResult | null> {
  try {
    return await markAgentAllRead(scope, projectId)
  } catch (err) {
    useToastStore
      .getState()
      .addToast(`Mark all as read for ${agentLabel} failed: ${err instanceof Error ? err.message : String(err)}`, 'error')
    return null
  }
}
