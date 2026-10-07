// Sidebar right-click → "Rename agent…" (0.45.0).
//
// Edits the agent's DISPLAY NAME only (what people see). The handle — the
// address other agents, Thread and `k2 msg` use — is never touched here, so
// connections and messages keep working. The handle changes only in
// Workspace Settings → Agent → Handle (which warns about federation).
//
// Daemon-first: the save goes to `POST /cli/workspace/set-agent-display-name`
// (route floor: Member). The daemon rewrites AGENT.md `display_name:` +
// `projects.name`, relabels the live canonical session, and emits
// SyncProjects → ProjectsChanged so every other client refetches. This
// client paints optimistically (projects store `renameAgentDisplayName`).

import { create } from 'zustand'
import type { ContextMenuItemDef } from './context-menu'
import { scopeMayWrite, type ServerScope } from '@/kessel/server-scope'

/** The daemon route the dialog writes through. */
export const RENAME_AGENT_ROUTE = 'workspace/set-agent-display-name'

/** Context-menu id + label for the sidebar row. */
export const RENAME_AGENT_MENU_ID = 'rename'
export const RENAME_AGENT_MENU_LABEL = 'Rename agent…'

/** The sidebar's Rename row. Disabled (never badged) when this window's
 *  server would refuse the write (view-only or a remote room without the
 *  route). Every Connect role (Owner/Admin/Member) meets the route's
 *  Member floor, so the scope check is the whole gate. */
export function renameAgentMenuItem(scope: ServerScope): ContextMenuItemDef {
  return {
    id: RENAME_AGENT_MENU_ID,
    label: RENAME_AGENT_MENU_LABEL,
    enabled: scopeMayWrite(scope, RENAME_AGENT_ROUTE),
  }
}

/** Trim, then mirror k2-core `validate_display_name`: not empty, ≤ 64
 *  chars, no `/` or `:`, no control characters. */
export function validateAgentDisplayName(raw: string): { name: string } | { error: string } {
  const name = raw.trim()
  if (name.length === 0) return { error: 'Name must not be empty.' }
  if (name.length > 64) return { error: 'Name must be at most 64 characters.' }
  if (name.includes('/')) return { error: "Name must not contain '/'." }
  if (name.includes(':')) return { error: "Name must not contain ':' (addresses use handle::host)." }
  // eslint-disable-next-line no-control-regex
  if (/[\u0000-\u001f\u007f-\u009f]/.test(name)) return { error: 'Name must not contain control characters.' }
  return { name }
}

interface RenameAgentDialogState {
  isOpen: boolean
  projectId: string | null
  projectPath: string | null
  currentName: string
  handle: string
  open: (args: { projectId: string; projectPath: string; currentName: string; handle: string }) => void
  close: () => void
}

export const useRenameAgentDialogStore = create<RenameAgentDialogState>((set) => ({
  isOpen: false,
  projectId: null,
  projectPath: null,
  currentName: '',
  handle: '',
  open: ({ projectId, projectPath, currentName, handle }) =>
    set({ isOpen: true, projectId, projectPath, currentName, handle }),
  close: () =>
    set({ isOpen: false, projectId: null, projectPath: null, currentName: '', handle: '' }),
}))
