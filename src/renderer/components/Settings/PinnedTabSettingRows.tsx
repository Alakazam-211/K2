// Workspace settings → Tabs: "Show the Design tab" and "Show the Mobile tab"
// (Rosson 2026-10-29). Each switch shows what the room does now and stores
// 0 or 1 when flipped. Mobile's default is auto (shown where this server can
// run a phone); a server that can't never shows a Mobile tab, so its switch
// is off and greyed there. The caller passes the server it writes to (MS79).

import { useCallback, useState } from 'react'
import type { ServerScope } from '@/kessel/server-scope'
import { useProjectsStore } from '@/stores/projects'
import { isDesignModeEnabled } from '@/components/Design/design-mode-setting'
import { SettingsGroup } from '@/components/Settings/controls/SettingControls'
import {
  isMobileTabShown,
  mobileTabSetting,
  TABS_SETTINGS_ID,
  type PinnedTabKind,
} from '@/lib/workspace-tab-settings'
import { writePinnedTab, type PatchProjects } from './pinned-tab-settings'
import { Toggle } from '@/components/ui'

export type PinnedTabProject = {
  id: string
  path: string
  designModeEnabled?: number
  mobileTabEnabled?: number | null
}

/** The window's own projects store (the Settings rows write the primary server). */
export const patchWindowProjects: PatchProjects = (fn) =>
  useProjectsStore.setState((s) => ({ projects: fn(s.projects) }))

const COPY: Record<PinnedTabKind, { label: string; on: string; noPhone?: string }> = {
  design: {
    label: 'Show the Design tab',
    on: 'Keeps the Design tab pinned next to Chat and Inbox. You can always open it from the + menu.',
  },
  mobile: {
    label: 'Show the Mobile tab',
    on: "Keeps this workspace's phone pinned next to Chat and Inbox. You can always open it from the + menu.",
    noPhone: "This server can't run a phone, so there is no Mobile tab here.",
  },
}

export function PinnedTabSettingRow({
  kind,
  project,
  scope,
}: {
  kind: PinnedTabKind
  project: PinnedTabProject
  /** The server the workspace lives on. */
  scope: ServerScope
}): React.JSX.Element {
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const canRunPhone = kind === 'mobile' ? scope.serverSupports('mobile-sim-v1') : true
  const enabled =
    kind === 'design' ? isDesignModeEnabled(project) : isMobileTabShown(mobileTabSetting(project), canRunPhone)
  const unavailable = kind === 'mobile' && !canRunPhone
  const copy = COPY[kind]

  const toggle = useCallback(async (): Promise<void> => {
    if (busy || unavailable) return
    setBusy(true)
    setError(null)
    try {
      await writePinnedTab(scope, project, kind, !enabled, scope.isPrimary ? patchWindowProjects : null)
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    } finally {
      setBusy(false)
    }
  }, [busy, unavailable, scope, project, kind, enabled])

  return (
    <div className="border border-[var(--color-border)] p-3">
      <div className="flex items-start gap-3">
        <Toggle size="sm"
          checked={enabled}
          onChange={() => void toggle()}
          aria-label={copy.label}
          disabled={busy || unavailable}
          data-settings-id={`projects.${kind}-tab`}
          className="mt-0.5"
        />
        <div className="flex-1 min-w-0">
          <div className="type-name text-[var(--color-text-primary)]">{copy.label}</div>
          <div className="type-hint text-[var(--color-text-muted)] mt-0.5 leading-relaxed">
            {unavailable ? copy.noPhone : copy.on}
          </div>
          {error ? (
            <div role="alert" className="type-hint text-[var(--color-danger,#c33)] mt-1">
              {error}
            </div>
          ) : null}
        </div>
      </div>
    </div>
  )
}

/** The Tabs group of a workspace's settings. */
export function WorkspaceTabsSettings({
  project,
  scope,
}: {
  project: PinnedTabProject
  scope: ServerScope
}): React.JSX.Element {
  return (
    <div data-settings-id={TABS_SETTINGS_ID}>
      <SettingsGroup title="Tabs">
        <PinnedTabSettingRow kind="design" project={project} scope={scope} />
        <PinnedTabSettingRow kind="mobile" project={project} scope={scope} />
      </SettingsGroup>
    </div>
  )
}
