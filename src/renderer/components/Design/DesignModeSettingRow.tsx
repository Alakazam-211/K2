// Workspace settings → Tabs → "Show the Design tab" (once the "Design mode"
// switch, late decision Q-F2; renamed 2026-10-29 when it stopped hiding
// + → Design). The row itself is the shared PinnedTabSettingRow; this keeps
// the Design module's old names. The caller passes the server it writes to
// (MS79: no `primaryScope()` here).

import type { ServerScope } from '@/kessel/server-scope'
import { PinnedTabSettingRow, patchWindowProjects } from '@/components/Settings/PinnedTabSettingRows'
import { writePinnedTab } from '@/components/Settings/pinned-tab-settings'

export type DesignModeProject = { id: string; path: string; designModeEnabled?: number }

export async function writeDesignMode(scope: ServerScope, project: DesignModeProject, next: boolean): Promise<void> {
  // The window's projects store mirrors only the window's server.
  await writePinnedTab(scope, project, 'design', next, scope.isPrimary ? patchWindowProjects : null)
}

export function DesignModeSettingRow({
  project,
  scope,
}: {
  project: DesignModeProject
  /** The server the workspace lives on. */
  scope: ServerScope
}): React.JSX.Element {
  return <PinnedTabSettingRow kind="design" project={project} scope={scope} />
}
