// Writing "Show the Design tab" / "Show the Mobile tab" from a client (the
// Settings switches and the pinned tab's right-click "Remove from this
// workspace's tabs"). The toast's link to the rows is open-tabs-settings.ts.

import { emit } from '@tauri-apps/api/event'
import type { ServerScope } from '@/kessel/server-scope'
import type { ProjectWithWorkspaces } from '@/stores/projects'
import { setPinnedTab, type PinnedTabKind } from '@/lib/workspace-tab-settings'

/** Patch a projects list in place (the window's store, or a room's). */
export type PatchProjects = (fn: (projects: ProjectWithWorkspaces[]) => ProjectWithWorkspaces[]) => void

/** Write the switch on the workspace's server, then mirror it into the
 *  given projects list right away (the daemon's `projects_changed` refetch
 *  brings every other client along). */
export async function writePinnedTab(
  scope: ServerScope,
  project: { id: string; path: string },
  kind: PinnedTabKind,
  on: boolean,
  patchProjects: PatchProjects | null,
): Promise<void> {
  await setPinnedTab(scope, project.path, kind, on)
  const key = kind === 'design' ? 'designModeEnabled' : 'mobileTabEnabled'
  patchProjects?.((projects) => projects.map((p) => (p.id === project.id ? { ...p, [key]: on ? 1 : 0 } : p)))
  // This computer's other windows (`sync:projects` is this daemon's bus).
  if (scope.isPrimary || !scope.isRemote) void emit('sync:projects').catch(() => {})
}
