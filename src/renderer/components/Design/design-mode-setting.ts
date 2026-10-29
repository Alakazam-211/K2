// Per-workspace "Show the Design tab" setting (late decision Q-F2,
// 2026-10-16, as "Design mode"; renamed 2026-10-29). Daemon-owned:
// `projects.design_mode_enabled` (0/1, default 1 = ON), written through
// `POST /cli/workspace/set`. Off removes the pinned Design tab only; + →
// Design opens an ordinary Design tab either way.

import { daemonCliPost } from '@/lib/daemon-cli'
import type { ServerScope } from '@/kessel/server-scope'

export const DESIGN_MODE_FIELD = 'design_mode_enabled'

/** The daemon Project row (`designModeEnabled: 0|1`) or a
 *  `projects_get_settings` blob (`designModeEnabled: boolean`). Missing
 *  (an older daemon) → ON. */
export function isDesignModeEnabled(project: { designModeEnabled?: number | boolean | null } | null | undefined): boolean {
  const v = project?.designModeEnabled
  if (v === undefined || v === null) return true
  if (typeof v === 'boolean') return v
  return v !== 0
}

export async function setDesignMode(scope: ServerScope, projectPath: string, on: boolean): Promise<void> {
  await daemonCliPost(scope, 'workspace/set', {
    project: projectPath,
    fields: { [DESIGN_MODE_FIELD]: on ? '1' : '0' },
  })
}
