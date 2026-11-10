// "Manage the org chart" for one agent (prd-org-chart-v1 §7; Rosson
// 2026-11-10: off by default, the Owner turns it on). On: the agent makes and
// changes orgs, members, heads and lines with `k2 organization` (it never
// moves cards, turns the Org Chart on or off, or writes another org's
// bulletin). The Owner turns it on; the Owner or an Admin turns it off; a
// Member never sees it. Shown only while the server's Org Chart is on.
//
// Writes POST /cli/agent-access/set {toggle: 'org_chart_manage', workspace,
// value}; the state reads back as that agent's `canManage` on the chart
// (the projects list doesn't carry it), so the row reads the chart once and
// again on `org_chart_changed`. The daemon enforces who may flip it.

import { useCallback, useEffect, useState } from 'react'
import { BetaTag, Toggle } from '@/components/ui'
import { fetchViewerRole } from '@/components/Presence/PresenceKickButton'
import { daemonErrorText } from '@/lib/daemon-error'
import { useSettingsStore } from '@/stores/settings'
import { onOrgChartChanged } from '@/stores/session-events'
import type { ServerScope } from '@/kessel/server-scope'
import type { ProjectWithWorkspaces } from '@/stores/projects'
import { orgApiFor, setAgentCanManageOrgChart } from '@/components/OrgChart/org-api'
import { SETTING_ROW_HINT, SETTING_ROW_LABEL, SaveNote, SettingSwitchRow, useSaveStatus } from '../controls/SettingControls'
import { AskedChip } from './PermissionCardsSection'

export const MANAGE_ORG_CHART_ROW = 'projects.agents-can-manage-org-chart'
export const MANAGE_ORG_CHART_LABEL = 'Manage the org chart'

type Role = 'owner' | 'admin' | 'member'

/** Who sees the row and who may flip it which way. */
export function manageOrgChartRights(role: Role | null, on: boolean): { show: boolean; canFlip: boolean } {
  if (role === 'member') return { show: false, canFlip: false }
  // The Owner turns it on; the Owner or an Admin turns it off.
  return { show: true, canFlip: role === 'owner' || (role === 'admin' && on) }
}

export function AgentsManageOrgChartToggle({
  project,
  scope,
  compact = false,
  readOnly = false,
}: {
  project: ProjectWithWorkspaces
  scope: ServerScope
  compact?: boolean
  readOnly?: boolean
}): React.JSX.Element | null {
  const orgChartOn = useSettingsStore((s) => s.orgChart) === true
  const [role, setRole] = useState<Role | null>(null)
  const [on, setOn] = useState<boolean | null>(null)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const save = useSaveStatus()

  const load = useCallback(async (): Promise<void> => {
    try {
      const chart = await orgApiFor(scope).chart()
      const me = chart.agents.find((a) => a.id === project.id)
      if (!me) throw new Error(`The Org Chart has no agent for ${project.name}.`)
      setOn(me.canManage)
    } catch (e) {
      setError(daemonErrorText(e))
    }
  }, [scope, project.id, project.name])

  useEffect(() => {
    if (!orgChartOn) return
    let live = true
    void fetchViewerRole().then((r) => {
      if (live) setRole(r)
    })
    void load()
    const off = onOrgChartChanged(scope, () => void load())
    return () => {
      live = false
      off()
    }
  }, [orgChartOn, load, scope])

  if (!orgChartOn) return null
  const { show, canFlip } = manageOrgChartRights(role, on === true)
  if (!show) return null

  const toggle = async (): Promise<void> => {
    if (busy || !canFlip || readOnly || on === null) return
    const next = !on
    setBusy(true)
    setError(null)
    setOn(next) // optimistic
    try {
      await setAgentCanManageOrgChart(scope, project.path, next)
      save.saved()
    } catch (e) {
      setOn(!next)
      setError(daemonErrorText(e))
      save.reset()
    } finally {
      setBusy(false)
    }
  }

  return (
    <SettingSwitchRow
      compact={compact}
      control={
        <>
          <SaveNote state={save.state} />
          <Toggle
            size={compact ? 'sm' : 'md'}
            checked={on === true}
            onChange={toggle}
            aria-label={MANAGE_ORG_CHART_LABEL}
            disabled={busy || !canFlip || readOnly || on === null}
            data-settings-id="projects.agents-can-manage-org-chart"
            data-testid="agents-manage-org-chart"
            title={
              role === null || on === null
                ? 'Checking this server'
                : !canFlip
                  ? "Only the server's owner turns this on"
                  : on
                    ? "This agent changes the server's Org Chart (logged)"
                    : "This agent can't change the Org Chart"
            }
          />
        </>
      }
    >
      <div className={SETTING_ROW_LABEL}>
        {MANAGE_ORG_CHART_LABEL}
        <BetaTag className="ml-2" />
        <AskedChip row="projects.agents-can-manage-org-chart" />
      </div>
      <div className={compact ? 'hidden' : SETTING_ROW_HINT}>
        It can make and change orgs, members, heads and lines. It can't move cards, turn the Org Chart on or off, or write another org's
        bulletin. It can make itself an org's head. Only the owner turns it on.
      </div>
      {error ? (
        <div className="type-hint text-[var(--color-status-error-soft)] mt-1" data-testid="agents-manage-org-chart-error">
          {error}
        </div>
      ) : null}
    </SettingSwitchRow>
  )
}
