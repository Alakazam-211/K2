// The workspace's pinned Design and Mobile tabs (Rosson 2026-10-29):
// "Show the Design tab" (`projects.design_mode_enabled`, 0179, default on)
// and "Show the Mobile tab" (`projects.mobile_tab_enabled`, 0250, NULL =
// auto). Both are daemon-owned and written through `POST /cli/workspace/set`;
// the daemon's `projects_changed` brings every client of the room along.
//
// Mobile auto: shown while the server can run a phone, which is the
// daemon's own lane probe as the `mobile-sim-v1` boot-status key. A server
// that can't run a phone never shows a Mobile tab (prd-mobile-dev-v1
// M-A13), so there `on` and `auto` both read as not shown.

import { daemonCliPost } from '@/lib/daemon-cli'
import type { ServerScope } from '@/kessel/server-scope'
import { DESIGN_MODE_FIELD } from '@/components/Design/design-mode-setting'

export const MOBILE_TAB_FIELD = 'mobile_tab_enabled'

/** The Settings row the "Bring it back" toast opens. */
export const TABS_SETTINGS_ID = 'projects.tabs'

export type PinnedTabKind = 'design' | 'mobile'
export type MobileTabSetting = 'auto' | 'on' | 'off'

/** A daemon Project row's `mobileTabEnabled` (null = auto, 0, 1). Missing
 *  (an older daemon) → auto. */
export function mobileTabSetting(project: { mobileTabEnabled?: number | boolean | null } | null | undefined): MobileTabSetting {
  const v = project?.mobileTabEnabled
  if (v === undefined || v === null) return 'auto'
  if (typeof v === 'boolean') return v ? 'on' : 'off'
  return v !== 0 ? 'on' : 'off'
}

/** The tri-state as the room's project index carries it. */
export function mobileTabIndexValue(project: { mobileTabEnabled?: number | boolean | null } | null | undefined): boolean | null {
  const w = mobileTabSetting(project)
  return w === 'auto' ? null : w === 'on'
}

/** Whether the room pins the Mobile tab: never where the server can't run a
 *  phone; otherwise unless the setting is off. Same rule as the daemon's
 *  `mobile_tab_shown`. */
export function isMobileTabShown(setting: MobileTabSetting, serverCanRunPhone: boolean): boolean {
  return serverCanRunPhone && setting !== 'off'
}

/** Flip one of the two switches. A person's flip stores 0 or 1 (never auto). */
export async function setPinnedTab(scope: ServerScope, projectPath: string, kind: PinnedTabKind, on: boolean): Promise<void> {
  const field = kind === 'design' ? DESIGN_MODE_FIELD : MOBILE_TAB_FIELD
  await daemonCliPost(scope, 'workspace/set', {
    project: projectPath,
    fields: { [field]: on ? '1' : '0' },
  })
}
