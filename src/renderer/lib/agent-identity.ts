// Agent rename live update — apply the daemon's `agent_identity_changed`
// (an agent renamed, re-handled, recolored or re-pictured by any client or
// the CLI) to client state without refetching the world.
//
// Before this, the renaming client painted optimistically and agent panes
// listened to the loopback-only Tauri `sync:projects`, so a Connect user on
// web/desktop, or a Home room on another server, kept the old name until it
// left the server and came back. Same pattern as the Files drawer's
// `workspace_resources_changed` / `fs_changed`: the daemon emits on its
// host-aware app bus, every room's scope subscribes.

import type { AgentIdentityChangedEvent } from '@/stores/session-events'

/** Fields this client patches from the event's values. Any other word
 *  (`image`, or one a newer daemon adds) needs a refetch. */
const PATCHED_FIELDS: ReadonlySet<string> = new Set(['name', 'displayName', 'handle', 'color'])

/** True when the event names a field the payload can't patch (the picture,
 *  or a word this client doesn't know): the caller refetches. */
export function agentIdentityNeedsRefetch(e: Pick<AgentIdentityChangedEvent, 'fields'>): boolean {
  return e.fields.some((f) => !PATCHED_FIELDS.has(f))
}

function trimSlash(p: string): string {
  return p.replace(/\\/g, '/').replace(/\/+$/, '')
}

/** Does the event belong to the agent at `projectPath`? */
export function agentIdentityMatchesPath(
  e: Pick<AgentIdentityChangedEvent, 'workspacePath'>,
  projectPath: string | null | undefined,
): boolean {
  if (!projectPath || !e.workspacePath) return false
  return trimSlash(e.workspacePath) === trimSlash(projectPath)
}

/** The name an agent pane shows after the event (AGENT.md display name,
 *  falling back to the sidebar name), or null when the event carries none. */
export function agentIdentityDisplayName(e: Pick<AgentIdentityChangedEvent, 'displayName' | 'name'>): string | null {
  const n = e.displayName.trim() || e.name.trim()
  return n ? n : null
}

interface IdentityRow {
  id: string
  name: string
  color: string
  handle?: string
}

/** Patch the matching row with the daemon's CURRENT name / handle / color.
 *  Returns the SAME array when nothing changed (so React and zustand skip
 *  the render), else a new array where only the matching row is new. */
export function applyAgentIdentity<T extends IdentityRow>(rows: T[], e: AgentIdentityChangedEvent): T[] {
  let changed = false
  const next = rows.map((r) => {
    if (r.id !== e.projectId) return r
    const patch: Partial<IdentityRow> = {}
    if (e.name && e.name !== r.name) patch.name = e.name
    if (e.color && e.color !== r.color) patch.color = e.color
    if (e.handle && e.handle !== r.handle) patch.handle = e.handle
    if (Object.keys(patch).length === 0) return r
    changed = true
    return { ...r, ...patch }
  })
  return changed ? next : rows
}
